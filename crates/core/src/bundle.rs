//! Sharing dictionaries between players: one text file with the user's
//! glossary, OCR fixes and own translations (export), merged into the local
//! files without duplicates (import).
//!
//! Format: sections `[[user.tsv]]`, `[[fixes.tsv]]`, `[[corrections.tsv]]`
//! followed by the file's lines — readable and editable in Notepad.

/// Files that are shared (the user's own, never the bundled ones).
pub const FILES: [&str; 3] = ["user.tsv", "fixes.tsv", "corrections.tsv"];

const HEADER: &str = "# Wuxia Local — словари для обмена. Импорт: Словари → Импорт…\n";

/// Pack (file name, content) pairs into one bundle.
pub fn pack(parts: &[(&str, &str)]) -> String {
    let mut s = String::from(HEADER);
    for (name, content) in parts {
        s.push_str(&format!("[[{name}]]\n"));
        for line in content.lines() {
            // A line looking like a section marker would split the file wrongly.
            if !(line.starts_with("[[") && line.trim_end().ends_with("]]")) {
                s.push_str(line);
                s.push('\n');
            }
        }
    }
    s
}

/// Sections of a bundle: (file name, content). Unknown names are kept so the
/// caller can report them.
pub fn unpack(s: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in s.lines() {
        let t = line.trim_end();
        if t.starts_with("[[") && t.ends_with("]]") && t.len() > 4 {
            out.push((t[2..t.len() - 2].trim().to_string(), String::new()));
        } else if let Some((_, content)) = out.last_mut() {
            content.push_str(line);
            content.push('\n');
        }
    }
    out
}

/// Append the meaningful lines of `incoming` that `existing` does not have.
/// Returns the new content and how many lines were added.
pub fn merge(existing: &str, incoming: &str) -> (String, usize) {
    let have: std::collections::HashSet<&str> = existing.lines().map(str::trim_end).collect();
    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    let mut added = 0;
    let mut seen = std::collections::HashSet::new();
    for line in incoming.lines().map(str::trim_end) {
        if line.trim().is_empty() || line.trim_start().starts_with('#') || have.contains(line) || !seen.insert(line) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        added += 1;
    }
    (out, added)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_merge() {
        let b = pack(&[("user.tsv", "# мой\nRover\tСкиталец\n"), ("fixes.tsv", "skip\tUser ID\n[[evil]]\n")]);
        let parts = unpack(&b);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], ("user.tsv".into(), "# мой\nRover\tСкиталец\n".into()));
        assert_eq!(parts[1].1, "skip\tUser ID\n");
        let (merged, n) = merge("# мой\nJiyan\tЦзиянь", &parts[0].1);
        assert_eq!(n, 1);
        assert_eq!(merged, "# мой\nJiyan\tЦзиянь\nRover\tСкиталец\n");
        let (again, n2) = merge(&merged, &parts[0].1);
        assert_eq!((again.as_str(), n2), (merged.as_str(), 0));
        assert!(unpack("no sections at all\n").is_empty());
    }
}
