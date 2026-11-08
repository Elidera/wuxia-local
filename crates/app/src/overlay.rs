//! The overlay: translations drawn over the game.
//!
//! A plain Win32 *layered* window (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST)
//! with a colour key: pixels of `raster::KEY_BGR` are transparent, the rest is
//! opaque. On Windows 10 only such a window (not an `UpdateLayeredWindow`
//! one) can be hidden from screen capture — then our own capture sees the
//! game under it, and translucent plates are composed by us over that live
//! picture (~10 times a second). «Поверх» erases the original text in that
//! picture and writes the translation in its place.
//!
//! * Text is rendered by GDI (Segoe UI, anti-aliased) into a black/white mask
//!   and composed with plates in `wuxia_core::raster`.
//! * The window is only as large as what is drawn right now, and hidden when
//!   there is nothing to show, so it never covers the whole game.
//! * Every 500 ms it re-asserts "topmost": some games raise themselves above
//!   other topmost windows when they get focus.
//! * «Своё окно»: translations go into a box the user places. While editing
//!   (Command::EditBox) the window stops being click-through: drag to move,
//!   drag an edge or corner to resize, mouse wheel for the font size,
//!   right click to finish.

use crate::config::Config;
use crate::state::{Command, Shared, UiState};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, DeleteObject, DrawTextW, EndPaint, GdiFlush, GetDC, ReleaseDC,
    SelectObject, SetBkMode, SetDIBitsToDevice, SetTextColor, ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DEFAULT_CHARSET,
    DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_WORDBREAK, HDC, HFONT, LOGFONTW,
    PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetCursorPos, GetWindowLongPtrW, LoadCursorW, PeekMessageW, RegisterClassW,
    SetCursor, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, GWL_EXSTYLE, HWND_TOPMOST,
    IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, LWA_COLORKEY, MSG, PM_REMOVE, SET_WINDOW_POS_FLAGS, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
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

/// The user's look for translations: font, colours, outline (Settings).
#[derive(Debug, Clone, PartialEq)]
struct Look {
    color: [u8; 3],
    name: [u8; 3],
    outline: bool,
}

/// What should be on screen, independent of pixels. Re-rendered only when it changes.
#[derive(Debug, Clone, PartialEq)]
enum El {
    /// A translation plate next to the original text (x, y, w, h in screen px).
    Plate { anchor: [f32; 4], speaker: Option<String>, body: String, dim: bool, size_pt: f32, position: String, opacity: f32, look: Look },
    /// Interface Mode: Russian label painted over the original one.
    Label { rect: [f32; 4], text: String, bg: [u8; 3], fg: [u8; 3], plate: bool, font_scale: f32 },
    /// Interface Mode "list": all labels in one box on the right.
    List { text: String },
    /// «Своё окно»: all current lines inside the user's box (speaker, text, still translating).
    BoxPlate { rect: [f32; 4], lines: Vec<(Option<String>, String, bool)>, size_pt: f32, opacity: f32, look: Look },
    /// The box while it is being moved / resized.
    EditBox { rect: [f32; 4], size_pt: f32 },
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

/// `over_ok`: the overlay is hidden from capture, so «Поверх» can erase the
/// original text; otherwise the translation goes under it.
fn scene(u: &UiState, cfg: &Config, screen: (f32, f32), scale: f32, over_ok: bool) -> Vec<El> {
    let mut out = Vec::new();
    if u.selecting_region || u.freeze.is_some() {
        return out; // the region picker / freeze frame is its own window
    }
    if u.paused_unfocused && !u.edit_box {
        // The game is not in front (Alt+Tab, another window): no translations
        // over other programs. The panel's test caption is still shown.
        push_test_badge(u, &mut out);
        return out;
    }
    let show_original = u.show_original;
    let opacity = cfg.dialogue.backdrop_opacity;
    let b = cfg.dialogue.box_rect;
    let box_px = [b[0] * screen.0, b[1] * screen.1, b[2] * screen.0, b[3] * screen.1];
    let in_box = cfg.dialogue.position == "box";
    let look = Look { color: cfg.dialogue.overlay_color, name: cfg.dialogue.overlay_name_color, outline: cfg.dialogue.overlay_outline };
    if u.edit_box {
        out.push(El::EditBox { rect: box_px, size_pt: cfg.dialogue.font_size });
    } else if u.interface.is_none() && in_box {
        let mut lines = Vec::new();
        if cfg.dialogue.mode == "auto" {
            for l in &u.auto_lines {
                if let Some((body, dim)) = plate_text(&l.source, &l.translation, l.pending, None, show_original) {
                    lines.push((if show_original { None } else { l.speaker.clone() }, body, dim));
                }
            }
        } else if let Some(d) = &u.dialogue {
            if let Some((body, dim)) = plate_text(&d.source, &d.translation, d.pending, d.error.as_ref(), show_original) {
                lines.push((if show_original { None } else { d.speaker.clone() }, body, dim));
            }
        }
        if !lines.is_empty() {
            out.push(El::BoxPlate { rect: box_px, lines, size_pt: cfg.dialogue.font_size, opacity, look: look.clone() });
        }
    }
    if u.edit_box || (u.interface.is_none() && in_box) {
        // Already placed in the box above.
    } else if let Some(view) = &u.interface {
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
            if position == "over" && !over_ok {
                position = "below".into();
            }
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
                look: look.clone(),
            });
        }
    } else if let Some(d) = &u.dialogue {
        if let Some((body, dim)) = plate_text(&d.source, &d.translation, d.pending, d.error.as_ref(), show_original) {
            let r = cfg.dialogue.region;
            // «Поверх» covers the line itself, not the whole strip.
            let anchor = match (&d.rect, cfg.dialogue.position == "over") {
                (Some(l), true) => [l.x, l.y, l.w, l.h],
                _ => [r[0] * screen.0, r[1] * screen.1, r[2] * screen.0, r[3] * screen.1],
            };
            let position = if cfg.dialogue.position == "over" && !over_ok { "below".to_string() } else { cfg.dialogue.position.clone() };
            out.push(El::Plate {
                anchor,
                speaker: if show_original { None } else { d.speaker.clone() },
                body,
                dim,
                size_pt: cfg.dialogue.font_size,
                position,
                opacity,
                look: look.clone(),
            });
        }
    }
    if u.dialogue_on && !u.paused_unfocused {
        if let Some(s) = u.server.as_ref().filter(|s| !s.is_ready()) {
            out.push(El::Badge { text: format!("Wuxia Local: {}", s.label()), corner: Corner::TopLeft, size_pt: 14.0 });
        }
    }
    push_test_badge(u, &mut out);
    out
}

fn push_test_badge(u: &UiState, out: &mut Vec<El>) {
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
    /// Plain rectangles drawn over the fill (frame and handles of the edited box).
    extras: Vec<[i32; 4]>,
    extra_color: Rgba,
    /// «Поверх»: this screen rectangle shows the game picture with its text
    /// erased (taken live from the screen), the translation drawn on it.
    patch: Option<[i32; 4]>,
}

impl Op {
    fn new(rect: [i32; 4], fill: Rgba, radius: f32, texts: Vec<TextJob>) -> Op {
        Op { rect, fill, radius, texts, extras: Vec::new(), extra_color: Rgba(0, 0, 0, 0), patch: None }
    }
}

/// «Поверх»: the original line is erased and the translation written in its
/// place in the game's style (light text with a dark outline), as large as
/// fits the original's box but not larger than the font size setting.
#[allow(clippy::too_many_arguments)]
fn layout_over(anchor: &[f32; 4], speaker: &Option<String>, body: &str, dim: bool, size_pt: f32, look: &Look, g: &mut Gdi, s: f32, screen: (f32, f32)) -> Option<Op> {
    let [ax, ay, aw, ah] = anchor.map(|v| v.round() as i32);
    let m = (6.0 * s).round() as i32;
    let wrap = ((aw as f32 * 1.06) as i32).max((200.0 * s) as i32);
    let max_px = (size_pt * s).round().max(10.0) as i32;
    let min_px = (12.0 * s).round().max(9.0) as i32;
    let mut px = max_px;
    let (mut bw, mut bh, mut sw, mut sh);
    loop {
        (bw, bh) = g.measure(body, px, false, PARA, wrap);
        (sw, sh) = match speaker {
            Some(sp) => g.measure(sp, px, true, PARA, wrap),
            None => (0, 0),
        };
        if bh + sh <= ah || px <= min_px {
            break;
        }
        px -= ((px as f32) * 0.06).max(1.0) as i32;
    }
    let th = bh + sh;
    let pw = (aw + 2 * m).max(bw.max(sw) + 2 * m);
    let ph = (ah + 2 * m).max(th + 2 * m);
    let x = (ax + aw / 2 - pw / 2).clamp(0, (screen.0 as i32 - pw).max(0));
    let y = (ay + ah / 2 - ph / 2).clamp(0, (screen.1 as i32 - ph).max(0));
    let top = y + (ph - th) / 2;
    let outline = Some((Rgba(0, 0, 0, 200), ((px as f32) / 15.0).round().max(1.0) as i32));
    let [cr, cg, cb] = look.color;
    let body_color = if dim { Rgba(((cr as u16 * 9) / 10) as u8, ((cg as u16 * 9) / 10) as u8, ((cb as u16 * 9) / 10) as u8, 255) } else { Rgba(cr, cg, cb, 255) };
    let mut texts = Vec::new();
    if let Some(sp) = speaker {
        texts.push(TextJob { text: sp.clone(), px, bold: true, flags: PARA, x: x + (pw - sw) / 2, y: top, w: sw + 2, h: sh, color: Rgba(look.name[0], look.name[1], look.name[2], 255), outline });
    }
    texts.push(TextJob { text: body.to_string(), px, bold: false, flags: PARA, x: x + (pw - bw) / 2, y: top + sh, w: bw + 2, h: bh, color: body_color, outline });
    let mut op = Op::new([x, y, pw, ph], Rgba(0, 0, 0, 0), 0.0, texts);
    op.patch = Some([x, y, pw, ph]);
    Some(op)
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
        El::Plate { anchor, speaker, body, dim, size_pt, position, opacity, look } => {
            if position == "over" {
                return layout_over(anchor, speaker, body, *dim, *size_pt, look, g, s, screen);
            }
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
            let top = (ph - th) / 2;
            let a = *opacity;
            let outline = look.outline.then(|| (Rgba(0, 0, 0, 200), (1.5 * s).round().max(1.0) as i32));
            let text_color = |dim: bool| {
                let [r, g, b] = look.color;
                if dim {
                    Rgba(((r as u16 * 9) / 10) as u8, ((g as u16 * 9) / 10) as u8, ((b as u16 * 9) / 10) as u8, 255)
                } else {
                    Rgba(r, g, b, 255)
                }
            };
            let name_color = Rgba(look.name[0], look.name[1], look.name[2], 255);
            let mut texts = Vec::new();
            if let Some(sp) = speaker {
                texts.push(TextJob {
                    text: sp.clone(),
                    px,
                    bold: true,
                    flags: PARA,
                    x: x + (pw - sw) / 2,
                    y: y + top,
                    w: sw + 2,
                    h: sh,
                    color: name_color,
                    outline,
                });
            }
            texts.push(TextJob {
                text: body.clone(),
                px,
                bold: false,
                flags: PARA,
                x: x + (pw - bw) / 2,
                y: y + top + sh,
                w: bw + 2,
                h: bh,
                color: text_color(*dim),
                outline,
            });
            Some(Op::new([x, y, pw, ph], Rgba(0, 0, 0, alpha(a)), 8.0 * s, texts))
        }
        El::BoxPlate { rect, lines, size_pt, opacity, look } => {
            let px = (size_pt * s).round().max(8.0) as i32;
            let (padx, pady) = ((16.0 * s) as i32, (10.0 * s) as i32);
            let [bx, by, bw, bh] = rect.map(|v| v as i32);
            let inner = (bw - 2 * padx).max(80);
            let gap = px / 2;
            let outline = look.outline.then(|| (Rgba(0, 0, 0, 200), (1.5 * s).round().max(1.0) as i32));
            let [cr, cg, cb] = look.color;
            // Measure every line first, then centre the whole block in the box.
            let mut parts: Vec<(String, bool, Rgba, i32, i32)> = Vec::new();
            for (i, (speaker, body, dim)) in lines.iter().enumerate() {
                if let Some(sp) = speaker {
                    let (w, h) = g.measure(sp, px, true, PARA, inner);
                    parts.push((sp.clone(), true, Rgba(look.name[0], look.name[1], look.name[2], 255), w, h));
                }
                let (w, h) = g.measure(body, px, false, PARA, inner);
                let color = if *dim { Rgba(((cr as u16 * 9) / 10) as u8, ((cg as u16 * 9) / 10) as u8, ((cb as u16 * 9) / 10) as u8, 255) } else { Rgba(cr, cg, cb, 255) };
                // The gap after a line is stored in its height.
                let extra = if i + 1 < lines.len() { gap } else { 0 };
                parts.push((body.clone(), false, color, w, h + extra));
            }
            let content: i32 = parts.iter().map(|p| p.4).sum();
            let h = bh.max(content + 2 * pady);
            let y0 = by.min(screen.1 as i32 - h).max(0);
            let mut y = y0 + (h - content) / 2;
            let mut texts = Vec::new();
            for (text, bold, color, w, ph) in parts {
                texts.push(TextJob { text, px, bold, flags: PARA, x: bx + (bw - w) / 2, y, w: w + 2, h: ph, color, outline });
                y += ph;
            }
            Some(Op::new([bx, y0, bw, h], Rgba(0, 0, 0, alpha(*opacity)), 8.0 * s, texts))
        }
        El::EditBox { rect, size_pt } => {
            let [bx, by, bw, bh] = rect.map(|v| v as i32);
            let px = (size_pt * s).round().max(8.0) as i32;
            let small = (13.0 * s) as i32;
            let help = "Перетащите окно мышью · край или угол — размер · колесо — шрифт · правый клик — готово";
            let inner = (bw - (20.0 * s) as i32).max(80);
            let (hw, hh) = g.measure(help, small, false, PARA, inner);
            let sample = "Пример перевода: так будет выглядеть реплика.";
            let (sw, sh) = g.measure(sample, px, false, PARA, inner);
            let top = by + (6.0 * s) as i32;
            let texts = vec![
                TextJob { text: help.into(), px: small, bold: false, flags: PARA, x: bx + (bw - hw) / 2, y: top, w: hw + 2, h: hh, color: Rgba(255, 225, 160, 255), outline: None },
                TextJob {
                    text: sample.into(),
                    px,
                    bold: false,
                    flags: PARA,
                    x: bx + (bw - sw) / 2,
                    y: (by + bh / 2 - sh / 2).max(top + hh),
                    w: sw + 2,
                    h: sh,
                    color: Rgba(250, 246, 235, 255),
                    outline: Some((Rgba(0, 0, 0, 200), 1)),
                },
            ];
            let t = (2.0 * s).max(2.0) as i32;
            let k = (12.0 * s) as i32;
            let mut op = Op::new([bx, by, bw, bh], Rgba(24, 26, 34, 200), 6.0 * s, texts);
            op.extras = vec![
                [bx, by, bw, t],
                [bx, by + bh - t, bw, t],
                [bx, by, t, bh],
                [bx + bw - t, by, t, bh],
                [bx, by, k, k],
                [bx + bw - k, by, k, k],
                [bx, by + bh - k, k, k],
                [bx + bw - k, by + bh - k, k, k],
            ];
            op.extra_color = Rgba(232, 184, 92, 255);
            Some(op)
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
            Some(Op::new(cover, fill, radius, vec![job]))
        }
        El::List { text } => {
            let px = (15.0 * s) as i32;
            let (tw, th) = g.measure(text, px, false, LEFT_PARA, (420.0 * s) as i32);
            let pad = (12.0 * s) as i32;
            let (w, h) = (tw + 2 * pad, th + 2 * pad);
            let x = screen.0 as i32 - w - (36.0 * s) as i32;
            let y = (60.0 * s) as i32;
            let job = TextJob { text: text.clone(), px, bold: false, flags: LEFT_PARA, x: x + pad, y: y + pad, w: tw + 2, h: th, color: Rgba(255, 255, 255, 255), outline: None };
            Some(Op::new([x, y, w, h], Rgba(0, 0, 0, 215), 8.0 * s, vec![job]))
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
            Some(Op::new([x, y, w, h], Rgba(0, 0, 0, fill), radius, vec![job]))
        }
    }
}

// ---------------------------------------------------------------- GDI text

struct Gdi {
    dc: HDC,
    fonts: HashMap<(i32, bool), HFONT>,
    /// Font family for everything drawn (the user's choice; Segoe UI by default).
    face: String,
}

impl Gdi {
    unsafe fn new() -> Self {
        Self { dc: CreateCompatibleDC(HDC::default()), fonts: HashMap::new(), face: "Segoe UI".into() }
    }

    /// Switch the font family (cached fonts of the old one are released).
    fn set_face(&mut self, face: &str) {
        let face = if face.trim().is_empty() { "Segoe UI" } else { face.trim() };
        if self.face != face {
            self.face = face.to_string();
            for (_, f) in self.fonts.drain() {
                unsafe {
                    let _ = DeleteObject(f);
                }
            }
        }
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
        for (i, c) in self.face.encode_utf16().take(31).enumerate() {
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

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// A mouse drag of the box in progress: which edges move (0 = the whole box).
#[derive(Clone, Copy)]
struct Drag {
    zone: u8,
    x0: i32,
    y0: i32,
    start: [f32; 4],
}

static DRAG: Mutex<Option<Drag>> = Mutex::new(None);

const LEFT: u8 = 1;
const RIGHT: u8 = 2;
const TOP: u8 = 4;
const BOTTOM: u8 = 8;

fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// The box in screen pixels.
fn box_px(sh: &Shared) -> [f32; 4] {
    let (sw, shh) = crate::win::screen_size();
    let b = sh.cfg.read().unwrap().dialogue.box_rect;
    [b[0] * sw as f32, b[1] * shh as f32, b[2] * sw as f32, b[3] * shh as f32]
}

/// Which edges of `r` are under the point (within `m` pixels of the border).
fn zone_at(r: [f32; 4], x: i32, y: i32, m: f32) -> u8 {
    let (x, y) = (x as f32, y as f32);
    let mut z = 0;
    if x < r[0] + m {
        z |= LEFT;
    } else if x > r[0] + r[2] - m {
        z |= RIGHT;
    }
    if y < r[1] + m {
        z |= TOP;
    } else if y > r[1] + r[3] - m {
        z |= BOTTOM;
    }
    z
}

/// New box after dragging by (dx, dy), kept on screen and not too small.
fn dragged(d: &Drag, dx: f32, dy: f32, screen: (f32, f32)) -> [f32; 4] {
    let [mut x0, mut y0, w, h] = d.start;
    let (mut x1, mut y1) = (x0 + w, y0 + h);
    let (min_w, min_h) = (160.0, 48.0);
    if d.zone == 0 {
        let nx = (x0 + dx).clamp(0.0, (screen.0 - w).max(0.0));
        let ny = (y0 + dy).clamp(0.0, (screen.1 - h).max(0.0));
        return [nx, ny, w, h];
    }
    if d.zone & LEFT != 0 {
        x0 = (x0 + dx).clamp(0.0, x1 - min_w);
    }
    if d.zone & RIGHT != 0 {
        x1 = (x1 + dx).clamp(x0 + min_w, screen.0);
    }
    if d.zone & TOP != 0 {
        y0 = (y0 + dy).clamp(0.0, y1 - min_h);
    }
    if d.zone & BOTTOM != 0 {
        y1 = (y1 + dy).clamp(y0 + min_h, screen.1);
    }
    [x0, y0, x1 - x0, y1 - y0]
}

/// Mouse handling while the box is being edited (the window is click-through otherwise).
extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_PAINT {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            blit(dc);
            let _ = EndPaint(hwnd, &ps);
            return LRESULT(0);
        }
        let Some(sh) = SHARED.get() else { return DefWindowProcW(hwnd, msg, wp, lp) };
        let margin = 14.0 * (GetDpiForWindow(hwnd).max(96) as f32 / 96.0);
        match msg {
            // Clicks must not take the focus away from the game.
            WM_MOUSEACTIVATE => return LRESULT(3), // MA_NOACTIVATE
            WM_SETCURSOR => {
                let (x, y) = cursor();
                let drag = *DRAG.lock().unwrap();
                let z = match drag {
                    Some(d) => d.zone,
                    None => zone_at(box_px(sh), x, y, margin),
                };
                let id = match z {
                    0 => IDC_SIZEALL,
                    LEFT | RIGHT => IDC_SIZEWE,
                    TOP | BOTTOM => IDC_SIZENS,
                    z if z == LEFT | TOP || z == RIGHT | BOTTOM => IDC_SIZENWSE,
                    _ => IDC_SIZENESW,
                };
                if let Ok(c) = LoadCursorW(None, id) {
                    SetCursor(c);
                }
                return LRESULT(1);
            }
            WM_LBUTTONDOWN => {
                let (x, y) = cursor();
                let r = box_px(sh);
                *DRAG.lock().unwrap() = Some(Drag { zone: zone_at(r, x, y, margin), x0: x, y0: y, start: r });
                SetCapture(hwnd);
                return LRESULT(0);
            }
            WM_MOUSEMOVE => {
                let d = *DRAG.lock().unwrap();
                if let Some(d) = d {
                    let (x, y) = cursor();
                    let (sw, shh) = crate::win::screen_size();
                    let screen = (sw as f32, shh as f32);
                    let r = dragged(&d, (x - d.x0) as f32, (y - d.y0) as f32, screen);
                    sh.cfg.write().unwrap().dialogue.box_rect = [r[0] / screen.0, r[1] / screen.1, r[2] / screen.0, r[3] / screen.1];
                }
                return LRESULT(0);
            }
            WM_LBUTTONUP => {
                if DRAG.lock().unwrap().take().is_some() {
                    let _ = ReleaseCapture();
                    sh.send(Command::SaveConfig);
                }
                return LRESULT(0);
            }
            WM_RBUTTONUP => {
                if sh.ui.lock().unwrap().edit_box {
                    sh.send(Command::EditBox); // finish editing
                }
                return LRESULT(0);
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16;
                {
                    let mut c = sh.cfg.write().unwrap();
                    let step = if delta > 0 { 1.0 } else { -1.0 };
                    c.dialogue.font_size = (c.dialogue.font_size + step).clamp(14.0, 56.0);
                }
                sh.send(Command::SaveConfig);
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

struct Window {
    hwnd: HWND,
    gdi: Gdi,
    shown: bool,
    /// Not click-through (the box is being edited).
    interactive: bool,
    first_show_logged: bool,
    /// Windows hides this window from screen capture: our own captures see the
    /// game under it (needed for «Поверх» and for live translucency).
    excluded: bool,
    /// What is drawn now: laid out once, composed again with a fresh picture of
    /// the game under it several times a second.
    cache: Option<Drawn>,
}

struct Drawn {
    ops: Vec<Op>,
    masks: Vec<Vec<Option<Vec<u8>>>>,
    union: [i32; 4],
    scale: f32,
}

/// The last composed picture (width, height, BGRA), for WM_PAINT.
static IMAGE: Mutex<Option<(i32, i32, Vec<u8>)>> = Mutex::new(None);

unsafe fn blit(dc: HDC) {
    let img = IMAGE.lock().unwrap();
    let Some((w, h, px)) = img.as_ref() else { return };
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: *w,
            biHeight: -*h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    SetDIBitsToDevice(dc, 0, 0, *w as u32, *h as u32, 0, 0, 0, *h as u32, px.as_ptr() as *const std::ffi::c_void, &bmi, DIB_RGB_COLORS);
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
        // Colour-keyed layered window (not UpdateLayeredWindow): only this kind
        // can be hidden from screen capture on Windows 10 (checked in CI,
        // tools/probe). Translucency is composed by us over the live game picture.
        let [b, g, r] = raster::KEY_BGR;
        SetLayeredWindowAttributes(hwnd, COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16), 0, LWA_COLORKEY)?;
        Ok(Window { hwnd, gdi: Gdi::new(), shown: false, interactive: false, first_show_logged: false, excluded: false, cache: None })
    }

    fn scale(&self) -> f32 {
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        if dpi == 0 {
            1.0
        } else {
            dpi as f32 / 96.0
        }
    }

    /// Click-through normally; takes the mouse while the box is being edited.
    fn set_interactive(&mut self, on: bool) {
        if self.interactive == on {
            return;
        }
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let t = WS_EX_TRANSPARENT.0 as isize;
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, if on { ex & !t } else { ex | t });
            if !on && DRAG.lock().unwrap().take().is_some() {
                let _ = ReleaseCapture();
            }
            self.raise();
        }
        self.interactive = on;
    }

    unsafe fn raise(&self) {
        let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }

    fn hide(&mut self) {
        self.cache = None;
        if self.shown {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.shown = false;
        }
    }

    /// Does the picture need the game under it redrawn now and then?
    fn live(&self) -> bool {
        self.excluded && self.cache.as_ref().map_or(false, |c| c.ops.iter().any(|o| o.patch.is_some() || (o.fill.3 > 0 && o.fill.3 < 255)))
    }

    /// Lay out `els` and draw them. Returns the screen rectangles drawn and
    /// where the marker pixel is.
    fn render(&mut self, els: &[El], screen: (i32, i32), face: &str) -> (Vec<[i32; 4]>, Option<(i32, i32)>) {
        self.gdi.set_face(face);
        // Text grows with the screen: font 26 is meant for 1080p, on 1440p and 4K
        // it would be tiny when Windows scaling is left at 100 %.
        let scale = self.scale().max(screen.1 as f32 / 1080.0);
        let sf = (screen.0 as f32, screen.1 as f32);
        let ops: Vec<Op> = els.iter().filter_map(|e| layout(e, &mut self.gdi, scale, sf)).collect();
        let rects: Vec<[i32; 4]> = ops.iter().map(|o| o.rect).collect();
        let Some(union) = raster::union_rect(&rects, screen) else {
            self.hide();
            return (rects, None);
        };
        let masks: Vec<Vec<Option<Vec<u8>>>> = ops.iter().map(|o| o.texts.iter().map(|t| self.gdi.mask(t)).collect()).collect();
        self.cache = Some(Drawn { ops, masks, union, scale });
        let probe = self.present();
        (rects, probe)
    }

    /// Compose the laid-out picture (with the game under it when we can see
    /// it) and put it on screen. Returns where the marker pixel is.
    fn present(&mut self) -> Option<(i32, i32)> {
        let c = self.cache.as_ref()?;
        let [bx, by, bw, bh] = c.union;
        let scale = c.scale;
        // The game under the window: our window is not in this capture.
        let bg = if self.excluded { crate::capture::capture(crate::capture::PxRect { x: bx, y: by, w: bw, h: bh }).ok().map(|f| f.bgra) } else { None };
        let mut buf = vec![0u8; (bw * bh * 4) as usize];
        let mut probe: Option<(i32, i32)> = None;
        {
            let mut cv = Canvas::new(bw, bh, &mut buf);
            for (o, ms) in c.ops.iter().zip(&c.masks) {
                if let Some([px, py, pw, ph]) = o.patch {
                    match &bg {
                        Some(b) => paste_erased(&mut cv, b, bw, [px - bx, py - by, pw, ph], (3.0 * c.scale).round().max(2.0) as usize),
                        // Cannot see the game: a dark plate instead.
                        None => cv.fill_round_rect(px - bx, py - by, pw, ph, 8.0 * c.scale, Rgba(0, 0, 0, 235)),
                    }
                }
                if o.fill.3 > 0 {
                    cv.fill_round_rect(o.rect[0] - bx, o.rect[1] - by, o.rect[2], o.rect[3], o.radius, o.fill);
                }
                for e in &o.extras {
                    cv.fill_round_rect(e[0] - bx, e[1] - by, e[2], e[3], 0.0, o.extra_color);
                }
                for (t, m) in o.texts.iter().zip(ms) {
                    let Some(m) = m else { continue };
                    let (x, y, w, h) = (t.x - bx, t.y - by, t.w.max(1), t.h.max(1));
                    match t.outline {
                        Some((sh, d)) => cv.draw_mask_outlined(m, w, h, x, y, t.color, sh, d),
                        None => cv.draw_mask(m, w, h, x, y, t.color),
                    }
                }
            }
            // Marker pixels (see `capture::overlay_seen`) inside the first dark plate.
            probe = c
                .ops
                .iter()
                .find(|o| o.patch.is_none() && o.fill.3 >= 150 && (o.fill.0 as u16 + o.fill.1 as u16 + o.fill.2 as u16) < 90 && o.rect[2] > 40 && o.rect[3] > 12)
                .map(|o| (o.rect[0] + o.rect[2] / 2, o.rect[1] + 2));
            if let Some((px, py)) = probe {
                let [b, g, r] = crate::capture::PROBE_BGR;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (x, y) = (px - bx + dx, py - by + dy);
                    if x >= 0 && y >= 0 && x < bw && y < bh {
                        let i = ((y * bw + x) * 4) as usize;
                        cv.px[i..i + 4].copy_from_slice(&[b, g, r, 255]);
                    }
                }
            }
        }
        let mut out = vec![0u8; buf.len()];
        raster::flatten(&buf, bg.as_deref(), &mut out);
        *IMAGE.lock().unwrap() = Some((bw, bh, out));
        unsafe {
            let flags = SWP_NOACTIVATE | if self.shown { SET_WINDOW_POS_FLAGS(0) } else { SWP_SHOWWINDOW };
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, bx, by, bw, bh, flags);
            let dc = GetDC(self.hwnd);
            blit(dc);
            ReleaseDC(self.hwnd, dc);
            if !self.shown {
                self.shown = true;
                if !self.first_show_logged {
                    self.first_show_logged = true;
                    tracing::info!("оверлей: первая надпись показана ({bw}×{bh} в {bx},{by}, масштаб {scale:.2})");
                }
            }
        }
        probe
    }
}

/// Copy the game picture of `r` (canvas coordinates) from `bg` into the canvas,
/// with its text erased.
fn paste_erased(cv: &mut Canvas, bg: &[u8], stride_w: i32, r: [i32; 4], radius: usize) {
    let [x0, y0, w, h] = r;
    let (x0, y0) = (x0.max(0), y0.max(0));
    let (w, h) = (w.min(cv.w - x0), h.min(cv.h - y0));
    if w <= 0 || h <= 0 {
        return;
    }
    let mut tmp = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let s = (((y0 + y) * stride_w + x0) * 4) as usize;
        let d = (y * w * 4) as usize;
        if s + (w * 4) as usize <= bg.len() {
            tmp[d..d + (w * 4) as usize].copy_from_slice(&bg[s..s + (w * 4) as usize]);
        }
    }
    raster::erase_text(&mut tmp, w as usize, h as usize, false, radius);
    for y in 0..h {
        for x in 0..w {
            let si = ((y * w + x) * 4) as usize;
            let di = (((y0 + y) * cv.w + x0 + x) * 4) as usize;
            cv.px[di..di + 3].copy_from_slice(&tmp[si..si + 3]);
            cv.px[di + 3] = 255;
        }
    }
}

/// Start the overlay thread. It polls the shared state ~30 times a second and
/// redraws only when what should be shown has changed.
pub fn spawn(shared: Arc<Shared>) {
    let _ = SHARED.set(shared.clone());
    let r = std::thread::Builder::new().name("overlay".into()).spawn(move || {
        let mut w = match unsafe { Window::create() } {
            Ok(w) => w,
            Err(e) => {
                shared.ui(|u| u.notice(format!("Окно перевода не создано: {e}")));
                return;
            }
        };
        tracing::info!("оверлей: окно создано (layered, цветовой ключ, topmost)");
        let mut applied_visibility: Option<bool> = None;
        let mut affinity_failed: Option<(bool, Instant)> = None;
        let mut over_notice = false;
        let mut affinity_failures = 0u32;
        let mut last: Option<(Vec<El>, (i32, i32), String)> = None;
        let mut last_raise = Instant::now();
        let mut last_live = Instant::now();
        loop {
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            let cfg = shared.cfg();
            // «Поверх» needs the overlay hidden from capture (our OCR and the
            // erased-text patch must see the game under it).
            let want = cfg.general.visible_in_screenshots && cfg.dialogue.position != "over";
            let retry = affinity_failed.map_or(false, |(v, t): (bool, Instant)| v == want && w.shown && t.elapsed() > Duration::from_secs(5));
            if applied_visibility != Some(want) || retry {
                match crate::win::set_capture_visible(w.hwnd.0 as isize, want) {
                    Ok(()) => {
                        tracing::info!("оверлей: видимость на скриншотах = {want}");
                        affinity_failed = None;
                        affinity_failures = 0;
                        w.excluded = !want;
                        *shared.overlay_in_capture.lock().unwrap() = Some(want);
                    }
                    Err(e) => {
                        if affinity_failed.is_none() || applied_visibility != Some(want) {
                            tracing::info!("оверлей: видимость на скриншотах не изменена ({e})");
                        }
                        affinity_failed = Some((want, Instant::now()));
                        affinity_failures += 1;
                        w.excluded = false;
                        *shared.overlay_in_capture.lock().unwrap() = Some(true);
                    }
                }
                applied_visibility = Some(want);
                last = None; // compose again with / without the game picture
            }
            // Not before a retry with the window on screen has failed too.
            if cfg.dialogue.position == "over" && !w.excluded && affinity_failures >= 2 && !over_notice {
                over_notice = true;
                shared.ui(|u| u.notice("На этом ПК Windows не даёт спрятать окно перевода от захвата экрана, поэтому перевод показан под текстом игры, а не вместо него."));
            }
            let screen = crate::win::screen_size();
            let scale = w.scale();
            let (els, editing) = {
                let u = shared.ui.lock().unwrap();
                (scene(&u, &cfg, (screen.0 as f32, screen.1 as f32), scale, w.excluded), u.edit_box)
            };
            w.set_interactive(editing);
            let face = cfg.dialogue.overlay_font.clone();
            let mut probe_now: Option<Option<(i32, i32)>> = None;
            if last.as_ref().map_or(true, |(e, s, f)| *e != els || *s != screen || *f != face) {
                let (rects, probe) = w.render(&els, screen, &face);
                *shared.overlay_px.lock().unwrap() = rects;
                probe_now = Some(probe);
                last = Some((els, screen, face));
                last_live = Instant::now();
            } else if w.shown && w.live() && last_live.elapsed() >= Duration::from_millis(100) {
                // The game moves under translucent plates and erased text: ~10 times a second.
                probe_now = Some(w.present());
                last_live = Instant::now();
            }
            if let Some(probe) = probe_now {
                let mut p = shared.overlay_probe.lock().unwrap();
                // Keep the time if the marker did not move (the check waits for it to be on screen).
                *p = match (probe, *p) {
                    (Some((x, y)), Some((ox, oy, t))) if (x, y) == (ox, oy) => Some((x, y, t)),
                    (Some((x, y)), _) => Some((x, y, Instant::now())),
                    (None, _) => None,
                };
            }
            if w.shown && last_raise.elapsed() > Duration::from_millis(500) {
                unsafe { w.raise() };
                last_raise = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(if w.live() { 15 } else { 33 }));
        }
    });
    if let Err(e) = r {
        tracing::warn!("оверлей: поток не запущен: {e}");
    }
}
