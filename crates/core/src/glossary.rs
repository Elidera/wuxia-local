//! Terminology glossary: names, titles, techniques, places.
//!
//! File format (UTF-8 TSV, editable in Notepad):
//! `源词<TAB>перевод<TAB>необязательная заметка`
//! Lines starting with `#` are comments. Later files override earlier ones,
//! so a user glossary can override the bundled one.
//!
//! * In dialogue, an English term written with a capital letter matches only
//!   words with a capital: the game term «Echo» yes, the ordinary word «echo» no.
//! * A note may end with «кроме 辽阔 辽远»: inside these words the term is not
//!   looked for (one-character Chinese terms hide inside ordinary words).

use crate::tsv;
use std::collections::{HashMap, HashSet};

/// Grammatical gender of a speaker: Russian past tense depends on it
/// («я пришёл» / «я пришла»). Set in a glossary note as «пол: ж» or «пол: м».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Male,
    Female,
}

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
    /// Terms from interface dictionaries (`*game_ui*.tsv`): short common words
    /// ("Back", "Map") that are right for menu labels but noise in dialogue.
    ui_only: HashSet<usize>,
    /// Words in which a term must not be found («кроме …» in its note).
    exclude: HashMap<usize, Vec<Vec<char>>>,
}

/// Split «государство киданей; кроме 辽阔 辽远» into the note and the exclusions.
fn split_exclusions(note: &str) -> (String, Vec<Vec<char>>) {
    match note.find("кроме") {
        Some(pos) => {
            let words = note[pos + "кроме".len()..]
                .split(|c: char| c.is_whitespace() || c == ',' || c == '，' || c == '、')
                .filter(|w| !w.is_empty())
                .map(|w| w.chars().collect())
                .collect();
            (note[..pos].trim().trim_end_matches([';', ',']).trim().to_string(), words)
        }
        None => (note.to_string(), Vec::new()),
    }
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
        let (note, exclude) = split_exclusions(note.trim());
        let t = Term { src: src.to_string(), dst: dst.to_string(), note };
        let key = key_of(src);
        let i = if let Some(&i) = self.index.get(&key) {
            self.terms[i] = t;
            self.ui_only.remove(&i);
            i
        } else {
            self.index.insert(key, self.terms.len());
            self.terms.push(t);
            self.terms.len() - 1
        };
        if exclude.is_empty() {
            self.exclude.remove(&i);
        } else {
            self.exclude.insert(i, exclude);
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

    /// Merge an interface dictionary: its terms are used for labels and screen
    /// translation, but not suggested to the model for dialogue lines.
    pub fn load_tsv_ui(&mut self, content: &str) -> usize {
        let mut n = 0;
        for row in tsv::parse(content) {
            if row.len() >= 2 {
                self.insert(&row[0], &row[1], row.get(2).map(String::as_str).unwrap_or(""));
                if let Some(&i) = self.index.get(&key_of(row[0].trim())) {
                    self.ui_only.insert(i);
                }
                n += 1;
            }
        }
        n
    }

    /// Like [`find_in`](Self::find_in) but without interface-only terms (for dialogue).
    pub fn find_in_dialogue(&self, text: &str, limit: usize) -> Vec<&Term> {
        self.find_impl(text, limit, true)
    }

    /// Exact lookup (used for short UI labels: whole string is a known term).
    pub fn exact(&self, s: &str) -> Option<&Term> {
        self.index.get(&key_of(s.trim())).map(|&i| &self.terms[i])
    }

    /// Gender of a character from its glossary note («пол: ж» / «пол: м»).
    pub fn gender_of(&self, name: &str) -> Option<Gender> {
        let note = self.exact(name)?.note.to_lowercase().replace(' ', "");
        if note.contains("пол:ж") {
            Some(Gender::Female)
        } else if note.contains("пол:м") {
            Some(Gender::Male)
        } else {
            None
        }
    }

    /// Find terms occurring in `text`, longest match first, non-overlapping,
    /// deduplicated, in order of first appearance. At most `limit` terms.
    /// Latin terms match case-insensitively and only as whole words
    /// (an English plural "-s"/"-es" is allowed: "Echoes" finds "Echo").
    pub fn find_in(&self, text: &str, limit: usize) -> Vec<&Term> {
        self.find_impl(text, limit, false)
    }

    /// Is the match at `i` part of one of the term's excluded words?
    fn excluded(&self, idx: usize, chars: &[char], i: usize) -> bool {
        let Some(words) = self.exclude.get(&idx) else { return false };
        words.iter().any(|w| {
            (0..w.len()).any(|off| i >= off && i - off + w.len() <= chars.len() && chars[i - off..i - off + w.len()] == w[..])
        })
    }

    fn find_impl(&self, text: &str, limit: usize, skip_ui: bool) -> Vec<&Term> {
        let original: Vec<char> = text.chars().collect();
        let chars: Vec<char> = original.iter().map(|c| c.to_ascii_lowercase()).collect();
        let mut found: Vec<usize> = Vec::new();
        let mut i = 0;
        'outer: while i < chars.len() && found.len() < limit {
            for &len in &self.lengths {
                if i + len > chars.len() {
                    continue;
                }
                let cand: String = chars[i..i + len].iter().collect();
                if let Some(&idx) = self.index.get(&cand) {
                    if self.excluded(idx, &chars, i) {
                        continue;
                    }
                    let latin = cand.chars().any(|c| c.is_ascii_alphabetic());
                    // «Echo» (game term) must not catch the ordinary word «echo».
                    let capital_term = self.terms[idx].src.chars().next().map_or(false, |c| c.is_uppercase());
                    // (Dialogue only: menu labels keep matching any case.)
                    if skip_ui && latin && capital_term && original[i].is_lowercase() {
                        continue;
                    }
                    let mut end = i + len;
                    if latin {
                        let word = |c: Option<&char>| c.map_or(false, |c| c.is_alphanumeric());
                        if i > 0 && word(chars.get(i - 1)) {
                            continue;
                        }
                        if word(chars.get(end)) {
                            // allow plural endings
                            let rest: String = chars[end..].iter().take(3).collect();
                            let suf = if rest.starts_with("es") && !word(chars.get(end + 2)) {
                                2
                            } else if rest.starts_with('s') && !word(chars.get(end + 1)) {
                                1
                            } else {
                                continue;
                            };
                            end += suf;
                        }
                    }
                    if !found.contains(&idx) && !(skip_ui && self.ui_only.contains(&idx)) {
                        found.push(idx);
                    }
                    i = end;
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
        self.find_impl(src, 64, true)
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

/// Index key: ASCII letters lower-cased, everything else as is.
fn key_of(s: &str) -> String {
    s.chars().map(|c| c.to_ascii_lowercase()).collect()
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
    fn english_terms() {
        let mut g = Glossary::new();
        g.load_tsv("Rover\tСкиталец\nEcho\tЭхо\nTacet Field\tПоле Тацет\nJinzhou\tЦзиньчжоу\n");
        let found: Vec<_> = g.find_in("The Rover absorbed two Echoes near the TACET FIELD of Jinzhou.", 10).iter().map(|t| t.dst.as_str()).collect();
        assert_eq!(found, vec!["Скиталец", "Эхо", "Поле Тацет", "Цзиньчжоу"]);
        // Not inside other words.
        assert!(g.find_in("Echolocation and Roverbot", 10).is_empty());
        assert_eq!(g.exact(" echo ").unwrap().dst, "Эхо");
        // Ordinary lower-case words are not game terms.
        assert!(g.find_in_dialogue("I heard an echo from the cave, said the rover.", 10).is_empty());
        assert_eq!(g.find_in_dialogue("Absorb the Echo, Rover.", 10).len(), 2);
        let mut lower = Glossary::new();
        lower.load_tsv("jinzhou\tЦзиньчжоу\n");
        assert_eq!(lower.find_in("to jinzhou", 5).len(), 1);
    }

    #[test]
    fn gender_from_note() {
        let mut g = Glossary::new();
        g.load_tsv("Yinlin\tИньлинь\tпол: ж\nJiyan\tЦзиянь\tгенерал; пол: м\nRover\tСкиталец\n");
        assert_eq!(g.gender_of("Yinlin"), Some(Gender::Female));
        assert_eq!(g.gender_of("jiyan"), Some(Gender::Male));
        assert_eq!(g.gender_of("Rover"), None);
        assert_eq!(g.gender_of("Langsheng"), None);
    }

    #[test]
    fn exclusions_for_one_character_terms() {
        let mut g = Glossary::new();
        g.load_tsv("辽\tЛяо\tгосударство киданей; кроме 辽阔 辽远\n");
        assert_eq!(g.terms()[0].note, "государство киданей");
        assert!(g.find_in("这片辽阔的草原", 5).is_empty());
        assert!(g.find_in("路途辽远", 5).is_empty());
        assert_eq!(g.find_in("辽军南下", 5).len(), 1);
    }

    #[test]
    fn ui_terms_not_in_dialogue() {
        let mut g = Glossary::new();
        g.load_tsv("Rover\tСкиталец\n");
        g.load_tsv_ui("Back\tНазад\nMap\tКарта\n");
        assert_eq!(g.find_in_dialogue("Rover, come back to the map!", 8).len(), 1);
        assert_eq!(g.find_in("Rover, come back to the map!", 8).len(), 3);
        assert_eq!(g.exact("Back").unwrap().dst, "Назад");
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
