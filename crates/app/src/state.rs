//! State shared between the async workers and the egui UI thread.

use crate::config::Config;
use crate::paths::Paths;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use tokio::sync::mpsc::UnboundedSender;
use wuxia_core::image::Rect;

#[derive(Debug, Clone, PartialEq)]
pub enum ServerStatus {
    Stopped,
    Starting(String),
    Ready { model: String },
    Failed(String),
}

impl ServerStatus {
    pub fn label(&self) -> String {
        match self {
            ServerStatus::Stopped => "остановлен".into(),
            ServerStatus::Starting(s) => format!("запуск: {s}"),
            ServerStatus::Ready { model } => format!("готов · {model}"),
            ServerStatus::Failed(e) => format!("ошибка: {e}"),
        }
    }
    pub fn is_ready(&self) -> bool {
        matches!(self, ServerStatus::Ready { .. })
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    ToggleDialogue,
    ScreenTranslate,
    ToggleOriginal,
    SelectRegion,
    RegionSelected([f32; 4]),
    CancelRegion,
    TogglePanel,
    ShowPanel,
    SwitchModel(String),
    RestartServer,
    SaveCorrection { src: String, dst: String },
    TestTranslate(String),
    ReloadGlossary,
    /// Game profile or text language changed: reload dictionaries, check OCR.
    GameChanged,
    /// Exit the program (tray menu or the panel's «Выход»).
    Quit,
    /// Open a web page in the default browser.
    OpenUrl(String),
    /// The panel was closed with ✕: explain once where the program went.
    PanelClosedToTray,
    RunBench,
    SaveConfig,
    Open(std::path::PathBuf),
}

#[derive(Debug, Clone, Default)]
pub struct DialogueView {
    pub source: String,
    pub speaker: Option<String>,
    pub translation: String,
    pub pending: bool,
    pub error: Option<String>,
    pub from_cache: bool,
}

#[derive(Debug, Clone)]
pub struct InterfaceItem {
    /// Physical screen pixels.
    pub rect: Rect,
    pub src: String,
    pub dst: Option<String>,
    pub bg: [u8; 3],
    pub fg: [u8; 3],
    /// Busy background: draw a plate instead of a flat mask.
    pub busy: bool,
}

#[derive(Debug, Clone, Default)]
pub struct InterfaceView {
    pub items: Vec<InterfaceItem>,
    pub pending: usize,
    pub generation: u64,
}

/// What the dialogue OCR saw last time (shown in the panel for troubleshooting).
#[derive(Debug, Clone)]
pub struct OcrDebug {
    pub frame: std::sync::Arc<wuxia_core::image::Frame>,
    pub text: String,
    pub ms: u128,
    pub id: u64,
}

#[derive(Debug, Clone)]
pub struct RecentLine {
    pub src: String,
    pub dst: String,
    pub model: String,
    pub ms: u128,
    pub cached: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub ocr_ms_avg: f32,
    pub llm_ms_avg: f32,
    pub translated: u64,
    pub cache_hits: u64,
}

impl Stats {
    pub fn add_ocr(&mut self, ms: u128) {
        self.ocr_ms_avg = ema(self.ocr_ms_avg, ms as f32);
    }
    pub fn add_llm(&mut self, ms: u128, cached: bool) {
        if cached {
            self.cache_hits += 1;
        } else {
            self.translated += 1;
            self.llm_ms_avg = ema(self.llm_ms_avg, ms as f32);
        }
    }
}

fn ema(prev: f32, x: f32) -> f32 {
    if prev == 0.0 {
        x
    } else {
        prev * 0.8 + x * 0.2
    }
}

#[derive(Debug, Default)]
pub struct UiState {
    pub dialogue: Option<DialogueView>,
    pub interface: Option<InterfaceView>,
    pub dialogue_on: bool,
    pub show_original: bool,
    pub selecting_region: bool,
    /// Screenshot shown in the region picker.
    pub region_shot: Option<std::sync::Arc<wuxia_core::image::Frame>>,
    pub panel_open: bool,
    /// Bring the panel to the front on the next frames (it can open behind the game).
    pub focus_panel: bool,
    pub recent: VecDeque<RecentLine>,
    pub stats: Stats,
    pub notices: VecDeque<String>,
    pub server: Option<ServerStatus>,
    pub bench_progress: Option<String>,
    pub ocr_debug: Option<OcrDebug>,
    /// Windows OCR availability (None = not checked yet).
    pub ocr_status: Option<Result<(), String>>,
    /// (glossary terms, fix rules)
    pub dict_counts: (usize, usize),
    /// Result of the panel's "check translation" field.
    pub test_result: Option<String>,
    pub paused_unfocused: bool,
}

impl UiState {
    pub fn notice(&mut self, s: impl Into<String>) {
        let s = s.into();
        tracing::warn!("{s}");
        self.notices.push_back(s);
        while self.notices.len() > 8 {
            self.notices.pop_front();
        }
    }
    pub fn push_recent(&mut self, r: RecentLine) {
        if self.recent.front().map_or(false, |x| x.src == r.src) {
            self.recent.pop_front();
        }
        self.recent.push_front(r);
        self.recent.truncate(60);
    }
}

pub struct Shared {
    pub paths: Paths,
    pub cfg: RwLock<Config>,
    pub ui: Mutex<UiState>,
    pub cmd: UnboundedSender<Command>,
    pub egui: OnceLock<eframe::egui::Context>,
    /// Incremented whenever a new dialogue line supersedes the previous one.
    pub dialogue_seq: AtomicU64,
    pub interface_gen: AtomicU64,
    pub reset_watcher: AtomicBool,
    /// Screen rectangles (physical px, x/y/w/h) the overlay painted in its last
    /// frame. Captures blank them out (see `capture::capture_clean`).
    pub overlay_px: Mutex<Vec<[i32; 4]>>,
}

impl Shared {
    pub fn repaint(&self) {
        if let Some(c) = self.egui.get() {
            c.request_repaint();
        }
    }
    pub fn send(&self, c: Command) {
        let _ = self.cmd.send(c);
    }
    pub fn cfg(&self) -> Config {
        self.cfg.read().unwrap().clone()
    }
    pub fn ui<R>(&self, f: impl FnOnce(&mut UiState) -> R) -> R {
        let r = f(&mut self.ui.lock().unwrap());
        self.repaint();
        r
    }
    pub fn next_dialogue_seq(&self) -> u64 {
        self.dialogue_seq.fetch_add(1, Ordering::SeqCst) + 1
    }
    pub fn dialogue_is_current(&self, seq: u64) -> bool {
        self.dialogue_seq.load(Ordering::SeqCst) == seq
    }
    pub fn save_config(&self) {
        let cfg = self.cfg();
        if let Err(e) = cfg.save(&self.paths) {
            self.ui(|u| u.notice(format!("Не удалось сохранить настройки: {e}")));
        }
    }
}
