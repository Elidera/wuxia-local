//! Standard Windows "Open" / "Save as" dialogs: choosing a model file (.gguf)
//! and exporting / importing dictionaries. They block the calling thread until
//! closed: call them from a separate thread, never from the UI.

use std::path::PathBuf;
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};

/// `filter` like "Модель нейросети (*.gguf)\0*.gguf\0" (pairs of description and mask).
fn dialog(save: bool, filter: &str, title: &str, default_name: &str, default_ext: &str) -> Option<PathBuf> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mut buf = vec![0u16; 2048];
        for (i, c) in default_name.encode_utf16().take(buf.len() - 1).enumerate() {
            buf[i] = c;
        }
        let filter: Vec<u16> = format!("{filter}Все файлы (*.*)\0*.*\0\0").encode_utf16().collect();
        let title = HSTRING::from(title);
        let ext = HSTRING::from(default_ext);
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(buf.as_mut_ptr()),
            nMaxFile: buf.len() as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            lpstrDefExt: if default_ext.is_empty() { PCWSTR::null() } else { PCWSTR(ext.as_ptr()) },
            Flags: if save { OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR } else { OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR },
            ..Default::default()
        };
        let ok = if save { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) };
        if !ok.as_bool() {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    }
}

/// Ask for a .gguf model file.
pub fn pick_gguf() -> Option<PathBuf> {
    dialog(false, "Модель нейросети (*.gguf)\0*.gguf\0", "Выберите файл модели (.gguf)", "", "")
}

/// Ask where to save the dictionary bundle.
pub fn save_dicts(default_name: &str) -> Option<PathBuf> {
    dialog(true, "Словари Wuxia Local (*.txt)\0*.txt\0", "Сохранить словари для обмена", default_name, "txt")
}

/// Ask for a dictionary bundle to import.
pub fn open_dicts() -> Option<PathBuf> {
    dialog(false, "Словари Wuxia Local (*.txt)\0*.txt\0", "Импорт словарей", "", "")
}
