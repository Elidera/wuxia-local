//! Memory that survives OCR noise.
//!
//! OCR reads the same subtitle a little differently from scan to scan
//! ("thisjob" / "this job", "Lang heng" / "Langsheng"). Without this the same
//! line was translated five times in a row and a name came out three ways.
//! * [`FuzzyMemory`] — recent translations found by similarity, not exact text;
//! * [`NameBook`] — every spelling of a name maps to the first one seen, so the
//!   name is translated (and cached) once.

use crate::similarity;
use std::collections::VecDeque;

/// Lower-case letters and digits only, accents folded: the part of a text OCR
/// noise does not change much.
pub fn fold(s: &str) -> String {
    s.chars()
        .filter_map(|c| {
            let c = match c {
                'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'a',
                'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => 'e',
                'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
                'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' => 'o',
                'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
                'ñ' | 'Ñ' => 'n',
                'ç' | 'Ç' => 'c',
                c => c,
            };
            c.is_alphanumeric().then(|| c.to_lowercase().next().unwrap_or(c))
        })
        .collect()
}

/// Recent (source, translation) pairs searched by similarity.
#[derive(Debug)]
pub struct FuzzyMemory {
    items: VecDeque<(String, String)>,
    cap: usize,
    threshold: f32,
}

impl FuzzyMemory {
    pub fn new(cap: usize, threshold: f32) -> Self {
        Self { items: VecDeque::new(), cap: cap.max(1), threshold }
    }

    pub fn remember(&mut self, src: &str, dst: &str) {
        let k = fold(src);
        if k.chars().count() < 12 {
            return; // short texts differ by one letter and mean something else
        }
        self.items.retain(|(s, _)| *s != k);
        self.items.push_front((k, dst.to_string()));
        self.items.truncate(self.cap);
    }

    /// Translation of a text that differs from `src` only by OCR noise.
    pub fn find(&self, src: &str) -> Option<&str> {
        let k = fold(src);
        let n = k.chars().count();
        if n < 12 {
            return None;
        }
        self.items
            .iter()
            .filter(|(s, _)| {
                let m = s.chars().count();
                n.abs_diff(m) * 10 <= n.max(m) // lengths within 10 %
            })
            .map(|(s, d)| (similarity::ratio(s, &k), d))
            .filter(|(r, _)| *r >= self.threshold)
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, d)| d.as_str())
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// Spellings of speaker names seen so far.
#[derive(Debug, Default)]
pub struct NameBook {
    names: Vec<(String, String)>, // (folded, as first seen)
}

impl NameBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// The first-seen spelling of this name (or `raw` itself, now remembered).
    pub fn canonical(&mut self, raw: &str) -> String {
        let raw = raw.trim();
        let k = fold(raw);
        if k.chars().count() < 3 {
            return raw.to_string();
        }
        if let Some(i) = self.names.iter().position(|(f, _)| *f == k || similarity::ratio(f, &k) >= 0.8) {
            // A clean spelling replaces one with an unread letter ("Langsh?ng").
            let messy = |n: &str| n.chars().any(|c| c == '?' || c.is_ascii_digit());
            if messy(&self.names[i].1) && !messy(raw) {
                self.names[i] = (k, raw.to_string());
            }
            return self.names[i].1.clone();
        }
        // Prefer a clean spelling: a variant with odd letters gets replaced later
        // only if it was never used; keep it simple — first seen wins.
        self.names.push((k, raw.to_string()));
        if self.names.len() > 500 {
            self.names.remove(0);
        }
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_hits_ocr_variants_only() {
        let mut m = FuzzyMemory::new(50, 0.9);
        m.remember(
            "There's nothing quite like being on duty in a bustling commercial area. All that time I spent applying for this job feels worth it.",
            "Ничего не сравнится…",
        );
        assert_eq!(
            m.find("There's nothing quite likefbeing on duty in a bustling commercial area. All thattime I spent applying forthisJ0b feels worth it."),
            Some("Ничего не сравнится…")
        );
        assert_eq!(m.find("There's nothing quite like being off duty in a quiet residential area. None of it was worth it."), None);
        assert_eq!(m.find("Thank you."), None);
    }

    #[test]
    fn names_unify() {
        let mut b = NameBook::new();
        assert_eq!(b.canonical("Langsheng"), "Langsheng");
        assert_eq!(b.canonical("Lang heng"), "Langsheng");
        assert_eq!(b.canonical("Lan sheng"), "Langsheng");
        assert_eq!(b.canonical("Längsheng"), "Langsheng");
        assert_eq!(b.canonical("Jiyan"), "Jiyan");
        let mut c = NameBook::new();
        assert_eq!(c.canonical("Langsh?ng"), "Langsh?ng");
        assert_eq!(c.canonical("Langsheng"), "Langsheng");
        assert_eq!(c.canonical("Langsh?ng"), "Langsheng");
        assert_eq!(b.canonical("Jinhsi"), "Jinhsi");
    }
}
