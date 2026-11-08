//! Software drawing for the overlay window.
//!
//! The overlay is a Win32 layered window updated with `UpdateLayeredWindow`,
//! which takes a *premultiplied* BGRA bitmap. Plates and text are composed here
//! in plain Rust (text arrives as a coverage mask rendered by GDI), so all the
//! pixel math is testable on any OS.

/// A colour with straight (not premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

/// A premultiplied BGRA8 picture, `w * h * 4` bytes.
pub struct Canvas<'a> {
    pub w: i32,
    pub h: i32,
    pub px: &'a mut [u8],
}

impl<'a> Canvas<'a> {
    pub fn new(w: i32, h: i32, px: &'a mut [u8]) -> Self {
        assert!(w >= 0 && h >= 0 && px.len() >= (w * h * 4) as usize);
        Self { w, h, px }
    }

    pub fn clear(&mut self) {
        self.px.iter_mut().for_each(|b| *b = 0);
    }

    /// Source-over one pixel with colour `c` at coverage `cov` (0..=1).
    #[inline]
    fn blend(&mut self, x: i32, y: i32, c: Rgba, cov: f32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h || cov <= 0.0 {
            return;
        }
        let a = (c.3 as f32 / 255.0) * cov.min(1.0);
        if a <= 0.0 {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        let inv = 1.0 - a;
        let p = &mut self.px[i..i + 4];
        p[0] = (c.2 as f32 * a + p[0] as f32 * inv).round() as u8;
        p[1] = (c.1 as f32 * a + p[1] as f32 * inv).round() as u8;
        p[2] = (c.0 as f32 * a + p[2] as f32 * inv).round() as u8;
        p[3] = (255.0 * a + p[3] as f32 * inv).round() as u8;
    }

    /// Filled rectangle with anti-aliased rounded corners of radius `r`.
    pub fn fill_round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: f32, c: Rgba) {
        if w <= 0 || h <= 0 {
            return;
        }
        let r = r.max(0.0).min(w.min(h) as f32 / 2.0);
        let (x0, y0) = (x as f32, y as f32);
        let (x1, y1) = (x0 + w as f32, y0 + h as f32);
        for py in y.max(0)..(y + h).min(self.h) {
            for px in x.max(0)..(x + w).min(self.w) {
                let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
                // Distance into the corner circle, if the pixel is in a corner square.
                let dx = if cx < x0 + r { x0 + r - cx } else if cx > x1 - r { cx - (x1 - r) } else { 0.0 };
                let dy = if cy < y0 + r { y0 + r - cy } else if cy > y1 - r { cy - (y1 - r) } else { 0.0 };
                let cov = if r > 0.0 && dx > 0.0 && dy > 0.0 { (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0) } else { 1.0 };
                self.blend(px, py, c, cov);
            }
        }
    }

    /// Paint colour `c` through a coverage mask (0 = nothing, 255 = full) placed at (x, y).
    pub fn draw_mask(&mut self, mask: &[u8], mw: i32, mh: i32, x: i32, y: i32, c: Rgba) {
        if mw <= 0 || mh <= 0 || mask.len() < (mw * mh) as usize {
            return;
        }
        for my in 0..mh {
            let py = y + my;
            if py < 0 || py >= self.h {
                continue;
            }
            for mx in 0..mw {
                let m = mask[(my * mw + mx) as usize];
                if m != 0 {
                    self.blend(x + mx, py, c, m as f32 / 255.0);
                }
            }
        }
    }

    /// Text with a dark outline (game-subtitle look): the mask is stamped in
    /// `shadow` around the position, then in `c` on top.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_mask_outlined(&mut self, mask: &[u8], mw: i32, mh: i32, x: i32, y: i32, c: Rgba, shadow: Rgba, d: i32) {
        let d = d.max(1);
        for (dx, dy) in [(-d, 0), (d, 0), (0, -d), (0, d), (d, d)] {
            self.draw_mask(mask, mw, mh, x + dx, y + dy, shadow);
        }
        self.draw_mask(mask, mw, mh, x, y, c);
    }
}

/// Where a translation plate of `size` goes relative to the original text `anchor`
/// (x, y, w, h in screen pixels): "below", "above" or "over" it, centred, kept on screen.
pub fn place_plate(anchor: [f32; 4], size: (f32, f32), position: &str, screen: (f32, f32), gap: f32) -> [f32; 4] {
    let [ax, ay, aw, ah] = anchor;
    let (pw, ph) = size;
    let top = match position {
        "above" => ay - ph - gap,
        "over" => ay + ah / 2.0 - ph / 2.0,
        _ => ay + ah + gap,
    };
    let top = top.clamp(0.0, (screen.1 - ph).max(0.0));
    let left = (ax + aw / 2.0 - pw / 2.0).clamp(0.0, (screen.0 - pw).max(0.0));
    [left, top, pw, ph]
}

/// Bounding box (x, y, w, h) of integer rectangles, clipped to the screen.
pub fn union_rect(rects: &[[i32; 4]], screen: (i32, i32)) -> Option<[i32; 4]> {
    let mut it = rects.iter().filter(|r| r[2] > 0 && r[3] > 0);
    let f = it.next()?;
    let (mut x0, mut y0, mut x1, mut y1) = (f[0], f[1], f[0] + f[2], f[1] + f[3]);
    for r in it {
        x0 = x0.min(r[0]);
        y0 = y0.min(r[1]);
        x1 = x1.max(r[0] + r[2]);
        y1 = y1.max(r[1] + r[3]);
    }
    let (x0, y0) = (x0.max(0), y0.max(0));
    let (x1, y1) = (x1.min(screen.0), y1.min(screen.1));
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}

// ---------------------------------------------------------------- opaque window

/// Colour key of the overlay window, B, G, R: pixels of exactly this colour are
/// transparent. (A colour-keyed layered window can be hidden from screen
/// capture on Windows 10, a per-pixel-alpha one cannot.)
pub const KEY_BGR: [u8; 3] = [254, 0, 255];

/// Make the premultiplied `canvas` opaque for the colour-keyed window: fully
/// transparent pixels become the key colour; the rest is composed over `bg`
/// (the game picture under the window, same size, BGRA) — or, without a
/// background, made solid.
pub fn flatten(canvas: &[u8], bg: Option<&[u8]>, out: &mut [u8]) {
    for (i, (c, o)) in canvas.chunks_exact(4).zip(out.chunks_exact_mut(4)).enumerate() {
        let a = c[3] as u32;
        if a == 0 {
            o[..3].copy_from_slice(&KEY_BGR);
            o[3] = 255;
            continue;
        }
        let mut px = [0u8; 3];
        for k in 0..3 {
            px[k] = match bg {
                Some(b) if a < 255 => (c[k] as u32 + b[i * 4 + k] as u32 * (255 - a) / 255).min(255) as u8,
                None if a < 255 => (c[k] as u32 * 255 / a).min(255) as u8,
                _ => c[k],
            };
        }
        if px == KEY_BGR {
            px[0] -= 1; // never punch a hole by accident
        }
        o[..3].copy_from_slice(&px);
        o[3] = 255;
    }
}

fn luma(p: &[u8]) -> u8 {
    ((p[2] as u32 * 77 + p[1] as u32 * 150 + p[0] as u32 * 29) >> 8) as u8
}

/// Remove text from a picture (BGRA, `w`×`h`): text pixels — much lighter
/// (or, with `dark_text`, darker) than the picture overall — and `r` pixels
/// around them (outline, shadow, anti-aliasing) are filled in from the
/// surrounding background, row by row, then smoothed vertically.
/// Returns how many pixels were replaced.
pub fn erase_text(px: &mut [u8], w: usize, h: usize, dark_text: bool, r: usize) -> usize {
    if w == 0 || h == 0 || px.len() < w * h * 4 {
        return 0;
    }
    let lum: Vec<u8> = px.chunks_exact(4).take(w * h).map(luma).collect();
    let mut sorted = lum.clone();
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2] as i32;
    let core: Vec<bool> = if dark_text {
        let t = (median - 55).min(90);
        lum.iter().map(|&l| (l as i32) <= t).collect()
    } else {
        let t = (median + 55).max(170);
        lum.iter().map(|&l| (l as i32) >= t).collect()
    };
    // Dilate: horizontal then vertical running window.
    let r = r as isize;
    let mut tmp = vec![false; w * h];
    for y in 0..h {
        let row = &core[y * w..(y + 1) * w];
        // Distance to the nearest core pixel along the row, both directions.
        let mut near = vec![false; w];
        let mut last: isize = -10_000;
        for x in 0..w {
            if row[x] {
                last = x as isize;
            }
            near[x] = x as isize - last <= r;
        }
        last = 10_000_000;
        for x in (0..w).rev() {
            if row[x] {
                last = x as isize;
            }
            if last - x as isize <= r {
                near[x] = true;
            }
        }
        tmp[y * w..(y + 1) * w].copy_from_slice(&near);
    }
    let mut mask = vec![false; w * h];
    for x in 0..w {
        let mut last: isize = -10_000;
        for y in 0..h {
            if tmp[y * w + x] {
                last = y as isize;
            }
            if y as isize - last <= r {
                mask[y * w + x] = true;
            }
        }
        last = 10_000_000;
        for y in (0..h).rev() {
            if tmp[y * w + x] {
                last = y as isize;
            }
            if last - y as isize <= r {
                mask[y * w + x] = true;
            }
        }
    }
    let n = mask.iter().filter(|&&m| m).count();
    if n == 0 {
        return 0;
    }
    // Rows: linear fill between the background pixels on both sides of a run.
    let mut unresolved = vec![false; w * h];
    for y in 0..h {
        let mut x = 0;
        while x < w {
            if !mask[y * w + x] {
                x += 1;
                continue;
            }
            let start = x;
            while x < w && mask[y * w + x] {
                x += 1;
            }
            let (l, rr) = (start.checked_sub(1), (x < w).then_some(x));
            for xi in start..x {
                let i = (y * w + xi) * 4;
                match (l, rr) {
                    (Some(a), Some(b)) => {
                        let t = (xi - a) as f32 / (b - a) as f32;
                        for k in 0..3 {
                            let (pa, pb) = (px[(y * w + a) * 4 + k] as f32, px[(y * w + b) * 4 + k] as f32);
                            px[i + k] = (pa + (pb - pa) * t).round() as u8;
                        }
                    }
                    (Some(a), None) | (None, Some(a)) => {
                        for k in 0..3 {
                            px[i + k] = px[(y * w + a) * 4 + k];
                        }
                    }
                    (None, None) => unresolved[y * w + xi] = true,
                }
            }
        }
    }
    // Rows that were all text: take the nearest resolved row.
    for x in 0..w {
        for y in 0..h {
            if !unresolved[y * w + x] {
                continue;
            }
            let up = (0..y).rev().find(|&yy| !unresolved[yy * w + x]);
            let down = (y + 1..h).find(|&yy| !unresolved[yy * w + x]);
            if let Some(src) = up.or(down) {
                for k in 0..3 {
                    px[(y * w + x) * 4 + k] = px[(src * w + x) * 4 + k];
                }
            }
        }
    }
    // Smooth the filled pixels vertically (rows were filled independently).
    let rad = (r * 2).max(2) as usize;
    let src = px.to_vec();
    for x in 0..w {
        for y in 0..h {
            if !mask[y * w + x] {
                continue;
            }
            let (y0, y1) = (y.saturating_sub(rad), (y + rad).min(h - 1));
            let mut acc = [0u32; 3];
            for yy in y0..=y1 {
                for k in 0..3 {
                    acc[k] += src[(yy * w + x) * 4 + k] as u32;
                }
            }
            let cnt = (y1 - y0 + 1) as u32;
            for k in 0..3 {
                px[(y * w + x) * 4 + k] = (acc[k] / cnt) as u8;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_and_mask_blend_premultiplied() {
        let mut buf = vec![0u8; 10 * 10 * 4];
        let mut c = Canvas::new(10, 10, &mut buf);
        c.fill_round_rect(0, 0, 10, 10, 0.0, Rgba(0, 0, 0, 128));
        // Black at half alpha: colour 0, alpha 128.
        assert_eq!(&c.px[0..4], &[0, 0, 0, 128]);
        let mask = vec![255u8; 4];
        c.draw_mask(&mask, 2, 2, 4, 4, Rgba(255, 255, 255, 255));
        let i = ((4 * 10 + 4) * 4) as usize;
        assert_eq!(&c.px[i..i + 4], &[255, 255, 255, 255]);
        // Outside the mask the plate is untouched.
        assert_eq!(&c.px[4..8], &[0, 0, 0, 128]);
        // Premultiplied: no channel above alpha anywhere.
        assert!(c.px.chunks(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
    }

    #[test]
    fn rounded_corner_is_transparent_and_clipping_is_safe() {
        let mut buf = vec![0u8; 20 * 20 * 4];
        let mut c = Canvas::new(20, 20, &mut buf);
        c.fill_round_rect(-5, -5, 40, 40, 0.0, Rgba(10, 20, 30, 255)); // larger than canvas
        c.clear();
        c.fill_round_rect(0, 0, 20, 20, 8.0, Rgba(255, 0, 0, 255));
        assert_eq!(c.px[3], 0, "corner pixel stays transparent");
        let mid = ((10 * 20 + 10) * 4) as usize;
        assert_eq!(&c.px[mid..mid + 4], &[0, 0, 255, 255]); // BGRA red
        c.draw_mask(&[255; 9], 3, 3, 18, 18, Rgba(0, 255, 0, 255)); // partly outside
    }

    #[test]
    fn plate_placement() {
        let s = (1920.0, 1080.0);
        let p = place_plate([800.0, 900.0, 320.0, 40.0], (400.0, 60.0), "below", s, 6.0);
        assert_eq!(p, [760.0, 946.0, 400.0, 60.0]);
        // Would run off the bottom: clamped.
        let p = place_plate([800.0, 1050.0, 320.0, 20.0], (400.0, 60.0), "below", s, 6.0);
        assert_eq!(p[1], 1020.0);
        let p = place_plate([0.0, 100.0, 50.0, 20.0], (400.0, 60.0), "above", s, 6.0);
        assert_eq!((p[0], p[1]), (0.0, 34.0));
    }

    #[test]
    fn union_clips() {
        assert_eq!(union_rect(&[], (100, 100)), None);
        assert_eq!(union_rect(&[[10, 10, 5, 5], [50, 40, 100, 10]], (100, 100)), Some([10, 10, 90, 40]));
        assert_eq!(union_rect(&[[-10, -10, 5, 5]], (100, 100)), None);
    }

    #[test]
    fn flatten_keys_and_composes() {
        let canvas = [0, 0, 0, 0, 0, 0, 0, 128, 10, 20, 30, 255];
        let bg = [100, 100, 100, 255, 200, 200, 200, 255, 0, 0, 0, 255];
        let mut out = [0u8; 12];
        flatten(&canvas, Some(&bg), &mut out);
        assert_eq!(&out[0..3], &KEY_BGR);
        assert_eq!(out[4], (200 * 127 / 255) as u8); // black at half alpha over grey
        assert_eq!(&out[8..11], &[10, 20, 30]);
        flatten(&canvas, None, &mut out);
        assert_eq!(&out[4..7], &[0, 0, 0]);
    }

    #[test]
    fn erase_text_fills_from_background() {
        // Horizontal gradient background with a white "word" in the middle.
        let (w, h) = (60usize, 20usize);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let v = 40 + x as u8;
                let i = (y * w + x) * 4;
                px[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let orig = px.clone();
        for y in 7..13 {
            for x in 20..40 {
                let i = (y * w + x) * 4;
                px[i..i + 3].copy_from_slice(&[250, 250, 250]);
            }
        }
        let n = erase_text(&mut px, w, h, false, 2);
        assert!(n >= 6 * 20, "erased {n}");
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                assert!((px[i] as i32 - orig[i] as i32).abs() <= 3, "pixel {x},{y}: {} vs {}", px[i], orig[i]);
            }
        }
        // Nothing to erase on a plain picture.
        let mut flat = orig.clone();
        assert_eq!(erase_text(&mut flat, w, h, false, 2), 0);
        assert_eq!(flat, orig);
    }
}
