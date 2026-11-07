//! Rolling dialogue context: the last few source lines and their translations.

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct Exchange {
    pub src: String,
    pub dst: String,
}

#[derive(Debug)]
pub struct History {
    cap: usize,
    items: VecDeque<Exchange>,
}

impl History {
    pub fn new(cap: usize) -> Self {
        Self { cap, items: VecDeque::with_capacity(cap + 1) }
    }

    pub fn push(&mut self, src: &str, dst: &str) {
        if self.cap == 0 {
            return;
        }
        if self.items.back().map_or(false, |e| e.src == src) {
            return;
        }
        self.items.push_back(Exchange { src: src.to_string(), dst: dst.to_string() });
        while self.items.len() > self.cap {
            self.items.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn items(&self) -> impl Iterator<Item = &Exchange> {
        self.items.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Source-only context (for translation-specialized models such as Hy-MT,
    /// whose contextual template expects plain preceding text).
    pub fn source_context(&self) -> String {
        self.items.iter().map(|e| e.src.as_str()).collect::<Vec<_>>().join("\n")
    }

    /// Paired context for instruct models.
    pub fn paired_context(&self) -> String {
        self.items
            .iter()
            .map(|e| format!("{} → {}", e.src, e.dst))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ring() {
        let mut h = History::new(2);
        h.push("一", "1");
        h.push("一", "1");
        h.push("二", "2");
        h.push("三", "3");
        assert_eq!(h.source_context(), "二\n三");
        assert_eq!(h.paired_context(), "二 → 2\n三 → 3");
    }
}
