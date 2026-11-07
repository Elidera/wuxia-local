//! Screen capture of a rectangle of the desktop via GDI BitBlt.
//!
//! Why GDI for the MVP: it is simple, has no D3D setup, and for a subtitle strip
//! costs ~1–3 ms. Works for games in *borderless windowed* mode (exclusive
//! fullscreen bypasses the desktop compositor — and also hides any overlay).
//! Our overlay is a layered window with WDA_EXCLUDEFROMCAPTURE, and BitBlt is
//! called without CAPTUREBLT, so it never ends up in our own OCR.
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
