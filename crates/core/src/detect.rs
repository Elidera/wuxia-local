//! Cheap "did the subtitle change?" detection, run on every captured frame so
//! that the expensive OCR only runs when a new, settled line is on screen.
//!
//! The fingerprint looks only at *text-like* pixels: bright, unsaturated pixels
//! that have a strong dark neighbour (glyph + outline/shadow). That way a moving
//! 3D scene behind the subtitles barely affects it, while a new line does.

use crate::image::{luma, Frame};

pub const GRID_W: usize = 48;
pub const GRID_H: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextColor {
    /// Light text on dark/outlined background (most subtitles).
    Light,
    /// Dark text on light background (parchment-style UI, letters).
    Dark,
}

#[derive(Debug, Clone)]
pub struct Fingerprint {
    cells: [f32; GRID_W * GRID_H],
    /// Fraction of ink pixels in the whole region.
    pub ink: f32,
}

impl Fingerprint {
    pub fn of(frame: &Frame, color: TextColor, threshold: u8) -> Fingerprint {
        let (w, h) = (frame.width as usize, frame.height as usize);
        let mut cells = [0f32; GRID_W * GRID_H];
        let mut counts = [0f32; GRID_W * GRID_H];
        if w < 3 || h < 1 {
            return Fingerprint { cells, ink: 0.0 };
        }
        let step = if w * h > 400_000 { 2 } else { 1 };
        let lum_at = |x: usize, y: usize| {
            let i = (y * w + x) * 4;
            luma(frame.bgra[i], frame.bgra[i + 1], frame.bgra[i + 2])
        };
        let mut ink_total = 0f32;
        let mut n_total = 0f32;
        for y in (0..h).step_by(step) {
            let cy = y * GRID_H / h;
            for x in (2..w - 2).step_by(step) {
                let i = (y * w + x) * 4;
                let (b, g, r) = (frame.bgra[i], frame.bgra[i + 1], frame.bgra[i + 2]);
                let l = luma(b, g, r);
                let sat = r.max(g).max(b) - r.min(g).min(b);
                let is_ink = match color {
                    TextColor::Light => {
                        l >= threshold && sat < 60 && {
                            let lo = lum_at(x - 2, y).min(lum_at(x + 2, y));
                            lo < threshold.saturating_sub(90)
                        }
                    }
                    TextColor::Dark => {
                        l <= threshold && {
                            let hi = lum_at(x - 2, y).max(lum_at(x + 2, y));
                            hi > threshold.saturating_add(90)
                        }
                    }
                };
                let c = cy * GRID_W + x * GRID_W / w;
                counts[c] += 1.0;
                n_total += 1.0;
                if is_ink {
                    cells[c] += 1.0;
                    ink_total += 1.0;
                }
            }
        }
        for (v, n) in cells.iter_mut().zip(counts.iter()) {
            if *n > 0.0 {
                *v /= *n;
            }
        }
        Fingerprint { cells, ink: if n_total > 0.0 { ink_total / n_total } else { 0.0 } }
    }

    /// Mean absolute difference of ink density, normalised so that ~0.01 is noise
    /// and a different line of text is typically > 0.05.
    pub fn diff(&self, o: &Fingerprint) -> f32 {
        let s: f32 = self.cells.iter().zip(o.cells.iter()).map(|(a, b)| (a - b).abs()).sum();
        s / (GRID_W * GRID_H) as f32 * 4.0
    }
}

#[derive(Debug, Clone)]
pub struct WatchConfig {
    /// Fingerprint difference considered a change.
    pub change_threshold: f32,
    /// Consecutive unchanged frames before the text is considered settled.
    pub stable_frames: u32,
    /// Minimal ink fraction for "there is text".
    pub min_ink: f32,
    /// Consecutive empty frames before the translation is hidden.
    pub clear_frames: u32,
    /// Fire anyway after this many changing frames (busy background).
    pub max_changing_frames: u32,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self { change_threshold: 0.03, stable_frames: 2, min_ink: 0.004, clear_frames: 4, max_changing_frames: 10 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Nothing to do.
    Idle,
    /// Text is changing (appearing / typewriter / fading): wait.
    Changing,
    /// A new settled text is on screen: run OCR now.
    Ready,
    /// Subtitle disappeared: hide translation.
    Cleared,
}

pub struct Watcher {
    cfg: WatchConfig,
    last: Option<Fingerprint>,
    processed: Option<Fingerprint>,
    stable: u32,
    changing: u32,
    empty: u32,
    shown: bool,
}

impl Watcher {
    pub fn new(cfg: WatchConfig) -> Self {
        Self { cfg, last: None, processed: None, stable: 0, changing: 0, empty: 0, shown: false }
    }

    /// Forget what was processed (e.g. after the user toggled the mode) so the
    /// current line is OCR'd again.
    pub fn reset(&mut self) {
        self.processed = None;
        self.stable = 0;
        self.changing = 0;
    }

    pub fn feed(&mut self, fp: Fingerprint) -> Event {
        if fp.ink < self.cfg.min_ink {
            self.empty += 1;
            self.stable = 0;
            self.changing = 0;
            self.last = Some(fp);
            if self.shown && self.empty >= self.cfg.clear_frames {
                self.shown = false;
                self.processed = None;
                return Event::Cleared;
            }
            return Event::Idle;
        }
        self.empty = 0;
        let d = self.last.as_ref().map_or(1.0, |l| l.diff(&fp));
        let fresh = self.processed.as_ref().map_or(true, |p| p.diff(&fp) > self.cfg.change_threshold);
        self.last = Some(fp.clone());
        if d > self.cfg.change_threshold {
            self.stable = 0;
            self.changing += 1;
            if self.changing >= self.cfg.max_changing_frames && fresh {
                self.changing = 0;
                return self.fire(fp);
            }
            return Event::Changing;
        }
        self.changing = 0;
        self.stable += 1;
        if self.stable >= self.cfg.stable_frames && fresh {
            return self.fire(fp);
        }
        Event::Idle
    }

    fn fire(&mut self, fp: Fingerprint) -> Event {
        self.processed = Some(fp);
        self.shown = true;
        Event::Ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draw white "glyph" bars with black outline at given columns.
    fn frame_with_text(cols: &[usize]) -> Frame {
        let (w, h) = (200u32, 40u32);
        let mut px = vec![60u8; (w * h * 4) as usize];
        for y in 10..30 {
            for &c in cols {
                for x in c..c + 6 {
                    let lum = if x == c || x == c + 5 { 0 } else { 255 };
                    let i = ((y * w + x as u32) * 4) as usize;
                    px[i..i + 3].copy_from_slice(&[lum, lum, lum]);
                }
            }
        }
        Frame::new(w, h, px)
    }

    #[test]
    fn detects_settled_new_text_and_clear() {
        let mut wt = Watcher::new(WatchConfig::default());
        let fp = |cols: &[usize]| Fingerprint::of(&frame_with_text(cols), TextColor::Light, 200);
        let empty = fp(&[]);
        assert!(empty.ink < 0.004);
        let a = [10, 30, 50, 70, 90];
        assert_eq!(wt.feed(fp(&a)), Event::Changing); // first sight
        assert_eq!(wt.feed(fp(&a)), Event::Idle);
        assert_eq!(wt.feed(fp(&a)), Event::Ready); // stable for 2 frames
        assert_eq!(wt.feed(fp(&a)), Event::Idle); // already processed
        let b = [110, 130, 150, 170, 185];
        assert_eq!(wt.feed(fp(&b)), Event::Changing);
        wt.feed(fp(&b));
        assert_eq!(wt.feed(fp(&b)), Event::Ready);
        for _ in 0..3 {
            assert_eq!(wt.feed(empty.clone()), Event::Idle);
        }
        assert_eq!(wt.feed(empty.clone()), Event::Cleared);
    }
}
