//! The overlay: translations drawn over the game.
//!
//! A plain Win32 *layered* window (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST)
//! updated with `UpdateLayeredWindow` from a premultiplied bitmap — the classic
//! way subtitle tools put text over borderless games. Earlier versions drew
//! with OpenGL into a full-screen transparent egui window; on the user's PC that
//! window never showed up over the game (and could not be hidden from capture).
//!
//! * Text is rendered by GDI (Segoe UI, anti-aliased) into a black/white mask
//!   and composed with plates in `wuxia_core::raster`.
//! * The window is only as large as what is drawn right now, and hidden when
//!   there is nothing to show, so it never covers the whole game.
//! * Every 500 ms it re-asserts "topmost": some games raise themselves above
//!   other topmost windows when they get focus.
//! * Layered windows updated this way are not seen by our BitBlt capture.

use crate::config::Config;
use crate::state::{Shared, UiState};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, DeleteObject, DrawTextW, GdiFlush, GetDC, ReleaseDC, SelectObject,
    SetBkMode, SetTextColor, ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DEFAULT_CHARSET, DIB_RGB_COLORS,
    DRAW_TEXT_FORMAT, DT_CALCRECT, DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_WORDBREAK, HDC, HFONT, LOGFONTW, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, SetWindowPos, ShowWindow, TranslateMessage,
    UpdateLayeredWindow, HWND_TOPMOST, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use wuxia_core::raster::{self, Canvas, Rgba};

pub const TITLE: &str = "Wuxia Local Overlay";

// ---------------------------------------------------------------- scene

#[derive(Debug, Clone, Copy, PartialEq)]
enum Corner {
    TopLeft,
    TopRight,
    Center,
}

/// What should be on screen, independent of pixels. Re-rendered only when it changes.
#[derive(Debug, Clone, PartialEq)]
enum El {
    /// A translation plate next to the original text (x, y, w, h in screen px).
    Plate { anchor: [f32; 4], speaker: Option<String>, body: String, dim: bool, size_pt: f32, position: String, opacity: f32 },
    /// Interface Mode: Russian label painted over the original one.
    Label { rect: [f32; 4], text: String, bg: [u8; 3], fg: [u8; 3], plate: bool, font_scale: f32 },
    /// Interface Mode "list": all labels in one box on the right.
    List { text: String },
    Badge { text: String, corner: Corner, size_pt: f32 },
}

fn plate_text(source: &str, translation: &str, pending: bool, error: Option<&String>, show_original: bool) -> Option<(String, bool)> {
    if show_original {
        Some((source.to_string(), false))
    } else if !translation.is_empty() {
        Some((translation.to_string(), pending))
    } else if pending {
        Some(("…".into(), true))
    } else {
        error.map(|e| (format!("{source}  [ошибка: {}]", e.chars().take(60).collect::<String>()), false))
    }
}

fn scene(u: &UiState, cfg: &Config, screen: (f32, f32), scale: f32) -> Vec<El> {
    let mut out = Vec::new();
    if u.selecting_region {
        return out; // the region picker is its own window
    }
    let show_original = u.show_original;
    let opacity = cfg.dialogue.backdrop_opacity;
    if let Some(view) = &u.interface {
        if !show_original {
            if cfg.interface.mode == "list" {
                let text: Vec<String> = view.items.iter().map(|it| format!("{}  —  {}", it.src, it.dst.as_deref().unwrap_or("…"))).collect();
                if !text.is_empty() {
                    out.push(El::List { text: text.join("\n") });
                }
            } else {
                for it in &view.items {
                    let Some(dst) = &it.dst else { continue };
                    out.push(El::Label {
                        rect: [it.rect.x, it.rect.y, it.rect.w, it.rect.h],
                        text: dst.clone(),
                        bg: it.bg,
                        fg: it.fg,
                        plate: it.busy || cfg.interface.mode == "backdrop",
                        font_scale: cfg.interface.font_scale,
                    });
                }
            }
        }
        if view.pending > 0 {
            out.push(El::Badge { text: format!("Перевод экрана… осталось {}", view.pending), corner: Corner::TopRight, size_pt: 14.0 });
        }
    } else if cfg.dialogue.mode == "auto" {
        for l in &u.auto_lines {
            let Some((body, dim)) = plate_text(&l.source, &l.translation, l.pending, None, show_original) else { continue };
            let anchor = [l.rect.x, l.rect.y, l.rect.w, l.rect.h];
            let mut position = cfg.dialogue.position.clone();
            if position == "below" && anchor[1] + anchor[3] + 60.0 * scale > screen.1 {
                position = "above".into();
            }
            out.push(El::Plate {
                anchor,
                speaker: if show_original { None } else { l.speaker.clone() },
                body,
                dim,
                size_pt: cfg.dialogue.font_size * 0.85,
                position,
                opacity,
            });
        }
    } else if let Some(d) = &u.dialogue {
        if let Some((body, dim)) = plate_text(&d.source, &d.translation, d.pending, d.error.as_ref(), show_original) {
            let r = cfg.dialogue.region;
            out.push(El::Plate {
                anchor: [r[0] * screen.0, r[1] * screen.1, r[2] * screen.0, r[3] * screen.1],
                speaker: if show_original { None } else { d.speaker.clone() },
                body,
                dim,
                size_pt: cfg.dialogue.font_size,
                position: cfg.dialogue.position.clone(),
                opacity,
            });
        }
    }
    if u.dialogue_on && !u.paused_unfocused {
        if let Some(s) = u.server.as_ref().filter(|s| !s.is_ready()) {
            out.push(El::Badge { text: format!("Wuxia Local: {}", s.label()), corner: Corner::TopLeft, size_pt: 14.0 });
        }
    }
    if let Some(t) = u.test_overlay_until {
        let left = t.saturating_duration_since(Instant::now());
        if !left.is_zero() {
            out.push(El::Badge {
                text: format!("Wuxia Local: надпись поверх игры видна  ({} с)", left.as_secs() + 1),
                corner: Corner::Center,
                size_pt: 30.0,
            });
        }
    }
    out
}

// ---------------------------------------------------------------- layout

struct TextJob {
    text: String,
    px: i32,
    bold: bool,
    flags: DRAW_TEXT_FORMAT,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: Rgba,
    outline: Option<(Rgba, i32)>,
}

struct Op {
    rect: [i32; 4],
    fill: Rgba,
    radius: f32,
    texts: Vec<TextJob>,
}

const PARA: DRAW_TEXT_FORMAT = DRAW_TEXT_FORMAT(DT_CENTER.0 | DT_WORDBREAK.0 | DT_NOPREFIX.0);
const LINE: DRAW_TEXT_FORMAT = DRAW_TEXT_FORMAT(DT_LEFT.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0);
const LEFT_PARA: DRAW_TEXT_FORMAT = DRAW_TEXT_FORMAT(DT_LEFT.0 | DT_WORDBREAK.0 | DT_NOPREFIX.0);

fn alpha(a: f32) -> u8 {
    (a.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn layout(el: &El, g: &mut Gdi, scale: f32, screen: (f32, f32)) -> Option<Op> {
    let s = scale;
    match el {
        El::Plate { anchor, speaker, body, dim, size_pt, position, opacity } => {
            let px = (size_pt * s).round().max(8.0) as i32;
            let wrap = (anchor[2] * 1.1).max(300.0 * s) as i32;
            let (bw, bh) = g.measure(body, px, false, PARA, wrap);
            let (sw, sh) = match speaker {
                Some(sp) => g.measure(sp, px, true, PARA, wrap),
                None => (0, 0),
            };
            let (padx, pady) = ((14.0 * s) as i32, (8.0 * s) as i32);
            let (tw, th) = (bw.max(sw), bh + sh);
            let (pw, ph) = (tw + 2 * padx, th + 2 * pady);
            let r = raster::place_plate(*anchor, (pw as f32, ph as f32), position, screen, 6.0 * s);
            let (x, y) = (r[0] as i32, r[1] as i32);
            let a = if position == "over" { opacity.max(0.92) } else { *opacity };
            let outline = Some((Rgba(0, 0, 0, 200), (1.5 * s).round() as i32));
            let mut texts = Vec::new();
            if let Some(sp) = speaker {
                texts.push(TextJob {
                    text: sp.clone(),
                    px,
                    bold: true,
                    flags: PARA,
                    x: x + (pw - sw) / 2,
                    y: y + pady,
                    w: sw + 2,
                    h: sh,
                    color: Rgba(240, 200, 120, 255),
                    outline,
                });
            }
            texts.push(TextJob {
                text: body.clone(),
                px,
                bold: false,
                flags: PARA,
                x: x + (pw - bw) / 2,
                y: y + pady + sh,
                w: bw + 2,
                h: bh,
                color: if *dim { Rgba(225, 225, 225, 255) } else { Rgba(250, 246, 235, 255) },
                outline,
            });
            Some(Op { rect: [x, y, pw, ph], fill: Rgba(0, 0, 0, alpha(a)), radius: 8.0 * s, texts })
        }
        El::Label { rect, text, bg, fg, plate, font_scale } => {
            let [x, y, w, h] = *rect;
            let mut px = (h * font_scale).clamp(9.0 * s, 64.0 * s);
            let (mut tw, mut th) = g.measure(text, px as i32, false, LINE, 10_000);
            let max_w = w * 1.15 + 8.0 * s;
            if tw as f32 > max_w {
                // Russian is longer: shrink to fit down to 55 %, then allow overflow.
                px = (px * max_w / tw as f32).max(px * 0.55);
                (tw, th) = g.measure(text, px as i32, false, LINE, 10_000);
            }
            let m = (2.0 * s) as i32;
            let cover = [x as i32 - m, y as i32 - m, (w as i32).max(tw) + 2 * m, h as i32 + 2 * m];
            let (fill, color, radius) = if *plate {
                (Rgba(12, 12, 14, 230), Rgba(245, 240, 225, 255), 4.0 * s)
            } else {
                (Rgba(bg[0], bg[1], bg[2], 255), Rgba(fg[0], fg[1], fg[2], 255), 1.0)
            };
            let job = TextJob {
                text: text.clone(),
                px: px as i32,
                bold: false,
                flags: LINE,
                x: x as i32,
                y: (y + h / 2.0) as i32 - th / 2,
                w: tw + 2,
                h: th,
                color,
                outline: None,
            };
            Some(Op { rect: cover, fill, radius, texts: vec![job] })
        }
        El::List { text } => {
            let px = (15.0 * s) as i32;
            let (tw, th) = g.measure(text, px, false, LEFT_PARA, (420.0 * s) as i32);
            let pad = (12.0 * s) as i32;
            let (w, h) = (tw + 2 * pad, th + 2 * pad);
            let x = screen.0 as i32 - w - (36.0 * s) as i32;
            let y = (60.0 * s) as i32;
            let job = TextJob { text: text.clone(), px, bold: false, flags: LEFT_PARA, x: x + pad, y: y + pad, w: tw + 2, h: th, color: Rgba(255, 255, 255, 255), outline: None };
            Some(Op { rect: [x, y, w, h], fill: Rgba(0, 0, 0, 215), radius: 8.0 * s, texts: vec![job] })
        }
        El::Badge { text, corner, size_pt } => {
            let px = (size_pt * s).round() as i32;
            let (tw, th) = g.measure(text, px, false, LINE, 10_000);
            let (padx, pady, fill, radius) = match corner {
                Corner::Center => ((20.0 * s) as i32, (14.0 * s) as i32, 220, 10.0 * s),
                _ => ((6.0 * s) as i32, (6.0 * s) as i32, 175, 6.0 * s),
            };
            let (w, h) = (tw + 2 * padx, th + 2 * pady);
            let m = (12.0 * s) as i32;
            let (x, y) = match corner {
                Corner::TopLeft => (m, m),
                Corner::TopRight => (screen.0 as i32 - w - 2 * m, m),
                Corner::Center => ((screen.0 as i32 - w) / 2, (screen.1 as i32 - h) / 2),
            };
            let job = TextJob {
                text: text.clone(),
                px,
                bold: false,
                flags: LINE,
                x: x + padx,
                y: y + pady,
                w: tw + 2,
                h: th,
                color: if *corner == Corner::Center { Rgba(255, 225, 160, 255) } else { Rgba(255, 240, 210, 255) },
                outline: None,
            };
            Some(Op { rect: [x, y, w, h], fill: Rgba(0, 0, 0, fill), radius, texts: vec![job] })
        }
    }
}

// ---------------------------------------------------------------- GDI text

struct Gdi {
    dc: HDC,
    fonts: HashMap<(i32, bool), HFONT>,
}

impl Gdi {
    unsafe fn new() -> Self {
        Self { dc: CreateCompatibleDC(HDC::default()), fonts: HashMap::new() }
    }

    unsafe fn font(&mut self, px: i32, bold: bool) -> HFONT {
        if let Some(f) = self.fonts.get(&(px, bold)) {
            return *f;
        }
        let mut lf = LOGFONTW {
            lfHeight: -px.max(6),
            lfWeight: if bold { 700 } else { 400 },
            lfCharSet: DEFAULT_CHARSET,
            lfQuality: ANTIALIASED_QUALITY,
            ..Default::default()
        };
        for (i, c) in "Segoe UI".encode_utf16().enumerate() {
            lf.lfFaceName[i] = c;
        }
        let f = CreateFontIndirectW(&lf);
        if self.fonts.len() > 64 {
            for (_, old) in self.fonts.drain() {
                let _ = DeleteObject(old);
            }
        }
        self.fonts.insert((px, bold), f);
        f
    }

    /// Size of `text` laid out with `flags` within `max_w` pixels.
    fn measure(&mut self, text: &str, px: i32, bold: bool, flags: DRAW_TEXT_FORMAT, max_w: i32) -> (i32, i32) {
        if text.is_empty() {
            return (0, 0);
        }
        unsafe {
            let f = self.font(px, bold);
            SelectObject(self.dc, f);
            let mut w: Vec<u16> = text.encode_utf16().collect();
            let mut r = RECT { left: 0, top: 0, right: max_w.max(1), bottom: 0 };
            DrawTextW(self.dc, &mut w, &mut r, flags | DT_CALCRECT);
            (r.right - r.left, r.bottom - r.top)
        }
    }

    /// Coverage mask (w×h, 0–255) of white anti-aliased text drawn on black.
    fn mask(&mut self, t: &TextJob) -> Option<Vec<u8>> {
        let (w, h) = (t.w.max(1), t.h.max(1));
        unsafe {
            let (bmp, bits) = dib(self.dc, w, h)?;
            let old = SelectObject(self.dc, bmp);
            let f = self.font(t.px, t.bold);
            SelectObject(self.dc, f);
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(self.dc, COLORREF(0x00FF_FFFF));
            let mut wtext: Vec<u16> = t.text.encode_utf16().collect();
            let mut r = RECT { left: 0, top: 0, right: w, bottom: h };
            DrawTextW(self.dc, &mut wtext, &mut r, t.flags);
            let _ = GdiFlush();
            let px = std::slice::from_raw_parts(bits, (w * h * 4) as usize);
            let m = px.chunks_exact(4).map(|p| p[0].max(p[1]).max(p[2])).collect();
            SelectObject(self.dc, old);
            let _ = DeleteObject(bmp);
            Some(m)
        }
    }
}

/// A zero-filled top-down 32-bit DIB section and a pointer to its pixels.
unsafe fn dib(dc: HDC, w: i32, h: i32) -> Option<(windows::Win32::Graphics::Gdi::HBITMAP, *mut u8)> {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    match CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0) {
        Ok(b) if !bits.is_null() => Some((b, bits as *mut u8)),
        Ok(b) => {
            let _ = DeleteObject(b);
            None
        }
        Err(e) => {
            tracing::warn!("оверлей: CreateDIBSection {w}×{h}: {e}");
            None
        }
    }
}

// ---------------------------------------------------------------- window

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

struct Window {
    hwnd: HWND,
    gdi: Gdi,
    shown: bool,
    ulw_errors: u32,
    first_show_logged: bool,
}

impl Window {
    unsafe fn create() -> anyhow::Result<Window> {
        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = HSTRING::from("WuxiaLocalOverlay");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: PCWSTR(class.as_ptr()), ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            &class,
            &HSTRING::from(TITLE),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            hinst,
            None,
        )?;
        Ok(Window { hwnd, gdi: Gdi::new(), shown: false, ulw_errors: 0, first_show_logged: false })
    }

    fn scale(&self) -> f32 {
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        if dpi == 0 {
            1.0
        } else {
            dpi as f32 / 96.0
        }
    }

    unsafe fn raise(&self) {
        let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }

    fn hide(&mut self) {
        if self.shown {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.shown = false;
        }
    }

    /// Lay out, draw and show `els`. Returns the screen rectangles drawn.
    fn render(&mut self, els: &[El], screen: (i32, i32)) -> Vec<[i32; 4]> {
        let scale = self.scale();
        let sf = (screen.0 as f32, screen.1 as f32);
        let ops: Vec<Op> = els.iter().filter_map(|e| layout(e, &mut self.gdi, scale, sf)).collect();
        let rects: Vec<[i32; 4]> = ops.iter().map(|o| o.rect).collect();
        let Some([bx, by, bw, bh]) = raster::union_rect(&rects, screen) else {
            self.hide();
            return rects;
        };
        // Text masks first (they use the shared GDI DC), then compose.
        let masks: Vec<Vec<Option<Vec<u8>>>> = ops.iter().map(|o| o.texts.iter().map(|t| self.gdi.mask(t)).collect()).collect();
        unsafe {
            let screen_dc = GetDC(HWND::default());
            let dc = CreateCompatibleDC(screen_dc);
            let Some((bmp, bits)) = dib(dc, bw, bh) else {
                let _ = windows::Win32::Graphics::Gdi::DeleteDC(dc);
                ReleaseDC(HWND::default(), screen_dc);
                return rects;
            };
            let old = SelectObject(dc, bmp);
            {
                let buf = std::slice::from_raw_parts_mut(bits, (bw * bh * 4) as usize);
                let mut c = Canvas::new(bw, bh, buf);
                for (o, ms) in ops.iter().zip(&masks) {
                    c.fill_round_rect(o.rect[0] - bx, o.rect[1] - by, o.rect[2], o.rect[3], o.radius, o.fill);
                    for (t, m) in o.texts.iter().zip(ms) {
                        let Some(m) = m else { continue };
                        let (x, y, w, h) = (t.x - bx, t.y - by, t.w.max(1), t.h.max(1));
                        match t.outline {
                            Some((sh, d)) => c.draw_mask_outlined(m, w, h, x, y, t.color, sh, d),
                            None => c.draw_mask(m, w, h, x, y, t.color),
                        }
                    }
                }
            }
            let dst = POINT { x: bx, y: by };
            let size = SIZE { cx: bw, cy: bh };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: 1 };
            let r = UpdateLayeredWindow(
                self.hwnd,
                screen_dc,
                Some(&dst as *const POINT),
                Some(&size as *const SIZE),
                dc,
                Some(&src as *const POINT),
                COLORREF(0),
                Some(&blend as *const BLENDFUNCTION),
                ULW_ALPHA,
            );
            SelectObject(dc, old);
            let _ = DeleteObject(bmp);
            let _ = windows::Win32::Graphics::Gdi::DeleteDC(dc);
            ReleaseDC(HWND::default(), screen_dc);
            if let Err(e) = r {
                self.ulw_errors += 1;
                if self.ulw_errors <= 3 {
                    tracing::warn!("оверлей: UpdateLayeredWindow: {e}");
                }
                return rects;
            }
            if !self.shown {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.raise();
                self.shown = true;
                if !self.first_show_logged {
                    self.first_show_logged = true;
                    tracing::info!("оверлей: первая надпись показана ({bw}×{bh} в {bx},{by}, масштаб {scale:.2})");
                }
            }
        }
        rects
    }
}

/// Start the overlay thread. It polls the shared state ~30 times a second and
/// redraws only when what should be shown has changed.
pub fn spawn(shared: Arc<Shared>) {
    let r = std::thread::Builder::new().name("overlay".into()).spawn(move || {
        let mut w = match unsafe { Window::create() } {
            Ok(w) => w,
            Err(e) => {
                shared.ui(|u| u.notice(format!("Окно перевода не создано: {e}")));
                return;
            }
        };
        tracing::info!("оверлей: окно создано (layered, topmost)");
        let mut applied_visibility: Option<bool> = None;
        let mut last: Option<(Vec<El>, (i32, i32))> = None;
        let mut last_raise = Instant::now();
        loop {
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            let cfg = shared.cfg();
            let want = cfg.general.visible_in_screenshots;
            if applied_visibility != Some(want) {
                match crate::win::set_capture_visible(w.hwnd.0 as isize, want) {
                    Ok(()) => tracing::info!("оверлей: видимость на скриншотах = {want}"),
                    Err(e) => tracing::info!("оверлей: видимость на скриншотах не изменена ({e}); на распознавание это не влияет"),
                }
                applied_visibility = Some(want);
            }
            let screen = crate::win::screen_size();
            let scale = w.scale();
            let els = {
                let u = shared.ui.lock().unwrap();
                scene(&u, &cfg, (screen.0 as f32, screen.1 as f32), scale)
            };
            if last.as_ref().map_or(true, |(e, s)| *e != els || *s != screen) {
                let rects = w.render(&els, screen);
                *shared.overlay_px.lock().unwrap() = rects;
                last = Some((els, screen));
            }
            if w.shown && last_raise.elapsed() > Duration::from_millis(500) {
                unsafe { w.raise() };
                last_raise = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(33));
        }
    });
    if let Err(e) = r {
        tracing::warn!("оверлей: поток не запущен: {e}");
    }
}
