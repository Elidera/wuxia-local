//! Wuxia Local — local neural game translator (English / Chinese → Russian).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("Wuxia Local работает только в Windows 10/11 x64. Ядро (wuxia-core) собирается и тестируется на любой ОС.");
}

#[cfg(windows)]
mod auto;
#[cfg(windows)]
mod bench;
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod config;
#[cfg(windows)]
mod dialogue;
#[cfg(windows)]
mod extras;
#[cfg(windows)]
mod filepick;
#[cfg(windows)]
mod hotkeys;
#[cfg(windows)]
mod interface;
#[cfg(windows)]
mod llm;
#[cfg(windows)]
mod mascot;
#[cfg(windows)]
mod ocr;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod paths;
#[cfg(windows)]
mod selftest;
#[cfg(windows)]
mod state;
#[cfg(windows)]
mod translate;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod ui;
#[cfg(windows)]
mod ui_panel;
#[cfg(windows)]
mod win;

/// "v0.2.0 · 08.11.2026 14:30 · abc1234" — the build date and commit are set by CI.
#[cfg(windows)]
pub fn build_info() -> String {
    match option_env!("WUXIA_BUILD") {
        Some(b) if !b.is_empty() => format!("v{} · {b}", env!("CARGO_PKG_VERSION")),
        _ => format!("v{} · локальная сборка", env!("CARGO_PKG_VERSION")),
    }
}

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
        // Screenshot tests (CI and the developer): no window, no second-copy check.
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).map(String::as_str) == Some("--selftest") {
            std::process::exit(crate::selftest::run_cli(&args));
        }
        // Only one copy: a second launch just brings up the panel of the first one.
        if !win::claim_single_instance() {
            win::signal_show_panel();
            return Ok(());
        }
        win::set_dpi_aware();
        let paths = Paths::detect()?;
        let _log_guard = init_logging(&paths);
        tracing::info!("Wuxia Local {} · данные: {}", crate::build_info(), paths.data.display());
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
            overlay_px: Mutex::new(Vec::new()),
        });

        // Hotkeys.
        let problems = hotkeys::spawn(&cfg.hotkeys, cmd_tx.clone());
        if let Ok(p) = problems.recv_timeout(std::time::Duration::from_secs(2)) {
            shared.ui(|u| u.hotkey_problems = p.clone());
            for msg in p {
                shared.ui(|u| u.notice(msg));
            }
        }

        // OCR.
        let ocr = match Ocr::spawn() {
            Ok(o) => {
                // A missing language pack is not fatal: the user may install it or switch the language.
                let status = o.check_blocking(cfg.source_lang()).map_err(|e| e.to_string());
                tracing::info!("Windows OCR ({}): {:?} · макс. размер {} px", cfg.source_lang().name_ru(), status, o.max_dim);
                let failed = status.is_err();
                shared.ui(|u| {
                    u.ocr_status = Some(status);
                    if failed {
                        u.panel_open = true;
                    }
                });
                Some(o)
            }
            Err(e) => {
                shared.ui(|u| {
                    u.ocr_status = Some(Err(e.to_string()));
                    u.panel_open = true;
                });
                None
            }
        };

        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("wuxia-rt").build()?;
        let client = LlmClient::new(&server::base_url(&cfg), cfg.llm.request_timeout_s);
        let translator = {
            let _g = rt.enter();
            Arc::new(Translator::new(paths.clone(), client.clone(), cfg.dialogue.context_lines, &cfg.general.profile))
        };
        {
            let counts = translator.dict_counts();
            shared.ui(|u| u.dict_counts = counts);
        }
        let (srv_tx, srv_rx) = mpsc::unbounded_channel::<ServerCmd>();
        rt.spawn(server::supervise(shared.clone(), client, srv_rx));
        if let Some(o) = &ocr {
            rt.spawn(dialogue::run(shared.clone(), translator.clone(), o.clone()));
        }
        rt.spawn(dispatcher(shared.clone(), translator.clone(), ocr.clone(), cmd_rx, srv_tx));

        // The panel is the only visible sign that the program runs: always show it at start.
        shared.ui(|u| {
            u.panel_open = true;
            u.focus_panel = true;
        });
        crate::overlay::spawn(shared.clone());
        crate::tray::spawn(cmd_tx.clone());
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
                // The root egui window only hosts the panel and the region picker:
                // it stays a transparent 1×1 px window. Translations are drawn by
                // the native overlay window (overlay.rs).
                .with_inner_size([1.0, 1.0])
                .with_icon(crate::mascot::icon_data()),
            renderer: eframe::Renderer::Glow,
            vsync: true,
            ..Default::default()
        };
        let sh2 = shared.clone();
        let res = eframe::run_native("wuxia-local", options, Box::new(move |cc| Ok(Box::new(ui::WuxiaApp::new(cc, sh2)))));
        crate::tray::remove();
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
            tracing::info!("команда: {}", format!("{cmd:?}").chars().take(120).collect::<String>());
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
                        let (sh, tr, o) = (shared.clone(), tr.clone(), o.clone());
                        tokio::spawn(async move {
                            // The panel would be photographed and "translated" too: hide it
                            // first and give Windows time to take it off the screen.
                            // Pressed in the panel: the panel goes to the tray either way.
                            let was_open = sh.ui.lock().unwrap().panel_open;
                            if was_open {
                                sh.ui(|u| u.panel_open = false);
                                sh.send(Command::PanelClosedToTray);
                                if sh.ui.lock().unwrap().interface.is_none() {
                                    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                                }
                            }
                            interface::toggle(sh, tr, o).await;
                        });
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
                    {
                        let mut c = shared.cfg.write().unwrap();
                        c.dialogue.region = r;
                        c.dialogue.region_set = true;
                        // A strip means "watch only here"; half the screen or more is
                        // better served by the auto mode (and too big for region mode).
                        if r[2] * r[3] < 0.5 {
                            c.dialogue.mode = "region".into();
                        } else {
                            c.dialogue.mode = "auto".into();
                        }
                    }
                    shared.save_config();
                    shared.reset_watcher.store(true, Ordering::SeqCst);
                    shared.ui(|u| {
                        u.selecting_region = false;
                        u.region_shot = None;
                        if r[2] * r[3] < 0.5 {
                            u.notice("Область субтитров сохранена. Режим «только область»; вернуть «Авто: весь экран» можно в настройках.");
                        } else {
                            u.notice("Область почти на весь экран — оставляю режим «Авто: весь экран», он для этого и сделан.");
                        }
                    });
                    tracing::info!("новая область субтитров: {r:?}");
                }
                Command::TogglePanel => shared.ui(|u| {
                    u.panel_open = !u.panel_open;
                    u.focus_panel = u.panel_open;
                }),
                Command::ShowPanel => shared.ui(|u| {
                    u.panel_open = true;
                    u.focus_panel = true;
                }),
                Command::SwitchModel(id) => {
                    shared.cfg.write().unwrap().llm.active_model = id;
                    shared.save_config();
                    tr.clear_history();
                    let _ = srv.send(ServerCmd::Restart);
                }
                Command::RestartServer => {
                    let _ = srv.send(ServerCmd::Restart);
                }
                Command::UseModelFile(path) => {
                    let id = {
                        let mut c = shared.cfg.write().unwrap();
                        let id = c.preset_for_file(&path);
                        if let Some(m) = c.models.iter_mut().find(|m| m.id == id) {
                            m.path = path.display().to_string();
                        }
                        c.llm.active_model = id.clone();
                        id
                    };
                    shared.save_config();
                    tr.clear_history();
                    tracing::info!("модель из файла: {} → {id}", path.display());
                    shared.ui(|u| u.notice(format!("Модель берётся из файла {} — скачивать ничего не нужно.", path.display())));
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
                Command::GameChanged => {
                    let cfg = shared.cfg();
                    let (n, f) = tr.reload_dictionaries(&cfg.general.profile);
                    tr.clear_history();
                    shared.reset_watcher.store(true, std::sync::atomic::Ordering::SeqCst);
                    let status = match &ocr {
                        Some(o) => o.check(cfg.source_lang()).await.map_err(|e| e.to_string()),
                        None => Err("OCR не запущен".into()),
                    };
                    shared.ui(|u| {
                        u.dict_counts = (n, f);
                        u.ocr_status = Some(status);
                    });
                    shared.save_config();
                }
                Command::EditBox => {
                    let on = shared.ui(|u| {
                        u.edit_box = !u.edit_box;
                        u.edit_box
                    });
                    if on {
                        // Editing the window means the user wants to use it.
                        shared.cfg.write().unwrap().dialogue.position = "box".into();
                    }
                    shared.save_config();
                    tracing::info!("окно перевода: {}", if on { "настройка" } else { "готово" });
                }
                Command::SaveCase => match &ocr {
                    Some(o) => {
                        tokio::spawn(crate::selftest::save_case(shared.clone(), tr.clone(), o.clone()));
                    }
                    None => shared.ui(|u| u.notice("OCR не готов — снимок для теста не сохранить")),
                },
                Command::ExportDicts(path) => crate::extras::export_dicts(&shared, &tr, &path),
                Command::ImportDicts(path) => crate::extras::import_dicts(&shared, &tr, &path),
                Command::BadTranslation => {
                    tokio::spawn(crate::extras::bad_translation(shared.clone(), tr.clone()));
                }
                Command::Freeze => match &ocr {
                    Some(o) => {
                        tokio::spawn(crate::extras::freeze(shared.clone(), o.clone()));
                    }
                    None => shared.ui(|u| u.notice("Стоп-кадр: распознавание текста не готово")),
                },
                Command::FreezeTranslate(i) => {
                    tokio::spawn(crate::extras::freeze_translate(shared.clone(), tr.clone(), i));
                }
                Command::FreezeClose => shared.ui(|u| u.freeze = None),
                Command::AddTerm { src, dst } => crate::extras::add_term(&shared, &tr, &src, &dst),
                Command::Diagnose => {
                    tokio::spawn(crate::extras::diagnose(shared.clone(), tr.clone(), ocr.clone()));
                }
                Command::TestOverlay => {
                    shared.ui(|u| u.test_overlay_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(10)));
                }
                Command::Quit => {
                    tracing::info!("выход по команде пользователя");
                    crate::tray::remove();
                    if let Some(ctx) = shared.egui.get() {
                        ctx.send_viewport_cmd_to(eframe::egui::ViewportId::ROOT, eframe::egui::ViewportCommand::Close);
                        ctx.request_repaint();
                    }
                    // Safety net: if the window loop does not end, leave anyway
                    // (llama-server dies with us through the job object).
                    std::thread::spawn(|| {
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        std::process::exit(0);
                    });
                }
                Command::PanelClosedToTray => {
                    let first = !shared.cfg().general.tray_hint_shown;
                    if first {
                        crate::tray::balloon(
                            "Wuxia Local работает в фоне",
                            "Перевод и горячие клавиши продолжают работать. Значок с енотиком — у часов: клик открывает панель, правый клик — «Выход».",
                        );
                        shared.cfg.write().unwrap().general.tray_hint_shown = true;
                        shared.save_config();
                    }
                }
                Command::ReloadGlossary => {
                    let (n, f) = tr.reload_dictionaries(&shared.cfg().general.profile);
                    shared.ui(|u| {
                        u.dict_counts = (n, f);
                        u.notice(format!("Словари перечитаны: терминов {n}, исправлений {f}"));
                    });
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
                        let lang = wuxia_core::lang::detect(&text).unwrap_or(cfg.source_lang());
                        let src = tr.fix_ocr(&wuxia_core::text::normalize_for(&text, lang));
                        let short = match lang {
                            wuxia_core::lang::Lang::Zh => src.chars().count() <= 8,
                            wuxia_core::lang::Lang::En => src.split_whitespace().count() <= 3,
                        };
                        let kind = if short && !src.contains(['，', '。', '！', '？', '.', '!', '?']) {
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
                Command::OpenUrl(u) => {
                    if u.starts_with("https://") {
                        let _ = std::process::Command::new("explorer").arg(&u).spawn();
                    }
                }
            }
        }
    }
}
