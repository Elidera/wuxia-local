//! Choosing between two OCR engines ("both" mode): Windows OCR and PaddleOCR
//! read the same picture, and for every place on screen the better reading is
//! kept.
//!
//! Lines of the two engines rarely match one to one (PaddleOCR may split a
//! line at a wide gap, Windows OCR may glue two), so lines are grouped into
//! clusters of boxes that overlap, and the whole cluster is taken from one
//! engine.

use crate::image::Rect;
use crate::lang::{self, Lang};
use crate::text;

/// One recognised line. `conf` is the engine's confidence (0–1) when it gives one.
#[derive(Debug, Clone, PartialEq)]
pub struct Read {
    pub text: String,
    pub rect: Rect,
    pub conf: Option<f32>,
}

/// Overlap of two boxes relative to the smaller one (0–1).
pub fn overlap(a: &Rect, b: &Rect) -> f32 {
    let ix = ((a.x + a.w).min(b.x + b.w) - a.x.max(b.x)).max(0.0);
    let iy = ((a.y + a.h).min(b.y + b.h) - a.y.max(b.y)).max(0.0);
    let min_area = (a.w * a.h).min(b.w * b.h).max(1.0);
    (ix * iy) / min_area
}

/// Characters a game text never has: OCR noise.
fn is_junk(c: char) -> bool {
    !(text::is_han(c)
        || text::is_cjk_punct(c)
        || c.is_alphanumeric()
        || c.is_whitespace()
        || ".,!?:;'\"()-%/+&…—".contains(c))
}

/// How good a reading is: roughly the number of characters that are real text
/// in this language, minus a penalty for noise. Longer correct text wins;
/// garbage loses even when long.
pub fn quality(s: &str, lang: Lang) -> f32 {
    let junk = s.chars().filter(|&c| is_junk(c)).count() as f32;
    match lang {
        Lang::Zh => {
            let han = text::han_count(s) as f32;
            let punct = s.chars().filter(|&c| text::is_cjk_punct(c)).count() as f32;
            let other = s.chars().filter(|c| c.is_ascii_alphanumeric()).count() as f32;
            han + 0.3 * punct + 0.2 * other - 2.0 * junk
        }
        Lang::En => {
            let mut score = 0.0;
            for w in s.split_whitespace() {
                let core = w.trim_matches(|c: char| !c.is_alphanumeric());
                let n = core.chars().count() as f32;
                if n == 0.0 {
                    continue;
                }
                let letters = core.chars().filter(|&c| lang::is_latin_letter(c)).count();
                let digits = core.chars().filter(|c| c.is_ascii_digit()).count();
                let vowel = core.chars().any(|c| "aeiouyAEIOUY".contains(c));
                let wordlike = letters == core.chars().count() && (vowel || letters <= 2) && letters <= 14;
                let number = digits == core.chars().count();
                score += if wordlike || number { n } else { -0.5 * n };
            }
            // Russian read as Latin look-alikes ("CnpaBKa") and glued words.
            score * (1.0 - lang::misread_share(s)) - 2.0 * junk
        }
    }
}

/// Text of a group of lines in reading order (top-to-bottom, left-to-right).
fn cluster_text(reads: &[&Read], lang: Lang) -> String {
    let mut v: Vec<&&Read> = reads.iter().collect();
    v.sort_by(|a, b| {
        let same_row = (a.rect.y - b.rect.y).abs() < 0.5 * a.rect.h.min(b.rect.h);
        if same_row {
            a.rect.x.partial_cmp(&b.rect.x).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            a.rect.y.partial_cmp(&b.rect.y).unwrap_or(std::cmp::Ordering::Equal)
        }
    });
    let sep = if lang == Lang::Zh { "" } else { " " };
    v.iter().map(|r| r.text.as_str()).collect::<Vec<_>>().join(sep)
}

/// Average confidence of a group (lines without one count as 0.8).
fn cluster_conf(reads: &[&Read]) -> f32 {
    if reads.is_empty() {
        return 0.0;
    }
    reads.iter().map(|r| r.conf.unwrap_or(0.8)).sum::<f32>() / reads.len() as f32
}

/// Which engine won where, for the log.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct PickStats {
    pub first: usize,
    pub second: usize,
}

/// Merge the readings of two engines. `second` (PaddleOCR) wins ties, since it
/// is the more accurate engine for Chinese; its lines with confidence below
/// `min_conf` are dropped when nothing from `first` is there to compare with.
pub fn merge(first: &[Read], second: &[Read], lang: Lang, min_conf: f32) -> (Vec<Read>, PickStats) {
    let n = first.len() + second.len();
    let rect = |i: usize| if i < first.len() { &first[i].rect } else { &second[i - first.len()].rect };
    // Union-find over lines of different engines whose boxes overlap.
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut Vec<usize>, i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        let mut j = i;
        while p[j] != r {
            let next = p[j];
            p[j] = r;
            j = next;
        }
        r
    }
    for i in 0..first.len() {
        for j in 0..second.len() {
            if overlap(rect(i), rect(first.len() + j)) > 0.4 {
                let (a, b) = (find(&mut parent, i), find(&mut parent, first.len() + j));
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }
    let mut roots: Vec<usize> = Vec::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        if !roots.contains(&r) {
            roots.push(r);
        }
    }
    let mut out = Vec::new();
    let mut stats = PickStats::default();
    for root in roots {
        let a: Vec<&Read> = (0..first.len()).filter(|&i| find(&mut parent, i) == root).map(|i| &first[i]).collect();
        let b: Vec<&Read> = (0..second.len()).filter(|&j| find(&mut parent, first.len() + j) == root).map(|j| &second[j]).collect();
        let take_second = if a.is_empty() {
            if cluster_conf(&b) < min_conf {
                continue;
            }
            true
        } else if b.is_empty() {
            false
        } else {
            let qa = quality(&cluster_text(&a, lang), lang);
            // Low confidence makes PaddleOCR's reading count for less.
            let qb = quality(&cluster_text(&b, lang), lang) * (0.6 + 0.4 * cluster_conf(&b).clamp(0.0, 1.0));
            qb >= qa * 0.95
        };
        if take_second {
            stats.second += 1;
            out.extend(b.into_iter().cloned());
        } else {
            stats.first += 1;
            out.extend(a.into_iter().cloned());
        }
    }
    (out, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(text: &str, x: f32, y: f32, w: f32, h: f32, conf: Option<f32>) -> Read {
        Read { text: text.into(), rect: Rect { x, y, w, h }, conf }
    }

    #[test]
    fn quality_prefers_real_text() {
        assert!(quality("少侠，此去开封路途遥远", Lang::Zh) > quality("少侠，此去开封", Lang::Zh));
        assert!(quality("少侠此去开封", Lang::Zh) > quality("少侠|此去■开封", Lang::Zh));
        assert!(quality("We need this job done.", Lang::En) > quality("We need thisjob d0ne.", Lang::En) - 0.01);
        assert!(quality("Langsheng, wait for me!", Lang::En) > quality("Längsheng, wa1t f0r me!", Lang::En));
        assert!(quality("Search", Lang::En) > quality("CnpaBKa", Lang::En));
    }

    #[test]
    fn picks_better_engine_per_place() {
        // Subtitle: Windows OCR lost characters; menu label: PaddleOCR misread.
        let win = vec![
            r("少侠此去", 100.0, 500.0, 300.0, 30.0, None),
            r("背包", 20.0, 20.0, 60.0, 24.0, None),
        ];
        let paddle = vec![
            r("少侠，此去开封路途遥远", 98.0, 497.0, 420.0, 36.0, Some(0.97)),
            r("背■", 18.0, 18.0, 64.0, 28.0, Some(0.55)),
        ];
        let (out, stats) = merge(&win, &paddle, Lang::Zh, 0.5);
        let texts: Vec<&str> = out.iter().map(|l| l.text.as_str()).collect();
        assert!(texts.contains(&"少侠，此去开封路途遥远"));
        assert!(texts.contains(&"背包"));
        assert_eq!(out.len(), 2);
        assert_eq!(stats, PickStats { first: 1, second: 1 });
    }

    #[test]
    fn split_line_is_one_place() {
        // PaddleOCR split the line at a wide gap: compared as a whole.
        let win = vec![r("Langsheng: Lang heng will wait.", 100.0, 500.0, 600.0, 30.0, None)];
        let paddle = vec![
            r("Langsheng:", 98.0, 498.0, 150.0, 34.0, Some(0.95)),
            r("Langsheng will wait.", 300.0, 498.0, 400.0, 34.0, Some(0.96)),
        ];
        let (out, _) = merge(&win, &paddle, Lang::En, 0.5);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|l| l.conf.is_some()));
    }

    #[test]
    fn keeps_lines_only_one_engine_saw() {
        let win = vec![r("开始游戏", 10.0, 10.0, 80.0, 20.0, None)];
        let paddle = vec![
            r("设置", 10.0, 300.0, 40.0, 20.0, Some(0.9)),
            r("口口", 500.0, 300.0, 40.0, 20.0, Some(0.2)),
        ];
        let (out, _) = merge(&win, &paddle, Lang::Zh, 0.5);
        let texts: Vec<&str> = out.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts.len(), 2);
        assert!(texts.contains(&"开始游戏") && texts.contains(&"设置"));
    }

    #[test]
    fn empty_inputs() {
        assert!(merge(&[], &[], Lang::Zh, 0.5).0.is_empty());
        let one = vec![r("设置", 0.0, 0.0, 40.0, 20.0, Some(0.9))];
        assert_eq!(merge(&one, &[], Lang::Zh, 0.5).0.len(), 1);
        assert_eq!(merge(&[], &one, Lang::Zh, 0.5).0.len(), 1);
    }
}
