//! Standard Windows "Open file" dialog for choosing a model file (.gguf) —
//! for when the automatic download from HuggingFace does not work.

use std::path::PathBuf;
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW};

/// Ask for a .gguf file. Blocks the calling thread until the dialog closes:
/// call it from a separate thread, not from the UI.
pub fn pick_gguf() -> Option<PathBuf> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mut buf = vec![0u16; 2048];
        let filter: Vec<u16> = "Модель нейросети (*.gguf)\0*.gguf\0Все файлы (*.*)\0*.*\0\0".encode_utf16().collect();
        let title = HSTRING::from("Выберите файл модели (.gguf)");
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(buf.as_mut_ptr()),
            nMaxFile: buf.len() as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        if !GetOpenFileNameW(&mut ofn).as_bool() {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    }
}
