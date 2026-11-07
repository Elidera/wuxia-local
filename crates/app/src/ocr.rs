//! Windows OCR (Windows.Media.Ocr) on a dedicated COM/WinRT thread.
//!
//! MVP engine: built into Windows 10/11, no extra download besides the Chinese
//! language OCR component, returns per-word boxes. PaddleOCR via ONNX Runtime
//! is planned behind the same `OcrLine` interface.

use anyhow::{anyhow, Context, Result};
use std::sync::mpsc;
use tokio::sync::oneshot;
use wuxia_core::image::{Frame, Rect};
use wuxia_core::text;

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
}

struct Job {
    frame: Frame,
    /// The frame was upscaled by this factor before OCR; boxes are divided back.
    scale: f32,
    reply: oneshot::Sender<Result<Vec<OcrLine>>>,
}

#[derive(Clone)]
pub struct Ocr {
    tx: mpsc::Sender<Job>,
    pub max_dim: u32,
}

impl Ocr {
    /// Start the OCR thread. Fails early with a readable message if the language
    /// is not installed.
    pub fn spawn(lang: &str) -> Result<Ocr> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32>>();
        let lang = lang.to_string();
        std::thread::Builder::new().name("ocr".into()).spawn(move || {
            unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
            }
            let engine = match create_engine(&lang) {
                Ok(e) => {
                    let max = OcrEngine::MaxImageDimension().unwrap_or(2600);
                    let _ = ready_tx.send(Ok(max));
                    e
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            while let Ok(job) = rx.recv() {
                let res = recognize(&engine, &job.frame, job.scale);
                let _ = job.reply.send(res);
            }
        })?;
        let max_dim = ready_rx.recv().map_err(|_| anyhow!("поток OCR завершился"))??;
        Ok(Ocr { tx, max_dim })
    }

    /// Recognize a frame. If it is larger than the engine limit it is split
    /// into overlapping tiles and the results are merged.
    pub async fn recognize(&self, frame: Frame, upscale: u32) -> Result<Vec<OcrLine>> {
        let k = upscale.max(1);
        let max = self.max_dim.max(512);
        // Fast path: fits after upscaling.
        if frame.width * k <= max && frame.height * k <= max {
            let f = if k > 1 { frame.upscale(k) } else { frame };
            return self.run(f, k as f32).await;
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
                for mut l in self.run(f, k as f32).await? {
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

    async fn run(&self, frame: Frame, scale: f32) -> Result<Vec<OcrLine>> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Job { frame, scale, reply }).map_err(|_| anyhow!("поток OCR недоступен"))?;
        rx.await.map_err(|_| anyhow!("поток OCR не ответил"))?
    }
}

fn overlap_ratio(a: &Rect, b: &Rect) -> f32 {
    let ix = ((a.x + a.w).min(b.x + b.w) - a.x.max(b.x)).max(0.0);
    let iy = ((a.y + a.h).min(b.y + b.h) - a.y.max(b.y)).max(0.0);
    let inter = ix * iy;
    let min_area = (a.w * a.h).min(b.w * b.h).max(1.0);
    inter / min_area
}

fn create_engine(tag: &str) -> Result<OcrEngine> {
    // Windows reports Simplified Chinese under slightly different tags depending on build.
    let mut tags = vec![tag.to_string()];
    if tag.to_ascii_lowercase().starts_with("zh") {
        for t in ["zh-Hans-CN", "zh-CN", "zh-Hans"] {
            if !tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                tags.push(t.to_string());
            }
        }
    }
    for t in &tags {
        let lang = Language::CreateLanguage(&HSTRING::from(t.as_str())).context("неверный код языка OCR")?;
        if OcrEngine::IsLanguageSupported(&lang).unwrap_or(false) {
            tracing::info!("OCR язык: {t}");
            return OcrEngine::TryCreateFromLanguage(&lang).context("не удалось создать OcrEngine");
        }
    }
    Err(anyhow!(
        "Windows OCR для языка {tag} не установлен. Переустановите Wuxia Local с галочкой «китайский модуль \
         распознавания» или откройте Параметры → Время и язык → Язык и регион → Добавить язык → \
         «中文(中华人民共和国)» и включите компонент «Оптическое распознавание символов»."
    ))
}

fn recognize(engine: &OcrEngine, frame: &Frame, scale: f32) -> Result<Vec<OcrLine>> {
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
        let norm = text::normalize_line(&raw);
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
            out.push(OcrLine { text: norm, rect: r.scale(1.0 / scale) });
        }
    }
    Ok(out)
}

/// Group OCR lines of a subtitle region into one text (top-to-bottom).
pub fn subtitle_text(mut lines: Vec<OcrLine>) -> String {
    lines.sort_by(|a, b| a.rect.y.partial_cmp(&b.rect.y).unwrap_or(std::cmp::Ordering::Equal));
    let texts: Vec<&str> = lines.iter().filter(|l| text::has_han(&l.text)).map(|l| l.text.as_str()).collect();
    text::join_lines(&texts)
}
