//! User configuration (`config.toml` in the data dir).
//! Every field has a default, so a partial or old config file still loads.

use crate::paths::Paths;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
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
    /// Shown to instruct models in the prompt.
    pub game_title: String,
    /// Substring of the game window title. Capture pauses while another window
    /// is in the foreground. Empty = always active.
    pub window_match: String,
    /// Optional exe name of the game (e.g. "wwm.exe"). If set, it is used instead of the title.
    pub process_match: String,
    pub pause_when_unfocused: bool,
    /// Show the overlay and the panel on screenshots (Win+Shift+S, OBS).
    /// Off by default: then they can never get into our own OCR.
    pub visible_in_screenshots: bool,
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
    /// "below", "above" or "over" the source subtitles.
    pub position: String,
    pub font_size: f32,
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
    /// BCP-47 tag for Windows OCR. Requires the Windows language pack's OCR
    /// component (Settings → Time & Language → Language → Add "中文(简体)").
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub toggle_dialogue: String,
    pub screen_translate: String,
    pub show_original: String,
    pub select_region: String,
    pub panel: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
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
        Self { game_title: "Where Winds Meet (燕云十六声)".into(), window_match: "Where Winds Meet".into(), process_match: String::new(), pause_when_unfocused: true, visible_in_screenshots: false }
    }
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
            region: [0.18, 0.80, 0.64, 0.12],
            interval_ms: 150,
            text_color: "light".into(),
            ink_threshold: 200,
            upscale: 2,
            preprocess: "none".into(),
            context_lines: 3,
            show_original: false,
            position: "below".into(),
            font_size: 26.0,
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
        Self { language: "zh-Hans-CN".into() }
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
        }
    }
}

impl Config {
    /// Load the user config; on first run copy the bundled default, falling back to built-ins.
    pub fn load(paths: &Paths) -> anyhow::Result<Config> {
        let p = paths.config();
        if !p.exists() {
            if paths.default_config().exists() {
                std::fs::copy(paths.default_config(), &p)?;
            } else {
                Config::default().save(paths)?;
            }
        }
        let text = std::fs::read_to_string(&p)?;
        let mut cfg: Config = toml::from_str(&text).map_err(|e| anyhow::anyhow!("Ошибка в {}: {e}", p.display()))?;
        if cfg.models.is_empty() {
            cfg.models = default_models();
        }
        Ok(cfg)
    }

    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        let tmp = paths.config().with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(tmp, paths.config())?;
        Ok(())
    }

    pub fn active_model(&self) -> &ModelPreset {
        self.models.iter().find(|m| m.id == self.llm.active_model).unwrap_or(&self.models[0])
    }

    pub fn active_model_mut(&mut self) -> &mut ModelPreset {
        let id = self.llm.active_model.clone();
        let idx = self.models.iter().position(|m| m.id == id).unwrap_or(0);
        &mut self.models[idx]
    }
}
