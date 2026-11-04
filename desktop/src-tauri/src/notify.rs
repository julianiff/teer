//! A notification when something worth knowing happens in a review whose
//! window is behind others: prepare finished, a second model disputes a
//! unit, the agent the notes went to is done.

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

/// Show `title` and `body`, unless window `label` is in front.
pub fn send(handle: &AppHandle, label: &str, title: &str, body: &str) {
    let in_front = handle
        .get_webview_window(label)
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if in_front {
        return;
    }
    let _ = handle
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();
}
