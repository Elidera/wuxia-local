//! Bundled assets must parse and interact sensibly with the core.
use wuxia_core::glossary::Glossary;
use wuxia_core::prompt::{build, PromptInput, PromptStyle, TextKind};

const GLOSSARY: &str = include_str!("../../../assets/glossary/wwm_wuxia_common.tsv");
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
        let inp = PromptInput { source: l, kind: TextKind::Dialogue, terms: g.find_in(l, 24), context_src: String::new(), context_pairs: String::new(), game: "WWM", style_hint: "" , speaker: None};
        for s in [PromptStyle::HyMt, PromptStyle::Instruct] {
            assert!(build(s, &inp).last().unwrap().content.ends_with(l));
        }
    }
    // Longest match: 燕云十六州 must win over 燕云.
    let t: Vec<_> = g.find_in("燕云十六州，自古便是兵家必争之地。", 5).iter().map(|t| t.src.clone()).collect();
    assert_eq!(t, vec!["燕云十六州"]);
}

#[test]
fn wuwa_glossary_and_english_lines() {
    let mut g = wuxia_core::glossary::Glossary::new();
    let n = g.load_tsv(include_str!("../../../assets/glossary/wuwa_terms.tsv"));
    assert!(n > 50, "WuWa glossary has {n} terms");
    let lines = include_str!("../../../assets/bench/wuwa_en_lines.txt");
    let mut with_terms = 0;
    for l in lines.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
        assert_eq!(wuxia_core::lang::detect(l), Some(wuxia_core::lang::Lang::En), "{l}");
        assert!(wuxia_core::lang::is_subtitle(l, wuxia_core::lang::Lang::En), "not a subtitle: {l}");
        if !g.find_in(l, 8).is_empty() {
            with_terms += 1;
        }
    }
    assert!(with_terms >= 10, "only {with_terms} test lines use glossary terms");
}

const EN_WORDS: &str = include_str!("../../../assets/dict/en_words.txt");
const EN_LINES: &str = include_str!("../../../assets/bench/wuwa_en_lines.txt");

#[test]
fn english_word_list_accepts_real_lines() {
    let w = wuxia_core::words::Words::parse(EN_WORDS);
    assert!(w.len() > 50_000, "words: {}", w.len());
    for l in EN_LINES.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
        assert!(w.english_ok(l), "real line rejected: {l}");
    }
    for bad in ["ierminal", "no Haxan.no", "hing Fros", "nogxcav VA", "esis", "ike", "ors", "er", "IoTOB", "Cy6Tmp", "5TOMaTHK"] {
        assert!(!w.english_ok(bad), "fragment accepted: {bad}");
    }
}
