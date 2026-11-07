//! User replacement rules (like LunaTranslator's per-game "translation optimization").
//!
//! TSV lines: `stage<TAB>from<TAB>to`
//! * `ocr` — fix a typical recognition error before translation (`ocr	未	末`);
//! * `ru`  — replace in the Russian output (`ru	Цзянху	цзянху`).
//! Longer patterns are applied first. `#` starts a comment.

use crate::tsv;

#[derive(Debug, Default, Clone)]
pub struct Fixes {
    ocr: Vec<(String, String)>,
    ru: Vec<(String, String)>,
}

impl Fixes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.ocr.len() + self.ru.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn load_tsv(&mut self, content: &str) -> usize {
        let mut n = 0;
        for row in tsv::parse(content) {
            if row.len() < 3 || row[1].is_empty() {
                continue;
            }
            let rule = (row[1].clone(), row[2].clone());
            match row[0].trim().to_lowercase().as_str() {
                "ocr" => self.ocr.push(rule),
                "ru" | "out" => self.ru.push(rule),
                _ => continue,
            }
            n += 1;
        }
        self.ocr.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count()));
        self.ru.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count()));
        n
    }

    fn apply(rules: &[(String, String)], s: &str) -> String {
        let mut out = s.to_string();
        for (from, to) in rules {
            if out.contains(from.as_str()) {
                out = out.replace(from.as_str(), to);
            }
        }
        out
    }

    pub fn apply_ocr(&self, s: &str) -> String {
        Self::apply(&self.ocr, s)
    }

    pub fn apply_ru(&self, s: &str) -> String {
        Self::apply(&self.ru, s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stages_and_order() {
        let mut f = Fixes::new();
        let n = f.load_tsv("# c\nocr\t末\t未\nocr\t末来\t未来\nru\tЦзянху\tцзянху\nbad\tx\ty\n");
        assert_eq!(n, 3);
        assert_eq!(f.apply_ocr("末来末"), "未来未");
        assert_eq!(f.apply_ru("Цзянху опасен"), "цзянху опасен");
    }
}
