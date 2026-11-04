//! The terminal front end: owns the terminal, feeds its events to
//! [`app::App`], draws it, and carries out the effects that need the
//! terminal (the editor, the agent CLI, git's commit, the setup screen);
//! [`Runtime`] carries out the rest.

pub mod answers;
pub mod app;
pub mod ask;
pub mod commits;
pub mod diff;
pub mod finish;
pub mod flagged_list;
pub mod notes;
pub mod picture;
pub mod pr_pick;
pub mod preview;
pub mod queue;
pub mod select;
pub mod setup;
pub mod slim;
pub mod theme;
pub mod ui;
pub mod unit;

use std::io::stdout;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    EventStream,
};
use crossterm::execute;
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;

use crate::review::store;
use crate::session::runtime::agent_program;
use crate::session::{Effect, Runtime};
use app::App;

static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the terminal is currently in TUI mode (raw, alternate screen).
pub fn is_active() -> bool {
    ACTIVE.load(std::sync::atomic::Ordering::SeqCst)
}

/// Leave TUI mode: mouse capture, bracketed paste, alternate screen, raw
/// mode. The one restore path, used on normal exit and from the signal handler.
pub fn restore_terminal() {
    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
}

pub async fn run(l: crate::start::Launch) -> Result<()> {
    let crate::start::Begun {
        app: core,
        runtime: mut rt,
        events: mut rx,
        initial,
    } = crate::start::begin(l)?;
    let mut app = App::wrap(core);

    let mut terminal = ratatui::init();
    ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
    // Ask what the terminal can draw now: in raw mode, before events are read.
    if app.cfg.images {
        app.pictures = picture::Pictures::detect();
    }
    execute!(stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let result = async {
        let mut quit = false;
        for e in initial {
            quit |= rt.exec(&mut app, e);
        }
        let mut events = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        while !quit {
            terminal.draw(|f| ui::draw(f, &app))?;
            let effects = tokio::select! {
                ev = events.next() => match ev {
                    Some(Ok(ev)) => app.on_event(ev),
                    Some(Err(e)) => return Err(e.into()),
                    None => vec![Effect::Quit],
                },
                Some(ev) = rx.recv() => app.on_app_event(ev),
                // SIGTERM/SIGHUP: leave through the normal path, which saves the review.
                _ = crate::cli::SHUTDOWN.notified() => vec![Effect::Quit],
                _ = tick.tick() => {
                    app.tick = app.tick.wrapping_add(1);
                    // A key prefix waiting past `timeoutlen` is dropped.
                    app.keys.tick(std::time::Instant::now());
                    // Waiting for copied notes' changes: look at the tree.
                    rt.watch(&mut app, std::time::Instant::now());
                    Vec::new()
                }
            };
            for e in effects {
                if let Effect::Edit { file, line } = e {
                    // The editor needs the terminal and stdin to itself.
                    drop(events);
                    let rerun = app.keys.label(crate::keymap::Action::ReviewRerun);
                    app.status = open_editor(
                        &mut terminal,
                        &rt.tools.editor,
                        &app.review.repo_root,
                        &file,
                        line,
                        &rerun,
                    );
                    events = EventStream::new();
                } else if let Effect::OpenAgent {
                    cli,
                    cwd,
                    prompt,
                    resume,
                } = e
                {
                    // The agent CLI needs the terminal and stdin to itself.
                    drop(events);
                    let ran = with_terminal_released(&mut terminal, || {
                        run_agent(&rt.tools, cli, &cwd, &prompt, resume.as_deref())
                    });
                    quit |= rt.agent_exited(&mut app, cli, ran);
                    events = EventStream::new();
                } else if let Effect::EditComment { text } = e {
                    drop(events);
                    let for_commit = app.edits_commit_message();
                    let outcome = with_terminal_released(&mut terminal, || {
                        if for_commit {
                            crate::approve::edit_message_in_editor(&rt.tools.editor, &text)
                        } else {
                            crate::approve::edit_in_editor(&rt.tools.editor, &text)
                        }
                    });
                    app.comment_edited(outcome);
                    events = EventStream::new();
                } else if let Effect::Commit {
                    repo,
                    paths,
                    message,
                } = e
                {
                    // git (hooks, signing, pinentry) may print or ask: it gets the terminal.
                    drop(events);
                    let outcome = with_terminal_released(&mut terminal, || {
                        run_commit_on_terminal(&repo, &paths, &message)
                    });
                    for follow in app.commit_finished(outcome) {
                        quit |= rt.exec(&mut app, follow);
                    }
                    events = EventStream::new();
                } else if let Effect::Settings = e {
                    // The setup screen draws on this terminal and reads its own keys.
                    drop(events);
                    quit |= open_settings(&mut terminal, &mut app, &mut rt).await;
                    events = EventStream::new();
                } else {
                    quit |= rt.exec(&mut app, e);
                }
            }
        }
        Ok(())
    }
    .await;
    restore_terminal();
    let _ = store::save(&app.review);
    // What `:w` did (a commit, a post), once the terminal is the shell's again.
    if let Some(line) = &app.finish.exit_line {
        println!("{line}");
    }
    result
}

/// Run the commit with the terminal handed to git. When it fails, wait for
/// Enter so the output stays readable.
fn run_commit_on_terminal(
    repo: &std::path::Path,
    paths: &[std::path::PathBuf],
    message: &str,
) -> crate::commit::Outcome {
    use crate::commit::{Io, Outcome, run_commit};
    let outcome = run_commit(repo, paths, message, Io::Inherit);
    if !matches!(outcome, Outcome::Committed { .. }) {
        if let Outcome::Refused(why) = &outcome {
            eprintln!("{why}");
        }
        println!("\ncommit failed, see output above — press Enter to return to teer");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }
    outcome
}

/// Give the terminal to a child process (leave the alternate screen and raw
/// mode), run `f`, then take it back.
fn with_terminal_released<T>(terminal: &mut ratatui::DefaultTerminal, f: impl FnOnce() -> T) -> T {
    let _ = execute!(
        stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
    let out = f();
    take_back(terminal);
    out
}

/// Take the terminal back from a program that had it: raw mode, the
/// alternate screen, the mouse and paste, and a blank screen the next frame
/// is drawn on whole. Not `Terminal::clear`: it first asks the terminal
/// where the cursor is, and when the answer doesn't come it gives up before
/// forcing the redraw, so the next frame drew only what changed on a blank
/// screen.
fn take_back(terminal: &mut ratatui::DefaultTerminal) {
    let _ = enable_raw_mode();
    let _ = execute!(
        stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        Clear(ClearType::All)
    );
    force_redraw(terminal);
}

/// The next frame is drawn whole: the last one is forgotten, as if the
/// screen were blank.
fn force_redraw<B: ratatui::backend::Backend>(terminal: &mut ratatui::Terminal<B>) {
    // After a draw the current buffer is blank and the other holds the frame:
    // swapping blanks that one too.
    terminal.swap_buffers();
}

/// Run `editor` on `file` at `line`, then take the terminal back (a GUI
/// editor's command returns at once). Returns the status line to show
/// afterwards; `rerun` is the key that re-runs the review.
fn open_editor(
    terminal: &mut ratatui::DefaultTerminal,
    editor: &crate::editor::Editor,
    repo: &std::path::Path,
    file: &std::path::Path,
    line: u32,
    rerun: &str,
) -> String {
    if !repo.join(file).exists() {
        return format!("{} no longer exists, nothing to open.", file.display());
    }
    // The whole path: a launcher (`open -a`, an IDE) may not run in the repo.
    let result = with_terminal_released(terminal, || {
        editor
            .command(&repo.join(file), line)
            .current_dir(repo)
            .status()
    });
    let name = editor.name();
    match result {
        Ok(_) => format!(
            "Opened {}:{line} in {name}. {rerun} re-runs the review if you changed the code.",
            file.display()
        ),
        Err(e) => format!("Couldn't start {name}: {e} (`editor` in the config)"),
    }
}

/// Run the agent CLI `cli` in `cwd` with `prompt` as its first message, on
/// this terminal, and wait for it to exit (Claude Code resumes a fork of
/// session `resume` when there is one, so a session open elsewhere stays as
/// it is). The caller hands it the terminal first; its exit, however it
/// went, is the signal that the changes to the notes are in.
pub fn run_agent(
    tools: &crate::approve::Tools,
    cli: crate::config::Provider,
    cwd: &std::path::Path,
    prompt: &str,
    resume: Option<&str>,
) -> std::io::Result<std::process::ExitStatus> {
    let mut cmd = std::process::Command::new(agent_program(tools, cli));
    if let Some(id) = resume {
        cmd.args(["--resume", id, "--fork-session"]);
    }
    cmd.arg(prompt).current_dir(cwd).status()
}

/// `<space>s`: the setup screen on the review's terminal, the review
/// waiting meanwhile. True when ctrl+c left it, which quits teer too.
async fn open_settings(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rt: &mut Runtime,
) -> bool {
    let path = store::config_path();
    let opened = setup::Opened::Review;
    match setup::run_on(terminal, app.cfg.clone(), &path, opened, app.keys.clone()).await {
        Ok(setup::Ended::Saved(cfg)) => app.status = rt.settings_saved(app, *cfg),
        Ok(setup::Ended::Left) => {}
        Ok(setup::Ended::Quit) => return true,
        Err(e) => app.status = format!("Couldn't change the settings: {e:#}"),
    }
    false
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::{Backend, TestBackend};
    use ratatui::widgets::Paragraph;

    use super::force_redraw;

    /// Back from the editor the screen is blank: the next frame must draw
    /// everything, not only what changed since the last one.
    #[test]
    fn after_the_screen_was_wiped_the_next_frame_is_drawn_whole() {
        let mut t = Terminal::new(TestBackend::new(20, 2)).unwrap();
        let frame = |t: &mut Terminal<TestBackend>| {
            t.draw(|f| f.render_widget(Paragraph::new("teer"), f.area()))
                .unwrap();
        };
        frame(&mut t);
        let wiped = |t: &mut Terminal<TestBackend>| {
            t.backend_mut()
                .clear_region(ratatui::backend::ClearType::All)
                .unwrap();
        };
        // Without it, the same frame draws nothing on the wiped screen.
        wiped(&mut t);
        frame(&mut t);
        let blank = " ".repeat(20);
        t.backend().assert_buffer_lines([blank.as_str(), &blank]);
        force_redraw(&mut t);
        frame(&mut t);
        t.backend()
            .assert_buffer_lines([format!("{:20}", "teer").as_str(), &blank]);
    }
}
