//! Minimal TSV escaping so user files stay editable in Notepad.

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o)
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse TSV lines, skipping blanks and `#` comments. Returns unescaped fields.
pub fn parse(content: &str) -> Vec<Vec<String>> {
    content
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| l.split('\t').map(unescape).collect())
        .collect()
}

pub fn line(fields: &[&str]) -> String {
    let mut s = fields.iter().map(|f| escape(f)).collect::<Vec<_>>().join("\t");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let s = "a\tb\nc\\d";
        assert_eq!(unescape(&escape(s)), s);
        let rows = parse(&format!("# c\n\n{}", line(&["江湖", "цзянху\tx"])));
        assert_eq!(rows, vec![vec!["江湖".to_string(), "цзянху\tx".to_string()]]);
    }
}
