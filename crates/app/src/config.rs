//! User configuration (`config.toml` in the data dir).
//! Every field has a default, so a partial or old config file still loads.

use crate::paths::Paths;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The config file was just created (first start): pick a model for this PC.
    #[serde(skip)]
    pub fresh: bool,
    /// Set when a broken config.toml was replaced on load: shown to the user.
    #[serde(skip)]
    pub load_note: Option<String>,
    pub general: General,
    pub llm: Llm,
    pub models: Vec<ModelPreset>,
    pub dialogue: Dialogue,
    pub interface: Interface,
    pub ocr: Ocr,
    pub hotkeys: Hotkeys,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Game profile id (see [`PROFILES`]): "wuwa", "wwm" or "custom".
    /// For a known profile `game_title` and `window_match` are filled from it.
    pub profile: String,
    /// Language of the game text: "en" or "zh".
    pub source_lang: String,
    /// Shown to instruct models in the prompt.
    pub game_title: String,
    /// Substrings of the game window title separated by `|`. Capture pauses while
    /// another window is in the foreground. Empty = always active.
    pub window_match: String,
    /// Kept for old config files; translation now always waits for the game window.
    pub pause_when_unfocused: bool,
    /// Show the overlay and the panel on screenshots (Win+Shift+S, OBS).
    /// Off by default: then they can never get into our own OCR.
    pub visible_in_screenshots: bool,
    /// The "I'm still here, in the tray" notice was shown after the first ✕.
    pub tray_hint_shown: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Llm {
    /// "managed": we start and watch llama-server ourselves.
    /// "external": connect to an already running OpenAI-compatible server
    /// (another llama-server, LM Studio, Ollama, a second PC on the LAN).
    pub mode: String,
    /// Relative to the install dir or absolute.
    pub server_exe: String,
    pub port: u16,
    pub external_url: String,
    pub active_model: String,
    /// Layers offloaded to GPU (-ngl). 0 = CPU only. 99 = everything.
    pub gpu_layers: i32,
    /// CPU threads for inference; 0 = auto (half of logical cores, min 2).
    pub threads: u32,
    pub ctx_size: u32,
    /// Run llama-server with below-normal priority so the game keeps its FPS.
    pub low_priority: bool,
    pub extra_args: Vec<String>,
    pub request_timeout_s: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelPreset {
    pub id: String,
    pub name: String,
    /// "hymt" or "instruct"
    pub prompt: String,
    /// HuggingFace reference for llama-server `-hf` (downloaded on first start).
    pub hf: String,
    /// Local .gguf path; filled automatically after the first download.
    pub path: String,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: i32,
    pub repeat_penalty: f32,
    pub max_tokens: u32,
    /// Ask the chat template to disable "thinking" (Qwen3.x).
    pub no_think: bool,
    /// Optional custom prompt template (see wuxia_core::prompt::render_custom).
    pub template: String,
    /// Approximate size for the UI.
    pub size_gb: f32,
}

impl Default for ModelPreset {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            prompt: "instruct".into(),
            hf: String::new(),
            path: String::new(),
            temperature: 0.3,
            top_p: 0.8,
            top_k: 20,
            repeat_penalty: 1.05,
            max_tokens: 256,
            no_think: false,
            template: String::new(),
            size_gb: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Dialogue {
    pub enabled: bool,
    /// "auto": read the whole screen and translate anything that looks like speech;
    /// "region": watch only the selected subtitle region.
    pub mode: String,
    /// How often the whole screen is read in «auto» mode.
    pub auto_interval_ms: u64,
    /// Subtitle region as fractions of the primary screen: [x, y, w, h].
    pub region: [f32; 4],
    pub interval_ms: u64,
    /// "light" (white subtitles) or "dark".
    pub text_color: String,
    /// Luminance threshold for text pixels (0–255).
    pub ink_threshold: u8,
    pub upscale: u32,
    /// "contrast" or "none"
    pub preprocess: String,
    pub context_lines: usize,
    pub show_original: bool,
    /// "below", "above" or "over" the source subtitles, or "box": in the user's
    /// own window (`box_rect`), moved and resized on screen.
    pub position: String,
    /// The user's translation window as fractions of the primary screen: [x, y, w, h].
    pub box_rect: [f32; 4],
    pub font_size: f32,
    /// Look of the translation over the game: font (any installed Windows
    /// font), text and speaker-name colours, dark outline around letters.
    pub overlay_font: String,
    pub overlay_color: [u8; 3],
    pub overlay_name_color: [u8; 3],
    pub overlay_outline: bool,
    pub backdrop_opacity: f32,
    pub stable_frames: u32,
    pub clear_frames: u32,
    pub change_threshold: f32,
    /// Russian free-text style wish passed to instruct models.
    pub style_hint: String,
    /// Translate speaker names in "Имя：реплика" separately (cached).
    pub split_speaker: bool,
    /// The user has picked the subtitle region at least once.
    pub region_set: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Interface {
    /// "replace", "backdrop" or "list"
    pub mode: String,
    /// 0 = auto (2× below 1440p).
    pub upscale: u32,
    pub font_scale: f32,
    pub max_lines: usize,
    /// Hide the translated screen automatically when the game picture changes.
    pub auto_hide: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Ocr {
    /// Deprecated: the OCR language comes from `general.source_lang`.
    pub language: String,
    /// "paddle" (PaddleOCR, falls back to Windows OCR), "windows", or "both".
    pub engine: String,
    /// PP-OCRv6 tier: "tiny" (fastest), "small" (bundled), "medium" (most accurate).
    pub paddle_model: String,
    /// CPU threads for PaddleOCR.
    pub paddle_threads: i32,
    /// Longer side of the picture for PaddleOCR text detection, px.
    pub paddle_max_side: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub toggle_dialogue: String,
    pub screen_translate: String,
    pub show_original: String,
    pub select_region: String,
    pub panel: String,
    /// Save the screen as a test case for the developer.
    pub save_case: String,
    /// Freeze frame: still picture, click a text to translate it.
    pub freeze: String,
    /// The last translation is bad: forget it and translate again.
    pub bad_translation: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fresh: false,
            load_note: None,
            general: General::default(),
            llm: Llm::default(),
            models: default_models(),
            dialogue: Dialogue::default(),
            interface: Interface::default(),
            ocr: Ocr::default(),
            hotkeys: Hotkeys::default(),
        }
    }
}

impl Default for General {
    fn default() -> Self {
        Self {
            profile: "wuwa".into(),
            source_lang: "en".into(),
            game_title: "Wuthering Waves".into(),
            window_match: "Wuthering Waves|鸣潮".into(),
            pause_when_unfocused: true,
            visible_in_screenshots: false,
            tray_hint_shown: false,
        }
    }
}

/// A supported game: how to recognise its window and which dictionaries to load
/// (glossary files named `<id>_*.tsv` are loaded only for that profile).
pub struct GameProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub title: &'static str,
    pub window_match: &'static str,
    /// Text languages the game offers that we can translate, the first is the default.
    pub langs: &'static [&'static str],
    /// Window class of the game, if known (Unreal Engine games: "UnrealWindow").
    /// Used when the window title does not match (other client or language).
    pub window_class: &'static str,
}

pub const PROFILES: &[GameProfile] = &[
    GameProfile { id: "wuwa", name: "Wuthering Waves", title: "Wuthering Waves", window_match: "Wuthering Waves|鸣潮", langs: &["en", "zh"], window_class: "UnrealWindow" },
    GameProfile { id: "wwm", name: "Where Winds Meet", title: "Where Winds Meet (燕云十六声)", window_match: "Where Winds Meet|燕云十六声", langs: &["zh", "en"], window_class: "" },
    GameProfile { id: "custom", name: "Другая игра", title: "", window_match: "", langs: &["en", "zh"], window_class: "" },
];

pub fn profile(id: &str) -> &'static GameProfile {
    PROFILES.iter().find(|p| p.id == id).unwrap_or(&PROFILES[PROFILES.len() - 1])
}

impl Default for Llm {
    fn default() -> Self {
        Self {
            mode: "managed".into(),
            server_exe: "llama\\llama-server.exe".into(),
            port: 8848,
            external_url: "http://127.0.0.1:8080".into(),
            active_model: "hymt2-1.8b".into(),
            gpu_layers: 99,
            threads: 0,
            ctx_size: 4096,
            low_priority: true,
            extra_args: vec![],
            request_timeout_s: 60,
        }
    }
}

pub fn default_models() -> Vec<ModelPreset> {
    vec![
        ModelPreset {
            id: "hymt2-1.8b".into(),
            name: "Hy-MT2 1.8B Q4_K_M — быстрый (по умолчанию)".into(),
            prompt: "hymt".into(),
            hf: "tencent/Hy-MT2-1.8B-GGUF:Q4_K_M".into(),
            temperature: 0.7,
            top_p: 0.6,
            top_k: 20,
            repeat_penalty: 1.05,
            size_gb: 1.13,
            ..Default::default()
        },
        ModelPreset {
            id: "hymt2-7b".into(),
            name: "Hy-MT2 7B Q4_K_M — качественный (мощный ПК)".into(),
            prompt: "hymt".into(),
            hf: "tencent/Hy-MT2-7B-GGUF:Q4_K_M".into(),
            temperature: 0.7,
            top_p: 0.6,
            top_k: 20,
            repeat_penalty: 1.05,
            size_gb: 4.62,
            ..Default::default()
        },
        ModelPreset {
            id: "qwen3.5-2b".into(),
            name: "Qwen3.5 2B Q4_K_M — альтернатива".into(),
            prompt: "instruct".into(),
            hf: "unsloth/Qwen3.5-2B-GGUF:Q4_K_M".into(),
            no_think: true,
            size_gb: 1.35,
            ..Default::default()
        },
        ModelPreset {
            id: "qwen3.5-4b".into(),
            name: "Qwen3.5 4B Q4_K_M — литературный".into(),
            prompt: "instruct".into(),
            hf: "unsloth/Qwen3.5-4B-GGUF:Q4_K_M".into(),
            no_think: true,
            size_gb: 2.9,
            ..Default::default()
        },
    ]
}

impl Default for Dialogue {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: "auto".into(),
            auto_interval_ms: 500,
            region: [0.18, 0.80, 0.64, 0.12],
            interval_ms: 150,
            text_color: "light".into(),
            ink_threshold: 200,
            upscale: 2,
            preprocess: "none".into(),
            context_lines: 3,
            show_original: false,
            position: "below".into(),
            box_rect: [0.2, 0.74, 0.6, 0.13],
            font_size: 26.0,
            overlay_font: "Segoe UI".into(),
            overlay_color: [250, 246, 235],
            overlay_name_color: [240, 200, 120],
            overlay_outline: true,
            backdrop_opacity: 0.72,
            stable_frames: 2,
            clear_frames: 4,
            change_threshold: 0.03,
            style_hint: String::new(),
            split_speaker: true,
            region_set: false,
        }
    }
}

impl Default for Interface {
    fn default() -> Self {
        Self { mode: "replace".into(), upscale: 0, font_scale: 0.82, max_lines: 120, auto_hide: true }
    }
}

impl Default for Ocr {
    fn default() -> Self {
        Self { language: "zh-Hans-CN".into(), engine: "paddle".into(), paddle_model: "small".into(), paddle_threads: 2, paddle_max_side: 1600 }
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            toggle_dialogue: "F7".into(),
            screen_translate: "F8".into(),
            show_original: "F9".into(),
            select_region: "Ctrl+F7".into(),
            panel: "F10".into(),
            save_case: "Ctrl+F9".into(),
            freeze: "Ctrl+F8".into(),
            bad_translation: "Ctrl+F11".into(),
        }
    }
}

impl Config {
    /// Load the user config; on first run copy the bundled default, falling back to built-ins.
    pub fn load(paths: &Paths) -> anyhow::Result<Config> {
        let p = paths.config();
        let fresh = !p.exists();
        if fresh {
            if paths.default_config().exists() {
                std::fs::copy(paths.default_config(), &p)?;
            } else {
                Config::default().save(paths)?;
            }
        }
        let text = std::fs::read_to_string(&p)?;
        let mut cfg = match Config::parse(&text) {
            Ok(c) => c,
            // A broken file must not stop the program: the last good copy, or the defaults.
            Err(e) => {
                let broken = p.with_extension("toml.broken");
                let _ = std::fs::copy(&p, &broken);
                tracing::error!("Ошибка в {}: {e}", p.display());
                let backup = std::fs::read_to_string(backup_path(&p)).ok().and_then(|t| Config::parse(&t).ok());
                let (mut c, from) = match backup {
                    Some(c) => (c, "последнюю рабочую копию"),
                    None => {
                        let d = std::fs::read_to_string(paths.default_config()).ok().and_then(|t| Config::parse(&t).ok()).unwrap_or_default();
                        (d, "стандартные настройки")
                    }
                };
                // The broken file is kept as .broken; removed first so that saving
                // does not make it the backup (the good copy stays in .bak).
                let _ = std::fs::remove_file(&p);
                c.save(paths)?;
                c.load_note = Some(format!(
                    "Файл настроек был повреждён ({e}). Загружена {from}; испорченный файл сохранён как {}.",
                    broken.display()
                ));
                c
            }
        };
        cfg.fresh = fresh;
        Ok(cfg)
    }

    /// Read a config file's text and bring old configs up to date.
    pub fn parse(text: &str) -> anyhow::Result<Config> {
        let mut cfg: Config = toml::from_str(text)?;
        if cfg.models.is_empty() {
            cfg.models = default_models();
        }
        cfg.apply_profile();
        // Region mode cannot work with a near-full-screen region (the 3D scene never
        // settles); such configs come from before the auto mode existed.
        let r = cfg.dialogue.region;
        if cfg.dialogue.mode == "region" && r[2] * r[3] >= 0.5 {
            cfg.dialogue.mode = "auto".into();
        }
        Ok(cfg)
    }

    /// Settings are saved from several threads (panel, overlay drag, the
    /// subtitle zone finder, the model supervisor): one at a time, through a
    /// temporary file, keeping the previous good file as `config.toml.bak`.
    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = SAVING.lock().unwrap_or_else(|e| e.into_inner());
        let p = paths.config();
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        if p.exists() {
            let _ = std::fs::copy(&p, backup_path(&p));
        }
        std::fs::rename(tmp, p)?;
        Ok(())
    }

    /// Fill game title / window match from a known profile ("custom" keeps the user's values).
    pub fn apply_profile(&mut self) {
        let p = profile(&self.general.profile);
        if p.id != "custom" {
            self.general.game_title = p.title.into();
            self.general.window_match = p.window_match.into();
        }
        if self.general.source_lang != "en" && self.general.source_lang != "zh" {
            self.general.source_lang = p.langs[0].into();
        }
    }

    pub fn source_lang(&self) -> wuxia_core::lang::Lang {
        wuxia_core::lang::Lang::from_code(&self.general.source_lang)
    }

    pub fn active_model(&self) -> &ModelPreset {
        self.models.iter().find(|m| m.id == self.llm.active_model).unwrap_or(&self.models[0])
    }

    /// Which preset a user-chosen .gguf belongs to, by its file name
    /// ("Hy-MT2-7B-Q4_K_M.gguf" → hymt2-7b); unknown files replace the active model.
    pub fn preset_for_file(&self, path: &std::path::Path) -> String {
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let name = name.replace(['-', '_', ' '], "");
        let guess = if name.contains("hymt") || name.contains("hunyuan") {
            if name.contains("7b") {
                "hymt2-7b"
            } else {
                "hymt2-1.8b"
            }
        } else if name.contains("qwen") {
            if name.contains("4b") {
                "qwen3.5-4b"
            } else {
                "qwen3.5-2b"
            }
        } else {
            ""
        };
        if self.models.iter().any(|m| m.id == guess) {
            guess.to_string()
        } else {
            self.active_model().id.clone()
        }
    }
}

fn backup_path(p: &std::path::Path) -> std::path::PathBuf {
    p.with_extension("toml.bak")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(name: &str) -> Paths {
        let dir = std::env::temp_dir().join(format!("wuxia-cfg-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Paths { install: dir.clone(), data: dir }
    }

    #[test]
    fn bundled_default_config_parses() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/config.default.toml")).unwrap();
        let c = Config::parse(&text).unwrap();
        assert!(!c.models.is_empty());
        assert!(c.models.iter().any(|m| m.id == c.llm.active_model), "active model {} has no preset", c.llm.active_model);
    }

    #[test]
    fn partial_config_gets_defaults() {
        let c = Config::parse("[dialogue]\nfont_size = 30.0\n").unwrap();
        assert_eq!(c.dialogue.font_size, 30.0);
        assert!(!c.models.is_empty());
        assert_eq!(c.ocr.engine, Config::default().ocr.engine);
    }

    #[test]
    fn old_full_screen_region_becomes_auto() {
        let c = Config::parse("[dialogue]\nmode = \"region\"\nregion = [0.0, 0.0, 1.0, 1.0]\n").unwrap();
        assert_eq!(c.dialogue.mode, "auto");
    }

    #[test]
    fn save_keeps_previous_as_backup() {
        let paths = temp_paths("save");
        let mut c = Config::default();
        c.dialogue.font_size = 20.0;
        c.save(&paths).unwrap();
        c.dialogue.font_size = 40.0;
        c.save(&paths).unwrap();
        let now = Config::parse(&std::fs::read_to_string(paths.config()).unwrap()).unwrap();
        let bak = Config::parse(&std::fs::read_to_string(backup_path(&paths.config())).unwrap()).unwrap();
        assert_eq!(now.dialogue.font_size, 40.0);
        assert_eq!(bak.dialogue.font_size, 20.0);
    }

    #[test]
    fn broken_file_falls_back_to_backup_and_keeps_it() {
        let paths = temp_paths("broken");
        let mut c = Config::default();
        c.dialogue.font_size = 33.0;
        c.save(&paths).unwrap();
        c.save(&paths).unwrap(); // now config.toml.bak is a good copy too
        std::fs::write(paths.config(), "[dialogue\nfont_size = ").unwrap();
        let loaded = Config::load(&paths).unwrap();
        assert_eq!(loaded.dialogue.font_size, 33.0);
        assert!(loaded.load_note.is_some());
        assert!(paths.config().with_extension("toml.broken").exists());
        // Broken twice in a row: the good copy is still there.
        std::fs::write(paths.config(), "garbage = = =").unwrap();
        let again = Config::load(&paths).unwrap();
        assert_eq!(again.dialogue.font_size, 33.0);
    }

    #[test]
    fn broken_file_without_backup_gives_defaults() {
        let paths = temp_paths("nobak");
        std::fs::write(paths.config(), "not toml at all [[[").unwrap();
        let loaded = Config::load(&paths).unwrap();
        assert!(loaded.load_note.is_some());
        assert_eq!(loaded.dialogue.font_size, Config::default().dialogue.font_size);
        assert!(!loaded.fresh);
    }
}
