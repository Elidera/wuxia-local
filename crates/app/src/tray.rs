//! Icon in the notification area (next to the clock).
//!
//! The panel's ✕ only hides the panel: translation and hotkeys keep working in
//! the background. The tray icon shows that the program is running and offers
//! «Открыть панель» and «Выход». Runs its own Win32 message loop on a thread.

use crate::state::Command;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::OnceLock;
use tokio::sync::mpsc::UnboundedSender;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_USER, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, GetCursorPos,
    GetMessageW, PostMessageW, RegisterClassW, SetForegroundWindow, TrackPopupMenu, TranslateMessage, HICON, MF_SEPARATOR,
    MF_STRING, MSG, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_LBUTTONDBLCLK,
    WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW, WS_POPUP,
};

const WM_TRAY: u32 = WM_APP + 1;
const ID_OPEN: usize = 1;
const ID_DIALOGUE: usize = 2;
const ID_EXIT: usize = 3;

static SENDER: OnceLock<UnboundedSender<Command>> = OnceLock::new();
static HWND_TRAY: AtomicIsize = AtomicIsize::new(0);
static HICON_TRAY: AtomicIsize = AtomicIsize::new(0);

fn send(c: Command) {
    if let Some(tx) = SENDER.get() {
        let _ = tx.send(c);
    }
}

fn wide<const N: usize>(s: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (i, c) in s.encode_utf16().take(N - 1).enumerate() {
        out[i] = c;
    }
    out
}

/// Build an HICON from 32×32 RGBA pixels (her head).
unsafe fn make_icon(hinst: HINSTANCE) -> Option<HICON> {
    let rgba = crate::mascot::ICON_32;
    let mut bgra = Vec::with_capacity(rgba.len());
    for p in rgba.chunks_exact(4) {
        bgra.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    let and_mask = vec![0u8; 32 * 32 / 8];
    CreateIcon(hinst, 32, 32, 1, 32, and_mask.as_ptr(), bgra.as_ptr()).ok()
}

fn base_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let _ = AppendMenuW(menu, MF_STRING, ID_OPEN, &HSTRING::from("Открыть панель"));
    let _ = AppendMenuW(menu, MF_STRING, ID_DIALOGUE, &HSTRING::from("Перевод диалогов: вкл / выкл"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, &HSTRING::from("Выход"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // Required so the menu closes when the user clicks elsewhere.
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, None);
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAY => {
                match (lp.0 as u32) & 0xFFFF {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => send(Command::ShowPanel),
                    WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                match wp.0 & 0xFFFF {
                    ID_OPEN => send(Command::ShowPanel),
                    ID_DIALOGUE => send(Command::ToggleDialogue),
                    ID_EXIT => send(Command::Quit),
                    _ => {}
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Add the tray icon. Clicking it opens the panel; right click shows the menu.
pub fn spawn(tx: UnboundedSender<Command>) {
    let _ = SENDER.set(tx);
    let r = std::thread::Builder::new().name("tray".into()).spawn(|| unsafe {
        let hinst: HINSTANCE = match GetModuleHandleW(None) {
            Ok(h) => h.into(),
            Err(e) => {
                tracing::warn!("трей: {e}");
                return;
            }
        };
        let class = HSTRING::from("WuxiaLocalTray");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: PCWSTR(class.as_ptr()), ..Default::default() };
        RegisterClassW(&wc);
        // A hidden ordinary window (never shown): message-only windows cannot own the popup menu.
        let hwnd = match CreateWindowExW(WINDOW_EX_STYLE(0), &class, &HSTRING::from("Wuxia Local Tray"), WS_POPUP, 0, 0, 0, 0, None, None, hinst, None) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("трей: окно не создано: {e}");
                return;
            }
        };
        HWND_TRAY.store(hwnd.0 as isize, Ordering::SeqCst);
        let mut nid = base_data(hwnd);
        nid.uFlags = NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.szTip = wide::<128>("Wuxia Local — переводчик работает. Клик — панель, правый клик — меню");
        if let Some(icon) = make_icon(hinst) {
            HICON_TRAY.store(icon.0 as isize, Ordering::SeqCst);
            nid.hIcon = icon;
            nid.uFlags |= NIF_ICON;
        }
        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            tracing::warn!("трей: значок не добавлен");
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
    if let Err(e) = r {
        tracing::warn!("трей: {e}");
    }
}

/// Pop-up notification from the tray icon (shown once after the first ✕).
pub fn balloon(title: &str, text: &str) {
    let h = HWND_TRAY.load(Ordering::SeqCst);
    if h == 0 {
        return;
    }
    unsafe {
        let mut nid = base_data(HWND(h as *mut _));
        nid.uFlags = NIF_INFO;
        nid.szInfoTitle = wide::<64>(title);
        nid.szInfo = wide::<256>(text);
        nid.dwInfoFlags = NIIF_USER;
        let icon = HICON_TRAY.load(Ordering::SeqCst);
        if icon != 0 {
            nid.hBalloonIcon = HICON(icon as *mut _);
        }
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Remove the icon on exit (otherwise it lingers until the mouse passes over it).
pub fn remove() {
    let h = HWND_TRAY.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &base_data(HWND(h as *mut _)));
        }
    }
}
