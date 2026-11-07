//! Terminology glossary: names, titles, techniques, places.
//!
//! File format (UTF-8 TSV, editable in Notepad):
//! `源词<TAB>перевод<TAB>необязательная заметка`
//! Lines starting with `#` are comments. Later files override earlier ones,
//! so a user glossary can override the bundled one.

use crate::tsv;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    pub src: String,
    pub dst: String,
    pub note: String,
}

#[derive(Debug, Default, Clone)]
pub struct Glossary {
    /// src → index in `terms`
    index: HashMap<String, usize>,
    terms: Vec<Term>,
    /// Distinct term lengths in chars, longest first (for longest-match scan).
    lengths: Vec<usize>,
}

impl Glossary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn insert(&mut self, src: &str, dst: &str, note: &str) {
        let src = src.trim();
        let dst = dst.trim();
        if src.is_empty() || dst.is_empty() {
            return;
        }
        let t = Term { src: src.to_string(), dst: dst.to_string(), note: note.trim().to_string() };
        if let Some(&i) = self.index.get(src) {
            self.terms[i] = t;
        } else {
            self.index.insert(t.src.clone(), self.terms.len());
            self.terms.push(t);
        }
        let n = src.chars().count();
        if !self.lengths.contains(&n) {
            self.lengths.push(n);
            self.lengths.sort_unstable_by(|a, b| b.cmp(a));
        }
    }

    /// Merge a TSV document. Returns number of terms read.
    pub fn load_tsv(&mut self, content: &str) -> usize {
        let mut n = 0;
        for row in tsv::parse(content) {
            if row.len() >= 2 {
                self.insert(&row[0], &row[1], row.get(2).map(String::as_str).unwrap_or(""));
                n += 1;
            }
        }
        n
    }

    /// Exact lookup (used for short UI labels: whole string is a known term).
    pub fn exact(&self, s: &str) -> Option<&Term> {
        self.index.get(s.trim()).map(|&i| &self.terms[i])
    }

    /// Find terms occurring in `text`, longest match first, non-overlapping,
    /// deduplicated, in order of first appearance. At most `limit` terms.
    pub fn find_in(&self, text: &str, limit: usize) -> Vec<&Term> {
        let chars: Vec<char> = text.chars().collect();
        let mut found: Vec<usize> = Vec::new();
        let mut i = 0;
        'outer: while i < chars.len() && found.len() < limit {
            for &len in &self.lengths {
                if i + len > chars.len() {
                    continue;
                }
                let cand: String = chars[i..i + len].iter().collect();
                if let Some(&idx) = self.index.get(&cand) {
                    if !found.contains(&idx) {
                        found.push(idx);
                    }
                    i += len;
                    continue 'outer;
                }
            }
            i += 1;
        }
        found.into_iter().map(|i| &self.terms[i]).collect()
    }

    pub fn terms(&self) -> &[Term] {
        &self.terms
    }

    /// Check a translation for glossary violations: terms present in the source
    /// whose required rendering is missing from the output. Used for logging/bench.
    pub fn violations<'a>(&'a self, src: &str, out: &str) -> Vec<&'a Term> {
        let lower = out.to_lowercase();
        self.find_in(src, 64)
            .into_iter()
            .filter(|t| {
                // Russian inflects: compare word stems. The term counts as used if the
                // stem of its longest word is present (skips pronouns like «ваша»).
                let want = t.dst.to_lowercase();
                let word: Vec<char> = want
                    .split(|c: char| !c.is_alphanumeric())
                    .max_by_key(|w| w.chars().count())
                    .unwrap_or("")
                    .chars()
                    .collect();
                if word.is_empty() {
                    return false;
                }
                let keep = if word.len() <= 4 { word.len() } else { (word.len() * 55 / 100).max(4) };
                let stem: String = word[..keep].iter().collect();
                !lower.contains(&stem)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> Glossary {
        let mut g = Glossary::new();
        g.load_tsv("# test\n江湖\tцзянху\n江湖客\tстранник цзянху\n内力\tвнутренняя сила\n开封\tКайфэн\tгород\n");
        g
    }

    #[test]
    fn longest_match_first() {
        let g = g();
        let found: Vec<_> = g.find_in("江湖客说开封的江湖很乱，内力深厚", 10).iter().map(|t| t.src.as_str()).collect();
        assert_eq!(found, vec!["江湖客", "开封", "江湖", "内力"]);
    }

    #[test]
    fn override_and_exact() {
        let mut g = g();
        g.load_tsv("开封\tКайфын\n");
        assert_eq!(g.exact("开封").unwrap().dst, "Кайфын");
        assert_eq!(g.len(), 4);
    }

    #[test]
    fn violations_tolerate_inflection() {
        let g = g();
        assert!(g.violations("去开封", "Едем в Кайфэне").is_empty());
        assert_eq!(g.violations("去开封", "Едем в столицу").len(), 1);
        let mut g2 = Glossary::new();
        g2.load_tsv("阁下\tваша милость\n");
        assert!(g2.violations("阁下", "прошу вашей милости").is_empty());
    }
}
