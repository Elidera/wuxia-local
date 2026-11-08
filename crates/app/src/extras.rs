//! Smaller user features: «плохой перевод» (Ctrl+F11), the freeze frame
//! (Ctrl+F8), adding a term from it, exchanging dictionaries and the
//! one-button self-check.

use crate::capture::{self, PxRect};
use crate::ocr::{self, Ocr};
use crate::state::{FreezeLine, FreezeView, RecentLine, Shared};
use crate::translate::{Opts, Translator};
use crate::win;
use std::sync::Arc;
use std::time::Duration;
use wuxia_core::lang::{self, Lang};
use wuxia_core::layout;
use wuxia_core::prompt::TextKind;
use wuxia_core::text;

fn short(s: &str) -> String {
    let t: String = s.chars().take(60).collect();
    if t.chars().count() < s.chars().count() {
        format!("{t}…")
    } else {
        t
    }
}

// ------------------------------------------------------------ bad translation

/// Ctrl+F11: the last dialogue translation is bad — forget it, note it in
/// bad.tsv and translate the line again (a new sample from the model).
pub async fn bad_translation(shared: Arc<Shared>, tr: Arc<Translator>) {
    let last: Option<RecentLine> = shared.ui.lock().unwrap().recent.front().cloned();
    let Some(r) = last else {
        shared.ui(|u| u.notice("Пока нечего отмечать: переведённых реплик ещё нет."));
        return;
    };
    let cfg = shared.cfg();
    let mut preset = cfg.active_model().clone();
    tr.forget(&preset.id, &r.src, &r.dst);
    shared.ui(|u| u.notice(format!("Плохой перевод отмечен, перевожу заново: «{}»", short(&r.src))));
    // A livelier sample than the default, so the new translation differs.
    preset.temperature = (preset.temperature + 0.2).min(1.0);
    let game = cfg.general.game_title.clone();
    let old = r.dst.clone();
    let res = tr
        .translate(&preset, &game, &cfg.dialogue.style_hint, &r.src, TextKind::Dialogue, Opts { use_cache: true, use_history: false }, |_| {})
        .await;
    match res {
        Ok(o) => shared.ui(|u| {
            // Replace the bad text wherever it is still shown.
            for l in u.auto_lines.iter_mut().filter(|l| l.translation == old) {
                l.translation = o.text.clone();
            }
            if let Some(d) = u.dialogue.as_mut().filter(|d| d.translation == old) {
                d.translation = o.text.clone();
            }
            if let Some(front) = u.recent.front_mut().filter(|f| f.src == r.src) {
                front.dst = o.text.clone();
                front.cached = false;
            }
            u.notice(format!("Новый перевод: {}", short(&o.text)));
        }),
        Err(e) => shared.ui(|u| u.notice(format!("Перевести заново не удалось: {e}"))),
    }
}

// ------------------------------------------------------------ freeze frame

/// Ctrl+F8: show a still picture of the screen with every text outlined;
/// a click on a text translates it (see ui.rs `freeze_window`). Again — close.
pub async fn freeze(shared: Arc<Shared>, ocr: Ocr) {
    if shared.ui.lock().unwrap().freeze.is_some() {
        shared.ui(|u| u.freeze = None);
        return;
    }
    let was_open = shared.ui.lock().unwrap().panel_open;
    shared.ui(|u| u.panel_open = false);
    if was_open {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let (sw, sh) = win::screen_size();
    // Plain capture: our layered overlay is not in it, the panel is hidden.
    let frame = match capture::capture(PxRect { x: 0, y: 0, w: sw, h: sh }) {
        Ok(f) => f,
        Err(e) => {
            shared.ui(|u| u.notice(format!("Стоп-кадр: {e}")));
            return;
        }
    };
    let shot = Arc::new(frame);
    let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
    shared.ui(|u| u.freeze = Some(FreezeView { shot: shot.clone(), lines: vec![], selected: None, busy: true, id }));
    let cfg = shared.cfg();
    let lang = cfg.source_lang();
    let up = crate::interface::upscale_for(cfg.interface.upscale, sh);
    let lines = match ocr.recognize((*shot).clone(), up, lang).await {
        Ok(l) => l,
        Err(e) => {
            shared.ui(|u| {
                if let Some(f) = u.freeze.as_mut() {
                    f.busy = false;
                }
                u.notice(format!("Стоп-кадр: распознавание: {e}"));
            });
            return;
        }
    };
    let found = freeze_lines(&lines, lang);
    tracing::info!("стоп-кадр: {} текстов", found.len());
    shared.ui(|u| {
        if let Some(f) = u.freeze.as_mut().filter(|f| f.id == id) {
            f.lines = found;
            f.busy = false;
        }
    });
}

/// Texts on the frozen picture: OCR lines grouped into paragraphs, anything
/// that reads as words of the game's language.
fn freeze_lines(lines: &[ocr::OcrLine], lang: Lang) -> Vec<FreezeLine> {
    let rects: Vec<_> = lines.iter().map(|l| l.rect).collect();
    let mut out = Vec::new();
    for idx in layout::group_lines(&rects) {
        let block: Vec<ocr::OcrLine> = idx.iter().map(|&i| lines[i].clone()).collect();
        let t = ocr::subtitle_text(block, lang);
        if lang::is_label(&t, lang) || lang::is_subtitle(&t, lang) {
            out.push(FreezeLine { rect: layout::block_rect(&rects, &idx), src: t, dst: None, pending: false });
        }
    }
    out
}

/// A click on a text of the freeze frame: translate it.
pub async fn freeze_translate(shared: Arc<Shared>, tr: Arc<Translator>, idx: usize) {
    let src = {
        let mut u = shared.ui.lock().unwrap();
        let Some(l) = u.freeze.as_mut().and_then(|f| f.lines.get_mut(idx)) else { return };
        if l.pending || l.dst.is_some() {
            return;
        }
        l.pending = true;
        l.src.clone()
    };
    shared.repaint();
    let cfg = shared.cfg();
    let preset = cfg.active_model().clone();
    let src_fixed = tr.fix_ocr(&src);
    let long = src_fixed.split_whitespace().count() >= 5 || text::han_count(&src_fixed) >= 8;
    let kind = if long { TextKind::Dialogue } else { TextKind::Interface };
    let res = tr.translate(&preset, &cfg.general.game_title, "", &src_fixed, kind, Opts { use_cache: true, use_history: false }, |_| {}).await;
    shared.ui(|u| {
        if let Some(l) = u.freeze.as_mut().and_then(|f| f.lines.get_mut(idx)).filter(|l| l.src == src) {
            l.pending = false;
            l.dst = Some(match &res {
                Ok(o) => o.text.clone(),
                Err(e) => format!("[ошибка: {e}]"),
            });
        }
    });
}

/// «В словарь» on the freeze frame: the pair goes to the user's glossary.
pub fn add_term(shared: &Shared, tr: &Translator, src: &str, dst: &str) {
    let (src, dst) = (src.trim(), dst.trim());
    if src.is_empty() || dst.is_empty() {
        return;
    }
    let path = shared.paths.user_glossary();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let line = wuxia_core::tsv::line(&[src, dst]);
    let (merged, added) = wuxia_core::bundle::merge(&existing, &line);
    if added > 0 && std::fs::write(&path, merged).is_ok() {
        let (n, f) = tr.reload_dictionaries(&shared.cfg().general.profile);
        shared.ui(|u| {
            u.dict_counts = (n, f);
            u.notice(format!("В глоссарий добавлено: {src} → {dst}"));
        });
    } else {
        shared.ui(|u| u.notice(format!("Уже есть в глоссарии: {src}")));
    }
}

// ------------------------------------------------------------ dictionaries

pub fn export_dicts(shared: &Shared, tr: &Translator, path: &std::path::Path) {
    match std::fs::write(path, tr.export_bundle()) {
        Ok(()) => shared.ui(|u| u.notice(format!("Словари сохранены: {}. Этот файл можно отправить другим игрокам.", path.display()))),
        Err(e) => shared.ui(|u| u.notice(format!("Экспорт словарей: {e}"))),
    }
}

pub fn import_dicts(shared: &Shared, tr: &Translator, path: &std::path::Path) {
    let profile = shared.cfg().general.profile.clone();
    let res = std::fs::read_to_string(path).map_err(anyhow::Error::from).and_then(|t| tr.import_bundle(&t, &profile));
    match res {
        Ok(report) => {
            let counts = tr.dict_counts();
            shared.ui(|u| {
                u.dict_counts = counts;
                u.notice(format!("Словари добавлены: {report}. Ваши записи сохранены, повторы пропущены."));
            });
        }
        Err(e) => shared.ui(|u| u.notice(format!("Импорт словарей: {e}"))),
    }
}

// ------------------------------------------------------------ self-check

/// One button in «Проверка»: everything that can go wrong, in one report file
/// (plus the end of today's log) — to read or to send to the developer.
pub async fn diagnose(shared: Arc<Shared>, tr: Arc<Translator>, ocr: Option<Ocr>) {
    let cfg = shared.cfg();
    let mut rows: Vec<(Option<bool>, String)> = Vec::new();
    let mut add = |ok: Option<bool>, s: String| rows.push((ok, s));

    add(None, format!("Wuxia Local {}", crate::build_info()));
    let (sw, sh) = win::screen_size();
    add(None, format!("Экран: {sw}×{sh}"));

    // Text recognition.
    match &ocr {
        None => add(Some(false), "Распознавание текста Windows не запустилось".into()),
        Some(o) => {
            for l in [Lang::En, Lang::Zh] {
                let r = o.check(l).await;
                let needed = l == cfg.source_lang();
                add(
                    Some(r.is_ok() || !needed),
                    match r {
                        Ok(()) => format!("Распознавание ({}) установлено", l.name_ru()),
                        Err(e) => format!("Распознавание ({}): {e}{}", l.name_ru(), if needed { "" } else { " — для текущей игры не нужно" }),
                    },
                );
            }
        }
    }

    // Model.
    let server = shared.ui.lock().unwrap().server.clone();
    let preset = cfg.active_model().clone();
    add(
        Some(server.as_ref().map_or(false, |s| s.is_ready())),
        format!("Нейросеть «{}»: {}", preset.name, server.map(|s| s.label()).unwrap_or_else(|| "не запущена".into())),
    );
    let model_file = std::path::Path::new(&preset.path);
    add(
        Some(model_file.exists()),
        if model_file.exists() {
            let mb = std::fs::metadata(model_file).map(|m| m.len() / 1_048_576).unwrap_or(0);
            format!("Файл модели: {} ({mb} МБ)", preset.path)
        } else {
            "Файл модели ещё не скачан (или путь неверный)".into()
        },
    );
    let (terms, fixes) = tr.dict_counts();
    add(Some(terms > 0), format!("Словари: терминов {terms}, правил исправлений {fixes}"));

    // Game window.
    let class = crate::config::profile(&cfg.general.profile).window_class;
    match win::find_game_window(&cfg.general.window_match, class) {
        None => add(Some(false), format!("Окно игры «{}» не найдено — запустите игру перед проверкой", cfg.general.game_title)),
        Some(g) => {
            add(Some(true), format!("Окно игры: «{}» [{}], {}×{} в {},{}", g.title.trim(), g.class, g.rect.2, g.rect.3, g.rect.0, g.rect.1));
            if g.minimized {
                add(None, "Игра сейчас свёрнута".into());
            }
            let full = g.rect.0 <= 0 && g.rect.1 <= 0 && g.rect.2 >= sw && g.rect.3 >= sh;
            add(None, format!("Игра {}", if full { "на весь экран" } else { "в окне" }));
            if g.topmost {
                add(None, "Окно игры «поверх всех» — перевод поднимается над ним раз в полсекунды".into());
            }
        }
    }
    add(
        Some(!win::exclusive_fullscreen()),
        if win::exclusive_fullscreen() {
            "Сейчас идёт эксклюзивный полноэкранный режим: поверх него перевод не виден. Нужен «Оконный без рамки»".into()
        } else {
            "Эксклюзивного полноэкранного режима нет".into()
        },
    );

    // Overlay and capture.
    add(
        Some(win::find_window(crate::overlay::TITLE).is_some()),
        if win::find_window(crate::overlay::TITLE).is_some() { "Окно перевода поверх игры создано".into() } else { "Окно перевода не создано".into() },
    );
    match capture::capture(PxRect { x: 0, y: 0, w: sw, h: sh }) {
        Ok(f) => {
            let n = (f.bgra.len() / 4).max(1);
            let mut sum = 0u64;
            for p in f.bgra.chunks_exact(4).step_by(97) {
                sum += wuxia_core::image::luma(p[0], p[1], p[2]) as u64;
            }
            let mean = sum as f64 / (n / 97).max(1) as f64;
            add(Some(mean > 2.0), format!("Снимок экрана работает (средняя яркость {mean:.0} из 255){}", if mean > 2.0 { "" } else { " — снимок чёрный!" }));
        }
        Err(e) => add(Some(false), format!("Снимок экрана: {e}")),
    }

    // Hotkeys and data folder.
    let hk = shared.ui.lock().unwrap().hotkey_problems.clone();
    add(Some(hk.is_empty()), if hk.is_empty() { "Горячие клавиши зарегистрированы".into() } else { format!("Горячие клавиши: {}", hk.join("; ")) });
    let probe = shared.paths.data.join("write-test.tmp");
    let writable = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    add(Some(writable), format!("Папка данных {}: {}", shared.paths.data.display(), if writable { "запись работает" } else { "запись НЕ работает" }));

    // Report.
    let ok = rows.iter().filter(|r| r.0 == Some(true)).count();
    let bad: Vec<&String> = rows.iter().filter(|r| r.0 == Some(false)).map(|r| &r.1).collect();
    let checked = ok + bad.len();
    let mut report = String::from("Wuxia Local — самодиагностика\n\n");
    for (st, s) in &rows {
        let mark = match st {
            Some(true) => "[ OK ]",
            Some(false) => "[ !! ]",
            None => "[    ]",
        };
        report.push_str(&format!("{mark} {s}\n"));
    }
    let (date, time) = win::local_date_time();
    let log = shared.paths.data.join("logs").join(format!("wuxia.log.{date}"));
    if let Ok(l) = std::fs::read_to_string(&log) {
        let tail: Vec<&str> = l.lines().rev().take(80).collect();
        report.push_str("\n--- конец сегодняшнего лога ---\n");
        for line in tail.into_iter().rev() {
            report.push_str(line);
            report.push('\n');
        }
    }
    let file = shared.paths.data.join(format!("diagnostics-{date}-{}.txt", time.replace(':', "-")));
    let saved = std::fs::write(&file, &report).is_ok();
    tracing::info!("самодиагностика: {ok} из {checked} в порядке");
    shared.ui(|u| {
        u.notice(format!(
            "Самодиагностика: в порядке {ok} из {checked}.{}{}",
            if bad.is_empty() { String::new() } else { format!(" Проблемы: {}.", bad.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("; ")) },
            if saved { format!(" Отчёт: {}", file.display()) } else { String::new() }
        ));
    });
    if saved {
        shared.send(crate::state::Command::Open(file));
    }
}
