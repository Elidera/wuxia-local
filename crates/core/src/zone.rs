//! Finding the subtitle area by itself (like "Find Subtitles" in other screen
//! translators): the auto mode remembers where real dialogue lines appeared;
//! once several of them sit in the same horizontal band, that band (with room
//! for a speaker name and longer lines) is the subtitle region.

use crate::image::Rect;

/// Subtitle region as fractions of the screen [x, y, w, h], or None while the
/// dialogue lines seen so far do not agree on a place.
pub fn learn(samples: &[Rect], screen: (f32, f32)) -> Option<[f32; 4]> {
    let (sw, sh) = screen;
    if samples.len() < 3 || sw <= 0.0 || sh <= 0.0 {
        return None;
    }
    // Largest group of lines whose vertical centres lie within 12 % of the screen height.
    let mut idx: Vec<usize> = (0..samples.len()).collect();
    let cy = |r: &Rect| r.y + r.h / 2.0;
    idx.sort_by(|&a, &b| cy(&samples[a]).partial_cmp(&cy(&samples[b])).unwrap_or(std::cmp::Ordering::Equal));
    let band = 0.12 * sh;
    let (mut best_lo, mut best_n) = (0, 0);
    let mut hi = 0;
    for lo in 0..idx.len() {
        while hi < idx.len() && cy(&samples[idx[hi]]) - cy(&samples[idx[lo]]) <= band {
            hi += 1;
        }
        if hi - lo > best_n {
            best_n = hi - lo;
            best_lo = lo;
        }
    }
    if best_n < 3 || (best_n as f32) < 0.6 * samples.len() as f32 {
        return None;
    }
    let group: Vec<&Rect> = idx[best_lo..best_lo + best_n].iter().map(|&i| &samples[i]).collect();
    let mut u = *group[0];
    for r in &group[1..] {
        u = u.union(r);
    }
    // Subtitles are centred: make the area symmetric and a bit wider than seen.
    let centre = u.x + u.w / 2.0;
    let centred = (centre - sw / 2.0).abs() < 0.1 * sw;
    let (x0, x1) = if centred {
        let half = (sw / 2.0 - u.x).max(u.x + u.w - sw / 2.0) + 0.06 * sw;
        (sw / 2.0 - half, sw / 2.0 + half)
    } else {
        (u.x - 0.06 * sw, u.x + u.w + 0.06 * sw)
    };
    // Room above for the speaker name, below for a third line.
    let line_h = group.iter().map(|r| r.h).fold(0.0f32, f32::max).max(0.02 * sh);
    let y0 = u.y - 0.04 * sh;
    let y1 = u.y + u.h + line_h;
    let (x0, x1) = (x0.max(0.0), x1.min(sw));
    let (y0, y1) = (y0.max(0.0), y1.min(sh));
    Some([x0 / sw, y0 / sh, (x1 - x0) / sw, (y1 - y0) / sh])
}

/// Overlap of two fractional rectangles relative to the smaller one (0..1).
pub fn overlap(a: [f32; 4], b: [f32; 4]) -> f32 {
    let ix = ((a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0])).max(0.0);
    let iy = ((a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1])).max(0.0);
    let small = (a[2] * a[3]).min(b[2] * b[3]).max(1e-6);
    ix * iy / small
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn finds_the_bottom_band() {
        let s = (2560.0, 1440.0);
        let lines = [
            r(680.0, 1180.0, 1200.0, 110.0), // two-line subtitle
            r(900.0, 1190.0, 760.0, 50.0),
            r(800.0, 1185.0, 960.0, 105.0),
            r(1720.0, 910.0, 520.0, 50.0), // a dialogue option on the right
        ];
        let z = learn(&lines, s).expect("zone");
        // Centred, covers the subtitles, starts above them, inside the screen.
        assert!((z[0] + z[2] / 2.0 - 0.5).abs() < 0.01, "{z:?}");
        assert!(z[0] * s.0 <= 680.0 && (z[0] + z[2]) * s.0 >= 1880.0, "{z:?}");
        assert!(z[1] * s.1 < 1180.0 && (z[1] + z[3]) * s.1 >= 1290.0, "{z:?}");
        assert!(z[1] + z[3] <= 1.0);
    }

    #[test]
    fn no_zone_without_agreement() {
        let s = (1920.0, 1080.0);
        assert_eq!(learn(&[r(100.0, 100.0, 500.0, 40.0), r(700.0, 900.0, 500.0, 40.0)], s), None);
        let scattered = [r(100.0, 100.0, 500.0, 40.0), r(700.0, 500.0, 500.0, 40.0), r(700.0, 900.0, 500.0, 40.0), r(300.0, 300.0, 500.0, 40.0)];
        assert_eq!(learn(&scattered, s), None);
        assert!(overlap([0.0, 0.0, 0.5, 0.5], [0.25, 0.25, 0.5, 0.5]) > 0.24);
    }
}
