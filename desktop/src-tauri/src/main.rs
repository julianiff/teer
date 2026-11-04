//! Teer.app: teer's review in a window. The review itself is teer's own
//! (`teer::session`, the same as in the terminal); the window draws its
//! view model and sends back keys, actions and clicks. Config and saved
//! reviews are the CLI's: a review started in one continues in the other.

mod commands;
mod front;
mod host;
mod menu;
mod pty;
mod recent;
mod settings;
mod window;

/// A review the app opened by itself (not the window's start screen) came
/// to nothing or failed: the window says so.
fn report(handle: &tauri::AppHandle, label: &str, opened: Result<Option<String>, String>) {
    use tauri::Emitter;
    let said = match opened {
        Ok(None) => return,
        Ok(Some(said)) => said,
        Err(e) => e,
    };
    let _ = handle.emit_to(tauri::EventTarget::webview_window(label), "said", said);
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
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
            let first = window::open(app.handle())?;
            // `teer-desktop [--no-agent] <repo> [<pr>]`: open that review in
            // the first window.
            let args: Vec<String> = std::env::args()
                .skip(1)
                .filter(|a| !a.starts_with("-psn"))
                .collect();
            let no_agent = args.iter().any(|a| a == "--no-agent");
            let mut args = args.into_iter().filter(|a| !a.starts_with("--"));
            if let Some(repo) = args.next() {
                let pr = args.next();
                let handle = app.handle().clone();
                let label = first.label().to_string();
                tauri::async_runtime::spawn(async move {
                    use tauri::Manager;
                    let windows = handle.state::<commands::Windows>();
                    let opened =
                        commands::open_in(&handle, &windows, &label, repo, None, pr, no_agent)
                            .await;
                    report(&handle, &label, opened);
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Teer.app failed to start");
}
