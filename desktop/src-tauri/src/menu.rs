//! The menu bar. Its shortcuts are the Mac's own (⌘O, ⌘W, ⌘C, ⌘,, ⌘Q), so
//! they never collide with teer's keys, which have no ⌘. Review items run
//! teer's actions in the window in focus; the rest the window shows itself
//! (the folder picker, the palette).

use tauri::menu::{
    CheckMenuItemBuilder, Menu, MenuBuilder, MenuEvent, MenuItemBuilder, PredefinedMenuItem,
    SubmenuBuilder,
};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::commands::Windows;
use crate::host::Input;

/// Items that run one of teer's actions: `action:<id>`.
const ACTIONS: &[(&str, &str, Option<&str>)] = &[
    ("Finish…", "finish.open", Some("CmdOrCtrl+Return")),
    ("Notes", "tool.notes", None),
    ("Flagged Units", "tool.flagged", None),
    ("Commits and Description", "tool.history", None),
    ("Run the Review Again", "review.rerun", Some("CmdOrCtrl+R")),
];

const VIEW_ACTIONS: &[(&str, &str)] = &[
    ("Show Units Accepted for You", "toggle.auto_accepted"),
    ("List by File", "toggle.by_file"),
    ("Every Key", "help.open"),
];

pub fn build<R: Runtime>(handle: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let app = SubmenuBuilder::new(handle, "Teer")
        .item(&PredefinedMenuItem::about(
            handle,
            Some("About Teer"),
            None,
        )?)
        .separator()
        .item(
            &MenuItemBuilder::with_id("settings", "Settings…")
                .accelerator("CmdOrCtrl+,")
                .build(handle)?,
        )
        .separator()
        .item(&PredefinedMenuItem::services(handle, None)?)
        .separator()
        .item(&PredefinedMenuItem::hide(handle, None)?)
        .item(&PredefinedMenuItem::hide_others(handle, None)?)
        .item(&PredefinedMenuItem::show_all(handle, None)?)
        .separator()
        .item(&PredefinedMenuItem::quit(handle, None)?)
        .build()?;
    let file = SubmenuBuilder::new(handle, "File")
        .item(
            &MenuItemBuilder::with_id("new_window", "New Window")
                .accelerator("CmdOrCtrl+N")
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("open_folder", "Open Folder…")
                .accelerator("CmdOrCtrl+O")
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("open_pr", "Open Pull Request…")
                .accelerator("CmdOrCtrl+Shift+O")
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("open_terminal", "Open in Terminal")
                .accelerator("CmdOrCtrl+Shift+T")
                .build(handle)?,
        )
        .separator()
        .item(&PredefinedMenuItem::close_window(handle, None)?)
        .build()?;
    // Copy and paste work in the window only with these.
    let edit = SubmenuBuilder::new(handle, "Edit")
        .item(&PredefinedMenuItem::cut(handle, None)?)
        .item(&PredefinedMenuItem::copy(handle, None)?)
        .item(&PredefinedMenuItem::paste(handle, None)?)
        .item(&PredefinedMenuItem::select_all(handle, None)?)
        .build()?;
    let mut review = SubmenuBuilder::new(handle, "Review").item(
        &MenuItemBuilder::with_id("palette", "Command Palette…")
            .accelerator("CmdOrCtrl+K")
            .build(handle)?,
    );
    review = review.separator();
    for (label, id, accel) in ACTIONS {
        let mut item = MenuItemBuilder::with_id(format!("action:{id}"), *label);
        if let Some(a) = accel {
            item = item.accelerator(*a);
        }
        review = review.item(&item.build(handle)?);
    }
    let mut view = SubmenuBuilder::new(handle, "View");
    for (label, id) in VIEW_ACTIONS {
        view = view.item(&MenuItemBuilder::with_id(format!("action:{id}"), *label).build(handle)?);
    }
    let view = view
        .separator()
        .item(
            &CheckMenuItemBuilder::with_id(crate::prefs::BUTTONS_ITEM, "Show Buttons")
                .checked(crate::prefs::load().buttons)
                .accelerator("CmdOrCtrl+Shift+B")
                .build(handle)?,
        )
        .separator()
        .item(&PredefinedMenuItem::fullscreen(handle, None)?)
        .build()?;
    let window = SubmenuBuilder::new(handle, "Window")
        .item(&PredefinedMenuItem::minimize(handle, None)?)
        .item(&PredefinedMenuItem::maximize(handle, None)?)
        .build()?;
    MenuBuilder::new(handle)
        .items(&[&app, &file, &edit, &review.build()?, &view, &window])
        .build()
}

/// A menu item was chosen.
pub fn on_event(handle: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    if id == "new_window" {
        let _ = crate::window::open(handle);
        return;
    }
    // Every window, whichever has the focus; the tick is set from what is
    // saved, not from the click's own toggling.
    if id == crate::prefs::BUTTONS_ITEM {
        crate::prefs::set_buttons(handle, !crate::prefs::load().buttons);
        return;
    }
    let Some(window) = crate::window::focused(handle) else {
        let _ = crate::window::open(handle);
        return;
    };
    if id == "open_terminal" {
        crate::links::to_terminal(handle, window.label());
        return;
    }
    if let Some(action) = id.strip_prefix("action:") {
        let windows = handle.state::<Windows>();
        if let Some(host) = windows.host(window.label()) {
            let _ = host.tx.send(Input::Action {
                id: action.to_string(),
                count: None,
            });
        }
        return;
    }
    // The window shows these itself.
    let _ = window.emit_to(
        tauri::EventTarget::webview_window(window.label()),
        "menu",
        id,
    );
}
