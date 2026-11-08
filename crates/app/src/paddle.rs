//! PaddleOCR (PP-OCRv6 via the `ocr-rs` crate, MNN runtime, CPU) on its own thread.
//!
//! The models (~16 MB) ship with the installer in `<install>\ocr`; a newer set
//! can be put into `<data>\models\ocr`. One model reads both Chinese and
//! English, so no Windows language packs are needed.

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use tokio::sync::oneshot;
use wuxia_core::image::{Frame, Rect};
use wuxia_core::lang::Lang;
use wuxia_core::text;

/// Model files of one PP-OCRv6 tier ("tiny", "small", "medium").
pub struct ModelFiles {
    pub det: PathBuf,
    pub rec: PathBuf,
    pub keys: PathBuf,
}

impl ModelFiles {
    fn in_dir(dir: &Path, tier: &str) -> ModelFiles {
        ModelFiles {
            det: dir.join(format!("PP-OCRv6_{tier}_det.mnn")),
            rec: dir.join(format!("PP-OCRv6_{tier}_rec.mnn")),
            keys: dir.join(format!("ppocr_keys_v6_{tier}.txt")),
        }
    }
    fn exist(&self) -> bool {
        self.det.is_file() && self.rec.is_file() && self.keys.is_file()
    }

    /// Look in the data folder first (user-supplied models), then next to the exe.
    pub fn find(dirs: &[PathBuf], tier: &str) -> Result<ModelFiles> {
        for d in dirs {
            let m = ModelFiles::in_dir(d, tier);
            if m.exist() {
                return Ok(m);
            }
        }
        let shown: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
        Err(anyhow!(
            "файлы PaddleOCR («PP-OCRv6_{tier}_det.mnn», «PP-OCRv6_{tier}_rec.mnn», «ppocr_keys_v6_{tier}.txt») не найдены в {}. Переустановите Wuxia Local.",
            shown.join(" и ")
        ))
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub tier: String,
    pub threads: i32,
    /// Longer picture side for text detection (bigger — finds smaller text, slower).
    pub max_side: u32,
}

/// A recognised line: normalized text, box in original frame coordinates, confidence.
pub struct PaddleLine {
    pub text: String,
    pub rect: Rect,
    pub conf: f32,
}

enum Job {
    Recognize { frame: Frame, scale: f32, lang: Lang, reply: oneshot::Sender<Result<Vec<PaddleLine>>> },
    Check { reply: oneshot::Sender<Result<()>> },
}

#[derive(Clone)]
pub struct Paddle {
    tx: mpsc::Sender<Job>,
}

impl Paddle {
    /// Start the thread; the models load there right away (about a second).
    /// A load error is returned by every later request, so the caller can fall
    /// back to Windows OCR.
    pub fn spawn(files: ModelFiles, s: Settings) -> Result<Paddle> {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new().name("paddle-ocr".into()).spawn(move || {
            let t0 = std::time::Instant::now();
            let det_opts = ocr_rs::DetOptions::default().with_max_side_len(s.max_side.clamp(640, 4096));
            let config = ocr_rs::OcrEngineConfig::new()
                .with_threads(s.threads.clamp(1, 16))
                .with_det_options(det_opts)
                .with_min_result_confidence(0.3);
            let engine = ocr_rs::OcrEngine::new(&files.det, &files.rec, &files.keys, Some(config)).map_err(|e| format!("{e}"));
            match &engine {
                Ok(_) => tracing::info!("PaddleOCR {} загружен за {} мс ({} потока)", s.tier, t0.elapsed().as_millis(), s.threads),
                Err(e) => tracing::error!("PaddleOCR не загрузился: {e}"),
            }
            while let Ok(job) = rx.recv() {
                match job {
                    Job::Recognize { frame, scale, lang, reply } => {
                        let res = match &engine {
                            Ok(e) => recognize(e, &frame, scale, lang),
                            Err(e) => Err(anyhow!("PaddleOCR не загрузился: {e}")),
                        };
                        let _ = reply.send(res);
                    }
                    Job::Check { reply } => {
                        let _ = reply.send(engine.as_ref().map(|_| ()).map_err(|e| anyhow!("PaddleOCR не загрузился: {e}")));
                    }
                }
            }
        })?;
        Ok(Paddle { tx })
    }

    /// Did the models load? Waits for the load to finish.
    pub fn check_blocking(&self) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Job::Check { reply }).map_err(|_| anyhow!("поток PaddleOCR недоступен"))?;
        rx.blocking_recv().map_err(|_| anyhow!("поток PaddleOCR не ответил"))?
    }

    pub async fn check(&self) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Job::Check { reply }).map_err(|_| anyhow!("поток PaddleOCR недоступен"))?;
        rx.await.map_err(|_| anyhow!("поток PaddleOCR не ответил"))?
    }

    /// `frame` may already be upscaled by `scale`; boxes are returned in the
    /// coordinates of the original frame.
    pub async fn recognize(&self, frame: Frame, scale: f32, lang: Lang) -> Result<Vec<PaddleLine>> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Job::Recognize { frame, scale, lang, reply }).map_err(|_| anyhow!("поток PaddleOCR недоступен"))?;
        rx.await.map_err(|_| anyhow!("поток PaddleOCR не ответил"))?
    }
}

fn to_image(frame: &Frame) -> Result<image::DynamicImage> {
    let mut rgb = Vec::with_capacity((frame.width * frame.height * 3) as usize);
    for p in frame.bgra.chunks_exact(4) {
        rgb.extend_from_slice(&[p[2], p[1], p[0]]);
    }
    let img = image::RgbImage::from_raw(frame.width, frame.height, rgb).ok_or_else(|| anyhow!("неверный размер кадра"))?;
    Ok(image::DynamicImage::ImageRgb8(img))
}

fn recognize(engine: &ocr_rs::OcrEngine, frame: &Frame, scale: f32, lang: Lang) -> Result<Vec<PaddleLine>> {
    if frame.width < 8 || frame.height < 8 {
        return Ok(Vec::new());
    }
    let img = to_image(frame)?;
    let results = engine.recognize(&img).map_err(|e| anyhow!("PaddleOCR: {e}"))?;
    let mut out = Vec::with_capacity(results.len());
    for r in results {
        let norm = text::normalize_for(&r.text, lang);
        if norm.is_empty() {
            continue;
        }
        let b = &r.bbox.rect;
        let rect = Rect { x: b.left() as f32, y: b.top() as f32, w: b.width() as f32, h: b.height() as f32 };
        out.push(PaddleLine { text: norm, rect: rect.scale(1.0 / scale), conf: r.confidence });
    }
    Ok(out)
}
