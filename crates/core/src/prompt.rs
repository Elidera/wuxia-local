//! Prompt construction for the two families of local models we support, and
//! cleanup of their output.
//!
//! * `HyMt` — Tencent Hy-MT / HY-MT translation models. They have no system
//!   prompt and are trained on fixed Chinese instruction templates
//!   (terminology, context, style). We follow those templates closely.
//! * `Instruct` — general instruct models (Qwen3.5, TranslateGemma, …) that
//!   accept a system prompt with detailed literary rules.
//! * A user-defined template can override both (see [`render_custom`]).

use crate::glossary::Term;
use crate::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptStyle {
    HyMt,
    Instruct,
}

impl PromptStyle {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "hymt" | "hy-mt" | "hunyuan" => PromptStyle::HyMt,
            _ => PromptStyle::Instruct,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// Spoken line / subtitle: literary register, context matters.
    Dialogue,
    /// Menu label, item name, quest text: concise, consistent.
    Interface,
    /// A character name alone.
    Name,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: &'static str,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct PromptInput<'a> {
    pub source: &'a str,
    pub kind: TextKind,
    pub terms: Vec<&'a Term>,
    /// Previous source lines (oldest first), one per line.
    pub context_src: String,
    /// Previous "source → translation" pairs (oldest first).
    pub context_pairs: String,
    /// Game title shown to instruct models.
    pub game: &'a str,
    /// Extra user style hint (Russian), e.g. «торжественнее», may be empty.
    pub style_hint: &'a str,
}

const HYMT_STYLE_DIALOGUE: &str = "武侠小说的文学风格，用词典雅，语言自然流畅";
const HYMT_STYLE_INTERFACE: &str = "游戏界面用语，简洁准确";

fn hymt_terms(terms: &[&Term]) -> String {
    if terms.is_empty() {
        return String::new();
    }
    let mut s = String::from("参考下面的翻译：\n");
    for t in terms {
        s.push_str(&t.src);
        s.push_str(" 翻译成 ");
        s.push_str(&t.dst);
        s.push('\n');
    }
    s.push('\n');
    s
}

fn build_hymt(inp: &PromptInput) -> Vec<Message> {
    let style = match inp.kind {
        TextKind::Dialogue => HYMT_STYLE_DIALOGUE,
        TextKind::Interface | TextKind::Name => HYMT_STYLE_INTERFACE,
    };
    let mut p = hymt_terms(&inp.terms);
    if inp.kind == TextKind::Dialogue && !inp.context_src.is_empty() {
        p.push_str(&inp.context_src);
        p.push_str("\n参考上面的信息，把下面的文本翻译成俄语，译文风格必须严格符合：");
        p.push_str(style);
        p.push_str("。注意不需要翻译上文，也不要额外解释：\n");
    } else {
        p.push_str("将以下文本翻译为俄语，译文风格必须严格符合：");
        p.push_str(style);
        p.push_str("。注意只需要输出翻译后的结果，不要额外解释：\n");
    }
    p.push_str(inp.source);
    vec![Message { role: "user", content: p }]
}

fn instruct_system(inp: &PromptInput) -> String {
    let mut s = format!(
        "Ты — профессиональный литературный переводчик китайских игр жанра уся на русский язык. \
Переводишь текст из игры «{game}».\n\
Правила:\n\
1. Точно передавай смысл. Ничего не добавляй и не выдумывай.\n\
2. Пиши естественным литературным русским языком. Сохраняй возвышенный тон уся без излишней архаики.\n\
3. Классический китайский (文言), чэнъюи, поговорки и стихи переводи по смыслу, сохраняя образность. \
Не переводи их дословно, если так теряется смысл.\n\
4. Имена и топонимы транскрибируй по системе Палладия (например, Ли Сюнь, Кайфэн), если их нет в глоссарии.\n\
5. Термины из глоссария используй строго в указанном виде, склоняя по правилам русского языка.\n\
6. Обращения (少侠, 前辈, 师父, 大人 и т. п.) передавай естественно по контексту: «молодой герой», «почтенный», «наставник», «господин».\n\
7. Выведи ТОЛЬКО перевод: без кавычек, пояснений, транскрипции и оригинала.",
        game = inp.game
    );
    match inp.kind {
        TextKind::Interface => s.push_str("\n8. Это надпись интерфейса: переводи кратко, как в официальной локализации; не добавляй точку в конце, если её нет в оригинале."),
        TextKind::Name => s.push_str("\n8. Это имя или прозвище персонажа: дай только русскую форму имени."),
        TextKind::Dialogue => {}
    }
    if !inp.style_hint.trim().is_empty() {
        s.push_str("\nДополнительное пожелание к стилю: ");
        s.push_str(inp.style_hint.trim());
    }
    s
}

fn build_instruct(inp: &PromptInput) -> Vec<Message> {
    let mut u = String::new();
    if !inp.terms.is_empty() {
        u.push_str("Глоссарий:\n");
        for t in &inp.terms {
            u.push_str(&format!("{} — {}", t.src, t.dst));
            if !t.note.is_empty() {
                u.push_str(&format!(" ({})", t.note));
            }
            u.push('\n');
        }
        u.push('\n');
    }
    if inp.kind == TextKind::Dialogue && !inp.context_pairs.is_empty() {
        u.push_str("Предыдущие реплики (не переводи их, это контекст):\n");
        u.push_str(&inp.context_pairs);
        u.push_str("\n\n");
    }
    u.push_str("Переведи на русский:\n");
    u.push_str(inp.source);
    vec![Message { role: "system", content: instruct_system(inp) }, Message { role: "user", content: u }]
}

pub fn build(style: PromptStyle, inp: &PromptInput) -> Vec<Message> {
    match style {
        PromptStyle::HyMt => build_hymt(inp),
        PromptStyle::Instruct => build_instruct(inp),
    }
}

/// Render a user template. Placeholders: `{source}`, `{terms}` (one "src — dst" per
/// line), `{context}` (paired), `{context_src}`, `{game}`, `{style}`.
/// A line `---` separates an optional system part from the user part.
pub fn render_custom(template: &str, inp: &PromptInput) -> Vec<Message> {
    let terms = inp.terms.iter().map(|t| format!("{} — {}", t.src, t.dst)).collect::<Vec<_>>().join("\n");
    let r = |s: &str| {
        s.replace("{source}", inp.source)
            .replace("{terms}", &terms)
            .replace("{context}", &inp.context_pairs)
            .replace("{context_src}", &inp.context_src)
            .replace("{game}", inp.game)
            .replace("{style}", inp.style_hint)
    };
    match template.split_once("\n---\n") {
        Some((sys, user)) => vec![
            Message { role: "system", content: r(sys.trim()) },
            Message { role: "user", content: r(user.trim()) },
        ],
        None => vec![Message { role: "user", content: r(template.trim()) }],
    }
}

/// Separator between items of a batch request (interface mode).
pub const BATCH_SEP: &str = "|||";

/// One request for many short interface labels: faster than one request per
/// label and keeps terminology consistent across the screen.
pub fn build_batch(style: PromptStyle, items: &[&str], terms: &[&Term], game: &str) -> Vec<Message> {
    let n = items.len();
    let joined = items.join(BATCH_SEP);
    match style {
        PromptStyle::HyMt => {
            let mut p = hymt_terms(terms);
            p.push_str(&format!(
                "将以下文本翻译为俄语。这是游戏界面中的{n}条独立文字，用“{BATCH_SEP}”分隔。\
                 译文必须保留相同数量的分隔符“{BATCH_SEP}”，且位置不变，每条文字单独翻译。\
                 译文风格必须严格符合：{HYMT_STYLE_INTERFACE}。注意只需要输出翻译后的结果，不要额外解释：\n"
            ));
            p.push_str(&joined);
            vec![Message { role: "user", content: p }]
        }
        PromptStyle::Instruct => {
            let dummy = PromptInput {
                source: "",
                kind: TextKind::Interface,
                terms: vec![],
                context_src: String::new(),
                context_pairs: String::new(),
                game,
                style_hint: "",
            };
            let mut sys = instruct_system(&dummy);
            sys.push_str(&format!(
                "\n9. На вход подаётся {n} отдельных надписей, разделённых «{BATCH_SEP}». Переведи каждую отдельно \
                 и выведи переводы в том же порядке, разделяя их тем же «{BATCH_SEP}». Число разделителей должно совпасть."
            ));
            let mut u = String::new();
            if !terms.is_empty() {
                u.push_str("Глоссарий:\n");
                for t in terms {
                    u.push_str(&format!("{} — {}\n", t.src, t.dst));
                }
                u.push('\n');
            }
            u.push_str("Переведи на русский:\n");
            u.push_str(&joined);
            vec![Message { role: "system", content: sys }, Message { role: "user", content: u }]
        }
    }
}

/// Split a batch answer back into items. `None` if the count does not match.
pub fn parse_batch(raw: &str, n: usize) -> Option<Vec<String>> {
    let s = strip_think(raw).replace("｜｜｜", BATCH_SEP).replace("| | |", BATCH_SEP);
    let mut s = s.trim();
    for p in PREFIXES {
        if s.to_lowercase().starts_with(p) {
            s = s[p.len()..].trim_start();
        }
    }
    let parts: Vec<String> = s.split(BATCH_SEP).map(|p| p.trim().to_string()).collect();
    (parts.len() == n && parts.iter().all(|p| !p.is_empty())).then_some(parts)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reject {
    Empty,
    /// Output is still mostly Chinese — the model did not translate.
    NotTranslated,
    /// Output is wildly longer than any sane translation (rambling / hallucination).
    TooLong,
}

fn strip_think(s: &str) -> String {
    let mut out = s.to_string();
    while let Some(a) = out.find("<think>") {
        match out[a..].find("</think>") {
            Some(b) => out.replace_range(a..a + b + "</think>".len(), ""),
            None => {
                out.truncate(a);
                break;
            }
        }
    }
    out
}

const PREFIXES: &[&str] = &["перевод:", "перевод на русский:", "translation:", "译文：", "译文:", "русский:", "ответ:"];

/// Turn raw model output into a displayable translation, or reject it.
pub fn clean_output(raw: &str, source: &str) -> Result<String, Reject> {
    let mut s = strip_think(raw).trim().to_string();
    // Remove label prefixes.
    loop {
        let low = s.to_lowercase();
        match PREFIXES.iter().find(|p| low.starts_with(*p)) {
            Some(p) => s = s[p.len()..].trim_start().to_string(),
            None => break,
        }
    }
    // Some models echo the source on its own line.
    if let Some(rest) = s.strip_prefix(source) {
        let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '→' || c == '-' || c == '—');
        if !rest.is_empty() {
            s = rest.to_string();
        }
    }
    // Keep only the first paragraph for short sources (models sometimes add notes).
    if source.chars().count() < 120 {
        if let Some(i) = s.find("\n\n") {
            s.truncate(i);
        }
    }
    let s = s.trim();
    // Strip wrapping quotes the source did not have.
    let src_quoted = source.starts_with(['“', '「', '"', '《']);
    let s = if !src_quoted {
        let pairs = [('«', '»'), ('"', '"'), ('“', '”'), ('„', '“')];
        let mut t = s;
        for (o, c) in pairs {
            if t.starts_with(o) && t.ends_with(c) && t.chars().count() >= 2 {
                let inner = &t[o.len_utf8()..t.len() - c.len_utf8()];
                if !inner.contains(o) {
                    t = inner;
                }
            }
        }
        t
    } else {
        s
    };
    if s.is_empty() {
        return Err(Reject::Empty);
    }
    if text::han_ratio(s) > 0.3 {
        return Err(Reject::NotTranslated);
    }
    // Russian is ~2–4× longer in chars than Chinese; allow generous slack.
    let limit = 40 + source.chars().count() * 8;
    if s.chars().count() > limit {
        return Err(Reject::TooLong);
    }
    Ok(text::tidy_russian(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(s: &str, d: &str) -> Term {
        Term { src: s.into(), dst: d.into(), note: String::new() }
    }

    #[test]
    fn hymt_with_terms_and_context() {
        let t = term("开封", "Кайфэн");
        let inp = PromptInput {
            source: "我们去开封。",
            kind: TextKind::Dialogue,
            terms: vec![&t],
            context_src: "少侠，你醒了？".into(),
            context_pairs: String::new(),
            game: "Where Winds Meet",
            style_hint: "",
        };
        let m = build(PromptStyle::HyMt, &inp);
        assert_eq!(m.len(), 1);
        let c = &m[0].content;
        assert!(c.starts_with("参考下面的翻译：\n开封 翻译成 Кайфэн\n"));
        assert!(c.contains("少侠，你醒了？\n参考上面的信息"));
        assert!(c.ends_with("\n我们去开封。"));
    }

    #[test]
    fn instruct_has_system() {
        let inp = PromptInput {
            source: "确定",
            kind: TextKind::Interface,
            terms: vec![],
            context_src: String::new(),
            context_pairs: "a → b".into(),
            game: "WWM",
            style_hint: "",
        };
        let m = build(PromptStyle::Instruct, &inp);
        assert_eq!(m[0].role, "system");
        assert!(m[0].content.contains("надпись интерфейса"));
        assert!(!m[1].content.contains("Предыдущие")); // no context for UI
    }

    #[test]
    fn custom_template() {
        let inp = PromptInput {
            source: "你好",
            kind: TextKind::Dialogue,
            terms: vec![],
            context_src: String::new(),
            context_pairs: String::new(),
            game: "G",
            style_hint: "",
        };
        let m = render_custom("Sys {game}\n---\nTranslate: {source}", &inp);
        assert_eq!(m[0].content, "Sys G");
        assert_eq!(m[1].content, "Translate: 你好");
    }

    #[test]
    fn batch_roundtrip() {
        let t = term("背包", "Инвентарь");
        let m = build_batch(PromptStyle::HyMt, &["背包", "任务", "设置"], &[&t], "G");
        assert!(m[0].content.contains("背包|||任务|||设置"));
        assert!(m[0].content.starts_with("参考下面的翻译"));
        let m = build_batch(PromptStyle::Instruct, &["背包", "任务"], &[], "G");
        assert_eq!(m[0].role, "system");
        assert_eq!(parse_batch("Инвентарь ||| Задания|||Настройки", 3).unwrap(), vec!["Инвентарь", "Задания", "Настройки"]);
        assert_eq!(parse_batch("Инвентарь｜｜｜Задания", 2).unwrap().len(), 2);
        assert!(parse_batch("Инвентарь|||Задания", 3).is_none());
        assert!(parse_batch("Инвентарь||||||", 3).is_none());
    }

    #[test]
    fn cleanup() {
        assert_eq!(clean_output("<think>hmm</think>\nПеревод: «Идём в Кайфэн.»", "我们去开封。").unwrap(), "Идём в Кайфэн.");
        assert_eq!(clean_output("我们去开封。\n→ Идём", "我们去开封。").unwrap(), "Идём");
        assert_eq!(clean_output("我们去开封", "我们去开封"), Err(Reject::NotTranslated));
        assert_eq!(clean_output("  ", "x"), Err(Reject::Empty));
        assert_eq!(clean_output("Да.\n\nПримечание: ...", "是。").unwrap(), "Да.");
    }
}
