//! OCR text cleanup and normalization for Chinese game text.

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
            if (1..=8).contains(&n) && !name.chars().any(|c| is_cjk_punct(c) || c.is_ascii_punctuation()) {
                let rest = s[pos + sep.len_utf8()..].trim_start();
                if !rest.is_empty() {
                    return (Some(name), rest);
                }
            }
        }
    }
    (None, s)
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
    fn ratio() {
        assert!(han_ratio("江湖") > 0.99);
        assert!(han_ratio("Lv 30") < 0.01);
    }

    #[test]
    fn russian_tidy() {
        assert_eq!(tidy_russian("- Он сказал \"нет\"..."), "— Он сказал «нет»…");
    }
}
