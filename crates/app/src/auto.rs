//! «Авто: весь экран» — no subtitle region needed.
//!
//! A couple of times per second the whole screen is read; lines are grouped
//! into blocks and only blocks that read like speech (subtitles, dialogue,
//! tooltips, quest text) are kept. A block is translated once its text stays
//! the same for two scans (typewriter subtitles have finished), and its
//! translation is shown right under it until the text disappears.

use crate::capture::{self, PxRect};
use crate::ocr::{self, Ocr};
use crate::state::{AutoLine, RecentLine, Shared};
use crate::translate::{Opts, Translator};
use crate::win;
use std::sync::Arc;
use std::time::Instant;
use wuxia_core::image::Rect;
use wuxia_core::{layout, zone};
use wuxia_core::prompt::TextKind;
use wuxia_core::{lang, similarity, text};

struct Active {
    id: u64,
    src: String,
    missed: u32,
}

#[derive(Default)]
pub struct AutoState {
    /// Speech-like texts of the previous scan (for the "stable" check).
    prev: Vec<String>,
    active: Vec<Active>,
    next_id: u64,
    /// Where real dialogue lines appeared (for finding the subtitle area).
    zone_samples: Vec<Rect>,
    zone_announced: bool,
}

/// Lines on screen at once; more is clutter (and model time).
const MAX_LINES: usize = 4;

impl AutoState {
    pub fn clear(&mut self, shared: &Shared) {
        if self.active.is_empty() && self.prev.is_empty() {
            return;
        }
        self.prev.clear();
        self.active.clear();
        shared.ui(|u| u.auto_lines.clear());
    }

    /// Find the subtitle area from where dialogue lines appeared, and use it as
    /// the region for «Только область» — unless the user picked a strip by hand.
    fn learn_zone(&mut self, shared: &Shared) {
        let cfg = shared.cfg();
        let r = cfg.dialogue.region;
        if cfg.dialogue.region_set && r[2] * r[3] < 0.5 {
            return;
        }
        let (sw, sh) = win::screen_size();
        let Some(z) = zone::learn(&self.zone_samples, (sw as f32, sh as f32)) else { return };
        if zone::overlap(z, r) > 0.9 && (z[3] - r[3]).abs() < 0.05 && (z[2] - r[2]).abs() < 0.05 {
            return; // already known
        }
        {
            let mut c = shared.cfg.write().unwrap();
            c.dialogue.region = z;
            c.dialogue.region_set = false; // found automatically: keeps adapting
        }
        shared.save_config();
        tracing::info!("область субтитров найдена автоматически: {z:?} (по {} репликам)", self.zone_samples.len());
        if !self.zone_announced {
            self.zone_announced = true;
            shared.ui(|u| {
                u.notice("Область субтитров найдена автоматически. На слабом ПК выберите «Где искать текст → Только область» — так быстрее.")
            });
        }
    }

    pub async fn scan(&mut self, shared: &Arc<Shared>, tr: &Arc<Translator>, ocr: &Ocr) {
        let cfg = shared.cfg();
        let lang = cfg.source_lang();
        // Only the game's own picture: no taskbar, desktop or other windows.
        let area: PxRect = capture::game_area(&cfg);
        let frame = match capture::capture_clean(shared, area) {
            Ok(f) => f,
            Err(e) => {
                tracing::debug!(target: "dialogue", "авто: захват: {e}");
                return;
            }
        };
        let t0 = Instant::now();
        let debug_frame = shared.ui.lock().unwrap().panel_open.then(|| Arc::new(frame.clone()));
        let mut lines = match ocr.recognize(frame, 1, lang).await {
            Ok(l) => l,
            Err(e) => {
                shared.ui(|u| u.notice(format!("OCR: {e}")));
                return;
            }
        };
        let ocr_ms = t0.elapsed().as_millis();
        for l in lines.iter_mut() {
            l.rect = l.rect.offset(area.x as f32, area.y as f32); // to screen coordinates
        }

        let found = speech_blocks(&lines, lang, tr);

        shared.ui(|u| {
            u.stats.add_ocr(ocr_ms);
            if let Some(f) = debug_frame {
                let id = u.ocr_debug.as_ref().map_or(0, |d| d.id) + 1;
                let text = found.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(" / ");
                u.ocr_debug = Some(crate::state::OcrDebug { frame: f, text, ms: ocr_ms, id });
            }
        });

        // Match with what is already shown; new texts must be stable for two scans.
        for a in self.active.iter_mut() {
            a.missed += 1;
        }
        let mut new: Vec<(String, wuxia_core::image::Rect)> = Vec::new();
        let mut moved: Vec<(u64, wuxia_core::image::Rect)> = Vec::new();
        for (t, r) in &found {
            // The same text read slightly differently (OCR jitter, a button partly
            // covered by an icon) is still the same line: keep its translation.
            let same = |a: &str| similarity::ratio(a, t) > 0.8 || similarity::is_growing(t, a) || similarity::is_growing(a, t);
            if let Some(a) = self.active.iter_mut().find(|a| same(&a.src)) {
                a.missed = 0;
                moved.push((a.id, *r));
                continue;
            }
            if self.prev.iter().any(|p| similarity::ratio(p, t) > 0.95) {
                new.push((t.clone(), *r));
            }
        }
        self.prev = found.into_iter().map(|(t, _)| t).collect();
        self.active.retain(|a| a.missed < 2);
        let room = MAX_LINES.saturating_sub(self.active.len());
        let mut started: Vec<AutoLine> = Vec::new();
        let mut learned = false;
        for (t, r) in new.into_iter().take(room) {
            // Real dialogue (not a menu label or a short hint) teaches where subtitles are.
            if t.split_whitespace().count() >= 6 {
                self.zone_samples.push(r);
                if self.zone_samples.len() > 12 {
                    self.zone_samples.remove(0);
                }
                learned = true;
            }
            self.next_id += 1;
            let id = self.next_id;
            tracing::debug!(target: "dialogue", "авто: {t}");
            self.active.push(Active { id, src: t.clone(), missed: 0 });
            started.push(AutoLine { id, rect: r, source: t.clone(), speaker: None, translation: String::new(), pending: true });
            tokio::spawn(translate_line(shared.clone(), tr.clone(), id, t));
        }
        if learned {
            self.learn_zone(shared);
        }
        let alive: Vec<u64> = self.active.iter().map(|a| a.id).collect();
        shared.ui(|u| {
            u.auto_lines.retain(|l| alive.contains(&l.id));
            for (id, r) in &moved {
                if let Some(l) = u.auto_lines.iter_mut().find(|l| l.id == *id) {
                    l.rect = *r;
                }
            }
            u.auto_lines.extend(started);
        });
    }
}

/// Speech-like text blocks on a whole-screen OCR result, as they will be
/// translated: "Name: text" when a speaker name sits above the line.
/// Shared by the game loop and the screenshot tests (`--selftest`).
pub fn speech_blocks(lines: &[ocr::OcrLine], lang: lang::Lang, tr: &Translator) -> Vec<(String, wuxia_core::image::Rect)> {
    let rects: Vec<_> = lines.iter().map(|l| l.rect).collect();
    let blocks = layout::group_lines(&rects);
    let brects: Vec<_> = blocks.iter().map(|idx| layout::block_rect(&rects, idx)).collect();
    let mut found: Vec<(String, wuxia_core::image::Rect)> = Vec::new();
    for (bi, idx) in blocks.iter().enumerate() {
        let block: Vec<ocr::OcrLine> = idx.iter().map(|&i| lines[i].clone()).collect();
        let t = tr.fix_ocr(&ocr::subtitle_text(block, lang));
        if !lang::is_subtitle(&t, lang) || tr.should_skip(&t) {
            continue;
        }
        let r = brects[bi];
        // A speaker name often sits a bit higher above the line (different colour
        // and size, so it is a separate block): a single short name-like line
        // centred above the speech within ~2.5 line heights.
        let speaker = blocks.iter().enumerate().find_map(|(si, sidx)| {
            if si == bi || sidx.len() != 1 {
                return None;
            }
            let sr = brects[si];
            let line_h = rects[idx[0]].h.max(1.0);
            let gap = r.y - (sr.y + sr.h);
            let centred = ((sr.x + sr.w / 2.0) - (r.x + r.w / 2.0)).abs() < r.w / 2.0;
            let name = lines[sidx[0]].text.trim();
            let ok = gap >= -2.0 && gap < 2.5 * line_h && centred && match lang {
                lang::Lang::En => text::looks_like_speaker_en(name) && name.split_whitespace().count() <= 3,
                lang::Lang::Zh => (1..=6).contains(&text::han_count(name)) && text::han_ratio(name) > 0.8,
            };
            ok.then(|| name.to_string())
        });
        match speaker {
            Some(name) if !t.contains(':') && !t.contains('：') => {
                found.push((format!("{name}: {t}"), r));
            }
            _ => found.push((t, r)),
        }
    }
    found
}

async fn translate_line(shared: Arc<Shared>, tr: Arc<Translator>, id: u64, full: String) {
    let cfg = shared.cfg();
    let preset = cfg.active_model().clone();
    let (speaker, body) = if cfg.dialogue.split_speaker {
        let (s, b) = text::split_speaker(&full);
        (s.map(|s| tr.canonical_name(s)), b.to_string())
    } else {
        (None, full.clone())
    };
    let game = cfg.general.game_title.clone();
    let speaker_ru = match &speaker {
        Some(s) => tr
            .translate(&preset, &game, "", s, TextKind::Name, Opts { use_cache: true, use_history: false }, |_| {})
            .await
            .map(|o| o.text)
            .ok()
            .or_else(|| Some(s.clone())),
        None => None,
    };
    let sh = shared.clone();
    let res = tr
        .translate_as(&preset, &game, &cfg.dialogue.style_hint, speaker.as_deref(), &body, TextKind::Dialogue, Opts::default(), move |partial| {
            let p = partial.to_string();
            sh.ui(|u| {
                if let Some(l) = u.auto_lines.iter_mut().find(|l| l.id == id) {
                    l.translation = p;
                }
            });
        })
        .await;
    shared.ui(|u| {
        let line = u.auto_lines.iter_mut().find(|l| l.id == id);
        match &res {
            Ok(o) => {
                if let Some(l) = line {
                    l.translation = o.text.clone();
                    l.speaker = speaker_ru.clone();
                    l.pending = false;
                }
                u.stats.add_llm(o.ms, o.cached);
                u.push_recent(RecentLine { src: body.clone(), dst: o.text.clone(), model: preset.id.clone(), ms: o.ms, cached: o.cached });
            }
            Err(e) => {
                if let Some(l) = line {
                    l.pending = false;
                }
                tracing::warn!("авто-перевод: {e}");
            }
        }
    });
}
