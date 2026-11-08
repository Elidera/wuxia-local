//! Dialogue Mode: watch the subtitle strip, OCR settled lines, translate, show.

use crate::capture::{self, PxRect};
use crate::ocr::{self, Ocr};
use crate::state::{DialogueView, RecentLine, Shared};
use crate::translate::{Opts, Translator};
use crate::win;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use wuxia_core::detect::{Event, Fingerprint, TextColor, WatchConfig, Watcher};
use wuxia_core::prompt::TextKind;
use wuxia_core::{lang, similarity, text};

fn watch_cfg(c: &crate::config::Dialogue) -> WatchConfig {
    WatchConfig {
        change_threshold: c.change_threshold,
        stable_frames: c.stable_frames.max(1),
        clear_frames: c.clear_frames.max(1),
        ..WatchConfig::default()
    }
}

pub async fn run(shared: Arc<Shared>, tr: Arc<Translator>, ocr: Ocr) {
    let mut cfg = shared.cfg();
    let mut watcher = Watcher::new(watch_cfg(&cfg.dialogue));
    let mut last_text = String::new();
    let mut current: Option<JoinHandle<()>> = None;
    let mut last_cfg_check = Instant::now();
    let mut fail_count = 0u32;
    let mut auto = crate::auto::AutoState::default();
    let mut last_auto = Instant::now();
    let mut last_excl_check = Instant::now();
    let mut excl_warned = false;

    loop {
        tokio::time::sleep(Duration::from_millis(cfg.dialogue.interval_ms.clamp(50, 2000))).await;

        if last_cfg_check.elapsed() > Duration::from_millis(500) {
            cfg = shared.cfg();
            last_cfg_check = Instant::now();
        }
        if shared.reset_watcher.swap(false, Ordering::SeqCst) {
            watcher = Watcher::new(watch_cfg(&cfg.dialogue));
            last_text.clear();
            tr.clear_history();
        }
        let (on, selecting) = {
            let u = shared.ui.lock().unwrap();
            (u.dialogue_on, u.selecting_region)
        };
        let auto_mode = cfg.dialogue.mode == "auto";
        if !on || selecting || !auto_mode {
            auto.clear(&shared);
        }
        if !on || selecting {
            continue;
        }
        let pause_on = cfg.general.pause_when_unfocused && !cfg.general.window_match.trim().is_empty();
        if !pause_on && shared.ui.lock().unwrap().paused_unfocused {
            shared.ui(|u| u.paused_unfocused = false); // the setting was just turned off
        }
        if pause_on {
            let focused = win::game_in_foreground(&cfg.general.window_match, crate::config::profile(&cfg.general.profile).window_class);
            let was = shared.ui.lock().unwrap().paused_unfocused;
            if focused == was {
                shared.ui(|u| u.paused_unfocused = !focused);
            }
            if !focused {
                auto.clear(&shared);
                continue;
            }
        }
        if last_excl_check.elapsed() > Duration::from_secs(3) {
            last_excl_check = Instant::now();
            let excl = win::exclusive_fullscreen();
            if excl && !excl_warned {
                shared.ui(|u| {
                    u.notice(
                        "Игра в эксклюзивном полноэкранном режиме: поверх неё Windows не показывает ничего. \
                         В настройках графики игры выберите «Оконный без рамки» (Borderless / Windowed Fullscreen).",
                    )
                });
            }
            excl_warned = excl;
        }

        if auto_mode {
            if last_auto.elapsed() >= Duration::from_millis(cfg.dialogue.auto_interval_ms.clamp(250, 3000)) {
                last_auto = Instant::now();
                auto.scan(&shared, &tr, &ocr).await;
            }
            continue;
        }

        let rect = PxRect::from_fraction(cfg.dialogue.region, win::screen_size());
        let frame = match capture::capture_clean(&shared, rect) {
            Ok(f) => {
                fail_count = 0;
                f
            }
            Err(e) => {
                fail_count += 1;
                if fail_count == 20 {
                    shared.ui(|u| u.notice(format!("Захват экрана не работает: {e}")));
                }
                continue;
            }
        };
        let color = if cfg.dialogue.text_color == "dark" { TextColor::Dark } else { TextColor::Light };
        let fp = Fingerprint::of(&frame, color, cfg.dialogue.ink_threshold);
        match watcher.feed(fp) {
            Event::Cleared => {
                last_text.clear();
                shared.ui(|u| {
                    if u.dialogue.as_ref().map_or(false, |d| !d.pending) {
                        u.dialogue = None;
                    }
                });
            }
            Event::Ready => {
                let t0 = Instant::now();
                let pre = if cfg.dialogue.preprocess == "contrast" { frame.contrast_gray() } else { frame };
                let debug_frame = shared.ui.lock().unwrap().panel_open.then(|| std::sync::Arc::new(pre.clone()));
                let lang = cfg.source_lang();
                let lines = match ocr.recognize(pre, cfg.dialogue.upscale, lang).await {
                    Ok(l) => l,
                    Err(e) => {
                        shared.ui(|u| u.notice(format!("OCR: {e}")));
                        continue;
                    }
                };
                let ocr_ms = t0.elapsed().as_millis();
                let raw_text: String = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" / ");
                let txt = tr.fix_ocr(&ocr::subtitle_text(lines, lang));
                shared.ui(|u| {
                    u.stats.add_ocr(ocr_ms);
                    if let Some(f) = debug_frame {
                        let id = u.ocr_debug.as_ref().map_or(0, |d| d.id) + 1;
                        u.ocr_debug = Some(crate::state::OcrDebug { frame: f, text: raw_text, ms: ocr_ms, id });
                    }
                });
                // OCR turns other scripts and icons into garbage: require real text in the game language.
                if !lang::is_subtitle(&txt, lang) {
                    continue;
                }
                let growing = similarity::is_growing(&last_text, &txt);
                if !growing && similarity::ratio(&txt, &last_text) > 0.85 {
                    continue; // same line, OCR jitter
                }
                tracing::debug!(target: "dialogue", "OCR {ocr_ms} мс: {txt}");
                last_text = txt.clone();
                if let Some(h) = current.take() {
                    h.abort();
                }
                let seq = shared.next_dialogue_seq();
                current = Some(tokio::spawn(translate_line(shared.clone(), tr.clone(), seq, txt)));
            }
            Event::Changing | Event::Idle => {}
        }
    }
}

async fn translate_line(shared: Arc<Shared>, tr: Arc<Translator>, seq: u64, full: String) {
    let cfg = shared.cfg();
    let preset = cfg.active_model().clone();
    let (speaker, body) = if cfg.dialogue.split_speaker {
        let (s, b) = text::split_speaker(&full);
        (s.map(str::to_string), b.to_string())
    } else {
        (None, full.clone())
    };

    shared.ui(|u| {
        u.dialogue = Some(DialogueView {
            source: full.clone(),
            speaker: None,
            translation: String::new(),
            pending: true,
            error: None,
            from_cache: false,
        })
    });

    let game = cfg.general.game_title.clone();
    let style = cfg.dialogue.style_hint.clone();

    // Speaker names repeat constantly: translate once, then cache/glossary.
    let speaker_ru = match &speaker {
        Some(s) => tr
            .translate(&preset, &game, "", s, TextKind::Name, Opts { use_cache: true, use_history: false }, |_| {})
            .await
            .map(|o| o.text)
            .ok()
            .or_else(|| Some(s.clone())),
        None => None,
    };
    if !shared.dialogue_is_current(seq) {
        return;
    }
    shared.ui(|u| {
        if let Some(d) = u.dialogue.as_mut() {
            d.speaker = speaker_ru.clone();
        }
    });

    let sh = shared.clone();
    let res = tr
        .translate(&preset, &game, &style, &body, TextKind::Dialogue, Opts::default(), move |partial| {
            if sh.dialogue_is_current(seq) {
                let p = partial.to_string();
                sh.ui(|u| {
                    if let Some(d) = u.dialogue.as_mut() {
                        d.translation = p;
                    }
                });
            }
        })
        .await;
    if !shared.dialogue_is_current(seq) {
        return; // a newer line already replaced this one
    }
    shared.ui(|u| {
        match &res {
            Ok(o) => {
                if let Some(d) = u.dialogue.as_mut() {
                    d.translation = o.text.clone();
                    d.pending = false;
                    d.from_cache = o.cached;
                }
                u.stats.add_llm(o.ms, o.cached);
                u.push_recent(RecentLine { src: body.clone(), dst: o.text.clone(), model: preset.id.clone(), ms: o.ms, cached: o.cached });
            }
            Err(e) => {
                if let Some(d) = u.dialogue.as_mut() {
                    d.pending = false;
                    d.error = Some(e.to_string());
                }
                tracing::warn!("перевод: {e}");
            }
        }
    });
}
