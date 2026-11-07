//! Model comparison on the same Chinese lines (concept §3 "Принцип выбора").
//!
//! For every configured model: start a temporary llama-server on port+1 (or
//! reuse the running one for the active model), translate each line with
//! dialogue context but without cache, and record latency and glossary misses.
//! Output: a Markdown table for side-by-side human review plus a TSV.

use crate::config::Config;
use crate::llm::client::LlmClient;
use crate::llm::server;
use crate::state::Shared;
use crate::translate::{Opts, Translator};
use anyhow::{Context, Result};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use wuxia_core::prompt::TextKind;

pub fn lines_file(shared: &Shared) -> PathBuf {
    let user = shared.paths.data.join("bench.txt");
    if user.exists() {
        user
    } else {
        shared.paths.install.join("bench").join(if shared.cfg().source_lang() == wuxia_core::lang::Lang::En { "wuwa_en_lines.txt" } else { "wwm_lines.txt" })
    }
}

struct Row {
    out: String,
    ms: u128,
    misses: usize,
}

pub async fn run(shared: Arc<Shared>) -> Result<PathBuf> {
    let file = lines_file(&shared);
    let text = std::fs::read_to_string(&file).with_context(|| format!("нет файла {}", file.display()))?;
    let lines: Vec<String> =
        text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_string).collect();
    let cfg = shared.cfg();
    let glossary = crate::translate::load_glossary(&shared.paths, &shared.cfg().general.profile);
    let progress = |s: String| shared.ui(|u| u.bench_progress = Some(s));

    let mut results: Vec<(String, Vec<Row>, String)> = Vec::new();
    for preset in &cfg.models {
        progress(format!("Бенчмарк: {} — запуск…", preset.name));
        let reuse = preset.id == cfg.llm.active_model
            && shared.ui.lock().unwrap().server.as_ref().map_or(false, |s| s.is_ready());
        let mut bench_cfg: Config = cfg.clone();
        let (client, mut child) = if reuse || cfg.llm.mode == "external" {
            (LlmClient::new(&server::base_url(&cfg), cfg.llm.request_timeout_s), None)
        } else {
            bench_cfg.llm.port = cfg.llm.port + 1;
            let client = LlmClient::new(&server::base_url(&bench_cfg), cfg.llm.request_timeout_s);
            let mut st = match server::spawn(&shared.paths, &bench_cfg, preset) {
                Ok(s) => s,
                Err(e) => {
                    results.push((preset.name.clone(), vec![], format!("не запустилась: {e}")));
                    continue;
                }
            };
            let name = preset.name.clone();
            let sh = shared.clone();
            if let Err(e) = server::wait_ready(&client, &mut st, |el, _| {
                sh.ui(|u| u.bench_progress = Some(format!("Бенчмарк: {name} — загрузка ({} с)", el.as_secs())))
            })
            .await
            {
                results.push((preset.name.clone(), vec![], format!("не запустилась: {e}")));
                continue;
            }
            if preset.path.is_empty() {
                if let Some(p) = server::find_downloaded(&shared.paths.models(), &preset.hf) {
                    let mut c = shared.cfg.write().unwrap();
                    if let Some(m) = c.models.iter_mut().find(|m| m.id == preset.id) {
                        m.path = p.to_string_lossy().into_owned();
                    }
                }
                shared.save_config();
            }
            (client, Some(st))
        };

        let tr = Translator::new(shared.paths.clone(), client, cfg.dialogue.context_lines, &cfg.general.profile);
        let mut rows = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            progress(format!("Бенчмарк: {} — {}/{}", preset.name, i + 1, lines.len()));
            let r = tr
                .translate(preset, &cfg.general.game_title, &cfg.dialogue.style_hint, l, TextKind::Dialogue, Opts { use_cache: false, use_history: true }, |_| {})
                .await;
            rows.push(match r {
                Ok(o) => Row { misses: glossary.violations(l, &o.text).len(), out: o.text, ms: o.ms },
                Err(e) => Row { out: format!("⚠ {e}"), ms: 0, misses: 0 },
            });
        }
        if let Some(st) = child.as_mut() {
            let _ = st.child.kill().await;
        }
        let ok: Vec<&Row> = rows.iter().filter(|r| r.ms > 0).collect();
        let avg = if ok.is_empty() { 0 } else { ok.iter().map(|r| r.ms).sum::<u128>() / ok.len() as u128 };
        let misses: usize = rows.iter().map(|r| r.misses).sum();
        results.push((preset.name.clone(), rows, format!("среднее {avg} мс, нарушений глоссария: {misses}")));
    }

    // Markdown report.
    let mut md = String::new();
    let _ = writeln!(md, "# Сравнение моделей — Wuxia Local\n\nИсточник: `{}`\n", file.display());
    for (name, _, summary) in &results {
        let _ = writeln!(md, "* **{name}** — {summary}");
    }
    let _ = writeln!(md, "\nОцените вручную: точность смысла, естественность, стиль уся, имена и термины, выдуманные детали.\n");
    for (i, l) in lines.iter().enumerate() {
        let _ = writeln!(md, "### {}. {l}\n", i + 1);
        for (name, rows, _) in &results {
            if let Some(r) = rows.get(i) {
                let _ = writeln!(md, "* *{name}* ({} мс{}): {}", r.ms, if r.misses > 0 { format!(", ⚠ термины: {}", r.misses) } else { String::new() }, r.out);
            }
        }
        md.push('\n');
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let out = shared.paths.data.join(format!("bench-{stamp}.md"));
    std::fs::write(&out, md)?;

    let mut tsv = String::from("line\tmodel\tms\tglossary_misses\ttranslation\n");
    for (i, l) in lines.iter().enumerate() {
        for (name, rows, _) in &results {
            if let Some(r) = rows.get(i) {
                tsv.push_str(&wuxia_core::tsv::line(&[l, name, &r.ms.to_string(), &r.misses.to_string(), &r.out]));
            }
        }
    }
    std::fs::write(out.with_extension("tsv"), tsv)?;
    Ok(out)
}
