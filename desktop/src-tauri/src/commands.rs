//! What a window asks of the app: open a review, send it keys, actions,
//! clicks and pasted text, and what to show before a review is open (the
//! recent repositories, the pull requests waiting).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, EventTarget, Manager, State, WebviewWindow};

use teer::config::Config;
use teer::review::store;
use teer::session::view::Hint;
use teer::start::{Opened, What};

use crate::host::{self, Click, Host, Input};
use crate::recent::{self, Recent};

/// Each window's review, by window label.
#[derive(Default)]
pub struct Windows(pub Mutex<HashMap<String, WindowState>>);

/// What a window is doing.
#[derive(Default)]
pub struct WindowState {
    /// The review on show.
    pub host: Option<Host>,
    /// The repository being opened, while it is.
    pub opening: Option<String>,
    /// What the window reviews, or is opening: the repository and the pull
    /// request.
    pub what: Option<(PathBuf, Option<String>)>,
}

impl Windows {
    /// The window that reviews, or is opening, `repo` (and `pr`).
    pub fn showing(&self, repo: &std::path::Path, pr: Option<&str>) -> Option<String> {
        let map = self.0.lock().ok()?;
        map.iter()
            .find(|(_, s)| {
                s.what
                    .as_ref()
                    .is_some_and(|(r, p)| same_dir(r, repo) && p.as_deref() == pr)
            })
            .map(|(l, _)| l.clone())
    }

    pub fn host(&self, label: &str) -> Option<Host> {
        self.0.lock().ok()?.get(label)?.host.clone()
    }

    fn send(&self, label: &str, input: Input) -> Result<(), String> {
        let host = self
            .host(label)
            .ok_or_else(|| "No review is open in this window.".to_string())?;
        host.tx
            .send(input)
            .map_err(|_| "The review has ended.".to_string())
    }
}

/// What a window that loads finds.
#[derive(Debug, Clone, Serialize)]
pub struct Loaded {
    /// The repository being opened, while it is.
    pub opening: Option<String>,
    /// The review's view, once one is open.
    pub view: Option<serde_json::Value>,
}

#[tauri::command]
pub fn loaded(window: WebviewWindow, windows: State<'_, Windows>) -> Loaded {
    let map = windows.0.lock().expect("windows");
    let state = map.get(window.label());
    Loaded {
        opening: state.and_then(|s| s.opening.clone()),
        view: state
            .and_then(|s| s.host.as_ref())
            .and_then(|h| h.view.lock().ok()?.clone()),
    }
}

/// Open a review of the repository at `repo` in this window: its changes
/// (or its branch since `base`), or with `pr` that pull request (a URL or
/// a number). `None` once it is open; else what there is to say instead
/// (nothing to review).
#[tauri::command]
pub async fn open_review(
    window: WebviewWindow,
    windows: State<'_, Windows>,
    repo: String,
    base: Option<String>,
    pr: Option<String>,
    no_agent: bool,
) -> Result<Option<String>, String> {
    let label = window.label().to_string();
    open_in(
        window.app_handle(),
        &windows,
        &label,
        repo,
        base,
        pr,
        no_agent,
    )
    .await
}

/// [`open_review`] for window `label`, from a command or the app itself
/// (a folder dropped on the Dock icon, `teer app`, a `teer://` link).
pub async fn open_in(
    handle: &AppHandle,
    windows: &Windows,
    label: &str,
    repo: String,
    base: Option<String>,
    pr: Option<String>,
    no_agent: bool,
) -> Result<Option<String>, String> {
    let cwd = PathBuf::from(&repo);
    {
        let mut map = windows.0.lock().map_err(|e| e.to_string())?;
        let state = map.entry(label.to_string()).or_default();
        if state.host.is_some() {
            return Err("This window has a review open already.".into());
        }
        if state.opening.is_some() {
            return Err("This window is opening a review already.".into());
        }
        state.opening = Some(repo.clone());
        state.what = Some((cwd.clone(), pr.clone().filter(|p| !p.trim().is_empty())));
    }
    let _ = handle.emit_to(EventTarget::webview_window(label), "opening", &repo);
    let what = match pr.filter(|p| !p.trim().is_empty()) {
        Some(p) => What::Pr(p.trim().to_string()),
        None => What::Repo {
            base: base.filter(|b| !b.trim().is_empty()),
        },
    };
    let opened = tauri::async_runtime::spawn_blocking(move || {
        let cfg = Config::load(&store::config_path())?;
        teer::start::open(&cwd, &what, cfg, no_agent)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r.map_err(|e| format!("{e:#}")));
    let done = |windows: &Windows| {
        if let Ok(mut map) = windows.0.lock()
            && let Some(s) = map.get_mut(label)
        {
            s.opening = None;
            s.what = None;
        }
    };
    let (launch, worktree) = match opened {
        Ok(Opened::Review { launch, worktree }) => (launch, worktree),
        Ok(Opened::Nothing(why)) => {
            done(windows);
            return Ok(Some(why));
        }
        Err(e) => {
            done(windows);
            return Err(e);
        }
    };
    let root = launch.review.repo_root.clone();
    let title = format!(
        "{} — {}",
        root.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        launch.review.subject()
    );
    let begun = match teer::start::begin(*launch, "Teer.app") {
        Ok(b) => b,
        Err(e) => {
            done(windows);
            return Err(format!("{e:#}"));
        }
    };
    if launch_is_repo(&worktree) {
        recent::add(&root);
    }
    let mut worktree = worktree;
    let host = host::spawn(
        handle.clone(),
        label.to_string(),
        begun,
        Box::new(move || {
            if let Some(w) = worktree.as_mut() {
                w.release();
            }
        }),
    );
    if let Some(w) = handle.get_webview_window(label) {
        let _ = w.set_title(&title);
    }
    let mut map = windows.0.lock().map_err(|e| e.to_string())?;
    let state = map.entry(label.to_string()).or_default();
    state.opening = None;
    state.host = Some(host);
    Ok(None)
}

/// Whether `a` and `b` are the same folder (or the same repository's root
/// and a folder in it).
fn same_dir(a: &std::path::Path, b: &std::path::Path) -> bool {
    let real = |p: &std::path::Path| {
        let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        teer::git::repo_root(&p).unwrap_or(p)
    };
    a == b || real(a) == real(b)
}

/// A pull request's checkout is no repository to list again.
fn launch_is_repo(worktree: &Option<teer::pr::PrWorktree>) -> bool {
    worktree.is_none()
}

/// Keys in Neovim notation, as the window read them.
#[tauri::command]
pub fn keys(
    window: WebviewWindow,
    windows: State<'_, Windows>,
    keys: String,
) -> Result<(), String> {
    windows.send(window.label(), Input::Keys(keys))
}

/// An action by its id.
#[tauri::command]
pub fn action(
    window: WebviewWindow,
    windows: State<'_, Windows>,
    id: String,
    count: Option<u32>,
) -> Result<(), String> {
    windows.send(window.label(), Input::Action { id, count })
}

#[tauri::command]
pub fn click(
    window: WebviewWindow,
    windows: State<'_, Windows>,
    click: Click,
) -> Result<(), String> {
    windows.send(window.label(), Input::Click(click))
}

#[tauri::command]
pub fn paste(
    window: WebviewWindow,
    windows: State<'_, Windows>,
    text: String,
) -> Result<(), String> {
    windows.send(window.label(), Input::Paste(text))
}

/// The text written in the window's editor for the commit message or the
/// review summary; `None` when it was cancelled.
#[tauri::command]
pub fn written(
    window: WebviewWindow,
    windows: State<'_, Windows>,
    text: Option<String>,
) -> Result<(), String> {
    windows.send(
        window.label(),
        Input::Returned(crate::host::Returned::Written(text)),
    )
}

/// Every action with its keys, for the command palette.
#[tauri::command]
pub async fn actions(
    window: WebviewWindow,
    windows: State<'_, Windows>,
) -> Result<Vec<Hint>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    windows.send(window.label(), Input::Actions(tx))?;
    rx.await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn recent_repos() -> Vec<Recent> {
    recent::list()
}

/// The pull requests of the repository at `repo` waiting for a review,
/// assigned, or the user's own (`teer pr`'s list).
#[tauri::command]
pub async fn list_prs(repo: String) -> Result<teer::pr_list::Listing, String> {
    let cwd = PathBuf::from(repo);
    let root = teer::git::repo_root(&cwd).map_err(|e| format!("{e:#}"))?;
    let target = {
        let root = root.clone();
        tauri::async_runtime::spawn_blocking(move || teer::pr_list::target_repo(&root))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?
    };
    teer::pr_list::fetch(&teer::gh::program(), &root, target)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Open a new window, on the start screen.
#[tauri::command]
pub fn new_window(handle: AppHandle) -> Result<(), String> {
    crate::window::open(&handle)
        .map(|_| ())
        .map_err(|e| e.to_string())
}
