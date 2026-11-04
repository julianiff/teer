//! Teer.app: teer's review in a window. The review itself is teer's own
//! (`teer::session`, the same as in the terminal); the window draws its
//! view model and sends back keys, actions and clicks. Config and saved
//! reviews are the CLI's: a review started in one continues in the other.

mod commands;
mod env;
mod front;
mod host;
mod links;
mod menu;
mod notify;
mod pty;
mod recent;
mod settings;
mod window;

fn main() {
    // Before any thread: PATH and the rest from the login shell.
    env::import();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .manage(commands::Windows::default())
        .manage(pty::Ptys::default())
        .menu(menu::build)
        .on_menu_event(menu::on_event)
        .invoke_handler(tauri::generate_handler![
            commands::loaded,
            commands::open_review,
            commands::keys,
            commands::action,
            commands::click,
            commands::paste,
            commands::actions,
            commands::recent_repos,
            commands::list_prs,
            commands::new_window,
            commands::open_elsewhere,
            commands::written,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            settings::setup_state,
            settings::setup_tools,
            settings::setup_models,
            settings::setup_test,
            settings::setup_save,
        ])
        .setup(|app| {
            window::open(app.handle())?;
            // `teer-desktop [--no-agent] <repo> [<pr>]`, or a `teer://` link.
            let args: Vec<String> = std::env::args().skip(1).collect();
            if let (Some(link), no_agent) = links::of_args(&args) {
                links::open(app.handle(), link, no_agent);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Teer.app failed to start");
    app.run(|handle, event| {
        // `teer app`, a `teer://` link, a folder dropped on the Dock icon.
        #[cfg(target_os = "macos")]
        match &event {
            tauri::RunEvent::Opened { urls } => {
                for link in urls.iter().filter_map(links::of_url) {
                    links::open(handle, link, false);
                }
            }
            // The Dock icon clicked with every window closed: a new one.
            tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } => {
                let _ = window::open(handle);
            }
            _ => {}
        }
        let _ = (handle, event);
    });
}
