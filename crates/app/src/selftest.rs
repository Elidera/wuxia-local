//! Screenshot tests and saving new test cases.
//!
//! * `wuxia-local.exe --selftest <folder> [--out report.md]` — for every
//!   `*.png` in the folder run the real pipeline (OCR, the auto mode's
//!   speech filter and grouping, F8's label filter) and compare with
//!   `<name>.expect` (see `wuxia_core::selftest`). Every available engine is
//!   run (PaddleOCR, Windows OCR, both), each gets its own section of the
//!   Markdown report (also written to the GitHub Actions summary in CI).
//!   PaddleOCR models are looked up in `<exe dir>\ocr` and `--models <dir>`.
//!   Exit code (of the default engine, PaddleOCR when it loads): 0 — all
//!   passed, 1 — some failed, 2 — no OCR available here.
//! * Ctrl+F9 in the game (Command::SaveCase) — saves the screen as a new case
//!   with a draft `.expect` built from what was found, so a bad situation can
//!   be sent to the developer and turned into a test.

use crate::capture::{self, PxRect};
use crate::ocr::{Engine, Ocr};
use crate::paths::Paths;
use crate::state::Shared;
use crate::translate::Translator;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use wuxia_core::image::Frame;
use wuxia_core::lang::Lang;
use wuxia_core::selftest::{self as st, CaseResult, Found};

/// Read a PNG into a BGRA frame.
pub fn load_png(path: &Path) -> Result<Frame> {
    let file = std::fs::File::open(path).with_context(|| format!("{}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let (w, h) = (info.width, info.height);
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        other => return Err(anyhow!("неподдерживаемый формат PNG: {other:?}")),
    };
    let stride = info.line_size;
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let row = &buf[y * stride..y * stride + w as usize * channels];
        for px in row.chunks_exact(channels) {
            let (r, g, b) = match channels {
                1 | 2 => (px[0], px[0], px[0]),
                _ => (px[0], px[1], px[2]),
            };
            bgra.extend_from_slice(&[b, g, r, 255]);
        }
    }
    Ok(Frame::new(w, h, bgra))
}

/// Write a BGRA frame as PNG.
pub fn save_png(frame: &Frame, path: &Path) -> Result<()> {
    let file = std::fs::File::create(path).with_context(|| format!("{}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header()?;
    let mut rgb = Vec::with_capacity((frame.width * frame.height * 3) as usize);
    for p in frame.bgra.chunks_exact(4) {
        rgb.extend_from_slice(&[p[2], p[1], p[0]]);
    }
    w.write_image_data(&rgb)?;
    Ok(())
}

/// Run the real pipeline on one picture: what the auto mode would translate and
/// what F8 would. Returns (found, OCR time in ms).
pub async fn analyse(frame: &Frame, lang: Lang, ocr: &Ocr, tr: &Translator) -> Result<(Found, u128)> {
    analyse_with(ocr.engine(), frame, lang, ocr, tr).await
}

pub async fn analyse_with(engine: Engine, frame: &Frame, lang: Lang, ocr: &Ocr, tr: &Translator) -> Result<(Found, u128)> {
    let t0 = Instant::now();
    let lines = ocr.recognize_with(engine, frame.clone(), 1, lang).await?;
    let ms = t0.elapsed().as_millis();
    let speech = crate::auto::speech_blocks(&lines, lang, tr).into_iter().map(|(t, _)| t).collect();
    let up = crate::interface::upscale_for(0, frame.height as i32);
    let label_lines = if up == 1 { lines } else { ocr.recognize_with(engine, frame.clone(), up, lang).await? };
    let mut labels: Vec<String> = Vec::new();
    for l in label_lines {
        if crate::interface::is_label_line(&l.text, lang, tr) {
            let t = tr.fix_ocr(&l.text);
            if !labels.contains(&t) {
                labels.push(t);
            }
        }
    }
    Ok((Found { speech, labels }, ms))
}

fn arg_after(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1)).cloned()
}

/// `--selftest <folder> [--out report.md]`. Returns the process exit code.
pub fn run_cli(args: &[String]) -> i32 {
    let dir = PathBuf::from(args.get(2).filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "tests/screens".into()));
    let out = arg_after(args, "--out").map(PathBuf::from).unwrap_or_else(|| dir.join("report.md"));
    let models = arg_after(args, "--models").map(PathBuf::from);
    let (code, md) = match run(&dir, models) {
        Ok((code, md)) => (code, md),
        Err(e) => (2, format!("# Проверка на снимках\n\nНе запустилась: {e:#}\n")),
    };
    let _ = std::fs::write(&out, &md);
    if let Some(summary) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(summary) {
            let _ = f.write_all(md.as_bytes());
        }
    }
    code
}

fn run(dir: &Path, models: Option<PathBuf>) -> Result<(i32, String)> {
    let paths = Paths::detect()?;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let client = crate::llm::client::LlmClient::new("http://127.0.0.1:9", 5);
    let tr = Translator::new(paths.clone(), client, 0, "wuwa");
    let mut dirs: Vec<PathBuf> = models.into_iter().collect();
    dirs.push(paths.install.join("ocr"));
    let ocr = Ocr::spawn_with(&crate::config::Ocr::default(), &dirs)?;
    let paddle_ok = match ocr.paddle_status_blocking() {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("проверка на снимках без PaddleOCR: {e:#}");
            false
        }
    };

    let mut pngs: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("папка {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |e| e.eq_ignore_ascii_case("png")))
        .collect();
    pngs.sort();
    anyhow::ensure!(!pngs.is_empty(), "в {} нет снимков *.png", dir.display());

    let engines: Vec<Engine> = if paddle_ok { vec![Engine::Paddle, Engine::Windows, Engine::Both] } else { vec![Engine::Windows] };
    let mut md = String::new();
    if !paddle_ok {
        md.push_str("> PaddleOCR не загрузился — проверены только снимки Windows OCR.\n\n");
    }
    let mut first_code = None;
    let mut summary = String::from("| Движок | Снимков прошло | Правил выполнено | OCR, мс всего |\n|---|---|---|---|\n");
    for engine in engines {
        let (code, part, line) = run_engine(&rt, engine, &pngs, &ocr, &tr);
        first_code.get_or_insert(code);
        summary.push_str(&line);
        md.push_str(&part);
        md.push('\n');
    }
    let md = format!("# Сравнение движков распознавания\n\n{summary}\n{md}");
    Ok((first_code.unwrap_or(2), md))
}

/// One engine over all pictures: (exit code, report section, summary table row).
fn run_engine(rt: &tokio::runtime::Runtime, engine: Engine, pngs: &[PathBuf], ocr: &Ocr, tr: &Translator) -> (i32, String, String) {
    let mut results = Vec::new();
    let mut no_ocr: Option<String> = None;
    for p in pngs {
        let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        let expect_path = p.with_extension("expect");
        let expect = match std::fs::read_to_string(&expect_path) {
            Ok(s) => st::parse(&s).map_err(|e| anyhow!("{}: {e}", expect_path.display())),
            Err(_) => Ok(st::Expect { lang: Lang::En, rules: vec![] }),
        };
        let mut res = CaseResult { name, found: Found::default(), checks: vec![], ocr_ms: 0, error: None };
        match expect {
            Err(e) => res.error = Some(e.to_string()),
            Ok(expect) => {
                let windows_needed = engine == Engine::Windows;
                let ready = if windows_needed { rt.block_on(ocr.check_windows(expect.lang)) } else { Ok(()) };
                if let Err(e) = ready {
                    no_ocr = Some(e.to_string());
                    res.error = Some(format!("Windows OCR для языка «{}» не установлен", expect.lang.name_ru()));
                } else {
                    match load_png(p).and_then(|f| rt.block_on(analyse_with(engine, &f, expect.lang, ocr, tr))) {
                        Ok((found, ms)) => {
                            res.checks = st::check(&expect, &found);
                            res.found = found;
                            res.ocr_ms = ms;
                        }
                        Err(e) => res.error = Some(format!("{e:#}")),
                    }
                }
            }
        }
        results.push(res);
    }
    let part = st::report(&results, &format!("Проверка на снимках: {}", engine.name_ru())).replacen("# ", "## ", 1).replace("\n## ", "\n### ");
    let passed = results.iter().filter(|r| r.passed()).count();
    let rules: usize = results.iter().map(|c| c.checks.len()).sum();
    let rules_ok: usize = results.iter().map(|c| c.checks.iter().filter(|x| x.ok).count()).sum();
    let ms: u128 = results.iter().map(|r| r.ocr_ms).sum();
    let line = format!("| {} | {passed} из {} | {rules_ok} из {rules} | {ms} |\n", engine.name_ru(), results.len());
    let code = if results.iter().all(CaseResult::passed) {
        0
    } else if no_ocr.is_some() && results.iter().all(|r| r.error.is_some()) {
        2
    } else {
        1
    };
    (code, part, line)
}

/// Ctrl+F9: save what is on screen now as a new test case.
pub async fn save_case(shared: Arc<Shared>, tr: Arc<Translator>, ocr: Ocr) {
    let (sw, sh) = crate::win::screen_size();
    let frame = match capture::capture_clean(&shared, PxRect { x: 0, y: 0, w: sw, h: sh }) {
        Ok(f) => f,
        Err(e) => {
            shared.ui(|u| u.notice(format!("Снимок для теста: {e}")));
            return;
        }
    };
    let dir = shared.paths.data.join("cases");
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let base = dir.join(format!("{}-{stamp}", shared.cfg().general.profile));
    let lang = shared.cfg().source_lang();
    let res: Result<PathBuf> = async {
        std::fs::create_dir_all(&dir)?;
        let png = base.with_extension("png");
        save_png(&frame, &png)?;
        let (found, _) = analyse(&frame, lang, &ocr, &tr).await?;
        std::fs::write(base.with_extension("expect"), st::draft(&found, lang))?;
        Ok::<PathBuf, anyhow::Error>(png)
    }
    .await;
    match res {
        Ok(p) => {
            tracing::info!("случай для теста сохранён: {}", p.display());
            shared.ui(|u| u.notice(format!("Снимок для теста сохранён: {} (и .expect рядом). Пришлите оба файла разработчику.", p.display())));
        }
        Err(e) => shared.ui(|u| u.notice(format!("Снимок для теста: {e:#}"))),
    }
}
