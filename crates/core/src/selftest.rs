//! Screenshot tests: what the translator must (and must not) find on a picture.
//!
//! Each test case is a game screenshot `name.png` with a text file `name.expect`
//! next to it. The app runs its real pipeline on the picture (Windows OCR, the
//! same filters and grouping as in the game, see `wuxia-local --selftest`) and
//! this module compares the result with the expectations.
//!
//! `.expect` format — one rule per line, `key<TAB>value` (or `key value`), `#` comments:
//!
//! ```text
//! lang      en                      # en or zh (default: en)
//! speaker   Langsheng               # a speech block has this speaker name
//! speech    nothing quite like being on duty   # some speech block contains this
//! absent    User ID                 # nothing found contains this
//! label     Union Level             # F8 (screen translation) finds this label
//! max_speech 3                      # at most this many speech blocks
//! ```
//! Matching is tolerant to OCR noise (case, spaces, accents, a few wrong letters).

use crate::lang::Lang;
use crate::memory::fold;
use crate::similarity;
use crate::text;

#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    Speech(String),
    Speaker(String),
    Absent(String),
    Label(String),
    MaxSpeech(usize),
}

#[derive(Debug, Clone)]
pub struct Expect {
    pub lang: Lang,
    pub rules: Vec<Rule>,
}

/// What the pipeline found on a screenshot.
#[derive(Debug, Clone, Default)]
pub struct Found {
    /// Speech blocks as the auto mode would translate them ("Name: text").
    pub speech: Vec<String>,
    /// Labels F8 would translate.
    pub labels: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub ok: bool,
    pub rule: String,
    pub detail: String,
}

pub fn parse(s: &str) -> Result<Expect, String> {
    let mut e = Expect { lang: Lang::En, rules: Vec::new() };
    for (n, raw) in s.lines().enumerate() {
        let line = raw.split(" #").next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, val) = match line.split_once('\t').or_else(|| line.split_once(' ')) {
            Some((k, v)) => (k.trim().to_lowercase(), v.trim().to_string()),
            None => (line.to_lowercase(), String::new()),
        };
        let need = |v: &str| if v.is_empty() { Err(format!("строка {}: «{key}» без значения", n + 1)) } else { Ok(()) };
        match key.as_str() {
            "lang" => e.lang = Lang::from_code(&val),
            "speech" => need(&val).map(|_| e.rules.push(Rule::Speech(val)))?,
            "speaker" => need(&val).map(|_| e.rules.push(Rule::Speaker(val)))?,
            "absent" => need(&val).map(|_| e.rules.push(Rule::Absent(val)))?,
            "label" => need(&val).map(|_| e.rules.push(Rule::Label(val)))?,
            "max_speech" => e.rules.push(Rule::MaxSpeech(val.parse().map_err(|_| format!("строка {}: max_speech — нужно число", n + 1))?)),
            other => return Err(format!("строка {}: неизвестное правило «{other}»", n + 1)),
        }
    }
    Ok(e)
}

/// How well `needle` occurs inside `hay` (1.0 = exactly), tolerant to OCR noise.
pub fn fuzzy_contains(hay: &str, needle: &str) -> f32 {
    let h: Vec<char> = fold(hay).chars().collect();
    let n: String = fold(needle);
    let m = n.chars().count();
    if m == 0 {
        return 1.0;
    }
    if h.len() <= m {
        return similarity::ratio(&h.iter().collect::<String>(), &n);
    }
    let mut best = 0.0f32;
    let step = if m > 40 { 2 } else { 1 };
    let mut i = 0;
    while i + m <= h.len() {
        for w in [m.saturating_sub(m / 10), m, m + m / 10] {
            if w == 0 || i + w > h.len() {
                continue;
            }
            let win: String = h[i..i + w].iter().collect();
            best = best.max(similarity::ratio(&win, &n));
        }
        if best >= 0.999 {
            break;
        }
        i += step;
    }
    best
}

const HIT: f32 = 0.85;

fn best_of<'a>(items: impl Iterator<Item = &'a String>, needle: &str) -> (f32, String) {
    items.map(|s| (fuzzy_contains(s, needle), s.clone())).fold((0.0, String::new()), |a, b| if b.0 > a.0 { b } else { a })
}

pub fn check(e: &Expect, f: &Found) -> Vec<Check> {
    let pct = |x: f32| format!("{:.0} %", x * 100.0);
    e.rules
        .iter()
        .map(|r| match r {
            Rule::Speech(s) => {
                let (score, block) = best_of(f.speech.iter(), s);
                Check {
                    ok: score >= HIT,
                    rule: format!("реплика содержит «{s}»"),
                    detail: if block.is_empty() { "реплик не найдено".into() } else { format!("лучшее совпадение {}: «{block}»", pct(score)) },
                }
            }
            Rule::Speaker(name) => {
                let speakers: Vec<String> = f.speech.iter().filter_map(|b| text::split_speaker(b).0.map(str::to_string)).collect();
                let best = speakers.iter().map(|s| (similarity::ratio(&fold(s), &fold(name)), s.clone())).fold((0.0, String::new()), |a, b| if b.0 > a.0 { b } else { a });
                Check {
                    ok: best.0 >= 0.8,
                    rule: format!("имя говорящего «{name}»"),
                    detail: if speakers.is_empty() { "имя не отделено ни в одной реплике".into() } else { format!("найдены имена: {}", speakers.join(", ")) },
                }
            }
            Rule::Absent(s) => {
                let (score, block) = best_of(f.speech.iter().chain(f.labels.iter()), s);
                Check {
                    ok: score < 0.9,
                    rule: format!("нет «{s}»"),
                    detail: if score < 0.9 { "не найдено — верно".into() } else { format!("найдено ({}) в «{block}»", pct(score)) },
                }
            }
            Rule::Label(s) => {
                let (score, block) = best_of(f.labels.iter(), s);
                Check {
                    ok: score >= HIT,
                    rule: format!("надпись «{s}» (F8)"),
                    detail: if block.is_empty() { "надписей не найдено".into() } else { format!("лучшее совпадение {}: «{block}»", pct(score)) },
                }
            }
            Rule::MaxSpeech(n) => Check {
                ok: f.speech.len() <= *n,
                rule: format!("реплик не больше {n}"),
                detail: format!("найдено {}", f.speech.len()),
            },
        })
        .collect()
}

/// One screenshot's result for the report.
pub struct CaseResult {
    pub name: String,
    pub found: Found,
    pub checks: Vec<Check>,
    pub ocr_ms: u128,
    pub error: Option<String>,
}

impl CaseResult {
    pub fn passed(&self) -> bool {
        self.error.is_none() && self.checks.iter().all(|c| c.ok)
    }
}

/// Markdown report (shown on the CI summary page and saved next to the cases).
pub fn report(cases: &[CaseResult], title: &str) -> String {
    let ok = cases.iter().filter(|c| c.passed()).count();
    let rules: usize = cases.iter().map(|c| c.checks.len()).sum();
    let rules_ok: usize = cases.iter().map(|c| c.checks.iter().filter(|x| x.ok).count()).sum();
    let mut s = format!("# {title}\n\nСнимков: **{ok} из {} прошли**, правил выполнено: {rules_ok} из {rules}.\n\n", cases.len());
    s.push_str("| Снимок | Итог | Правила | OCR |\n|---|---|---|---|\n");
    for c in cases {
        let mark = if c.passed() { "✅" } else { "❌" };
        let n_ok = c.checks.iter().filter(|x| x.ok).count();
        s.push_str(&format!("| {} | {mark} | {n_ok}/{} | {} мс |\n", c.name, c.checks.len(), c.ocr_ms));
    }
    for c in cases {
        s.push_str(&format!("\n## {} {}\n\n", if c.passed() { "✅" } else { "❌" }, c.name));
        if let Some(e) = &c.error {
            s.push_str(&format!("**Ошибка:** {e}\n\n"));
        }
        for ch in &c.checks {
            s.push_str(&format!("- {} {} — {}\n", if ch.ok { "✅" } else { "❌" }, ch.rule, ch.detail));
        }
        s.push_str("\nНайденные реплики:\n");
        if c.found.speech.is_empty() {
            s.push_str("- (нет)\n");
        }
        for b in &c.found.speech {
            s.push_str(&format!("- `{}`\n", b.replace('`', "'")));
        }
        if !c.found.labels.is_empty() {
            let labels: Vec<String> = c.found.labels.iter().take(60).map(|l| format!("`{}`", l.replace('`', "'"))).collect();
            s.push_str(&format!("\nНадписи для F8 ({}): {}\n", c.found.labels.len(), labels.join(" · ")));
        }
    }
    s
}

/// Draft `.expect` for a freshly saved case: what was found now, to be corrected
/// by hand (wrong lines deleted or turned into `absent`).
pub fn draft(found: &Found, lang: Lang) -> String {
    let mut s = format!(
        "# Что программа должна найти на этом снимке. Исправьте: уберите лишнее,\n\
         # ошибочное превратите в «absent …», допишите пропущенное.\n\
         lang\t{}\n",
        lang.code()
    );
    for b in &found.speech {
        let (sp, body) = text::split_speaker(b);
        if let Some(sp) = sp {
            s.push_str(&format!("speaker\t{sp}\n"));
        }
        s.push_str(&format!("speech\t{}\n", body.chars().take(60).collect::<String>()));
    }
    s.push_str(&format!("max_speech\t{}\n", found.speech.len().max(1)));
    for l in found.labels.iter().take(12) {
        s.push_str(&format!("# label\t{l}\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found() -> Found {
        Found {
            speech: vec![
                "Lang heng: There's nothing quite likefbeing on duty in a bustling commercial area. All thattime I spent applying forthisJ0b feels worth it.".into(),
                "You volunteered to come here?".into(),
            ],
            labels: vec!["Union Level".into(), "Events".into()],
        }
    }

    #[test]
    fn parse_and_check() {
        let e = parse(
            "# demo\nlang\ten\nspeaker\tLangsheng\nspeech\tnothing quite like being on duty\nspeech applying for this job\nabsent\tUser ID\nlabel\tUnion Level\nmax_speech 3\n",
        )
        .unwrap();
        assert_eq!(e.rules.len(), 6);
        let r = check(&e, &found());
        assert!(r.iter().all(|c| c.ok), "{r:?}");
    }

    #[test]
    fn failures_are_reported() {
        let e = parse("speaker\tJiyan\nspeech\tthe frontline won't hold\nabsent\tvolunteered\nlabel\tData Bank\nmax_speech 1\n").unwrap();
        let r = check(&e, &found());
        assert_eq!(r.iter().filter(|c| !c.ok).count(), 5, "{r:?}");
        assert!(parse("wat\tx\n").is_err());
        assert!(parse("speech\n").is_err());
    }

    #[test]
    fn fuzzy_contains_tolerates_ocr() {
        assert!(fuzzy_contains("All thattime I spent applying forthisJ0b", "applying for this job") > 0.85);
        assert!(fuzzy_contains("Thank you.", "applying for this job") < 0.5);
    }

    #[test]
    fn report_and_draft() {
        let e = parse("speech\tnothing quite like\n").unwrap();
        let c = CaseResult { name: "a".into(), checks: check(&e, &found()), found: found(), ocr_ms: 120, error: None };
        let md = report(&[c], "Проверка");
        assert!(md.contains("1 из 1 прошли"));
        let d = draft(&found(), Lang::En);
        assert!(parse(&d).is_ok(), "{d}");
        assert!(d.contains("speaker\tLang heng"));
    }
}
