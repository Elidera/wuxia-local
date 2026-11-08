//! English word list: is an OCR line real English or a fragment / misread?
//!
//! PaddleOCR finds much more small text than Windows OCR, including pieces of
//! words cut by an overlay or a window edge ("ierminal", "hing Fros") and
//! Russian read as Latin ("no Haxan.no"). Such lines must not be translated
//! as speech. The list (`assets/dict/en_words.txt`, from SCOWL / Hunspell)
//! holds base forms; common endings are stripped when looking a word up.

use std::collections::HashSet;

#[derive(Debug, Default, Clone)]
pub struct Words {
    set: HashSet<String>,
}

impl Words {
    /// One lower-case word per line; `#` starts a comment.
    pub fn parse(text: &str) -> Words {
        let set = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.to_lowercase())
            .collect();
        Words { set }
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    fn has(&self, w: &str) -> bool {
        !w.is_empty() && self.set.contains(w)
    }

    /// Is this a known English word (any case, common endings allowed)?
    pub fn known(&self, word: &str) -> bool {
        let w = word.to_lowercase().replace('’', "'");
        if self.has(&w) {
            return true;
        }
        // Possessives and contractions: "Rover's", "you'll", "isn't".
        for tail in ["'s", "'re", "'ll", "'ve", "'d", "'m", "n't", "s'"] {
            if let Some(stem) = w.strip_suffix(tail) {
                if self.has(stem) || self.has(&format!("{stem}s")) {
                    return true;
                }
            }
        }
        // Inflections: "words", "boxes", "tried", "making", "running", "quickly".
        for (tail, add) in [
            ("ies", "y"),
            ("ied", "y"),
            ("iest", "y"),
            ("ier", "y"),
            ("ily", "y"),
            ("es", ""),
            ("s", ""),
            ("ed", ""),
            ("ed", "e"),
            ("d", ""),
            ("ing", ""),
            ("ing", "e"),
            ("ly", ""),
            ("er", ""),
            ("er", "e"),
            ("est", ""),
            ("est", "e"),
            ("ness", ""),
            ("ment", ""),
        ] {
            if let Some(stem) = w.strip_suffix(tail) {
                let base = format!("{stem}{add}");
                // "ors" is not "or" + s: very short bases are not inflected here.
                if stem.len() < 2 || (base.len() < 3 && matches!(tail, "s" | "es" | "d" | "er" | "ly" | "est")) {
                    continue;
                }
                if self.has(&base) {
                    return true;
                }
                // Doubled consonant: "running" → "run", "stopped" → "stop".
                let b = stem.as_bytes();
                if add.is_empty() && b.len() >= 3 && b[b.len() - 1] == b[b.len() - 2] && self.has(&stem[..stem.len() - 1]) {
                    return true;
                }
            }
        }
        false
    }

    /// Does this line read as English speech? Names and game terms (Title
    /// Case words, all-caps abbreviations) are neither known nor unknown.
    /// A short line (up to 3 words) must be all known words or end like a
    /// sentence; a longer one needs at least 60 % known words. With an empty
    /// list everything passes.
    pub fn english_ok(&self, s: &str) -> bool {
        if self.is_empty() {
            return true;
        }
        let tokens: Vec<&str> =
            s.split(|c: char| !(c.is_alphabetic() || c == '\'' || c == '’')).map(|w| w.trim_matches(|c| c == '\'' || c == '’')).filter(|w| !w.is_empty()).collect();
        if tokens.is_empty() {
            return false;
        }
        let mut known = 0usize;
        let mut unknown = 0usize;
        let mut neutral = 0usize;
        for w in &tokens {
            if self.known(w) {
                known += 1;
                continue;
            }
            let mut chars = w.chars();
            let first_upper = chars.next().map_or(false, char::is_uppercase);
            let rest: Vec<char> = chars.collect();
            let title = first_upper && rest.len() >= 2 && rest.iter().all(|c| c.is_lowercase());
            let caps = w.chars().count() >= 2 && w.chars().all(char::is_uppercase);
            if title || caps {
                neutral += 1;
            } else {
                unknown += 1;
            }
        }
        let words = tokens.len();
        let ends = s.trim_end().ends_with(['.', '!', '?', '…', '"', '”']);
        if words <= 3 {
            // "Follow me" — all ordinary words; "Jinzhou!", "Rover, run!" — a finished sentence.
            // A lone two-letter word without punctuation ("er", "no") is a fragment.
            let lone_short = words == 1 && tokens[0].chars().count() <= 2;
            (known == words && !lone_short) || (ends && known >= unknown && known + neutral >= 1)
        } else {
            known >= 2 && known as f32 >= 0.6 * (known + unknown) as f32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Words {
        Words::parse(
            "# test\nyou\nvolunteer\nto\ncome\nhere\nthere\nnothing\nquite\nlike\nbe\non\nduty\nin\na\nbustle\ncommercial\narea\nall\nthat\ntime\ni\nspend\nspent\napply\nfor\nthis\njob\nfeel\nworth\nit\nno\nwait\nme\nrun\nbox\ntry\nmake\nquick\nfollow\nwe\nshould\nhead\nbefore\ndark\nterminal\nedit\nbio",
        )
    }

    #[test]
    fn inflections() {
        let w = list();
        for ok in ["volunteered", "being", "bustling", "applying", "feels", "running", "boxes", "tried", "making", "quickly", "You", "there's", "isn't"] {
            if ok == "isn't" {
                continue; // "is" is not in the small test list
            }
            assert!(w.known(ok), "{ok}");
        }
        for bad in ["ierminal", "hing", "esis", "nogxcav", "thisjob"] {
            assert!(!w.known(bad), "{bad}");
        }
    }

    #[test]
    fn real_speech_passes() {
        let w = list();
        assert!(w.english_ok("Langsheng: There's nothing quite like being on duty in a bustling commercial area. All that time I spent applying for this job feels worth it."));
        assert!(w.english_ok("You volunteered to come here?"));
        assert!(w.english_ok("You volunteered to come"));
        assert!(w.english_ok("Rover, wait for me!"));
        assert!(w.english_ok("Follow me"));
        assert!(w.english_ok("Jinzhou!"));
        assert!(w.english_ok("We should head to Jinzhou before dark."));
    }

    #[test]
    fn fragments_fail() {
        let w = list();
        for bad in ["ierminal", "no Haxan.no", "hing Fros", "nogxcav VA", "esis", "ike", "ors", "er"] {
            assert!(!w.english_ok(bad), "{bad}");
        }
    }

    #[test]
    fn empty_list_passes_everything() {
        assert!(Words::default().english_ok("ierminal"));
    }
}
