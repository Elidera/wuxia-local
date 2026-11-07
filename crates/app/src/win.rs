//! Thin wrappers over Win32 calls used across the app.
//! Window handles are passed around as `isize` because `HWND` is `!Send`.

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetForegroundWindow, GetSystemMetrics, GetWindowLongPtrW, GetWindowTextW, SetWindowDisplayAffinity,
    SetWindowLongPtrW, GWL_EXSTYLE, SM_CXSCREEN, SM_CYSCREEN, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// Make the process per-monitor DPI aware so all coordinates are physical pixels.
/// Must be called before any window is created.
pub fn set_dpi_aware() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Primary screen size in physical pixels.
pub fn screen_size() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

pub fn foreground_title() -> String {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

pub fn find_window(title: &str) -> Option<isize> {
    unsafe {
        let t = HSTRING::from(title);
        match FindWindowW(PCWSTR::null(), &t) {
            Ok(h) if !h.0.is_null() => Some(h.0 as isize),
            _ => None,
        }
    }
}

/// Mark the overlay as a tool window that never takes focus from the game.
pub fn make_tool_window(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd as *mut _);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as isize);
    }
}

/// Make a window visible to / hidden from screen capture (screenshots, OBS, our OCR).
pub fn set_capture_visible(hwnd: isize, visible: bool) -> bool {
    let mode = if visible { WDA_NONE } else { WDA_EXCLUDEFROMCAPTURE };
    unsafe { SetWindowDisplayAffinity(HWND(hwnd as *mut _), mode).is_ok() }
}

/// Put a child process into a job that kills it when we exit (even on crash),
/// so llama-server never stays behind holding gigabytes of RAM.
pub fn kill_with_parent(process: *mut std::ffi::c_void) -> anyhow::Result<()> {
    unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null())?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )?;
        AssignProcessToJobObject(job, HANDLE(process))?;
        // Intentionally leak the job handle: it closes when our process ends.
        std::mem::forget(job);
        Ok(())
    }
}

/// Parse "Ctrl+Shift+F7" → (modifiers bitmask for RegisterHotKey, virtual key).
pub fn parse_hotkey(s: &str) -> Option<(u32, u32)> {
    const MOD_ALT: u32 = 0x1;
    const MOD_CONTROL: u32 = 0x2;
    const MOD_SHIFT: u32 = 0x4;
    const MOD_WIN: u32 = 0x8;
    let mut mods = 0;
    let mut vk = None;
    for part in s.split('+').map(|p| p.trim()) {
        let up = part.to_ascii_uppercase();
        match up.as_str() {
            "CTRL" | "CONTROL" => mods |= MOD_CONTROL,
            "ALT" => mods |= MOD_ALT,
            "SHIFT" => mods |= MOD_SHIFT,
            "WIN" => mods |= MOD_WIN,
            _ => {
                vk = Some(match up.as_str() {
                    f if f.starts_with('F') && f.len() > 1 && f[1..].parse::<u32>().is_ok() => {
                        let n: u32 = f[1..].parse().ok()?;
                        if !(1..=24).contains(&n) {
                            return None;
                        }
                        0x70 + n - 1
                    }
                    "INSERT" | "INS" => 0x2D,
                    "DELETE" | "DEL" => 0x2E,
                    "HOME" => 0x24,
                    "END" => 0x23,
                    "PAGEUP" | "PGUP" => 0x21,
                    "PAGEDOWN" | "PGDN" => 0x22,
                    "PAUSE" => 0x13,
                    "SCROLLLOCK" => 0x91,
                    "`" | "TILDE" => 0xC0,
                    k if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric()) => k.as_bytes()[0] as u32,
                    k if k.starts_with("NUM") && k.len() == 4 && k.as_bytes()[3].is_ascii_digit() => {
                        0x60 + (k.as_bytes()[3] - b'0') as u32
                    }
                    _ => return None,
                })
            }
        }
    }
    vk.map(|v| (mods, v))
}
