//! Group OCR lines of a whole screen into text blocks (a two-line subtitle,
//! a speaker name above its line, a tooltip paragraph).

use crate::image::Rect;

/// Indices of `lines` grouped into blocks, each block top-to-bottom.
/// Two lines join when the second starts just below the first (gap under ~0.9
/// line height), they overlap horizontally or share a centre, and their heights
/// are similar (a big title is not part of the small text under it).
pub fn group_lines(lines: &[Rect]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|&a, &b| lines[a].y.partial_cmp(&lines[b].y).unwrap_or(std::cmp::Ordering::Equal));
    let mut blocks: Vec<Vec<usize>> = Vec::new();
    for i in order {
        let l = &lines[i];
        let mut joined = false;
        for b in blocks.iter_mut().rev() {
            let last = &lines[*b.last().unwrap()];
            let h = last.h.max(l.h).max(1.0);
            let gap = l.y - (last.y + last.h);
            let similar_height = (l.h / last.h.max(1.0)).clamp(0.0, 10.0);
            let x_overlap = (l.x + l.w).min(last.x + last.w) - l.x.max(last.x);
            let centres = ((l.x + l.w / 2.0) - (last.x + last.w / 2.0)).abs() < l.w.max(last.w) / 2.0;
            if gap > -0.4 * h && gap < 0.9 * h && (0.6..=1.7).contains(&similar_height) && (x_overlap > 0.0 || centres) {
                b.push(i);
                joined = true;
                break;
            }
        }
        if !joined {
            blocks.push(vec![i]);
        }
    }
    blocks
}

/// Bounding box of a block.
pub fn block_rect(lines: &[Rect], idx: &[usize]) -> Rect {
    let mut r = lines[idx[0]];
    for &i in &idx[1..] {
        r = r.union(&lines[i]);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn subtitle_with_speaker_and_separate_menu() {
        let lines = [
            r(1100.0, 1150.0, 120.0, 30.0), // speaker name
            r(700.0, 1190.0, 1000.0, 34.0), // line 1
            r(760.0, 1232.0, 880.0, 34.0),  // line 2
            r(60.0, 100.0, 200.0, 30.0),    // menu item far away
            r(60.0, 300.0, 200.0, 30.0),    // another menu item, big gap
        ];
        let g = group_lines(&lines);
        assert_eq!(g.len(), 3);
        assert!(g.contains(&vec![0, 1, 2]));
        let b = block_rect(&lines, &[0, 1, 2]);
        assert_eq!((b.y, b.y + b.h), (1150.0, 1266.0));
    }

    #[test]
    fn title_not_glued_to_body() {
        let lines = [r(100.0, 100.0, 600.0, 80.0), r(100.0, 190.0, 600.0, 30.0)];
        assert_eq!(group_lines(&lines).len(), 2);
    }
}
