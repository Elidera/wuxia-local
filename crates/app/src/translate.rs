//! Translation service: corrections → glossary → cache → model.

use crate::config::ModelPreset;
use crate::llm::client::LlmClient;
use crate::paths::Paths;
use anyhow::{anyhow, Result};
use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;
use wuxia_core::cache::Store;
use wuxia_core::fixes::Fixes;
use wuxia_core::glossary::Glossary;
use wuxia_core::history::History;
use wuxia_core::memory::{FuzzyMemory, NameBook};
use wuxia_core::prompt::{self, PromptInput, PromptStyle, Reject, TextKind};

pub struct Translator {
    paths: Paths,
    client: LlmClient,
    glossary: Mutex<Glossary>,
    fixes: Mutex<Fixes>,
    cache: Mutex<Store>,
    corrections: Mutex<Store>,
    history: Mutex<History>,
    /// Recent dialogue translations found by similarity (OCR reads a line a bit
    /// differently each time; without this it was translated again and again).
    fuzzy: Mutex<FuzzyMemory>,
    /// All OCR spellings of a speaker name → one.
    names: Mutex<NameBook>,
    /// When the last dialogue line was translated: after a long silence the
    /// scene has changed and old lines are no longer useful context.
    last_dialogue: Mutex<Option<Instant>>,
    /// llama-server runs with one slot: serialize requests, dialogue first.
    gate: tokio::sync::Mutex<()>,
}

#[derive(Debug, Clone)]
pub struct Output {
    pub text: String,
    pub cached: bool,
    pub ms: u128,
}

#[derive(Debug, Clone, Copy)]
pub struct Opts {
    pub use_cache: bool,
    pub use_history: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self { use_cache: true, use_history: true }
    }
}

fn kind_tag(k: TextKind) -> &'static str {
    match k {
        TextKind::Dialogue => "d",
        TextKind::Interface => "ui",
        TextKind::Name => "name",
    }
}

fn append(path: &std::path::Path, line: &str) {
    let r = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = r {
        tracing::warn!("запись {}: {e}", path.display());
    }
}

/// Bundled and user glossaries. A file named `<profile>_*.tsv` (e.g. `wuwa_terms.tsv`)
/// belongs to that game and is loaded only when the game is selected.
pub fn load_glossary(paths: &Paths, profile: &str) -> Glossary {
    let mut g = Glossary::new();
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for dir in [paths.bundled_glossary_dir(), paths.user_glossary_dir()] {
        let mut v: Vec<_> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().map_or(false, |e| e == "tsv"))
                    .filter(|p| {
                        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
                        crate::config::PROFILES.iter().all(|g| g.id == profile || !stem.starts_with(&format!("{}_", g.id)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort(); // deterministic; user dir comes last and overrides
        files.extend(v);
    }
    for f in files {
        match std::fs::read_to_string(&f) {
            Ok(s) => {
                let ui = f.file_stem().and_then(|s| s.to_str()).map_or(false, |s| s.contains("game_ui"));
                let n = if ui { g.load_tsv_ui(&s) } else { g.load_tsv(&s) };
                tracing::info!("глоссарий {}: {n} терминов", f.display());
            }
            Err(e) => tracing::warn!("глоссарий {}: {e}", f.display()),
        }
    }
    g
}

pub fn load_fixes(paths: &Paths) -> Fixes {
    let mut f = Fixes::new();
    if let Ok(s) = std::fs::read_to_string(paths.fixes()) {
        let n = f.load_tsv(&s);
        tracing::info!("исправления fixes.tsv: {n} правил");
    }
    f
}

impl Translator {
    pub fn new(paths: Paths, client: LlmClient, context_lines: usize, profile: &str) -> Self {
        let mut cache = Store::new();
        if let Ok(s) = std::fs::read_to_string(paths.cache()) {
            let n = cache.load_tsv(&s);
            // Compact the append-only log when it has grown with duplicates.
            if n > cache.len() * 2 + 1000 {
                let _ = std::fs::write(paths.cache(), cache.dump());
            }
        }
        let mut corrections = Store::new();
        if let Ok(s) = std::fs::read_to_string(paths.corrections()) {
            corrections.load_tsv(&s);
        }
        if !paths.user_glossary().exists() {
            let _ = std::fs::write(
                paths.user_glossary(),
                "# Ваш глоссарий: оригинал<TAB>русский<TAB>заметка. Перекрывает встроенный.\n\
                 # Оригинал может быть китайским или английским (английский — без учёта регистра).\n\
                 # «пол: ж» или «пол: м» в заметке к имени — модель узнает пол говорящего.\n\
                 # Примеры:\n# 沈七\tШэнь Ци\tимя; пол: м\n# Rover\tСкиталец\tгерой\n# Langsheng\tЛаншэн\tпол: м\n",
            );
        }
        if !paths.fixes().exists() {
            let _ = std::fs::write(
                paths.fixes(),
                "# Исправления: этап<TAB>что<TAB>на что\n\
                 # ocr — исправить типичную ошибку распознавания ДО перевода\n\
                 # ru  — заменить в русском переводе\n\
                 # Примеры (уберите # в начале строки, чтобы включить):\n\
                 # ocr\t末来\t未来\n\
                 # ru\tЦзянху\tцзянху\n\
                 # skip — никогда не переводить текст, где это встречается:\n\
                 # skip\tUser ID\n",
            );
        }
        let fixes = load_fixes(&paths);
        let glossary = load_glossary(&paths, profile);
        tracing::info!("кэш: {} · исправления: {} · глоссарий: {}", cache.len(), corrections.len(), glossary.len());
        Self {
            paths,
            client,
            glossary: Mutex::new(glossary),
            fixes: Mutex::new(fixes),
            cache: Mutex::new(cache),
            corrections: Mutex::new(corrections),
            history: Mutex::new(History::new(context_lines)),
            fuzzy: Mutex::new(FuzzyMemory::new(400, 0.9)),
            names: Mutex::new(NameBook::new()),
            last_dialogue: Mutex::new(None),
            gate: tokio::sync::Mutex::new(()),
        }
    }

    /// Re-read glossaries and fixes. Returns (terms, fix rules).
    pub fn reload_dictionaries(&self, profile: &str) -> (usize, usize) {
        let g = load_glossary(&self.paths, profile);
        let f = load_fixes(&self.paths);
        let n = (g.len(), f.len());
        *self.glossary.lock().unwrap() = g;
        *self.fixes.lock().unwrap() = f;
        n
    }

    pub fn dict_counts(&self) -> (usize, usize) {
        (self.glossary.lock().unwrap().len(), self.fixes.lock().unwrap().len())
    }

    /// Fix typical OCR errors before translation (fixes.tsv, stage `ocr`).
    pub fn fix_ocr(&self, s: &str) -> String {
        self.fixes.lock().unwrap().apply_ocr(s)
    }

    /// The session log: every translated dialogue line as text, one file per
    /// day (`logs\\dialogues-ГГГГ-ММ-ДД.txt`) — to reread the story or find bad
    /// translations. Lines taken from memory are not repeated.
    fn log_session(&self, speaker: Option<&str>, src: &str, dst: &str) {
        let (date, time) = crate::win::local_date_time();
        let path = self.paths.data.join("logs").join(format!("dialogues-{date}.txt"));
        let who = speaker.map(|s| format!("{s}: ")).unwrap_or_default();
        let entry = format!("[{time}] {who}{src}\n          → {dst}\n\n");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(entry.as_bytes());
        }
    }

    /// The user's `skip` rules (fixes.tsv): never translate such text.
    pub fn should_skip(&self, s: &str) -> bool {
        self.fixes.lock().unwrap().should_skip(s)
    }

    /// One spelling per speaker name, whatever OCR read this time.
    pub fn canonical_name(&self, s: &str) -> String {
        self.names.lock().unwrap().canonical(s)
    }

    fn fix_ru(&self, s: &str) -> String {
        self.fixes.lock().unwrap().apply_ru(s)
    }

    pub fn glossary_len(&self) -> usize {
        self.glossary.lock().unwrap().len()
    }

    pub fn clear_history(&self) {
        self.history.lock().unwrap().clear();
    }

    pub fn save_correction(&self, src: &str, dst: &str) {
        let line = {
            let mut c = self.corrections.lock().unwrap();
            if dst.trim().is_empty() {
                c.remove(&[src])
            } else {
                c.insert(&[src], dst.trim())
            }
        };
        append(&self.paths.corrections(), &line);
        tracing::info!("исправление: {src} → {dst}");
    }

    /// Instant answer without the model, if any.
    pub fn lookup(&self, model_id: &str, src: &str, kind: TextKind) -> Option<String> {
        if let Some(c) = self.corrections.lock().unwrap().get(&[src]) {
            return Some(c.to_string());
        }
        if kind != TextKind::Dialogue {
            let exact = self.glossary.lock().unwrap().exact(src).map(|t| t.dst.clone());
            if let Some(t) = exact {
                return Some(self.fix_ru(&t));
            }
        }
        let hit = self.cache.lock().unwrap().get(&[model_id, kind_tag(kind), src]).map(str::to_string);
        hit.map(|h| self.fix_ru(&h))
    }

    pub async fn translate(
        &self,
        preset: &ModelPreset,
        game: &str,
        style_hint: &str,
        src: &str,
        kind: TextKind,
        opts: Opts,
        on_partial: impl FnMut(&str),
    ) -> Result<Output> {
        self.translate_as(preset, game, style_hint, None, src, kind, opts, on_partial).await
    }

    /// Like [`translate`](Self::translate) for a dialogue line said by `speaker`
    /// (source spelling): the speaker's gender from the glossary («пол: ж/м»)
    /// goes to the model, and the line is written to the session log.
    #[allow(clippy::too_many_arguments)]
    pub async fn translate_as(
        &self,
        preset: &ModelPreset,
        game: &str,
        style_hint: &str,
        speaker: Option<&str>,
        src: &str,
        kind: TextKind,
        opts: Opts,
        mut on_partial: impl FnMut(&str),
    ) -> Result<Output> {
        let t0 = Instant::now();
        let src = src.trim();
        if src.is_empty() {
            return Err(anyhow!("пустой текст"));
        }
        if kind == TextKind::Dialogue && opts.use_history {
            // Two minutes without subtitles: a new scene, the old lines would mislead.
            let mut last = self.last_dialogue.lock().unwrap();
            if last.map_or(false, |t| t.elapsed() > std::time::Duration::from_secs(120)) {
                self.history.lock().unwrap().clear();
                tracing::debug!(target: "translate", "долгая пауза — контекст сброшен (новая сцена)");
            }
            *last = Some(Instant::now());
        }
        if opts.use_cache {
            if let Some(hit) = self.lookup(&preset.id, src, kind) {
                if kind == TextKind::Dialogue {
                    self.fuzzy.lock().unwrap().remember(src, &hit);
                    if opts.use_history {
                        self.history.lock().unwrap().push(src, &hit);
                    }
                }
                return Ok(Output { text: hit, cached: true, ms: t0.elapsed().as_millis() });
            }
            if kind == TextKind::Dialogue {
                let near = self.fuzzy.lock().unwrap().find(src).map(str::to_string);
                if let Some(hit) = near {
                    tracing::debug!(target: "translate", "похожая реплика уже переведена: {src}");
                    return Ok(Output { text: hit, cached: true, ms: t0.elapsed().as_millis() });
                }
            }
        }

        // Build prompt under short-lived locks (no await while holding them).
        let messages = {
            let g = self.glossary.lock().unwrap();
            let h = self.history.lock().unwrap();
            let (ctx_src, ctx_pairs) = if kind == TextKind::Dialogue && opts.use_history {
                (h.source_context(), h.paired_context())
            } else {
                (String::new(), String::new())
            };
            let inp = PromptInput {
                source: src,
                kind,
                terms: if kind == TextKind::Dialogue { g.find_in_dialogue(src, 24) } else { g.find_in(src, 24) },
                context_src: ctx_src,
                context_pairs: ctx_pairs,
                game,
                style_hint,
                speaker: speaker.filter(|_| kind == TextKind::Dialogue).and_then(|s| g.gender_of(s).map(|gen| (s, gen))),
            };
            if preset.template.trim().is_empty() {
                prompt::build(PromptStyle::parse(&preset.prompt), &inp)
            } else {
                prompt::render_custom(&preset.template, &inp)
            }
        };

        let _slot = self.gate.lock().await;
        // The model samples (temperature 0.7, as Hy-MT recommends). A dialogue
        // translation that dropped a glossary term gets one more, calmer try
        // (temperature 0.2); a translation that still ignores the glossary is
        // not cached, so the next time the line gets another chance.
        let mut attempt_preset = preset.clone();
        let mut best: Option<(String, usize)> = None;
        for attempt in 0..2 {
            let raw = self
                .client
                .chat(&messages, &attempt_preset, |acc| {
                    if let Ok(p) = prompt::clean_output(acc, src) {
                        on_partial(&p);
                    }
                })
                .await?;
            let text = match prompt::clean_output(&raw, src) {
                Ok(t) => t,
                Err(e) if best.is_some() => {
                    tracing::debug!(target: "translate", "повтор не удался: {e:?}");
                    break;
                }
                Err(Reject::NotTranslated) => return Err(anyhow!("модель вернула непереведённый текст")),
                Err(Reject::TooLong) => return Err(anyhow!("модель выдала слишком длинный ответ")),
                Err(Reject::Empty) => return Err(anyhow!("пустой ответ модели")),
            };
            let missed: Vec<String> = {
                let g = self.glossary.lock().unwrap();
                g.violations(src, &text).iter().map(|t| t.src.clone()).collect()
            };
            if !missed.is_empty() {
                tracing::info!(target: "translate", "глоссарий не соблюдён ({missed:?}), попытка {}", attempt + 1);
            }
            let better = best.as_ref().map_or(true, |(_, n)| missed.len() < *n);
            if better {
                best = Some((text, missed.len()));
            }
            if missed.is_empty() || kind != TextKind::Dialogue || preset.temperature <= 0.25 {
                break;
            }
            attempt_preset.temperature = 0.2;
        }
        drop(_slot);
        let (text, missed) = best.ok_or_else(|| anyhow!("пустой ответ модели"))?;
        if let Ok(p) = prompt::clean_output(&text, src) {
            on_partial(&p);
        }
        let ms = t0.elapsed().as_millis();
        tracing::info!(target: "translate", "[{}|{}|{ms} мс] {src} → {text}", preset.id, kind_tag(kind));
        let follows_glossary = missed == 0;
        if opts.use_cache && follows_glossary {
            let line = self.cache.lock().unwrap().insert(&[&preset.id, kind_tag(kind), src], &text);
            append(&self.paths.cache(), &line);
        }
        let text = self.fix_ru(&text);
        if kind == TextKind::Dialogue {
            if follows_glossary {
                self.fuzzy.lock().unwrap().remember(src, &text);
            }
            if opts.use_history {
                self.history.lock().unwrap().push(src, &text);
            }
            self.log_session(speaker, src, &text);
        }
        Ok(Output { text, cached: false, ms })
    }

    /// Translate many interface labels in one request (see prompt::build_batch).
    /// Returns translations in the same order; each is cached. Errors if the
    /// answer cannot be split — the caller then falls back to one-by-one.
    pub async fn translate_batch(&self, preset: &ModelPreset, game: &str, items: &[String]) -> Result<Vec<Option<String>>> {
        if items.is_empty() {
            return Ok(vec![]);
        }
        if !preset.template.trim().is_empty() {
            return Err(anyhow!("свой шаблон промпта: пакетный режим отключён"));
        }
        let t0 = Instant::now();
        let messages = {
            let g = self.glossary.lock().unwrap();
            let mut terms: Vec<&wuxia_core::glossary::Term> = Vec::new();
            for it in items {
                for t in g.find_in(it, 24) {
                    if !terms.iter().any(|x| x.src == t.src) {
                        terms.push(t);
                    }
                }
            }
            terms.truncate(40);
            let refs: Vec<&str> = items.iter().map(String::as_str).collect();
            prompt::build_batch(PromptStyle::parse(&preset.prompt), &refs, &terms, game)
        };
        let mut p = preset.clone();
        p.max_tokens = 48 * items.len() as u32 + 64;
        let _slot = self.gate.lock().await;
        let raw = self.client.chat(&messages, &p, |_| {}).await?;
        drop(_slot);
        let parts = prompt::parse_batch_indexed(&raw, items.len());
        if parts.iter().any(Option::is_none) {
            tracing::info!(target: "translate", "пакет разобран не полностью, ответ модели: {}", raw.chars().take(600).collect::<String>());
        }
        if parts.iter().all(Option::is_none) {
            return Err(anyhow!("пакетный ответ не удалось разобрать"));
        }
        let mut out: Vec<Option<String>> = Vec::with_capacity(items.len());
        let mut lines = String::new();
        {
            let mut cache = self.cache.lock().unwrap();
            for (src, part) in items.iter().zip(parts) {
                out.push(part.and_then(|part| match prompt::clean_output(&part, src) {
                    Ok(t) => {
                        lines.push_str(&cache.insert(&[&preset.id, kind_tag(TextKind::Interface), src], &t));
                        Some(t)
                    }
                    Err(_) => None, // the caller retries this one alone
                }));
            }
        }
        if !lines.is_empty() {
            append(&self.paths.cache(), &lines);
        }
        tracing::info!(target: "translate", "[{}|ui-batch|{} мс] {}", preset.id, t0.elapsed().as_millis(), wuxia_core::text::count_ru(items.len(), "надпись", "надписи", "надписей"));
        Ok(out.into_iter().map(|t| t.map(|t| self.fix_ru(&t))).collect())
    }
}
