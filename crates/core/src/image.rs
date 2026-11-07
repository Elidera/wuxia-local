//! Small image helpers on raw BGRA8 buffers (as produced by GDI / WGC capture).

#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Tightly packed BGRA8, `width * height * 4` bytes.
    pub bgra: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{})", self.width, self.height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn union(&self, o: &Rect) -> Rect {
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.w).max(o.x + o.w);
        let y1 = (self.y + self.h).max(o.y + o.h);
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
    pub fn scale(&self, k: f32) -> Rect {
        Rect { x: self.x * k, y: self.y * k, w: self.w * k, h: self.h * k }
    }
    pub fn offset(&self, dx: f32, dy: f32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..*self }
    }
}

#[inline]
pub fn luma(b: u8, g: u8, r: u8) -> u8 {
    ((r as u32 * 77 + g as u32 * 150 + b as u32 * 29) >> 8) as u8
}

impl Frame {
    pub fn new(width: u32, height: u32, bgra: Vec<u8>) -> Self {
        debug_assert_eq!(bgra.len(), (width * height * 4) as usize);
        Self { width, height, bgra }
    }

    #[inline]
    pub fn px(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.bgra[i], self.bgra[i + 1], self.bgra[i + 2], self.bgra[i + 3]]
    }

    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Frame {
        let x = x.min(self.width);
        let y = y.min(self.height);
        let w = w.min(self.width - x);
        let h = h.min(self.height - y);
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in y..y + h {
            let s = ((row * self.width + x) * 4) as usize;
            out.extend_from_slice(&self.bgra[s..s + (w * 4) as usize]);
        }
        Frame::new(w, h, out)
    }

    /// Bilinear upscale by an integer factor (OCR engines read small game fonts
    /// much better at 2×).
    pub fn upscale(&self, k: u32) -> Frame {
        if k <= 1 || self.width == 0 || self.height == 0 {
            return self.clone();
        }
        let (w, h) = (self.width * k, self.height * k);
        let mut out = vec![0u8; (w * h * 4) as usize];
        let kf = k as f32;
        for y in 0..h {
            let sy = ((y as f32 + 0.5) / kf - 0.5).max(0.0);
            let y0 = (sy as u32).min(self.height - 1);
            let y1 = (y0 + 1).min(self.height - 1);
            let fy = sy - y0 as f32;
            for x in 0..w {
                let sx = ((x as f32 + 0.5) / kf - 0.5).max(0.0);
                let x0 = (sx as u32).min(self.width - 1);
                let x1 = (x0 + 1).min(self.width - 1);
                let fx = sx - x0 as f32;
                let (a, b, c, d) = (self.px(x0, y0), self.px(x1, y0), self.px(x0, y1), self.px(x1, y1));
                let o = ((y * w + x) * 4) as usize;
                for ch in 0..4 {
                    let top = a[ch] as f32 * (1.0 - fx) + b[ch] as f32 * fx;
                    let bot = c[ch] as f32 * (1.0 - fx) + d[ch] as f32 * fx;
                    out[o + ch] = (top * (1.0 - fy) + bot * fy).round() as u8;
                }
            }
        }
        Frame::new(w, h, out)
    }

    /// Grayscale with linear contrast stretch (2nd–98th percentile), kept as BGRA.
    /// Helps with the semi-transparent subtitle plates of many games.
    pub fn contrast_gray(&self) -> Frame {
        let n = (self.width * self.height) as usize;
        let mut lum = Vec::with_capacity(n);
        let mut hist = [0u32; 256];
        for p in self.bgra.chunks_exact(4) {
            let l = luma(p[0], p[1], p[2]);
            hist[l as usize] += 1;
            lum.push(l);
        }
        let pct = |q: f32| {
            let target = (n as f32 * q) as u32;
            let mut acc = 0;
            for (i, &c) in hist.iter().enumerate() {
                acc += c;
                if acc >= target {
                    return i as f32;
                }
            }
            255.0
        };
        let (lo, hi) = (pct(0.02), pct(0.98));
        let span = (hi - lo).max(1.0);
        let mut out = Vec::with_capacity(n * 4);
        for l in lum {
            let v = (((l as f32 - lo) / span) * 255.0).clamp(0.0, 255.0) as u8;
            out.extend_from_slice(&[v, v, v, 255]);
        }
        Frame::new(self.width, self.height, out)
    }

    /// Median colour of a 2-px ring just outside `r` (clamped to the frame):
    /// the background a text mask should be filled with.
    pub fn ring_color(&self, r: &Rect) -> [u8; 3] {
        let x0 = (r.x - 2.0).max(0.0) as u32;
        let y0 = (r.y - 2.0).max(0.0) as u32;
        let x1 = ((r.x + r.w + 2.0) as u32).min(self.width.saturating_sub(1));
        let y1 = ((r.y + r.h + 2.0) as u32).min(self.height.saturating_sub(1));
        let mut rs = Vec::new();
        let mut gs = Vec::new();
        let mut bs = Vec::new();
        let mut push = |x: u32, y: u32| {
            let p = self.px(x, y);
            bs.push(p[0]);
            gs.push(p[1]);
            rs.push(p[2]);
        };
        if x1 <= x0 || y1 <= y0 {
            return [0, 0, 0];
        }
        for x in (x0..=x1).step_by(2) {
            push(x, y0);
            push(x, y1);
        }
        for y in (y0..=y1).step_by(2) {
            push(x0, y);
            push(x1, y);
        }
        let med = |v: &mut Vec<u8>| {
            v.sort_unstable();
            v[v.len() / 2]
        };
        [med(&mut rs), med(&mut gs), med(&mut bs)]
    }

    /// Standard deviation of luminance inside `r` — high means busy background,
    /// where a flat mask would look bad (use a backdrop plate instead).
    pub fn busyness(&self, r: &Rect) -> f32 {
        let x0 = r.x.max(0.0) as u32;
        let y0 = r.y.max(0.0) as u32;
        let x1 = ((r.x + r.w) as u32).min(self.width);
        let y1 = ((r.y + r.h) as u32).min(self.height);
        let mut sum = 0f64;
        let mut sq = 0f64;
        let mut n = 0f64;
        for y in (y0..y1).step_by(2) {
            for x in (x0..x1).step_by(2) {
                let p = self.px(x, y);
                let l = luma(p[0], p[1], p[2]) as f64;
                sum += l;
                sq += l * l;
                n += 1.0;
            }
        }
        if n == 0.0 {
            return 0.0;
        }
        let m = sum / n;
        ((sq / n - m * m).max(0.0)).sqrt() as f32
    }
}

/// Pick black or white text for a background colour.
pub fn contrast_text(rgb: [u8; 3]) -> [u8; 3] {
    if luma(rgb[2], rgb[1], rgb[0]) > 140 {
        [16, 16, 16]
    } else {
        [245, 240, 225]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Frame {
        Frame::new(w, h, c.repeat((w * h) as usize))
    }

    #[test]
    fn crop_upscale() {
        let f = solid(10, 10, [10, 20, 30, 255]);
        let c = f.crop(8, 8, 5, 5);
        assert_eq!((c.width, c.height), (2, 2));
        let u = c.upscale(2);
        assert_eq!((u.width, u.height), (4, 4));
        assert_eq!(u.px(3, 3), [10, 20, 30, 255]);
    }

    #[test]
    fn ring_and_contrast() {
        let f = solid(20, 20, [0, 0, 200, 255]); // BGRA red
        let c = f.ring_color(&Rect { x: 5.0, y: 5.0, w: 5.0, h: 5.0 });
        assert_eq!(c, [200, 0, 0]);
        assert_eq!(contrast_text([250, 250, 250]), [16, 16, 16]);
        assert_eq!(f.busyness(&Rect { x: 0.0, y: 0.0, w: 20.0, h: 20.0 }), 0.0);
    }
}
