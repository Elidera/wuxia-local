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
}
