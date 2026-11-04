//! The app's own preferences, kept beside teer's reviews like the recent
//! repositories: whether the windows show the buttons, or only the keys
//! as the terminal does. View › Show Buttons (⇧⌘B) and the palette switch
//! it for every window at once.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::menu::MenuItemKind;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// The menu item that shows and hides the buttons.
pub const BUTTONS_ITEM: &str = "buttons";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// The windows show the buttons where their actions act.
    pub buttons: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { buttons: true }
    }
}

fn file() -> PathBuf {
    teer::review::store::data_dir().join("desktop-prefs.json")
}

/// The preferences saved, else the defaults.
pub fn load() -> Prefs {
    load_from(&file())
}

fn load_from(path: &Path) -> Prefs {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_to(path: &Path, prefs: Prefs) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(&prefs)?)
}

/// Show the buttons (or not) in every window: saved, ticked in the menu,
/// told to each window.
pub fn set_buttons<R: Runtime>(handle: &AppHandle<R>, on: bool) {
    let prefs = Prefs { buttons: on };
    let _ = save_to(&file(), prefs);
    let item = handle.menu().and_then(|m| {
        m.items().ok()?.into_iter().find_map(|k| match k {
            MenuItemKind::Submenu(s) => s.get(BUTTONS_ITEM),
            _ => None,
        })
    });
    if let Some(MenuItemKind::Check(c)) = item {
        let _ = c.set_checked(on);
    }
    // Each window listens to what is sent to it alone.
    for label in handle.webview_windows().into_keys() {
        let _ = handle.emit_to(tauri::EventTarget::webview_window(label), "buttons", on);
    }
}

/// The window asks to show the buttons, or not.
#[tauri::command]
pub fn set_buttons_shown(handle: AppHandle, on: bool) {
    set_buttons(&handle, on);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buttons_show_until_hidden_and_stay_hidden() {
        let dir = std::env::temp_dir().join(format!("teer-prefs-{}", std::process::id()));
        let path = dir.join("desktop-prefs.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_from(&path).buttons, "shown by default");
        save_to(&path, Prefs { buttons: false }).unwrap();
        assert!(!load_from(&path).buttons);
        // A file from before a preference existed keeps its default.
        std::fs::write(&path, "{}").unwrap();
        assert!(load_from(&path).buttons);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
