//! Review windows: one review each, opened on the start screen.

use std::sync::atomic::{AtomicU32, Ordering};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use crate::commands::Windows;
use crate::host::Input;

static NEXT: AtomicU32 = AtomicU32::new(1);

/// A new window on the start screen.
pub fn open(handle: &AppHandle) -> tauri::Result<WebviewWindow> {
    let label = format!("review-{}", NEXT.fetch_add(1, Ordering::SeqCst));
    let w = WebviewWindowBuilder::new(handle, &label, WebviewUrl::App("index.html".into()))
        .title("Teer")
        .inner_size(1320.0, 860.0)
        .min_inner_size(760.0, 480.0)
        .build()?;
    let h = handle.clone();
    let l = label.clone();
    w.on_window_event(move |ev| {
        if let WindowEvent::Destroyed = ev {
            // The review saves and lets its checkout go.
            let windows = h.state::<Windows>();
            if let Ok(mut map) = windows.0.lock()
                && let Some(state) = map.remove(&l)
                && let Some(host) = state.host
            {
                let _ = host.tx.send(Input::Close);
            }
        }
    });
    Ok(w)
}

/// The window in focus, else any.
pub fn focused(handle: &AppHandle) -> Option<WebviewWindow> {
    let windows = handle.webview_windows();
    windows
        .values()
        .find(|w| w.is_focused().unwrap_or(false))
        .or_else(|| windows.values().next())
        .cloned()
}

/// A window with no review open or opening, to open one in; a new one
/// when every window has its review.
pub fn free(handle: &AppHandle) -> tauri::Result<WebviewWindow> {
    let windows = handle.state::<Windows>();
    let busy: Vec<String> = windows
        .0
        .lock()
        .map(|m| {
            m.iter()
                .filter(|(_, s)| s.host.is_some() || s.opening.is_some())
                .map(|(l, _)| l.clone())
                .collect()
        })
        .unwrap_or_default();
    if let Some(w) = handle
        .webview_windows()
        .into_values()
        .find(|w| !busy.contains(&w.label().to_string()))
    {
        return Ok(w);
    }
    open(handle)
}
