//! Bundled assets must parse and interact sensibly with the core.
use wuxia_core::glossary::Glossary;
use wuxia_core::prompt::{build, PromptInput, PromptStyle, TextKind};

const GLOSSARY: &str = include_str!("../../../assets/glossary/00_wuxia_common.tsv");
const BENCH: &str = include_str!("../../../assets/bench/wwm_lines.txt");

#[test]
fn glossary_parses() {
    let mut g = Glossary::new();
    let n = g.load_tsv(GLOSSARY);
    assert!(n >= 50, "terms: {n}");
    assert_eq!(n, g.len(), "duplicate source terms in bundled glossary");
    assert_eq!(g.exact("开封").unwrap().dst, "Кайфэн");
}

#[test]
fn bench_lines_get_terms_and_prompts() {
    let mut g = Glossary::new();
    g.load_tsv(GLOSSARY);
    let lines: Vec<&str> = BENCH.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).collect();
    assert!(lines.len() >= 20);
    let with_terms = lines.iter().filter(|l| !g.find_in(l, 24).is_empty()).count();
    assert!(with_terms >= 10, "lines with glossary hits: {with_terms}");
    for l in &lines {
        assert_eq!(&wuxia_core::text::normalize_line(l), l, "bench line not normalized: {l}");
        let inp = PromptInput { source: l, kind: TextKind::Dialogue, terms: g.find_in(l, 24), context_src: String::new(), context_pairs: String::new(), game: "WWM", style_hint: "" };
        for s in [PromptStyle::HyMt, PromptStyle::Instruct] {
            assert!(build(s, &inp).last().unwrap().content.ends_with(l));
        }
    }
    // Longest match: 燕云十六州 must win over 燕云.
    let t: Vec<_> = g.find_in("燕云十六州，自古便是兵家必争之地。", 5).iter().map(|t| t.src.clone()).collect();
    assert_eq!(t, vec!["燕云十六州"]);
}
