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

/// Does English text read like a sentence rather than a block of menu labels?
/// Menus are Title Case ("Union Level", "Data Bank"); speech is mostly
/// lower-case words.
pub fn sentence_like_en(s: &str) -> bool {
    let words: Vec<&str> = s
        .split(|c: char| c.is_whitespace() || c == '/' || c == '|')
        .filter(|w| w.chars().next().map_or(false, |c| c.is_alphabetic()))
        .collect();
    let lower = words.iter().filter(|w| w.chars().next().map_or(false, |c| c.is_lowercase())).count();
    let ends = s.trim_end().ends_with(['.', '!', '?', '…', '"']);
    if words.len() <= 2 {
        lower >= 1 || ends
    } else {
        // Game terms are Title Case ("3 Tacet Discords near the Resonance Beacon."):
        // a finished sentence with a few ordinary words is still speech.
        lower * 2 >= words.len() || (ends && lower >= 2)
    }
}

/// Russian text read by the English OCR turns into Latin look-alikes:
/// «Поиск» → "noøcK", «Справка» → "CnpaBKa", «Это того стоило» → "3TO Toro
/// CTOVIJIO", «Пионер» → "nv10Hep", «Магазин» → "CTopa"-like words. Real English
/// almost never has these shapes (rare "iPhone", "McDonald").
fn suspicious_word(w: &str) -> bool {
    let w = w.trim_matches(|c: char| !c.is_alphanumeric());
    let chars: Vec<char> = w.chars().collect();
    let letters = chars.iter().filter(|c| c.is_alphabetic()).count();
    if letters < 2 {
        return false;
    }
    // A capital after a lower-case letter: "noøcK", "naHopaua".
    let mut seen_lower = false;
    for &c in &chars {
        if c.is_lowercase() {
            seen_lower = true;
        } else if c.is_uppercase() && seen_lower {
            return true;
        }
    }
    // Digits inside or before letters: "nv10Hep", "3TO", "56nxcoa" — but not
    // "SOL3", "Lv70", "2nd", "10am".
    if let Some(first_digit) = chars.iter().position(|c| c.is_ascii_digit()) {
        let letters_after = chars[first_digit..].iter().filter(|c| c.is_alphabetic()).count();
        let tail: String = chars[first_digit..].iter().filter(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
        let ordinal = ["st", "nd", "rd", "th", "am", "pm", "s", "x", "d", "k", "m"].contains(&tail.as_str());
        if letters_after >= 2 && !ordinal {
            return true;
        }
    }
    // Two or more capitals then lower-case: "CTopa", "LIHocan" (not "NPCs").
    let caps = chars.iter().take_while(|c| c.is_uppercase()).count();
    if caps >= 2 && chars.len() >= 4 && chars.len() > caps {
        let rest: String = chars[caps..].iter().collect();
        if rest != "s" && rest.chars().all(|c| c.is_lowercase()) {
            return true;
        }
    }
    // Capital look-alikes of Cyrillic letters: "CTOVIJIO", "MOTOUVIKJI".
    w.contains("IJ") || w.contains("JI")
}

/// Share of words that look like misread Cyrillic (see `suspicious_word`).
pub fn misread_share(s: &str) -> f32 {
    // Hyphenated and underscored words are separate words: "Sub-Stats", "Baishi_Shu".
    let words: Vec<&str> = s
        .split(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '/' | '—'))
        .filter(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 2)
        .collect();
    if words.is_empty() {
        return 0.0;
    }
    words.iter().filter(|w| suspicious_word(w)).count() as f32 / words.len() as f32
}

fn looks_misread(s: &str) -> bool {
    let share = misread_share(s);
    let words = s.split_whitespace().count();
    share > 0.0 && (words <= 4 || share >= 0.2)
}

/// Is this OCR result real subtitle text in `lang` (and not noise, icons or another script)?
pub fn is_subtitle(s: &str, lang: Lang) -> bool {
    match lang {
        Lang::Zh => text::han_count(s) >= 2 && text::han_ratio(s) >= 0.4,
        Lang::En => {
            // At least one real word of 3+ letters and mostly letters.
            let has_word = s.split(|c: char| !is_latin_letter(c)).any(|w| w.chars().count() >= 3);
            latin_count(s) >= 4 && latin_ratio(s) >= 0.55 && has_word && sentence_like_en(s) && !looks_misread(s)
        }
    }
}

/// Is a single OCR line on a game screen (menu label, button) worth translating?
pub fn is_label(s: &str, lang: Lang) -> bool {
    match lang {
        Lang::Zh => text::has_han(s),
        Lang::En => {
            let has_word = s.split(|c: char| !is_latin_letter(c)).any(|w| w.chars().count() >= 2 && w.chars().any(|c| "aeiouyAEIOUY".contains(c)));
            latin_count(s) >= 2 && latin_ratio(s) >= 0.5 && has_word && !looks_misread(s)
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
        // Menu blocks seen in Wuthering Waves are not subtitles.
        assert!(!is_subtitle("Birthday: Apr 26 Rank 8 SOL3 Phase Union Level Union EXP Expedition Guidebook Synthesis Motorbike", Lang::En));
        assert!(!is_subtitle("Ambient Volume: Interface Volume Off Mute when Game is in the Background", Lang::En));
        assert!(!is_subtitle("Resolution: Global Filter Image Display Mode Apply Recomme", Lang::En));
        assert!(!is_subtitle("KURO: LGAMES", Lang::En));
        assert!(is_subtitle("Jiyan: The frontline won't hold for long. We need reinforcements from Jinzhou by nightfall.", Lang::En));
        assert!(is_subtitle("Thank you.", Lang::En));
        assert!(is_subtitle("Jinhsi: Jinzhou stands because its people refuse to kneel. Remember that.", Lang::En));
    }

    #[test]
    fn misread_russian_is_not_english() {
        for s in [
            "p noøcK",
            "CnpaBKa",
            "HeipoceTb Songtell...",
            "rnaBHas",
            "LIJMH0 naHopaua -.",
            "Hy-MT2 1.8B Q4_K_M — 6blCTpblä YMonqaHmo)",
            ", — 3TO Toro CTOVIJIO.",
            "nv10Hep",
            "CTopa",
            "MOTOUVIKJI",
            "06Haw6 LW6TMTn6",
        ] {
            assert!(!is_subtitle(s, Lang::En), "{s}");
            assert!(!is_label(s, Lang::En), "{s}");
        }
        assert!(is_subtitle("There's nothing quite like being on duty in a bustling commercial area.", Lang::En));
        assert!(is_subtitle("I bought a new iPhone, and it was worth every coin I had saved.", Lang::En));
        assert!(is_label("Data Bank", Lang::En));
        assert!(is_label("Sub-Stats", Lang::En));
        assert!(is_label("SOL3 Phase", Lang::En));
        assert!(is_label("Lv70 NPCs Rewards", Lang::En));
        assert!(is_subtitle("Meet me on the 2nd floor at 10am, and bring the TV.", Lang::En));
    }

    #[test]
    fn label_filter() {
        assert!(is_label("Inventory", Lang::En));
        assert!(is_label("Lv. 70", Lang::En) == false);
        assert!(is_label("Go", Lang::En));
        assert!(!is_label("XP", Lang::En));
    }
}
