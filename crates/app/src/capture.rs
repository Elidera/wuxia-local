//! Screen capture of a rectangle of the desktop via GDI BitBlt.
//!
//! Why GDI for the MVP: it is simple, has no D3D setup, and for a subtitle strip
//! costs ~1–3 ms. Works for games in *borderless windowed* mode (exclusive
//! fullscreen bypasses the desktop compositor — and also hides any overlay).
//! Our overlay is a layered window, normally excluded from capture
//! (WDA_EXCLUDEFROMCAPTURE). Whether that worked is checked on every capture
//! with a marker pixel; if the overlay is visible, its rectangles are blanked.
//! A Windows.Graphics.Capture backend can replace this behind the same function.

use wuxia_core::image::Frame;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, SRCCOPY,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PxRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PxRect {
    /// From fractions of the primary screen.
    pub fn from_fraction(f: [f32; 4], screen: (i32, i32)) -> PxRect {
        let (sw, sh) = (screen.0 as f32, screen.1 as f32);
        let x = (f[0].clamp(0.0, 1.0) * sw) as i32;
        let y = (f[1].clamp(0.0, 1.0) * sh) as i32;
        let w = ((f[2].clamp(0.0, 1.0) * sw) as i32).clamp(8, screen.0 - x);
        let h = ((f[3].clamp(0.0, 1.0) * sh) as i32).clamp(8, screen.1 - y);
        PxRect { x, y, w, h }
    }
}

/// What to read: the game window's picture when the game is in front (no
/// taskbar, no other windows, no desktop around a windowed game), otherwise the
/// whole primary screen.
pub fn game_area(cfg: &crate::config::Config) -> PxRect {
    let (sw, sh) = crate::win::screen_size();
    let full = PxRect { x: 0, y: 0, w: sw, h: sh };
    let g = &cfg.general;
    if g.window_match.trim().is_empty() || !crate::win::game_in_foreground(&g.window_match, crate::config::profile(&g.profile).window_class) {
        return full;
    }
    crate::win::foreground_client_rect().map_or(full, |(x, y, w, h)| PxRect { x, y, w, h })
}

/// Capture and blank out everything our overlay is drawing right now (dialogue
/// plate, screen-translation labels, status badges). On some PCs Windows refuses
/// to hide the overlay from capture, and then OCR would read our own Russian text.
///
/// When the overlay is known to be invisible to capture (Windows hid it, or it
/// is hidden for this very capture), only the panel is blanked: in «Поверх»
/// mode the original text under our plate must stay readable.
pub fn capture_clean(shared: &crate::state::Shared, r: PxRect) -> anyhow::Result<Frame> {
    let mut f = capture(r)?;
    let seen = overlay_seen(shared, &f, r);
    if seen {
        mask_overlay(shared, &mut f, r);
    } else if let Some(p) = crate::win::visible_window_rect(crate::ui::PANEL_TITLE) {
        apply_masks(&mut f, r, &[p]);
    }
    Ok(f)
}

/// Does this capture contain our overlay? Checked with the marker pixel the
/// overlay draws (`Shared::overlay_probe`); the answer is remembered for
/// captures that do not cover the marker. Unknown counts as "yes" (blank it).
fn overlay_seen(shared: &crate::state::Shared, f: &Frame, r: PxRect) -> bool {
    use std::sync::atomic::Ordering;
    if shared.overlay_hidden.load(Ordering::SeqCst) {
        return false;
    }
    let probe = *shared.overlay_probe.lock().unwrap();
    let mut known = shared.overlay_in_capture.lock().unwrap();
    if let Some((px, py, since)) = probe {
        // Give the compositor a moment to put a new frame on screen.
        let (x, y) = (px - r.x, py - r.y);
        if since.elapsed().as_millis() > 150 && x >= 0 && y >= 0 && x + 1 < f.width as i32 && y + 1 < f.height as i32 {
            let i = ((y as u32 * f.width + x as u32) * 4) as usize;
            let seen = f.bgra.get(i..i + 3) == Some(&PROBE_BGR[..]);
            if *known != Some(seen) {
                tracing::info!("оверлей {} при захвате экрана", if seen { "виден" } else { "не виден" });
                *known = Some(seen);
            }
        }
    }
    known.unwrap_or(true)
}

/// Colour of the marker pixel, B, G, R (as stored in a frame). Next to black,
/// invisible on our dark plates, and practically never found in a game picture
/// as an exact 2×2 block.
pub const PROBE_BGR: [u8; 3] = [3, 2, 1];

/// Blank the current overlay rectangles in a frame captured at `r`.
pub fn mask_overlay(shared: &crate::state::Shared, f: &mut Frame, r: PxRect) {
    apply_masks(f, r, &own_rects(shared));
}

/// Everything of ours on screen right now: overlay drawings and the panel window
/// (an ordinary window: if it is on screen, it is not the game).
pub fn own_rects(shared: &crate::state::Shared) -> Vec<[i32; 4]> {
    let mut rects = shared.overlay_px.lock().unwrap().clone();
    if let Some(p) = crate::win::visible_window_rect(crate::ui::PANEL_TITLE) {
        rects.push(p);
    }
    rects
}

pub fn apply_masks(f: &mut Frame, r: PxRect, rects: &[[i32; 4]]) {
    for [x, y, w, h] in rects {
        f.mask(x - r.x - 4, y - r.y - 4, w + 8, h + 8);
    }
}

pub fn capture(r: PxRect) -> anyhow::Result<Frame> {
    anyhow::ensure!(r.w > 0 && r.h > 0, "пустая область захвата");
    unsafe {
        let screen = GetDC(HWND::default());
        anyhow::ensure!(!screen.is_invalid(), "GetDC failed");
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, r.w, r.h);
        let old = SelectObject(mem, bmp);
        let blt = BitBlt(mem, 0, 0, r.w, r.h, screen, r.x, r.y, SRCCOPY);

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: r.w,
                biHeight: -r.h, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (r.w * r.h * 4) as usize];
        let lines = GetDIBits(mem, bmp, 0, r.h as u32, Some(buf.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);

        SelectObject(mem, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
        ReleaseDC(HWND::default(), screen);

        blt?;
        anyhow::ensure!(lines == r.h, "GetDIBits вернул {lines} строк из {}", r.h);
        for px in buf.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Ok(Frame::new(r.w as u32, r.h as u32, buf))
    }
}
