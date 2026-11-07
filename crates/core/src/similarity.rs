//! Character-level similarity used to ignore OCR jitter on the same subtitle.

/// Levenshtein distance over Unicode scalar values (two-row DP).
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// 1.0 = identical, 0.0 = nothing in common.
pub fn ratio(a: &str, b: &str) -> f32 {
    let la = a.chars().count();
    let lb = b.chars().count();
    let max = la.max(lb);
    if max == 0 {
        return 1.0;
    }
    1.0 - levenshtein(a, b) as f32 / max as f32
}

/// True when `next` is the same subtitle as `prev`, possibly still being typed out
/// (typewriter effect): `prev` is a prefix of `next`.
pub fn is_growing(prev: &str, next: &str) -> bool {
    !prev.is_empty() && next.len() > prev.len() && next.starts_with(prev)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basics() {
        assert_eq!(levenshtein("江湖", "江湖"), 0);
        assert_eq!(levenshtein("江湖", "江河"), 1);
        assert!(ratio("少侠请留步", "少侠请留歩") > 0.79);
        assert!(ratio("你好", "再见") < 0.01);
        assert!(is_growing("此去", "此去开封"));
        assert!(!is_growing("此去开封", "此去"));
    }
}
