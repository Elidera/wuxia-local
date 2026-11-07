//! Interface Mode: on a hotkey, OCR the whole screen and paint Russian text over
//! the Chinese labels (mask + text), keeping the layout.

use crate::capture::{self, PxRect};
use crate::ocr::Ocr;
use crate::state::{InterfaceItem, InterfaceView, Shared};
use crate::translate::{Opts, Translator};
use crate::win;
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
    let screen = win::screen_size();
    let full = PxRect { x: 0, y: 0, w: screen.0, h: screen.1 };
    let frame = match capture::capture(full) {
        Ok(f) => f,
        Err(e) => {
            shared.ui(|u| u.notice(format!("Захват экрана: {e}")));
            return;
        }
    };
    let up = if cfg.interface.upscale == 0 {
        if screen.1 < 1440 {
            2
        } else {
            1
        }
    } else {
        cfg.interface.upscale
    };
    shared.ui(|u| u.interface = Some(InterfaceView { items: vec![], pending: 1, generation: gen }));
    let t0 = Instant::now();
    let lang = cfg.source_lang();
    let lines = match ocr.recognize(frame.clone(), up, lang).await {
        Ok(l) => l,
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
        .filter(|l| wuxia_core::lang::is_label(&l.text, lang))
        .take(cfg.interface.max_lines)
        .map(|l| {
            let bg = frame.ring_color(&l.rect);
            InterfaceItem {
                fg: contrast_text(bg),
                busy: frame.busyness(&l.rect) > 30.0,
                bg,
                rect: l.rect,
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
    shared.ui(|u| u.interface = Some(InterfaceView { items, pending: n_pending, generation: gen }));

    // Auto-hide watcher: the game screen changed (menu closed / scrolled).
    if cfg.interface.auto_hide {
        let sh = shared.clone();
        let base = coarse(&frame);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(400)).await;
                if sh.interface_gen.load(Ordering::SeqCst) != gen {
                    return;
                }
                let Ok(f) = capture::capture(full) else { continue };
                if coarse_diff(&base, &coarse(&f)) > 0.05 {
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
                        tracing::info!("пакет: {} надписей переведу по одной", rest.len());
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
