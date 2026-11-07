//! Translation memory.
//!
//! Two stores share one type:
//! * the automatic cache, keyed by (model, kind, source) — a different model
//!   gets its own entries so comparing models is fair;
//! * user corrections, keyed by source only — always win over the model.
//!
//! Persistence is append-only TSV (`key fields… <TAB> translation`): crash-safe,
//! human-readable, and the last line for a key wins on reload.

use crate::tsv;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct Store {
    map: HashMap<String, String>,
}

fn key(parts: &[&str]) -> String {
    parts.join("\u{1f}")
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Load TSV where the last column is the value and all previous columns form the key.
    pub fn load_tsv(&mut self, content: &str) -> usize {
        let mut n = 0;
        for row in tsv::parse(content) {
            if row.len() >= 2 {
                let (val, k) = row.split_last().unwrap();
                let k: Vec<&str> = k.iter().map(String::as_str).collect();
                if val.is_empty() {
                    self.map.remove(&key(&k)); // empty value = deletion marker
                } else {
                    self.map.insert(key(&k), val.clone());
                }
                n += 1;
            }
        }
        n
    }

    pub fn get(&self, parts: &[&str]) -> Option<&str> {
        self.map.get(&key(parts)).map(String::as_str)
    }

    /// Insert and return the TSV line to append to the backing file.
    pub fn insert(&mut self, parts: &[&str], value: &str) -> String {
        self.map.insert(key(parts), value.to_string());
        let mut fields: Vec<&str> = parts.to_vec();
        fields.push(value);
        tsv::line(&fields)
    }

    /// Remove and return the deletion line to append.
    pub fn remove(&mut self, parts: &[&str]) -> String {
        self.map.remove(&key(parts));
        let mut fields: Vec<&str> = parts.to_vec();
        fields.push("");
        tsv::line(&fields)
    }

    /// Compact representation (one line per live key) for rewriting the file.
    pub fn dump(&self) -> String {
        let mut keys: Vec<&String> = self.map.keys().collect();
        keys.sort();
        let mut out = String::new();
        for k in keys {
            let mut fields: Vec<&str> = k.split('\u{1f}').collect();
            fields.push(&self.map[k]);
            out.push_str(&tsv::line(&fields));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn append_only_last_wins() {
        let mut s = Store::new();
        let mut file = String::new();
        file += &s.insert(&["hymt", "d", "你好"], "Привет");
        file += &s.insert(&["hymt", "d", "你好"], "Здравствуй");
        file += &s.insert(&["hymt", "d", "再见"], "Пока");
        file += &s.remove(&["hymt", "d", "再见"]);
        let mut r = Store::new();
        r.load_tsv(&file);
        assert_eq!(r.get(&["hymt", "d", "你好"]), Some("Здравствуй"));
        assert_eq!(r.get(&["hymt", "d", "再见"]), None);
        assert_eq!(r.len(), 1);
        let mut r2 = Store::new();
        r2.load_tsv(&r.dump());
        assert_eq!(r2.get(&["hymt", "d", "你好"]), Some("Здравствуй"));
    }
}
