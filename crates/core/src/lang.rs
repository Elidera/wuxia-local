//! Source languages we translate from, and cheap script-based detection.

use crate::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

impl Lang {
    /// "zh" / "en" (config values). Anything unknown → Chinese.
    pub fn from_code(s: &str) -> Lang {
        if s.trim().to_ascii_lowercase().starts_with("en") {
            Lang::En
        } else {
            Lang::Zh
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }

    pub fn name_ru(self) -> &'static str {
        match self {
            Lang::Zh => "китайский",
            Lang::En => "английский",
        }
    }
}

pub fn is_latin_letter(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c as u32, 0xC0..=0x24F)
}

pub fn is_cyrillic(c: char) -> bool {
    matches!(c as u32, 0x400..=0x4FF)
}

pub fn latin_count(s: &str) -> usize {
    s.chars().filter(|&c| is_latin_letter(c)).count()
}

pub fn cyrillic_count(s: &str) -> usize {
    s.chars().filter(|&c| is_cyrillic(c)).count()
}

/// Share of Latin letters among non-space characters.
pub fn latin_ratio(s: &str) -> f32 {
    let total = s.chars().filter(|c| !c.is_whitespace()).count();
    if total == 0 {
        0.0
    } else {
        latin_count(s) as f32 / total as f32
    }
}

/// Guess the language of a piece of game text by script.
pub fn detect(s: &str) -> Option<Lang> {
    let han = text::han_count(s);
    let lat = latin_count(s);
    if han >= 1 && han * 3 >= lat {
        Some(Lang::Zh)
    } else if lat >= 2 {
        Some(Lang::En)
    } else {
        None
    }
}

/// Is this OCR result real subtitle text in `lang` (and not noise, icons or another script)?
pub fn is_subtitle(s: &str, lang: Lang) -> bool {
    match lang {
        Lang::Zh => text::han_count(s) >= 2 && text::han_ratio(s) >= 0.4,
        Lang::En => {
            // At least one real word of 3+ letters and mostly letters.
            let has_word = s.split(|c: char| !is_latin_letter(c)).any(|w| w.chars().count() >= 3);
            latin_count(s) >= 4 && latin_ratio(s) >= 0.55 && has_word
        }
    }
}

/// Is a single OCR line on a game screen (menu label, button) worth translating?
pub fn is_label(s: &str, lang: Lang) -> bool {
    match lang {
        Lang::Zh => text::has_han(s),
        Lang::En => {
            let has_word = s.split(|c: char| !is_latin_letter(c)).any(|w| w.chars().count() >= 2 && w.chars().any(|c| "aeiouyAEIOUY".contains(c)));
            latin_count(s) >= 2 && latin_ratio(s) >= 0.5 && has_word
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects() {
        assert_eq!(detect("少侠，此去开封"), Some(Lang::Zh));
        assert_eq!(detect("Rover, wait for me!"), Some(Lang::En));
        assert_eq!(detect("Lv. 30 剑法"), Some(Lang::Zh));
        assert_eq!(detect("30 / 40"), None);
        assert_eq!(Lang::from_code("en-US"), Lang::En);
        assert_eq!(Lang::from_code("zh-Hans-CN"), Lang::Zh);
    }

    #[test]
    fn subtitle_filter() {
        assert!(is_subtitle("We should head to Jinzhou before dark.", Lang::En));
        assert!(!is_subtitle("Il |", Lang::En));
        assert!(!is_subtitle("12:45 / 99", Lang::En));
        assert!(!is_subtitle("少侠，此去开封", Lang::En));
        assert!(is_subtitle("少侠，此去开封", Lang::Zh));
        assert!(!is_subtitle("Hello there", Lang::Zh));
    }

    #[test]
    fn label_filter() {
        assert!(is_label("Inventory", Lang::En));
        assert!(is_label("Lv. 70", Lang::En) == false);
        assert!(is_label("Go", Lang::En));
        assert!(!is_label("XP", Lang::En));
    }
}
