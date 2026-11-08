//! Thin wrappers over Win32 calls used across the app.
//! Window handles are passed around as `isize` because `HWND` is `!Send`.

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetClassNameW, GetClientRect, GetForegroundWindow, GetWindowRect, IsIconic, IsWindowVisible, GetSystemMetrics, GetWindowLongPtrW, GetWindowTextW,
    SetForegroundWindow, SetWindowDisplayAffinity, ShowWindow, SW_RESTORE,
    SetWindowLongPtrW, GWL_EXSTYLE, SM_CXSCREEN, SM_CYSCREEN, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST,
};
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};

/// Client area (the picture, without frame and title) of the foreground window
/// in screen pixels, clipped to the primary screen.
pub fn foreground_client_rect() -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let h = GetForegroundWindow();
        if h.0.is_null() {
            return None;
        }
        let mut rc = RECT::default();
        GetClientRect(h, &mut rc).ok()?;
        let mut pt = POINT { x: 0, y: 0 };
        if !ClientToScreen(h, &mut pt).as_bool() {
            return None;
        }
        let (sw, sh) = screen_size();
        let (x0, y0) = (pt.x.max(0), pt.y.max(0));
        let (x1, y1) = ((pt.x + rc.right).min(sw), (pt.y + rc.bottom).min(sh));
        (x1 - x0 >= 320 && y1 - y0 >= 240).then_some((x0, y0, x1 - x0, y1 - y0))
    }
}

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

/// Screen rectangle (x, y, w, h in physical px) of our own visible window with
/// this title, e.g. the panel — so captures can blank it out.
pub fn visible_window_rect(title: &str) -> Option<[i32; 4]> {
    let h = HWND(find_window(title)? as *mut _);
    unsafe {
        if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
            return None;
        }
        let mut r = RECT::default();
        GetWindowRect(h, &mut r).ok()?;
        Some([r.left, r.top, r.right - r.left, r.bottom - r.top])
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
/// Returns the Win32 error text on failure.
pub fn set_capture_visible(hwnd: isize, visible: bool) -> Result<(), String> {
    let mode = if visible { WDA_NONE } else { WDA_EXCLUDEFROMCAPTURE };
    unsafe { SetWindowDisplayAffinity(HWND(hwnd as *mut _), mode).map_err(|e| e.to_string()) }
}

/// Restore and focus a window (the panel may open behind the game or the browser).
pub fn bring_to_front(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd as *mut _);
        let _ = ShowWindow(h, SW_RESTORE);
        let _ = SetForegroundWindow(h);
    }
}

/// Window class of the foreground window (e.g. "UnrealWindow", "Chrome_WidgetWin_1").
/// Reads only the window itself: no handle to the game process is ever opened,
/// so anti-cheat has nothing to see.
pub fn foreground_class() -> String {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

/// Window classes that are never the game even if the title mentions it
/// (a browser tab "Wuthering Waves guide", Explorer folder, this chat, a video…).
/// Chromium and Electron apps (Chrome, Edge, Yandex, Discord, VS Code, Claude) share
/// `Chrome_WidgetWin_*`; Telegram and OBS are Qt.
pub const NOT_GAME_CLASSES: &[&str] = &[
    "Chrome_WidgetWin", "MozillaWindowClass", "CabinetWClass", "ExploreWClass", "Notepad", "Qt", "ApplicationFrameWindow",
    "ConsoleWindowClass", "CASCADIA_HOSTING_WINDOW_CLASS", "TaskManagerWindow", "SDL_app",
];

/// Is the foreground window the game? `window_match` holds title parts separated
/// by `|`; `class_hint` (e.g. "UnrealWindow") also identifies it. Every change of
/// the foreground window is logged once, so a wrong match is easy to see.
pub fn game_in_foreground(window_match: &str, class_hint: &str) -> bool {
    static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let class = foreground_class();
    let title_raw = foreground_title();
    let title = title_raw.to_lowercase();
    let result = if NOT_GAME_CLASSES.iter().any(|c| class.starts_with(c)) {
        false
    } else {
        (!class_hint.is_empty() && class == class_hint)
            || window_match.split('|').map(str::trim).filter(|m| !m.is_empty()).any(|m| title.contains(&m.to_lowercase()))
    };
    let key = format!("{title_raw}\u{1}{class}");
    let mut last = LAST.lock().unwrap();
    if *last != key {
        let (topmost, full) = foreground_geometry();
        tracing::info!(
            "активное окно: «{title_raw}» [{class}] → {} (поверх всех: {}, на весь экран: {}, эксклюзивный полноэкранный: {})",
            if result { "игра" } else { "не игра" },
            yes_no(topmost),
            yes_no(full),
            yes_no(exclusive_fullscreen())
        );
        *last = key;
    }
    result
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "да"
    } else {
        "нет"
    }
}

/// Is the foreground window "always on top", and does it cover the whole primary screen?
fn foreground_geometry() -> (bool, bool) {
    unsafe {
        let h = GetForegroundWindow();
        if h.0.is_null() {
            return (false, false);
        }
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let topmost = ex & WS_EX_TOPMOST.0 as isize != 0;
        let (sw, sh) = screen_size();
        let mut r = RECT::default();
        let full = GetWindowRect(h, &mut r).is_ok() && r.left <= 0 && r.top <= 0 && r.right >= sw && r.bottom >= sh;
        (topmost, full)
    }
}

/// A Direct3D game in *exclusive* fullscreen: Windows shows nothing over it,
/// no overlay can work — the game must run in borderless / windowed fullscreen.
pub fn exclusive_fullscreen() -> bool {
    unsafe { matches!(SHQueryUserNotificationState(), Ok(s) if s == QUNS_RUNNING_D3D_FULL_SCREEN) }
}

const INSTANCE_MUTEX: &str = "Local\\WuxiaLocal.SingleInstance";
const SHOW_PANEL_EVENT: &str = "Local\\WuxiaLocal.ShowPanel";

/// True if this is the first running copy. The mutex handle is leaked on purpose:
/// it lives as long as the process.
pub fn claim_single_instance() -> bool {
    unsafe {
        let name = HSTRING::from(INSTANCE_MUTEX);
        match CreateMutexW(None, true, &name) {
            Ok(_h) => GetLastError() != ERROR_ALREADY_EXISTS,
            Err(_) => true, // cannot tell: allow running
        }
    }
}

/// Ask the already running copy to show its panel.
pub fn signal_show_panel() {
    unsafe {
        let name = HSTRING::from(SHOW_PANEL_EVENT);
        if let Ok(h) = OpenEventW(EVENT_MODIFY_STATE, false, &name) {
            let _ = SetEvent(h);
            let _ = CloseHandle(h);
        }
    }
}

/// In the first copy: call `on_signal` every time another copy is started.
pub fn listen_show_panel(on_signal: impl Fn() + Send + 'static) {
    let h = unsafe {
        let name = HSTRING::from(SHOW_PANEL_EVENT);
        match CreateEventW(None, false, false, &name) {
            Ok(h) => h.0 as isize,
            Err(_) => return,
        }
    };
    let _ = std::thread::Builder::new().name("instance".into()).spawn(move || loop {
        unsafe {
            WaitForSingleObject(HANDLE(h as *mut _), INFINITE);
        }
        on_signal();
    });
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
