//! Supervisor for a local `llama-server.exe` child process.
//!
//! * starts it hidden, below-normal priority, bound to 127.0.0.1 only;
//! * ties its lifetime to ours with a Job Object;
//! * on first use downloads the model via llama-server's own `-hf` support into
//!   our models dir, then pins the local .gguf path in the config so later
//!   starts are fully offline;
//! * restarts it with back-off if it crashes;
//! * restarts on model switch.

use crate::config::{Config, ModelPreset};
use crate::paths::Paths;
use crate::state::{ServerStatus, Shared};
use anyhow::{anyhow, Context, Result};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc::UnboundedReceiver;

use super::client::LlmClient;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

#[derive(Debug)]
pub enum ServerCmd {
    Restart,
}

pub fn base_url(cfg: &Config) -> String {
    if cfg.llm.mode == "external" {
        cfg.llm.external_url.clone()
    } else {
        format!("http://127.0.0.1:{}", cfg.llm.port)
    }
}

fn auto_threads() -> u32 {
    let n = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
    // Leave the other half to the game.
    (n / 2).max(2)
}

pub struct Started {
    pub child: Child,
    pub log: Arc<Mutex<VecDeque<String>>>,
}

/// Spawn llama-server for `preset` (does not wait for readiness).
pub fn spawn(paths: &Paths, cfg: &Config, preset: &ModelPreset) -> Result<Started> {
    let exe = paths.resolve(&cfg.llm.server_exe);
    if !exe.exists() {
        return Err(anyhow!("не найден {}", exe.display()));
    }
    let mut cmd = Command::new(&exe);
    cmd.current_dir(exe.parent().unwrap_or(Path::new(".")));
    cmd.env("LLAMA_CACHE", paths.models());
    cmd.args(["--host", "127.0.0.1", "--port", &cfg.llm.port.to_string()]);
    let local = (!preset.path.is_empty()).then(|| PathBuf::from(&preset.path)).filter(|p| p.exists());
    match (&local, preset.hf.is_empty()) {
        (Some(p), _) => {
            cmd.arg("-m").arg(p);
        }
        (None, false) => {
            cmd.args(["-hf", &preset.hf]);
        }
        (None, true) => return Err(anyhow!("для модели «{}» не указан ни path, ни hf", preset.name)),
    }
    let threads = if cfg.llm.threads == 0 { auto_threads() } else { cfg.llm.threads };
    cmd.args(["-c", &cfg.llm.ctx_size.to_string()]);
    cmd.args(["-ngl", &cfg.llm.gpu_layers.to_string()]);
    cmd.args(["-t", &threads.to_string()]);
    cmd.args(["-np", "1", "--jinja"]);
    cmd.args(&cfg.llm.extra_args);
    let mut flags = CREATE_NO_WINDOW;
    if cfg.llm.low_priority {
        flags |= BELOW_NORMAL_PRIORITY_CLASS;
    }
    cmd.creation_flags(flags);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.kill_on_drop(true);
    tracing::info!("запуск llama-server: {:?}", cmd.as_std());
    let mut child = cmd.spawn().with_context(|| format!("не удалось запустить {}", exe.display()))?;
    if let Some(h) = child.raw_handle() {
        if let Err(e) = crate::win::kill_with_parent(h) {
            tracing::warn!("job object: {e}");
        }
    }
    let log = Arc::new(Mutex::new(VecDeque::with_capacity(64)));
    for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
                   child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)]
        .into_iter()
        .flatten()
    {
        let log = log.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                tracing::debug!(target: "llama", "{l}");
                let mut g = log.lock().unwrap();
                g.push_back(l);
                while g.len() > 60 {
                    g.pop_front();
                }
            }
        });
    }
    Ok(Started { child, log })
}

/// Wait until /health is OK or the process exits.
pub async fn wait_ready(client: &LlmClient, st: &mut Started, mut on_wait: impl FnMut(Duration, Option<String>)) -> Result<()> {
    let t0 = Instant::now();
    loop {
        if let Some(status) = st.child.try_wait()? {
            let tail: Vec<String> = st.log.lock().unwrap().iter().rev().take(6).cloned().collect();
            let tail: Vec<String> = tail.into_iter().rev().collect();
            return Err(anyhow!("llama-server завершился ({status}). Последние строки лога:\n{}", tail.join("\n")));
        }
        if client.health().await.unwrap_or(false) {
            return Ok(());
        }
        let last = st.log.lock().unwrap().back().cloned();
        on_wait(t0.elapsed(), last);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Find the .gguf llama-server downloaded for an `-hf` reference.
pub fn find_downloaded(models: &Path, hf: &str) -> Option<PathBuf> {
    let (repo, quant) = hf.split_once(':').unwrap_or((hf, ""));
    let repo_name = repo.rsplit('/').next().unwrap_or(repo).to_lowercase().replace("-gguf", "");
    let quant = quant.to_lowercase();
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut stack = vec![models.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let full = p.to_string_lossy().to_lowercase();
            let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !name.ends_with(".gguf") || name.contains("mmproj") {
                continue;
            }
            let repo_hint = repo_name.split(['-', '_', '.']).filter(|t| t.len() > 1).all(|t| full.contains(t));
            if !repo_hint || (!quant.is_empty() && !name.contains(&quant)) {
                continue;
            }
            let m = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            if best.as_ref().map_or(true, |(t, _)| m > *t) {
                best = Some((m, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Supervisor loop. Runs for the lifetime of the app.
pub async fn supervise(shared: Arc<Shared>, client: LlmClient, mut rx: UnboundedReceiver<ServerCmd>) {
    let set = |s: ServerStatus| shared.ui(|u| u.server = Some(s));
    let mut failures = 0u32;
    // Crashes soon after a good start (e.g. video memory runs out mid-game):
    // each next restart waits longer, and the user is told once, not every time.
    let mut quick_crashes = 0u32;
    loop {
        let cfg = shared.cfg();
        let preset = cfg.active_model().clone();

        if cfg.llm.mode == "external" {
            // Just monitor the external server.
            let ok = client.health().await.unwrap_or(false);
            set(if ok {
                ServerStatus::Ready { model: format!("{} (внешний)", preset.name) }
            } else {
                ServerStatus::Failed(format!("внешний сервер {} недоступен", client.base()))
            });
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                cmd = rx.recv() => if cmd.is_none() { return; },
            }
            continue;
        }

        set(ServerStatus::Starting(preset.name.clone()));
        let mut st = match spawn(&shared.paths, &cfg, &preset) {
            Ok(s) => s,
            Err(e) => {
                set(ServerStatus::Failed(e.to_string()));
                // Wait for the user to fix things (e.g. switch model).
                if rx.recv().await.is_none() {
                    return;
                }
                continue;
            }
        };
        let downloading = preset.path.is_empty() || !Path::new(&preset.path).exists();
        let name = preset.name.clone();
        let ready = {
            let shared2 = shared.clone();
            let wait = wait_ready(&client, &mut st, move |el, last| {
                let msg = if downloading {
                    let tail = last.unwrap_or_default();
                    let tail: String = tail.chars().rev().take(60).collect::<Vec<_>>().into_iter().rev().collect();
                    format!("{name}: первая загрузка модели ({} с) {tail}", el.as_secs())
                } else {
                    format!("{name}: загрузка в память ({} с)", el.as_secs())
                };
                shared2.ui(|u| u.server = Some(ServerStatus::Starting(msg)));
            });
            tokio::select! {
                r = wait => Some(r),
                cmd = rx.recv() => { if cmd.is_none() { return; } None }
            }
        };
        match ready {
            None => {
                let _ = st.child.kill().await;
                continue; // restart requested while starting
            }
            Some(Err(e)) => {
                failures += 1;
                tracing::error!("{e}");
                set(ServerStatus::Failed(e.to_string()));
                let backoff = Duration::from_secs((2u64 << failures.min(5)).min(60));
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    cmd = rx.recv() => if cmd.is_none() { return; },
                }
                continue;
            }
            Some(Ok(())) => {}
        }
        failures = 0;
        tracing::info!("llama-server готов: {}", preset.name);
        set(ServerStatus::Ready { model: preset.name.clone() });
        let up_since = std::time::Instant::now();

        // Pin the downloaded file so the next start is offline.
        if downloading && !preset.hf.is_empty() {
            if let Some(p) = find_downloaded(&shared.paths.models(), &preset.hf) {
                tracing::info!("модель сохранена: {}", p.display());
                {
                    let mut c = shared.cfg.write().unwrap();
                    if let Some(m) = c.models.iter_mut().find(|m| m.id == preset.id) {
                        m.path = p.to_string_lossy().into_owned();
                    }
                }
                shared.save_config();
            }
        }

        tokio::select! {
            status = st.child.wait() => {
                let msg = format!("llama-server неожиданно завершился: {:?}", status.ok());
                tracing::error!("{msg}");
                quick_crashes = if up_since.elapsed() < Duration::from_secs(120) { quick_crashes + 1 } else { 1 };
                match quick_crashes {
                    1 => shared.ui(|u| u.notice("Переводчик (модель) неожиданно закрылся — перезапускаю.")),
                    3 => shared.ui(|u| u.notice(
                        "Модель перевода падает снова и снова. Скорее всего, не хватает памяти видеокарты: \
                         выберите модель поменьше в «Модели».",
                    )),
                    _ => {}
                }
                set(ServerStatus::Failed(msg));
                let wait = Duration::from_secs((2u64 << quick_crashes.saturating_sub(1).min(5)).min(60));
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    cmd = rx.recv() => if cmd.is_none() { return; },
                }
            }
            cmd = rx.recv() => {
                let _ = st.child.kill().await;
                if cmd.is_none() { return; }
                quick_crashes = 0; // a new model or settings: a fresh start
            }
        }
    }
}
