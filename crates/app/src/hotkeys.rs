//! Global hotkeys via RegisterHotKey on a dedicated message-loop thread.
//! Note: a registered key is consumed by Windows and does not reach the game,
//! so pick keys the game does not use (configurable in config.toml).

use crate::config::Hotkeys;
use crate::state::Command;
use crate::win::parse_hotkey;
use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

/// Returns human-readable problems (unparsable or already taken keys).
pub fn spawn(keys: &Hotkeys, tx: UnboundedSender<Command>) -> std::sync::mpsc::Receiver<Vec<String>> {
    let table: Vec<(String, Command)> = vec![
        (keys.toggle_dialogue.clone(), Command::ToggleDialogue),
        (keys.screen_translate.clone(), Command::ScreenTranslate),
        (keys.show_original.clone(), Command::ToggleOriginal),
        (keys.select_region.clone(), Command::SelectRegion),
        (keys.panel.clone(), Command::TogglePanel),
        (keys.save_case.clone(), Command::SaveCase),
    ];
    let (rep_tx, rep_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("hotkeys".into())
        .spawn(move || unsafe {
            let mut problems = Vec::new();
            let mut ids: Vec<(i32, Command)> = Vec::new();
            for (i, (key, cmd)) in table.into_iter().enumerate() {
                if key.trim().is_empty() {
                    continue;
                }
                let Some((mods, vk)) = parse_hotkey(&key) else {
                    problems.push(format!("Не распознана горячая клавиша «{key}»"));
                    continue;
                };
                let id = 100 + i as i32;
                let m = HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT;
                if RegisterHotKey(HWND::default(), id, m, vk).is_err() {
                    problems.push(format!("Клавиша «{key}» уже занята другой программой"));
                    continue;
                }
                ids.push((id, cmd));
            }
            let _ = rep_tx.send(problems);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
                if msg.message == WM_HOTKEY {
                    let id = msg.wParam.0 as i32;
                    if let Some((_, c)) = ids.iter().find(|(i, _)| *i == id) {
                        if tx.send(c.clone()).is_err() {
                            break;
                        }
                    }
                }
            }
        })
        .expect("hotkey thread");
    rep_rx
}
