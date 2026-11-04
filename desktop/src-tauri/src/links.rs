//! Reviews the app is asked to open by itself rather than from a window's
//! start screen: `teer app` and `teer://` links, folders dropped on the
//! Dock icon, the command line. A window that shows the review already
//! comes to the front; else a free window (or a new one) opens it. And
//! the way back: "Open in Terminal" hands the window's review to `teer`.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};
use teer::app_link::Link;

use crate::commands::{Windows, open_in};
use crate::host::Input;

/// Open `link`'s review: in the window showing it, else a free one.
pub fn open(handle: &AppHandle, link: Link, no_agent: bool) {
    let windows = handle.state::<Windows>();
    if let Some(label) = windows.showing(&link.repo, link.pr.as_deref()) {
        if let Some(w) = handle.get_webview_window(&label) {
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        return;
    }
    let Ok(window) = crate::window::free(handle) else {
        return;
    };
    let _ = window.set_focus();
    let label = window.label().to_string();
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let windows = handle.state::<Windows>();
        let repo = link.repo.display().to_string();
        // The window hears how it ended: `open_in` tells it.
        let _ = open_in(
            &handle, &windows, &label, repo, link.base, link.pr, no_agent,
        )
        .await;
    });
}

/// What a URL the system handed the app asks for: a `teer://` link, or a
/// folder (dropped on the Dock icon, opened with Teer).
pub fn of_url(url: &tauri::Url) -> Option<Link> {
    if url.scheme() == teer::app_link::SCHEME {
        return teer::app_link::parse(url.as_str());
    }
    let path = url.to_file_path().ok()?;
    path.is_dir().then_some(Link {
        repo: path,
        pr: None,
        base: None,
    })
}

/// What the command line asks for: `[--no-agent] <repo> [<pr>]`, or a
/// `teer://` link (how Linux hands links to apps). And whether without
/// an agent.
pub fn of_args(args: &[String]) -> (Option<Link>, bool) {
    let no_agent = args.iter().any(|a| a == "--no-agent");
    let mut rest = args
        .iter()
        .filter(|a| !a.starts_with("--") && !a.starts_with("-psn"));
    let link = match rest.next() {
        Some(a) if a.starts_with("teer://") => teer::app_link::parse(a),
        Some(repo) => Some(Link {
            repo: absolute(Path::new(repo)),
            pr: rest.next().cloned(),
            base: None,
        }),
        None => None,
    };
    (link, no_agent)
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// "Open in Terminal": the window's review goes to `teer` in the user's
/// terminal. The window lets go of it first (saved, unlocked), then the
/// terminal opens a `.command` file that runs teer in the repository.
pub fn to_terminal(handle: &AppHandle, label: &str) {
    let windows = handle.state::<Windows>();
    let (what, host) = match windows.0.lock() {
        Ok(map) => match map.get(label) {
            Some(s) => (s.what.clone(), s.host.clone()),
            None => (None, None),
        },
        Err(_) => return,
    };
    let Some((repo, pr)) = what else {
        return;
    };
    let handle = handle.clone();
    let label = label.to_string();
    tauri::async_runtime::spawn(async move {
        if let Some(host) = host {
            let (done, closed) = tokio::sync::oneshot::channel();
            if host.tx.send(Input::Close(Some(done))).is_ok() {
                let _ = closed.await;
            }
        }
        if let Err(e) = run_in_terminal(&repo, pr.as_deref()) {
            let repo = repo.display().to_string();
            let text = format!("Couldn't open the terminal: {e}");
            crate::commands::say(&handle, &label, &repo, &text, true);
        }
    });
}

/// Run `teer [pr]` in `repo` in the user's terminal app (whichever opens
/// `.command` files: Terminal, or the one the user chose).
fn run_in_terminal(repo: &Path, pr: Option<&str>) -> std::io::Result<()> {
    let dir = teer::review::store::data_dir().join("tmp");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!(
        "teer-{}.command",
        teer::util::hash_hex(&[repo.to_string_lossy().as_bytes()])
    ));
    std::fs::write(&file, script(repo, pr))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700))?;
    }
    let status = std::process::Command::new("open").arg(&file).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("`open` failed"))
    }
}

/// The script the terminal runs: into the repository, then teer.
fn script(repo: &Path, pr: Option<&str>) -> String {
    let mut run = String::from("exec teer");
    if let Some(pr) = pr {
        run.push(' ');
        run.push_str(&quote(pr));
    }
    format!(
        "#!/bin/sh\n# Teer.app handed this review to the terminal.\ncd {} || exit 1\n{run}\n",
        quote(&repo.to_string_lossy())
    )
}

/// `s` in single quotes for sh.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_names_a_review_or_a_link() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (link, no_agent) = of_args(&args(&["--no-agent", "/work/shop", "482"]));
        assert!(no_agent);
        let link = link.unwrap();
        assert_eq!(link.repo, Path::new("/work/shop"));
        assert_eq!(link.pr.as_deref(), Some("482"));
        let (link, _) = of_args(&args(&["teer://open?repo=/work/shop&base=main"]));
        assert_eq!(link.unwrap().base.as_deref(), Some("main"));
        assert!(of_args(&args(&["-psn_0_123"])).0.is_none());
    }

    #[test]
    fn the_terminal_script_quotes_what_it_runs() {
        let s = script(Path::new("/Users/ada/it's here"), Some("https://x/pull/7"));
        assert!(s.contains("cd '/Users/ada/it'\\''s here' || exit 1"), "{s}");
        assert!(s.contains("exec teer 'https://x/pull/7'"), "{s}");
    }

    #[test]
    fn folders_and_links_from_the_system() {
        let dir = std::env::temp_dir();
        let url = tauri::Url::from_directory_path(&dir).unwrap();
        assert!(of_url(&url).is_some());
        let link = tauri::Url::parse("teer://open?repo=/work/shop").unwrap();
        assert_eq!(of_url(&link).unwrap().repo, Path::new("/work/shop"));
        assert!(of_url(&tauri::Url::parse("https://example.com").unwrap()).is_none());
    }
}
