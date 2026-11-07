//! Wuxia Local — local neural game translator (Chinese → Russian).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("Wuxia Local работает только в Windows 10/11 x64. Ядро (wuxia-core) собирается и тестируется на любой ОС.");
}

#[cfg(windows)]
mod bench;
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod config;
#[cfg(windows)]
mod dialogue;
#[cfg(windows)]
mod hotkeys;
#[cfg(windows)]
mod interface;
#[cfg(windows)]
mod llm;
#[cfg(windows)]
mod ocr;
#[cfg(windows)]
mod paths;
#[cfg(windows)]
mod state;
#[cfg(windows)]
mod translate;
#[cfg(windows)]
mod ui;
#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    if let Err(e) = app::run() {
        app::fatal(&format!("{e:#}"));
    }
}

#[cfg(windows)]
mod app {
    use crate::config::Config;
    use crate::llm::client::LlmClient;
    use crate::llm::server::{self, ServerCmd};
    use crate::ocr::Ocr;
    use crate::paths::Paths;
    use crate::state::{Command, Shared, UiState};
    use crate::translate::Translator;
    use crate::{bench, capture, dialogue, hotkeys, interface, ui, win};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, RwLock};
    use tokio::sync::mpsc;

    pub fn fatal(msg: &str) {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        tracing::error!("{msg}");
        unsafe {
            MessageBoxW(HWND::default(), &HSTRING::from(msg), &HSTRING::from("Wuxia Local — ошибка"), MB_OK | MB_ICONERROR);
        }
    }

    fn init_logging(paths: &Paths) -> tracing_appender::non_blocking::WorkerGuard {
        use tracing_subscriber::{fmt, EnvFilter};
        let file = tracing_appender::rolling::daily(paths.logs(), "wuxia.log");
        let (nb, guard) = tracing_appender::non_blocking(file);
        let filter = EnvFilter::try_from_env("WUXIA_LOG").unwrap_or_else(|_| EnvFilter::new("info,llama=debug,dialogue=debug"));
        fmt().with_env_filter(filter).with_writer(nb).with_ansi(false).init();
        guard
    }

    fn open_path(p: &std::path::Path) {
        let r = if p.is_dir() {
            std::process::Command::new("explorer").arg(p).spawn()
        } else {
            if !p.exists() {
                let _ = std::fs::write(p, "");
            }
            std::process::Command::new("notepad").arg(p).spawn()
        };
        if let Err(e) = r {
            tracing::warn!("открыть {}: {e}", p.display());
        }
    }

    pub fn run() -> anyhow::Result<()> {
        // Only one copy: a second launch just brings up the panel of the first one.
        if !win::claim_single_instance() {
            win::signal_show_panel();
            return Ok(());
        }
        win::set_dpi_aware();
        let paths = Paths::detect()?;
        let _log_guard = init_logging(&paths);
        tracing::info!("Wuxia Local {} · данные: {}", env!("CARGO_PKG_VERSION"), paths.data.display());
        std::panic::set_hook(Box::new(|info| {
            tracing::error!("panic: {info}");
        }));

        let cfg = Config::load(&paths)?;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Command>();
        let shared = Arc::new(Shared {
            paths: paths.clone(),
            cfg: RwLock::new(cfg.clone()),
            ui: Mutex::new(UiState { dialogue_on: cfg.dialogue.enabled, show_original: cfg.dialogue.show_original, ..Default::default() }),
            cmd: cmd_tx.clone(),
            egui: OnceLock::new(),
            dialogue_seq: AtomicU64::new(0),
            interface_gen: AtomicU64::new(0),
            reset_watcher: AtomicBool::new(false),
        });

        // Hotkeys.
        let problems = hotkeys::spawn(&cfg.hotkeys, cmd_tx.clone());
        if let Ok(p) = problems.recv_timeout(std::time::Duration::from_secs(2)) {
            for msg in p {
                shared.ui(|u| u.notice(msg));
            }
        }

        // OCR.
        let ocr = match Ocr::spawn(&cfg.ocr.language) {
            Ok(o) => {
                tracing::info!("Windows OCR готов ({}), макс. размер {} px", cfg.ocr.language, o.max_dim);
                Some(o)
            }
            Err(e) => {
                shared.ui(|u| {
                    u.notice(e.to_string());
                    u.panel_open = true;
                });
                None
            }
        };

        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("wuxia-rt").build()?;
        let client = LlmClient::new(&server::base_url(&cfg), cfg.llm.request_timeout_s);
        let translator = {
            let _g = rt.enter();
            Arc::new(Translator::new(paths.clone(), client.clone(), cfg.dialogue.context_lines))
        };
        let (srv_tx, srv_rx) = mpsc::unbounded_channel::<ServerCmd>();
        rt.spawn(server::supervise(shared.clone(), client, srv_rx));
        if let Some(o) = &ocr {
            rt.spawn(dialogue::run(shared.clone(), translator.clone(), o.clone()));
        }
        rt.spawn(dispatcher(shared.clone(), translator.clone(), ocr.clone(), cmd_rx, srv_tx));

        // The panel is the only visible sign that the program runs: always show it at start.
        shared.ui(|u| u.panel_open = true);
        {
            let tx = cmd_tx.clone();
            win::listen_show_panel(move || {
                let _ = tx.send(Command::ShowPanel);
            });
        }

        let options = eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_title(ui::OVERLAY_TITLE)
                .with_transparent(true)
                .with_decorations(false)
                .with_always_on_top()
                .with_mouse_passthrough(true)
                .with_taskbar(false)
                .with_resizable(false)
                .with_position([0.0, 0.0])
                // Starts as a 1×1 window; ui.rs expands it to full screen once it renders
                // transparently, so the screen never flashes black.
                .with_inner_size([1.0, 1.0]),
            renderer: eframe::Renderer::Glow,
            vsync: true,
            ..Default::default()
        };
        let sh2 = shared.clone();
        let res = eframe::run_native("wuxia-local", options, Box::new(move |cc| Ok(Box::new(ui::WuxiaApp::new(cc, sh2)))));
        tracing::info!("выход");
        rt.shutdown_timeout(std::time::Duration::from_secs(2));
        res.map_err(|e| anyhow::anyhow!("окно: {e}"))
    }

    async fn dispatcher(
        shared: Arc<Shared>,
        tr: Arc<Translator>,
        ocr: Option<Ocr>,
        mut rx: mpsc::UnboundedReceiver<Command>,
        srv: mpsc::UnboundedSender<ServerCmd>,
    ) {
        while let Some(cmd) = rx.recv().await {
            tracing::debug!("команда: {cmd:?}");
            match cmd {
                Command::ToggleDialogue => {
                    shared.reset_watcher.store(true, Ordering::SeqCst);
                    shared.ui(|u| {
                        u.dialogue_on = !u.dialogue_on;
                        if !u.dialogue_on {
                            u.dialogue = None;
                        }
                    });
                }
                Command::ScreenTranslate => match &ocr {
                    Some(o) => {
                        tokio::spawn(interface::toggle(shared.clone(), tr.clone(), o.clone()));
                    }
                    None => shared.ui(|u| u.notice("OCR не готов — перевод экрана недоступен")),
                },
                Command::ToggleOriginal => shared.ui(|u| u.show_original = !u.show_original),
                Command::SelectRegion => {
                    let selecting = shared.ui.lock().unwrap().selecting_region;
                    if selecting {
                        shared.ui(|u| {
                            u.selecting_region = false;
                            u.region_shot = None;
                        });
                    } else {
                        // Hide our own windows, let them disappear, then take the screenshot.
                        shared.ui(|u| {
                            u.dialogue = None;
                            u.interface = None;
                            u.panel_open = false;
                        });
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        let (sw, sh) = win::screen_size();
                        match capture::capture(capture::PxRect { x: 0, y: 0, w: sw, h: sh }) {
                            Ok(f) => shared.ui(|u| {
                                u.region_shot = Some(Arc::new(f));
                                u.selecting_region = true;
                            }),
                            Err(e) => shared.ui(|u| u.notice(format!("Снимок экрана: {e}"))),
                        }
                    }
                }
                Command::CancelRegion => shared.ui(|u| {
                    u.selecting_region = false;
                    u.region_shot = None;
                }),
                Command::RegionSelected(r) => {
                    shared.cfg.write().unwrap().dialogue.region = r;
                    shared.save_config();
                    shared.reset_watcher.store(true, Ordering::SeqCst);
                    shared.ui(|u| {
                        u.selecting_region = false;
                        u.region_shot = None;
                        u.notice("Область субтитров сохранена");
                    });
                    tracing::info!("новая область субтитров: {r:?}");
                }
                Command::TogglePanel => shared.ui(|u| u.panel_open = !u.panel_open),
                Command::ShowPanel => shared.ui(|u| u.panel_open = true),
                Command::SwitchModel(id) => {
                    shared.cfg.write().unwrap().llm.active_model = id;
                    shared.save_config();
                    tr.clear_history();
                    let _ = srv.send(ServerCmd::Restart);
                }
                Command::RestartServer => {
                    let _ = srv.send(ServerCmd::Restart);
                }
                Command::SaveCorrection { src, dst } => {
                    tr.save_correction(&src, &dst);
                    shared.ui(|u| {
                        for r in u.recent.iter_mut().filter(|r| r.src == src) {
                            r.dst = dst.clone();
                        }
                    });
                }
                Command::ReloadGlossary => {
                    let (n, f) = tr.reload_dictionaries();
                    shared.ui(|u| u.notice(format!("Словари перечитаны: терминов {n}, исправлений {f}")));
                }
                Command::RunBench => {
                    let sh = shared.clone();
                    tokio::spawn(async move {
                        let r = bench::run(sh.clone()).await;
                        sh.ui(|u| {
                            u.bench_progress = None;
                            match &r {
                                Ok(p) => u.notice(format!("Бенчмарк готов: {}", p.display())),
                                Err(e) => u.notice(format!("Бенчмарк: {e}")),
                            }
                        });
                        if let Ok(p) = r {
                            open_path(&p);
                        }
                    });
                }
                Command::TestTranslate(text) => {
                    let (sh, tr) = (shared.clone(), tr.clone());
                    tokio::spawn(async move {
                        sh.ui(|u| u.test_result = Some("…".into()));
                        let cfg = sh.cfg();
                        let preset = cfg.active_model().clone();
                        let src = tr.fix_ocr(&wuxia_core::text::normalize_line(&text));
                        let kind = if src.chars().count() <= 8 && !src.contains(['，', '。', '！', '？']) {
                            wuxia_core::prompt::TextKind::Interface
                        } else {
                            wuxia_core::prompt::TextKind::Dialogue
                        };
                        let opts = crate::translate::Opts { use_cache: true, use_history: false };
                        let r = tr.translate(&preset, &cfg.general.game_title, &cfg.dialogue.style_hint, &src, kind, opts, |_| {}).await;
                        sh.ui(|u| match r {
                            Ok(o) => {
                                u.test_result = Some(format!("{}  ({} мс{})", o.text, o.ms, if o.cached { ", кэш" } else { "" }));
                                u.push_recent(crate::state::RecentLine { src: src.clone(), dst: o.text, model: preset.id.clone(), ms: o.ms, cached: o.cached });
                            }
                            Err(e) => u.test_result = Some(format!("Ошибка: {e}")),
                        });
                    });
                }
                Command::SaveConfig => shared.save_config(),
                Command::Open(p) => open_path(&p),
            }
        }
    }
}
