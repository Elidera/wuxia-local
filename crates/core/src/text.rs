//! OCR text cleanup and normalization for Chinese and English game text.

use crate::lang::Lang;

/// CJK ideographs (incl. extensions A and compatibility block).
pub fn is_han(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF)
}

/// Chinese/full-width punctuation that should hug neighbouring ideographs.
pub fn is_cjk_punct(c: char) -> bool {
    matches!(c as u32, 0x3000..=0x303F | 0xFF00..=0xFF0F | 0xFF1A..=0xFF20 | 0xFF3B..=0xFF40 | 0xFF5B..=0xFF65)
        || matches!(c, '…' | '—' | '“' | '”' | '‘' | '’' | '·')
}

fn is_cjkish(c: char) -> bool {
    is_han(c) || is_cjk_punct(c)
}

pub fn has_han(s: &str) -> bool {
    s.chars().any(is_han)
}

pub fn han_count(s: &str) -> usize {
    s.chars().filter(|&c| is_han(c)).count()
}

/// Share of ideographs among non-space characters.
pub fn han_ratio(s: &str) -> f32 {
    let total = s.chars().filter(|c| !c.is_whitespace()).count();
    if total == 0 {
        0.0
    } else {
        han_count(s) as f32 / total as f32
    }
}

/// Characters OCR engines often emit as noise around subtitle boxes.
fn is_edge_junk(c: char) -> bool {
    matches!(c, '|' | '_' | '-' | '~' | '`' | '\'' | '"' | '.' | ',' | '=' | '+' | '*' | '^' | '¦' | '丨' | '一')
        || c.is_whitespace()
}

/// Map half-width punctuation that sits between ideographs to the full-width form,
/// so the cache key does not depend on which variant OCR happened to produce.
fn fullwidth_punct(c: char) -> Option<char> {
    Some(match c {
        ',' => '，',
        '.' => '。',
        '?' => '？',
        '!' => '！',
        ':' => '：',
        ';' => '；',
        '(' => '（',
        ')' => '）',
        _ => return None,
    })
}

/// Normalize one OCR line:
/// * drop the spaces Windows OCR puts between every ideograph;
/// * unify punctuation between ideographs;
/// * trim edge noise (`|`, `_`, stray dashes) but keep a leading `一` only if it
///   is followed by another ideograph (it is a real word then, e.g. 一起).
pub fn normalize_line(raw: &str) -> String {
    let mut raw = raw.trim().replace("...", "……").replace("。。。", "……");
    // A lone leading "一" separated by a space is almost always a box border.
    if let Some(rest) = raw.strip_prefix('一') {
        if rest.starts_with(char::is_whitespace) {
            raw = rest.to_string();
        }
    }
    let chars: Vec<char> = raw.chars().filter(|c| !c.is_control()).collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            // Look at the previous kept char and the next non-space char.
            let prev = out.last().copied();
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            let next = chars.get(j).copied();
            let glue = matches!((prev, next), (Some(p), Some(n)) if is_cjkish(p) || is_cjkish(n));
            if !glue && prev.is_some() && next.is_some() {
                out.push(' ');
            }
            i = j;
            continue;
        }
        let prev_han = out.last().map_or(false, |&p| is_han(p));
        let next_han = chars.get(i + 1).map_or(false, |&n| is_han(n) || n.is_whitespace());
        if let Some(fw) = fullwidth_punct(c) {
            if prev_han || (next_han && out.last().map_or(false, |&p| is_cjkish(p))) {
                out.push(fw);
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }

    // Trim edge junk.
    let mut start = 0;
    while start < out.len() {
        let c = out[start];
        let real_yi = c == '一' && out.get(start + 1).map_or(false, |&n| is_han(n));
        if is_edge_junk(c) && !real_yi {
            start += 1;
        } else {
            break;
        }
    }
    let mut end = out.len();
    while end > start {
        let c = out[end - 1];
        // keep sentence-final ASCII '.'/'!' only if the text is not Chinese
        if is_edge_junk(c) && c != '一' {
            end -= 1;
        } else {
            break;
        }
    }
    out[start..end].iter().collect()
}

/// Normalize one OCR line of English (or other Latin-script) text:
/// collapse spaces, unify quotes/dashes, trim box borders but keep sentence
/// punctuation and quotes.
pub fn normalize_line_en(raw: &str) -> String {
    let s: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '‘' | '’' | '`' | '´' => '\'',
            '“' | '”' | '„' => '"',
            '…' => '\u{2026}',
            c => c,
        })
        .collect();
    let mut s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    // OCR often splits punctuation off the word: "Wait , Rover !" → "Wait, Rover!"
    for p in [" ,", " .", " !", " ?", " ;", " :"] {
        while s.contains(p) {
            s = s.replace(p, &p[1..]);
        }
    }
    let s = fix_digits_in_words(&s);
    let junk = |c: char| matches!(c, '|' | '_' | '~' | '=' | '+' | '*' | '^' | '¦' | '丨' | '•' | '·') || c.is_whitespace();
    let s = s.trim_matches(junk);
    // A lone dash/bullet at the start is a box border or list marker.
    let s = s.trim_start_matches(|c: char| c == '-' || c == '—').trim_start();
    s.to_string()
}

/// Typical OCR digit-for-letter mix-ups inside English words: "J0b" → "Job",
/// "a1l" → "all", "|" between letters → "l". Numbers ("SOL3", "300", "2nd")
/// are left alone: only a digit with letters on both sides is changed.
fn fix_digits_in_words(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let letter = |i: usize| c.get(i).map_or(false, |ch| ch.is_alphabetic());
    let mut out = String::with_capacity(s.len());
    for i in 0..c.len() {
        let between = i > 0 && letter(i - 1) && letter(i + 1);
        let ch = match c[i] {
            '0' if between => 'o',
            '1' | '|' if between => 'l',
            ch => ch,
        };
        out.push(ch);
    }
    out
}

/// Normalize for the given source language.
pub fn normalize_for(raw: &str, lang: Lang) -> String {
    match lang {
        Lang::Zh => normalize_line(raw),
        Lang::En => normalize_line_en(raw),
    }
}

/// Join English subtitle lines: with spaces, undoing hyphenated line breaks
/// ("transla-" + "tion" → "translation").
pub fn join_lines_en<S: AsRef<str>>(lines: &[S]) -> String {
    let mut out = String::new();
    for l in lines {
        let l = normalize_line_en(l.as_ref());
        if l.is_empty() {
            continue;
        }
        if out.is_empty() {
            out = l;
            continue;
        }
        let hyphen_wrap = out.ends_with('-')
            && out.chars().rev().nth(1).map_or(false, |c| c.is_alphabetic())
            && l.chars().next().map_or(false, |c| c.is_lowercase());
        if hyphen_wrap {
            out.pop();
        } else {
            out.push(' ');
        }
        out.push_str(&l);
    }
    out
}

pub fn join_lines_for<S: AsRef<str>>(lines: &[S], lang: Lang) -> String {
    match lang {
        Lang::Zh => join_lines(lines),
        Lang::En => join_lines_en(lines),
    }
}

/// Does a short OCR line look like an English speaker name shown above the
/// dialogue line ("Jiyan", "Black Shores Researcher")? Title-case words, no
/// sentence punctuation, at most 4 words.
pub fn looks_like_name_en(s: &str) -> bool {
    // OCR sometimes reads one letter of a name as "?": "Langsh?ng".
    let cleaned = drop_inner_question_marks(s.trim());
    let s = cleaned.as_str();
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.is_empty() || words.len() > 4 || s.chars().count() > 32 {
        return false;
    }
    if s.chars().any(|c| matches!(c, '.' | ',' | '!' | '?' | ';' | ':' | '"' | '…') || c.is_ascii_digit()) {
        return false;
    }
    let small = ["of", "the", "de", "von", "a"];
    words.iter().all(|w| {
        w.chars().next().map_or(false, |c| c.is_uppercase()) || small.contains(&w.to_lowercase().as_str())
    }) && words[0].chars().next().map_or(false, |c| c.is_uppercase())
}

/// "Langsh?ng" → "Langshng": a "?" between two letters is an unread letter.
fn drop_inner_question_marks(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    (0..c.len())
        .filter(|&i| !(c[i] == '?' && i > 0 && i + 1 < c.len() && c[i - 1].is_alphabetic() && c[i + 1].is_alphabetic()))
        .map(|i| c[i])
        .collect()
}

/// A speaker name line above English dialogue. Like [`looks_like_name_en`], but
/// tolerates OCR breaking a name in two ("Lang heng" for "Langsheng"): two words,
/// the first capitalised, short.
pub fn looks_like_speaker_en(s: &str) -> bool {
    if looks_like_name_en(s) {
        return true;
    }
    let s = s.trim();
    let words: Vec<&str> = s.split_whitespace().collect();
    // "Remember this:", "Listen up:" are speech, not names.
    const COMMON: &[&str] = &[
        "this", "that", "these", "those", "it", "its", "me", "you", "your", "him", "her", "them", "us", "our", "now", "then",
        "here", "there", "is", "was", "are", "were", "the", "and", "but", "what", "why", "how", "who", "not", "all", "one",
        "out", "off", "up", "down", "too", "yes", "well", "please", "again", "first", "next", "last", "said", "says", "see",
    ];
    words.len() == 2
        && !COMMON.contains(&words[1].to_lowercase().as_str())
        && s.chars().count() <= 16
        && words[0].chars().next().map_or(false, |c| c.is_uppercase())
        && words[1].chars().count() >= 3
        && s.chars().all(|c| c.is_alphabetic() || c == ' ' || c == '-' || c == '\'')
}

/// Join several OCR lines of one subtitle block.
/// Chinese lines are concatenated without a separator; Latin ones with a space.
pub fn join_lines<S: AsRef<str>>(lines: &[S]) -> String {
    let mut out = String::new();
    for l in lines {
        let l = normalize_line(l.as_ref());
        if l.is_empty() {
            continue;
        }
        if let (Some(p), Some(n)) = (out.chars().last(), l.chars().next()) {
            if !(is_cjkish(p) || is_cjkish(n)) {
                out.push(' ');
            }
        }
        out.push_str(&l);
    }
    trim_trailing_garbage(&out)
}

/// Drop short OCR garbage after the last sentence-final mark
/// (box borders, icons read as letters): `你来了。Il` → `你来了。`.
pub fn trim_trailing_garbage(s: &str) -> String {
    let finals = ['。', '！', '？', '…', '」', '”', '』', '）'];
    if let Some((idx, ch)) = s.char_indices().filter(|(_, c)| finals.contains(c)).last() {
        let tail = &s[idx + ch.len_utf8()..];
        let n = tail.chars().count();
        if n > 0 && n <= 3 && !tail.chars().any(is_han) {
            return s[..idx + ch.len_utf8()].to_string();
        }
    }
    s.to_string()
}

/// Split a `Speaker：line` subtitle if the prefix looks like a name (short, no punctuation).
pub fn split_speaker(s: &str) -> (Option<&str>, &str) {
    for sep in ['：', ':'] {
        if let Some(pos) = s.find(sep) {
            let name = &s[..pos];
            let n = name.chars().count();
            let ok = if has_han(name) {
                (1..=8).contains(&n) && !name.chars().any(|c| is_cjk_punct(c) || c.is_ascii_punctuation())
            } else {
                looks_like_speaker_en(name)
            };
            if ok {
                let rest = s[pos + sep.len_utf8()..].trim_start();
                if !rest.is_empty() {
                    return (Some(name), rest);
                }
            }
        }
    }
    (None, s)
}

/// "1 надпись", "2 надписи", "5 надписей", "21 надпись", "12 надписей".
pub fn count_ru(n: usize, one: &str, few: &str, many: &str) -> String {
    let word = match (n % 10, n % 100) {
        (_, 11..=14) => many,
        (1, _) => one,
        (2..=4, _) => few,
        _ => many,
    };
    format!("{n} {word}")
}

/// Light Russian typography cleanup on model output.
pub fn tidy_russian(s: &str) -> String {
    let mut t = s.trim().to_string();
    t = t.replace("...", "…");
    // Dialogue dash at line start.
    if let Some(rest) = t.strip_prefix("- ") {
        t = format!("— {rest}");
    }
    t = t.replace(" - ", " — ");
    // Straight double quotes → «ёлочки», alternating.
    if t.contains('"') {
        let mut open = true;
        t = t
            .chars()
            .map(|c| {
                if c == '"' {
                    let r = if open { '«' } else { '»' };
                    open = !open;
                    r
                } else {
                    c
                }
            })
            .collect();
    }
    // Collapse doubled spaces.
    while t.contains("  ") {
        t = t.replace("  ", " ");
    }
    t
}

#[cfg(test)]
mod tests {
    #[test]
    fn ocr_digits_inside_words() {
        assert_eq!(normalize_line_en("applying forthisJ0b feels a1l right"), "applying forthisJob feels all right");
        assert_eq!(normalize_line_en("SOL3 phase, 300 credits, the 2nd floor, Q4_K_M"), "SOL3 phase, 300 credits, the 2nd floor, Q4_K_M");
        assert_eq!(normalize_line_en("He|lo"), "Hello");
    }

    #[test]
    fn russian_plural() {
        let f = |n| count_ru(n, "надпись", "надписи", "надписей");
        assert_eq!(f(1), "1 надпись");
        assert_eq!(f(2), "2 надписи");
        assert_eq!(f(5), "5 надписей");
        assert_eq!(f(11), "11 надписей");
        assert_eq!(f(21), "21 надпись");
        assert_eq!(f(112), "112 надписей");
    }

    #[test]
    fn speaker_split_by_ocr() {
        assert!(looks_like_speaker_en("Lang heng"));
        assert!(looks_like_speaker_en("Langsh?ng"));
        assert!(!looks_like_speaker_en("Really?"));
        assert!(looks_like_speaker_en("Black Shores Researcher"));
        assert!(!looks_like_speaker_en("Thank you for coming"));
        assert!(!looks_like_speaker_en("Wait, what"));
        assert_eq!(split_speaker("Lan sheng: Hello there."), (Some("Lan sheng"), "Hello there."));
    }

    use super::*;

    #[test]
    fn removes_spaces_between_han() {
        assert_eq!(normalize_line("江 湖 险 恶 , 少 侠 保 重 !"), "江湖险恶，少侠保重！");
    }

    #[test]
    fn keeps_latin_spaces() {
        assert_eq!(normalize_line("Lv. 30  剑法"), "Lv. 30剑法");
        assert_eq!(normalize_line("Hello   world"), "Hello world");
    }

    #[test]
    fn trims_edge_noise() {
        assert_eq!(normalize_line("| _ 你来了 |"), "你来了");
        assert_eq!(normalize_line("一起走吧"), "一起走吧");
        assert_eq!(normalize_line("一 你好"), "你好");
        assert_eq!(normalize_line("等等..."), "等等……");
    }

    #[test]
    fn joins_two_line_subtitle() {
        assert_eq!(join_lines(&["此去开封", "路途遥远"]), "此去开封路途遥远");
        assert_eq!(join_lines(&["Press", "F to talk"]), "Press F to talk");
    }

    #[test]
    fn trailing_garbage() {
        assert_eq!(trim_trailing_garbage("你来了。Il"), "你来了。");
        assert_eq!(trim_trailing_garbage("你来了。好"), "你来了。好");
        assert_eq!(trim_trailing_garbage("你来了"), "你来了");
        assert_eq!(join_lines(&["你终于来了！", "lI"]), "你终于来了！");
    }

    #[test]
    fn speaker_split() {
        assert_eq!(split_speaker("沈七：你终于来了。"), (Some("沈七"), "你终于来了。"));
        assert_eq!(split_speaker("他说：好，走吧。"), (Some("他说"), "好，走吧。"));
        assert_eq!(split_speaker("你好，世界：不"), (None, "你好，世界：不"));
    }

    #[test]
    fn english_lines() {
        assert_eq!(normalize_line_en("| We should go , Rover !  _"), "We should go, Rover!");
        assert_eq!(normalize_line_en("“Don’t worry.”"), "\"Don't worry.\"");
        assert_eq!(normalize_line_en("- Wait..."), "Wait...");
        assert_eq!(join_lines_en(&["The resonance is unsta-", "ble here.", "Be careful."]), "The resonance is unstable here. Be careful.");
        assert_eq!(join_lines_en(&["Black Shores", "-"]), "Black Shores");
        assert!(looks_like_name_en("Jiyan"));
        assert!(looks_like_name_en("Black Shores Researcher"));
        assert!(!looks_like_name_en("Wait for me."));
        assert!(!looks_like_name_en("we go now"));
        assert_eq!(split_speaker("Jiyan: The frontline needs us."), (Some("Jiyan"), "The frontline needs us."));
        assert_eq!(split_speaker("Remember this: never give up."), (None, "Remember this: never give up."));
    }

    #[test]
    fn ratio() {
        assert!(han_ratio("江湖") > 0.99);
        assert!(han_ratio("Lv 30") < 0.01);
    }

    #[test]
    fn russian_tidy() {
        assert_eq!(tidy_russian("- Он сказал \"нет\"..."), "— Он сказал «нет»…");
    }
}
