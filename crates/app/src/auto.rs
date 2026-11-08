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
use wuxia_core::layout;
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

    pub async fn scan(&mut self, shared: &Arc<Shared>, tr: &Arc<Translator>, ocr: &Ocr) {
        let cfg = shared.cfg();
        let lang = cfg.source_lang();
        let (sw, sh) = win::screen_size();
        let full = PxRect { x: 0, y: 0, w: sw, h: sh };
        let frame = match capture::capture_clean(shared, full) {
            Ok(f) => f,
            Err(e) => {
                tracing::debug!(target: "dialogue", "авто: захват: {e}");
                return;
            }
        };
        let t0 = Instant::now();
        let debug_frame = shared.ui.lock().unwrap().panel_open.then(|| Arc::new(frame.clone()));
        let lines = match ocr.recognize(frame, 1, lang).await {
            Ok(l) => l,
            Err(e) => {
                shared.ui(|u| u.notice(format!("OCR: {e}")));
                return;
            }
        };
        let ocr_ms = t0.elapsed().as_millis();

        // Blocks of speech-like text.
        let rects: Vec<_> = lines.iter().map(|l| l.rect).collect();
        let blocks = layout::group_lines(&rects);
        let brects: Vec<_> = blocks.iter().map(|idx| layout::block_rect(&rects, idx)).collect();
        let mut found: Vec<(String, wuxia_core::image::Rect)> = Vec::new();
        for (bi, idx) in blocks.iter().enumerate() {
            let block: Vec<ocr::OcrLine> = idx.iter().map(|&i| lines[i].clone()).collect();
            let t = tr.fix_ocr(&ocr::subtitle_text(block, lang));
            if !lang::is_subtitle(&t, lang) {
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
                    lang::Lang::En => text::looks_like_name_en(name) && name.split_whitespace().count() <= 3,
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
            if let Some(a) = self.active.iter_mut().find(|a| similarity::ratio(&a.src, t) > 0.9) {
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
        for (t, r) in new.into_iter().take(room) {
            self.next_id += 1;
            let id = self.next_id;
            tracing::debug!(target: "dialogue", "авто: {t}");
            self.active.push(Active { id, src: t.clone(), missed: 0 });
            started.push(AutoLine { id, rect: r, source: t.clone(), speaker: None, translation: String::new(), pending: true });
            tokio::spawn(translate_line(shared.clone(), tr.clone(), id, t));
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

async fn translate_line(shared: Arc<Shared>, tr: Arc<Translator>, id: u64, full: String) {
    let cfg = shared.cfg();
    let preset = cfg.active_model().clone();
    let (speaker, body) = if cfg.dialogue.split_speaker {
        let (s, b) = text::split_speaker(&full);
        (s.map(str::to_string), b.to_string())
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
        .translate(&preset, &game, &cfg.dialogue.style_hint, &body, TextKind::Dialogue, Opts::default(), move |partial| {
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
