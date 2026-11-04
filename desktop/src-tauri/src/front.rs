//! The effects that in the terminal take over the screen, the window's
//! way: the editor (`gf`) opens directly when it has windows of its own
//! and in the terminal pane when it is a terminal editor; the agent CLI the
//! notes go to runs in the terminal pane, and quitting it re-runs the
//! review; the commit message and the review summary are written in the
//! window; the commit runs with its output streamed into a panel; the
//! settings are the window's own screen. What they come back with reaches
//! the review as [`Returned`].

use std::path::{Path, PathBuf};
use std::process::Stdio;

use portable_pty::CommandBuilder;
use serde::Serialize;
use tauri::{Emitter, EventTarget};

use teer::config::Provider;
use teer::keymap::Action;
use teer::session::Effect;
use teer::session::runtime::agent_program;

use crate::host::{Input, Returned, Session};

/// Editors that draw in a terminal: they open in the terminal pane. Any
/// other editor is taken to open windows of its own.
const TERMINAL_EDITORS: &[&str] = &[
    "nvim", "vim", "vi", "view", "nano", "pico", "emacs", "hx", "helix", "micro", "kak", "joe",
    "ne", "mg", "ed", "amp", "lvim",
];

/// Whether the editor named `name` draws in a terminal.
pub fn in_terminal(name: &str) -> bool {
    TERMINAL_EDITORS.contains(&name)
}

/// What the window is asked to edit.
#[derive(Debug, Clone, Serialize)]
struct EditText {
    text: String,
    /// What it is: the commit message, or the review's summary comment.
    title: &'static str,
    for_commit: bool,
}

/// Carry out `e` if it is the window's; else hand it back for the runtime.
pub fn take(s: &mut Session, e: Effect) -> Option<Effect> {
    match e {
        Effect::Edit { file, line } => edit(s, &file, line),
        Effect::OpenAgent {
            cli,
            cwd,
            prompt,
            resume,
        } => return agent(s, cli, &cwd, &prompt, resume.as_deref()),
        Effect::EditComment { text } => {
            let for_commit = s.app.edits_commit_message();
            let title = if for_commit {
                "Commit message"
            } else {
                "Review summary"
            };
            emit(
                s,
                "edit",
                EditText {
                    text,
                    title,
                    for_commit,
                },
            );
        }
        Effect::Commit {
            repo,
            paths,
            message,
        } => commit(s, repo, paths, message),
        Effect::Settings => emit(s, "settings", ()),
        Effect::OpenApp => s.app.status = "This is Teer.app.".into(),
        other => return Some(other),
    }
    None
}

fn emit<T: Serialize + Clone>(s: &Session, event: &str, payload: T) {
    let _ = s
        .handle
        .emit_to(EventTarget::webview_window(&s.label), event, payload);
}

/// `gf`: `file` at `line` in the configured editor.
fn edit(s: &mut Session, file: &Path, line: u32) {
    let repo = s.app.review.repo_root.clone();
    let path = repo.join(file);
    if !path.exists() {
        s.app.status = format!("{} no longer exists, nothing to open.", file.display());
        return;
    }
    let editor = s.rt.tools.editor.clone();
    let name = editor.name();
    let rerun = s.app.keys.label(Action::ReviewRerun);
    let at = format!("{}:{line}", file.display());
    // The whole path: a launcher (`open -a`, an IDE) may not run in the repo.
    let c = editor.command(&path, line);
    if in_terminal(&name) {
        let mut cmd = CommandBuilder::new(c.get_program());
        cmd.args(c.get_args());
        cmd.cwd(&repo);
        let tx = s.tx.clone();
        let done = format!("Closed {name}. {rerun} re-runs the review if you changed the code.");
        let spawned = crate::pty::spawn(
            &s.handle,
            &s.label,
            &name,
            cmd,
            Box::new(move |_| {
                let _ = tx.send(Input::Returned(Returned::Status(done)));
            }),
        );
        s.app.status = match spawned {
            Ok(_) => format!("{at} is open in {name}, in the terminal pane."),
            Err(e) => format!("Couldn't start {name}: {e:#} (`editor` in the config)"),
        };
        return;
    }
    let mut c = c;
    let spawned = c
        .current_dir(&repo)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    s.app.status = match spawned {
        Ok(mut child) => {
            // A launcher returns at once; an editor that waits is let be.
            std::thread::spawn(move || child.wait());
            format!("Opened {at} in {name}. {rerun} re-runs the review if you changed the code.")
        }
        Err(e) => format!("Couldn't start {name}: {e} (`editor` in the config)"),
    };
}

/// The notes go to the agent CLI, in the terminal pane: when it exits the
/// review runs again ([`teer::session::Runtime::agent_exited`]).
fn agent(
    s: &mut Session,
    cli: Provider,
    cwd: &Path,
    prompt: &str,
    resume: Option<&str>,
) -> Option<Effect> {
    let mut cmd = CommandBuilder::new(agent_program(&s.rt.tools, cli));
    if let Some(id) = resume {
        cmd.args(["--resume", id, "--fork-session"]);
    }
    cmd.arg(prompt);
    cmd.cwd(cwd);
    let tx = s.tx.clone();
    let spawned = crate::pty::spawn(
        &s.handle,
        &s.label,
        cli.title(),
        cmd,
        Box::new(move |ran| {
            let _ = tx.send(Input::Returned(Returned::Agent { cli, ran }));
        }),
    );
    match spawned {
        Ok(_) => {
            s.app.status = format!(
                "{} has the notes, in the terminal pane. Quit it, and the review runs again on what it changed.",
                cli.title()
            );
            None
        }
        Err(e) => {
            let ran = Err(std::io::Error::other(format!("{e:#}")));
            s.rt.agent_exited(&mut s.app, cli, ran)
                .then_some(Effect::Quit)
        }
    }
}

/// Commit, git's output (hooks, signing) streamed into the window's
/// commit panel.
fn commit(s: &mut Session, repo: PathBuf, paths: Vec<PathBuf>, message: String) {
    if s.committing {
        s.app.status = "Committing… (the commit on its way first)".into();
        return;
    }
    s.committing = true;
    let (lines, rx) = std::sync::mpsc::channel::<String>();
    emit(s, "commit-start", ());
    let (handle, label) = (s.handle.clone(), s.label.clone());
    std::thread::spawn(move || {
        for line in rx {
            let _ = handle.emit_to(EventTarget::webview_window(&label), "commit-line", line);
        }
    });
    let (handle, label, tx) = (s.handle.clone(), s.label.clone(), s.tx.clone());
    std::thread::spawn(move || {
        let out =
            teer::commit::run_commit(&repo, &paths, &message, teer::commit::Io::Stream(lines));
        let ok = matches!(out, teer::commit::Outcome::Committed { .. });
        let _ = handle.emit_to(EventTarget::webview_window(&label), "commit-end", ok);
        let _ = tx.send(Input::Returned(Returned::Committed(out)));
    });
    s.app.status = "Committing…".into();
}

/// What a handoff came back with, into the review.
pub fn returned(s: &mut Session, r: Returned) -> Vec<Effect> {
    match r {
        Returned::Agent { cli, ran } => {
            if ran.is_ok() {
                crate::notify::send(
                    &s.handle,
                    &s.label,
                    &format!("{} is done", cli.title()),
                    "The review runs again on what it changed.",
                );
            }
            if s.rt.agent_exited(&mut s.app, cli, ran) {
                return vec![Effect::Quit];
            }
            Vec::new()
        }
        Returned::Written(text) => {
            let outcome = match text {
                Some(t) => teer::approve::edited(&t, s.app.edits_commit_message()),
                None => teer::approve::EditOutcome::Cancelled,
            };
            s.app.comment_edited(outcome);
            Vec::new()
        }
        Returned::Committed(outcome) => {
            s.committing = false;
            s.app.commit_finished(outcome)
        }
        Returned::Settings(cfg) => {
            s.app.status = s.rt.settings_saved(&mut s.app, *cfg);
            Vec::new()
        }
        Returned::Status(line) => {
            s.app.status = line;
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_editors_open_in_the_pane_and_the_rest_directly() {
        assert!(in_terminal("nvim"));
        assert!(in_terminal("hx"));
        assert!(!in_terminal("code"));
        assert!(!in_terminal("zed"));
        assert!(!in_terminal("open"));
    }
}
