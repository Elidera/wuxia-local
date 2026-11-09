//! Interface Mode: on a hotkey, OCR the whole screen and paint Russian text over
//! the Chinese labels (mask + text), keeping the layout.

use crate::capture::{self, PxRect};
use crate::ocr::Ocr;
use crate::state::{InterfaceItem, InterfaceView, Shared};
use crate::translate::{Opts, Translator};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wuxia_core::image::{contrast_text, luma, Frame};
use wuxia_core::prompt::TextKind;

/// Coarse 32×18 luminance grid of the whole screen, for auto-hide.
fn coarse(frame: &Frame) -> Vec<f32> {
    const GW: u32 = 32;
    const GH: u32 = 18;
    let mut out = vec![0f32; (GW * GH) as usize];
    for gy in 0..GH {
        for gx in 0..GW {
            let x = gx * frame.width / GW + frame.width / GW / 2;
            let y = gy * frame.height / GH + frame.height / GH / 2;
            let mut s = 0f32;
            for (dx, dy) in [(0, 0), (3, 0), (0, 3), (3, 3)] {
                let p = frame.px((x + dx).min(frame.width - 1), (y + dy).min(frame.height - 1));
                s += luma(p[0], p[1], p[2]) as f32;
            }
            out[(gy * GW + gx) as usize] = s / 4.0 / 255.0;
        }
    }
    out
}

fn coarse_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32
}

/// OCR upscale for F8: small fonts below 1440p read much better at 2×.
pub fn upscale_for(configured: u32, screen_h: i32) -> u32 {
    match configured {
        0 if screen_h < 1440 => 2,
        0 => 1,
        k => k,
    }
}

/// Is this OCR line a label F8 translates? (Shared with `--selftest`.)
pub fn is_label_line(text: &str, lang: wuxia_core::lang::Lang, tr: &Translator) -> bool {
    wuxia_core::lang::is_label(text, lang) && !tr.should_skip(text)
}

pub async fn toggle(shared: Arc<Shared>, tr: Arc<Translator>, ocr: Ocr) {
    // Second press hides the current translation.
    let showing = shared.ui.lock().unwrap().interface.is_some();
    if showing {
        shared.interface_gen.fetch_add(1, Ordering::SeqCst);
        shared.ui(|u| u.interface = None);
        return;
    }
    let gen = shared.interface_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let cfg = shared.cfg();
    // The game's own picture when it is in front (no taskbar or other windows).
    let full: PxRect = capture::game_area(&cfg);
    // «Busy» from the first moment: a second press during the wait below cancels.
    shared.ui(|u| u.interface = Some(InterfaceView { items: vec![], pending: 1, generation: gen }));
    let cancelled = || shared.interface_gen.load(Ordering::SeqCst) != gen;
    // Game menus open with an animation (labels slide and grow): a picture taken
    // mid-animation gives boxes smaller than the final text, and the original
    // then shows around the translation. Wait until two pictures 250 ms apart
    // match (at most ~1.5 s).
    let mut frame = match capture::capture_clean(&shared, full) {
        Ok(f) => f,
        Err(e) => {
            shared.ui(|u| {
                u.interface = None;
                u.notice(format!("Захват экрана: {e}"))
            });
            return;
        }
    };
    for _ in 0..6 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let Ok(next) = capture::capture_clean(&shared, full) else { break };
        let d = coarse_diff(&coarse(&frame), &coarse(&next));
        frame = next;
        if d < 0.012 {
            break;
        }
        tracing::debug!(target: "interface", "экран ещё меняется ({d:.3}), жду");
        if cancelled() {
            return;
        }
    }
    if cancelled() {
        return;
    }
    let up = upscale_for(cfg.interface.upscale, full.h);
    let t0 = Instant::now();
    let lang = cfg.source_lang();
    let lines = match ocr.recognize(frame.clone(), up, lang).await {
        Ok(l) => l,
        Err(_) if cancelled() => return,
        Err(e) => {
            shared.ui(|u| {
                u.interface = None;
                u.notice(format!("OCR: {e}"))
            });
            return;
        }
    };
    let ocr_ms = t0.elapsed().as_millis();
    let items: Vec<InterfaceItem> = lines
        .into_iter()
        .filter(|l| is_label_line(&l.text, lang, &tr))
        .take(cfg.interface.max_lines)
        .map(|l| {
            let bg = frame.ring_color(&l.rect);
            InterfaceItem {
                fg: contrast_text(bg),
                busy: frame.busyness(&l.rect) > 30.0,
                bg,
                rect: l.rect.offset(full.x as f32, full.y as f32), // to screen coordinates
                src: tr.fix_ocr(&l.text),
                dst: None,
            }
        })
        .collect();
    tracing::info!(target: "interface", "OCR экрана: {} строк за {ocr_ms} мс", items.len());
    if items.is_empty() {
        shared.ui(|u| {
            u.interface = None;
            u.notice("На экране не найден текст на языке игры")
        });
        return;
    }

    let preset = cfg.active_model().clone();
    // Instant answers first (cache/glossary/corrections), then the model.
    let mut unique: Vec<String> = Vec::new();
    let mut done: HashMap<String, String> = HashMap::new();
    for it in &items {
        if done.contains_key(&it.src) || unique.contains(&it.src) {
            continue;
        }
        match tr.lookup(&preset.id, &it.src, TextKind::Interface) {
            Some(t) => {
                done.insert(it.src.clone(), t);
            }
            None => unique.push(it.src.clone()),
        }
    }
    let mut items = items;
    for it in items.iter_mut() {
        it.dst = done.get(&it.src).cloned();
    }
    let n_pending = unique.len();
    if cancelled() {
        return; // pressed again while the screen was being read
    }
    shared.ui(|u| u.interface = Some(InterfaceView { items, pending: n_pending, generation: gen }));

    // Auto-hide watcher: the game screen changed (menu closed / scrolled).
    // Everything of ours that was ever on screen (translation labels, the panel
    // while it was closing) is blanked in both pictures, so only a real change
    // of the game screen counts. Menus have animated backgrounds (characters,
    // particles), so the change must be large and last ~1.5 s.
    if cfg.interface.auto_hide {
        let sh = shared.clone();
        let mut base = frame.clone();
        let mut masks: Vec<[i32; 4]> = capture::own_rects(&shared);
        tokio::spawn(async move {
            let mut changed = 0;
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if sh.interface_gen.load(Ordering::SeqCst) != gen {
                    return;
                }
                let Ok(mut f) = capture::capture(full) else { continue };
                for m in capture::own_rects(&sh) {
                    if !masks.contains(&m) {
                        masks.push(m);
                    }
                }
                capture::apply_masks(&mut f, full, &masks);
                capture::apply_masks(&mut base, full, &masks);
                let d = coarse_diff(&coarse(&base), &coarse(&f));
                changed = if d > 0.12 { changed + 1 } else { 0 };
                if changed >= 3 {
                    tracing::info!(target: "interface", "экран изменился ({d:.3}) — прячу перевод");
                    if sh.interface_gen.load(Ordering::SeqCst) == gen {
                        sh.interface_gen.fetch_add(1, Ordering::SeqCst);
                        sh.ui(|u| u.interface = None);
                    }
                    return;
                }
            }
        });
    }

    let game = cfg.general.game_title.clone();
    let set_result = |src: &str, dst: String| {
        shared.ui(|u| {
            if let Some(v) = u.interface.as_mut().filter(|v| v.generation == gen) {
                for it in v.items.iter_mut().filter(|it| it.src == src) {
                    it.dst = Some(dst.clone());
                }
                v.pending = v.pending.saturating_sub(1);
            }
        })
    };
    // Batches of up to 20 labels / ~600 chars: one model call each.
    let mut chunks: Vec<Vec<String>> = Vec::new();
    for src in unique {
        let fits = chunks.last().map_or(false, |c: &Vec<String>| {
            c.len() < 20 && c.iter().map(|s| s.chars().count()).sum::<usize>() + src.chars().count() < 600
        });
        if fits {
            chunks.last_mut().unwrap().push(src);
        } else {
            chunks.push(vec![src]);
        }
    }
    for chunk in chunks {
        if shared.interface_gen.load(Ordering::SeqCst) != gen {
            return; // hidden or replaced
        }
        let mut rest: Vec<String> = chunk.clone();
        if chunk.len() > 1 {
            let t0 = std::time::Instant::now();
            match tr.translate_batch(&preset, &game, &chunk).await {
                Ok(out) => {
                    shared.ui(|u| u.stats.add_llm(t0.elapsed().as_millis() / chunk.len() as u128, false));
                    rest.clear();
                    for (src, dst) in chunk.iter().zip(out) {
                        match dst {
                            Some(d) => set_result(src, d),
                            None => rest.push(src.clone()),
                        }
                    }
                    if !rest.is_empty() {
                        tracing::info!("пакет: {} переведу по одной", wuxia_core::text::count_ru(rest.len(), "надпись", "надписи", "надписей"));
                    }
                }
                Err(e) => tracing::warn!("пакетный перевод экрана: {e}; перевожу по одной"),
            }
        }
        for src in rest {
            if shared.interface_gen.load(Ordering::SeqCst) != gen {
                return;
            }
            let res = tr.translate(&preset, &game, "", &src, TextKind::Interface, Opts { use_cache: true, use_history: false }, |_| {}).await;
            let dst = match res {
                Ok(o) => {
                    shared.ui(|u| u.stats.add_llm(o.ms, o.cached));
                    o.text
                }
                Err(e) => {
                    tracing::warn!("UI-перевод «{src}»: {e}");
                    src.clone()
                }
            };
            set_result(&src, dst);
        }
    }
}
