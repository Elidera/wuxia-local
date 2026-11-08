//! Text recognition: Windows OCR (Windows.Media.Ocr, on a dedicated COM/WinRT
//! thread) and PaddleOCR (`crate::paddle`) behind one `OcrLine` interface.
//!
//! Engine choice (`[ocr] engine`): `paddle` (default; falls back to Windows OCR
//! if its models fail to load), `windows`, or `both` — both engines read the
//! picture and the better reading wins for every place on screen
//! (`wuxia_core::ocr_pick`).

use anyhow::{anyhow, Context, Result};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use tokio::sync::oneshot;
use wuxia_core::image::{Frame, Rect};
use std::collections::HashMap;
use wuxia_core::lang::{self, Lang};
use wuxia_core::ocr_pick;
use wuxia_core::text;

use crate::paddle::{self, Paddle};

use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

#[derive(Debug, Clone)]
pub struct OcrLine {
    /// Normalized text (spaces between ideographs removed).
    pub text: String,
    /// Bounding box in the coordinates of the *original* (un-upscaled) frame.
    pub rect: Rect,
    /// Engine confidence 0–1 (PaddleOCR only).
    pub conf: Option<f32>,
}

/// Which engine reads the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Windows,
    Paddle,
    Both,
}

impl Engine {
    pub fn from_code(s: &str) -> Engine {
        match s.trim().to_ascii_lowercase().as_str() {
            "windows" | "win" => Engine::Windows,
            "both" | "оба" => Engine::Both,
            _ => Engine::Paddle,
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Engine::Windows => "windows",
            Engine::Paddle => "paddle",
            Engine::Both => "both",
        }
    }
    pub fn name_ru(self) -> &'static str {
        match self {
            Engine::Windows => "Windows OCR",
            Engine::Paddle => "PaddleOCR",
            Engine::Both => "PaddleOCR + Windows OCR",
        }
    }
    fn to_u8(self) -> u8 {
        match self {
            Engine::Windows => 0,
            Engine::Paddle => 1,
            Engine::Both => 2,
        }
    }
    fn from_u8(v: u8) -> Engine {
        match v {
            0 => Engine::Windows,
            2 => Engine::Both,
            _ => Engine::Paddle,
        }
    }
}

enum Job {
    Recognize {
        frame: Frame,
        /// The frame was upscaled by this factor before OCR; boxes are divided back.
        scale: f32,
        lang: Lang,
        reply: oneshot::Sender<Result<Vec<OcrLine>>>,
    },
    Check {
        lang: Lang,
        reply: oneshot::Sender<Result<()>>,
    },
}

#[derive(Clone)]
pub struct Ocr {
    tx: mpsc::Sender<Job>,
    pub max_dim: u32,
    paddle: Option<Paddle>,
    /// PaddleOCR shrinks pictures to this longer side: upscaling past it is wasted.
    paddle_side: u32,
    /// Engine asked for in the settings (`Engine` as u8).
    wanted: Arc<AtomicU8>,
    /// PaddleOCR failed to load: everything goes to Windows OCR.
    paddle_failed: Arc<std::sync::atomic::AtomicBool>,
}

impl Ocr {
    /// Start Windows OCR and, if its models are found, PaddleOCR.
    pub fn spawn_with(cfg: &crate::config::Ocr, model_dirs: &[std::path::PathBuf]) -> Result<Ocr> {
        let mut o = Ocr::spawn()?;
        o.wanted.store(Engine::from_code(&cfg.engine).to_u8(), Ordering::SeqCst);
        o.paddle_side = cfg.paddle_max_side.clamp(640, 4096);
        let settings = paddle::Settings { tier: cfg.paddle_model.clone(), threads: cfg.paddle_threads, max_side: cfg.paddle_max_side };
        match paddle::ModelFiles::find(model_dirs, &cfg.paddle_model).and_then(|f| Paddle::spawn(f, settings)) {
            Ok(p) => o.paddle = Some(p),
            Err(e) => {
                tracing::warn!("PaddleOCR недоступен: {e:#}");
                o.paddle_failed.store(true, Ordering::SeqCst);
            }
        }
        Ok(o)
    }

    /// Engine that will actually be used now.
    pub fn engine(&self) -> Engine {
        let want = Engine::from_u8(self.wanted.load(Ordering::SeqCst));
        if want != Engine::Windows && (self.paddle.is_none() || self.paddle_failed.load(Ordering::SeqCst)) {
            Engine::Windows
        } else {
            want
        }
    }

    /// Is PaddleOCR loaded and working? (Waits for the models to load.)
    pub fn paddle_status_blocking(&self) -> Result<()> {
        let p = self.paddle.as_ref().ok_or_else(|| anyhow!("файлы PaddleOCR не найдены"))?;
        let r = p.check_blocking();
        if r.is_err() {
            self.paddle_failed.store(true, Ordering::SeqCst);
        }
        r
    }

    /// Start the Windows OCR thread only. Engines are created per language on
    /// first use, so switching the game language needs no restart.
    pub fn spawn() -> Result<Ocr> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<u32>();
        std::thread::Builder::new().name("ocr".into()).spawn(move || {
            unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
            }
            let _ = ready_tx.send(OcrEngine::MaxImageDimension().unwrap_or(2600));
            let mut engines: HashMap<&'static str, OcrEngine> = HashMap::new();
            let mut engine_for = |lang: Lang| -> Result<OcrEngine> {
                if let Some(e) = engines.get(lang.code()) {
                    return Ok(e.clone());
                }
                let e = create_engine(lang)?;
                engines.insert(lang.code(), e.clone());
                Ok(e)
            };
            while let Ok(job) = rx.recv() {
                match job {
                    Job::Recognize { frame, scale, lang, reply } => {
                        let res = engine_for(lang).and_then(|e| recognize(&e, &frame, scale, lang));
                        let _ = reply.send(res);
                    }
                    Job::Check { lang, reply } => {
                        let _ = reply.send(engine_for(lang).map(|_| ()));
                    }
                }
            }
        })?;
        let max_dim = ready_rx.recv().map_err(|_| anyhow!("поток OCR завершился"))?;
        Ok(Ocr {
            tx,
            max_dim,
            paddle: None,
            paddle_side: 1600,
            wanted: Arc::new(AtomicU8::new(Engine::Windows.to_u8())),
            paddle_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    /// Can the current engine read this language? (PaddleOCR reads Chinese and
    /// English with one model; Windows OCR needs the language component.)
    pub fn check_blocking(&self, lang: Lang) -> Result<()> {
        if self.engine() != Engine::Windows && self.paddle_status_blocking().is_ok() {
            return Ok(());
        }
        self.check_windows_blocking(lang)
    }

    pub async fn check(&self, lang: Lang) -> Result<()> {
        if self.engine() != Engine::Windows {
            if let Some(p) = &self.paddle {
                match p.check().await {
                    Ok(()) => return Ok(()),
                    Err(_) => self.paddle_failed.store(true, Ordering::SeqCst),
                }
            }
        }
        self.check_windows(lang).await
    }

    /// Is Windows OCR for this language installed?
    pub fn check_windows_blocking(&self, lang: Lang) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Job::Check { lang, reply }).map_err(|_| anyhow!("поток OCR недоступен"))?;
        rx.blocking_recv().map_err(|_| anyhow!("поток OCR не ответил"))?
    }

    pub async fn check_windows(&self, lang: Lang) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Job::Check { lang, reply }).map_err(|_| anyhow!("поток OCR недоступен"))?;
        rx.await.map_err(|_| anyhow!("поток OCR не ответил"))?
    }

    /// Recognize a frame with the engine chosen in the settings.
    pub async fn recognize(&self, frame: Frame, upscale: u32, lang: Lang) -> Result<Vec<OcrLine>> {
        self.recognize_with(self.engine(), frame, upscale, lang).await
    }

    /// Recognize a frame with a given engine. PaddleOCR errors fall back to
    /// Windows OCR (and in `Both` one engine failing leaves the other).
    pub async fn recognize_with(&self, engine: Engine, frame: Frame, upscale: u32, lang: Lang) -> Result<Vec<OcrLine>> {
        let paddle = match (&self.paddle, engine) {
            (Some(p), Engine::Paddle | Engine::Both) => p,
            _ => return self.recognize_windows(frame, upscale, lang).await,
        };
        let side = frame.width.max(frame.height).max(1);
        let k = upscale.max(1).min((self.paddle_side / side).max(1));
        let pf = if k > 1 { frame.upscale(k) } else { frame.clone() };
        let pres = paddle.recognize(pf, k as f32, lang).await.map(|v| {
            v.into_iter().map(|l| OcrLine { text: l.text, rect: l.rect, conf: Some(l.conf) }).collect::<Vec<_>>()
        });
        match engine {
            Engine::Paddle => match pres {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::warn!("{e:#} — читаю Windows OCR");
                    if paddle.check().await.is_err() {
                        self.paddle_failed.store(true, Ordering::SeqCst);
                    }
                    self.recognize_windows(frame, upscale, lang).await
                }
            },
            _ => {
                let wres = self.recognize_windows(frame, upscale, lang).await;
                match (wres, pres) {
                    (Ok(w), Ok(p)) => Ok(pick(w, p, lang)),
                    (Ok(w), Err(e)) => {
                        tracing::warn!("{e:#}");
                        Ok(w)
                    }
                    (Err(e), Ok(p)) => {
                        tracing::debug!("Windows OCR: {e:#}");
                        Ok(p)
                    }
                    (Err(e), Err(_)) => Err(e),
                }
            }
        }
    }

    /// Windows OCR. If the frame is larger than the engine limit it is split
    /// into overlapping tiles and the results are merged.
    async fn recognize_windows(&self, frame: Frame, upscale: u32, lang: Lang) -> Result<Vec<OcrLine>> {
        let k = upscale.max(1);
        let max = self.max_dim.max(512);
        // Fast path: fits after upscaling.
        if frame.width * k <= max && frame.height * k <= max {
            let f = if k > 1 { frame.upscale(k) } else { frame };
            return self.run(f, k as f32, lang).await;
        }
        // Tile in original coordinates; each tile after upscale must fit `max`.
        let tile = (max / k).saturating_sub(16).max(256);
        let overlap = 64u32.min(tile / 4);
        let mut all: Vec<OcrLine> = Vec::new();
        let mut y = 0;
        while y < frame.height {
            let mut x = 0;
            while x < frame.width {
                let part = frame.crop(x, y, tile, tile);
                let f = if k > 1 { part.upscale(k) } else { part };
                for mut l in self.run(f, k as f32, lang).await? {
                    l.rect = l.rect.offset(x as f32, y as f32);
                    let dup = all.iter().any(|o| o.text == l.text && overlap_ratio(&o.rect, &l.rect) > 0.5);
                    if !dup {
                        all.push(l);
                    }
                }
                if x + tile >= frame.width {
                    break;
                }
                x += tile - overlap;
            }
            if y + tile >= frame.height {
                break;
            }
            y += tile - overlap;
        }
        Ok(all)
    }

    async fn run(&self, frame: Frame, scale: f32, lang: Lang) -> Result<Vec<OcrLine>> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Job::Recognize { frame, scale, lang, reply }).map_err(|_| anyhow!("поток OCR недоступен"))?;
        rx.await.map_err(|_| anyhow!("поток OCR не ответил"))?
    }
}

/// "Both" mode: keep the better reading for every place on screen.
fn pick(win: Vec<OcrLine>, paddle: Vec<OcrLine>, lang: Lang) -> Vec<OcrLine> {
    let conv = |v: Vec<OcrLine>| v.into_iter().map(|l| ocr_pick::Read { text: l.text, rect: l.rect, conf: l.conf }).collect::<Vec<_>>();
    let (merged, stats) = ocr_pick::merge(&conv(win), &conv(paddle), lang, 0.5);
    if stats.first + stats.second > 0 {
        tracing::debug!("OCR оба: PaddleOCR {} · Windows {}", stats.second, stats.first);
    }
    merged.into_iter().map(|r| OcrLine { text: r.text, rect: r.rect, conf: r.conf }).collect()
}

fn overlap_ratio(a: &Rect, b: &Rect) -> f32 {
    let ix = ((a.x + a.w).min(b.x + b.w) - a.x.max(b.x)).max(0.0);
    let iy = ((a.y + a.h).min(b.y + b.h) - a.y.max(b.y)).max(0.0);
    let inter = ix * iy;
    let min_area = (a.w * a.h).min(b.w * b.h).max(1.0);
    inter / min_area
}

fn create_engine(lang: Lang) -> Result<OcrEngine> {
    // Windows reports languages under slightly different tags depending on build.
    let tags: &[&str] = match lang {
        Lang::Zh => &["zh-Hans-CN", "zh-CN", "zh-Hans"],
        Lang::En => &["en-US", "en-GB", "en"],
    };
    for t in tags {
        let l = Language::CreateLanguage(&HSTRING::from(*t)).context("неверный код языка OCR")?;
        if OcrEngine::IsLanguageSupported(&l).unwrap_or(false) {
            tracing::info!("OCR язык: {t}");
            return OcrEngine::TryCreateFromLanguage(&l).context("не удалось создать OcrEngine");
        }
    }
    let (what, add) = match lang {
        Lang::Zh => ("китайского", "«中文(中华人民共和国)»"),
        Lang::En => ("английского", "«English (United States)»"),
    };
    Err(anyhow!(
        "Windows OCR для {what} не установлен. Переустановите Wuxia Local с галочкой «модуль распознавания» \
         или откройте Параметры → Время и язык → Язык и регион → Добавить язык → {add} \
         и включите компонент «Оптическое распознавание символов»."
    ))
}

fn recognize(engine: &OcrEngine, frame: &Frame, scale: f32, lang: Lang) -> Result<Vec<OcrLine>> {
    let writer = DataWriter::new()?;
    writer.WriteBytes(&frame.bgra)?;
    let buffer = writer.DetachBuffer()?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        frame.width as i32,
        frame.height as i32,
        BitmapAlphaMode::Premultiplied,
    )?;
    let result = engine.RecognizeAsync(&bitmap)?.get()?;
    let mut out = Vec::new();
    for line in result.Lines()? {
        let raw = line.Text()?.to_string_lossy();
        let norm = text::normalize_for(&raw, lang);
        if norm.is_empty() {
            continue;
        }
        let mut rect: Option<Rect> = None;
        for w in line.Words()? {
            let r = w.BoundingRect()?;
            let wr = Rect { x: r.X, y: r.Y, w: r.Width, h: r.Height };
            rect = Some(match rect {
                Some(a) => a.union(&wr),
                None => wr,
            });
        }
        if let Some(r) = rect {
            out.push(OcrLine { text: norm, rect: r.scale(1.0 / scale), conf: None });
        }
    }
    Ok(out)
}

/// Group OCR lines of a subtitle region into one text (top-to-bottom).
/// For English, a short Title-Case first line is the speaker name shown above
/// the line; it is returned as "Name: text" so `split_speaker` picks it up.
pub fn subtitle_text(mut lines: Vec<OcrLine>, lang: Lang) -> String {
    lines.sort_by(|a, b| a.rect.y.partial_cmp(&b.rect.y).unwrap_or(std::cmp::Ordering::Equal));
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).filter(|t| lang::is_label(t, lang)).collect();
    if lang == Lang::En && texts.len() >= 2 && text::looks_like_speaker_en(texts[0]) && texts[0].split_whitespace().count() <= 3 {
        let body = text::join_lines_en(&texts[1..]);
        if !body.is_empty() && lang::sentence_like_en(&body) {
            return format!("{}: {body}", texts[0]);
        }
    }
    text::join_lines_for(&texts, lang)
}
