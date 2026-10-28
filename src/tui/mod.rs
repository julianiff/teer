//! Terminal UI runtime: owns the terminal, feeds events to [`app::App`],
//! and carries out the effects it returns (agent calls, saving, the editor).

pub mod app;
pub mod ask;
pub mod commits;
pub mod diff;
pub mod flagged;
pub mod notes;
pub mod picture;
pub mod pr_pick;
pub mod preview;
pub mod select;
pub mod setup;
pub mod slim;
pub mod swipe;
pub mod syntax;
pub mod theme;
pub mod ui;

use std::io::stdout;
use std::sync::Arc;
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
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::agent::AgentAdapter;
use crate::config::{Config, KeyMap};
use crate::review::dive::DiveEvent;
use crate::review::model::{DiveKind, Review};
use crate::review::{BuildOptions, prepare, store};
use app::{App, AppEvent, Effect};

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

pub struct Launch {
    pub review: Review,
    pub agent: std::result::Result<crate::cli::Agent, String>,
    pub session_arg: Option<String>,
    pub swipe: bool,
    pub cfg: Config,
    pub keymap: KeyMap,
}

/// Carries out effects. Public so tests can drive it with a mock agent.
pub struct Runtime {
    pub adapter: Option<Arc<dyn AgentAdapter>>,
    pub tx: UnboundedSender<AppEvent>,
    pub build: BuildOptions,
    /// The external programs (gh, browser opener, editor).
    pub tools: crate::approve::Tools,
}

pub async fn run(l: Launch) -> Result<()> {
    let base = if l.review.head == crate::git::WORKTREE {
        None
    } else {
        Some(l.review.base.clone())
    };
    let build = BuildOptions {
        base,
        languages: l.cfg.languages.clone(),
    };
    let mut app = App::new(l.review, l.cfg, l.keymap);
    let repo = app.review.repo_root.clone();
    let (adapter, sessions, note) = match l.agent {
        Ok(a) => (Some(a.adapter), a.sessions, None),
        Err(why) => (None, Vec::new(), Some(why)),
    };
    // Without --session, teer runs the agent headless in the repo; no picker.
    let session = match l.session_arg {
        Some(id) => {
            let mut s = crate::cli::session_by_id(&id, &sessions, &repo);
            if app.review.pr.is_some() {
                // Read the PR's code, not the checkout the session was made in.
                s.cwd = repo.clone();
            }
            s
        }
        None => crate::review::model::SessionRef::fresh(app.cfg.provider().name(), repo.clone()),
    };
    let initial = app.start(adapter.is_some(), note, sessions, Some(session), l.swipe);
    if let Some(b) = app.review.pr.as_ref().and_then(|p| p.base_assumed.clone()) {
        app.status = format!(
            "base assumed: {} (PR has conflicts)",
            crate::util::sanitize(&b)
        );
    }

    let (tx, mut rx) = unbounded_channel();
    let rt = Runtime {
        adapter,
        tx,
        build,
        tools: crate::approve::Tools {
            editor: app.cfg.editor()?,
            ..Default::default()
        },
    };

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
                    Vec::new()
                }
            };
            for e in effects {
                if let Effect::Edit { file, line } = e {
                    // The editor needs the terminal and stdin to itself.
                    drop(events);
                    let rerun = app.keys.label(crate::config::Action::Rerun);
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
                    let rerun = app.keys.label(crate::config::Action::Rerun);
                    app.status = open_agent(
                        &mut terminal,
                        cli,
                        &rt.tools,
                        &cwd,
                        &prompt,
                        resume.as_deref(),
                        &rerun,
                    );
                    events = EventStream::new();
                } else if let Effect::EditComment { text } = e {
                    drop(events);
                    let for_commit = matches!(app.overlay, app::Overlay::Commit { .. });
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
    result
}

impl Runtime {
    fn agent(
        &self,
        app: &App,
    ) -> Option<(Arc<dyn AgentAdapter>, crate::review::model::SessionRef)> {
        Some((self.adapter.clone()?, app.review.agent_session.clone()?))
    }

    /// Open a unit's Visualize page with the opener. A page that is gone, or
    /// not in teer's `visuals/` folder, is written again from the saved
    /// picture first. The status line, and whether the stored path changed.
    fn open_visual(&self, app: &mut App, unit_id: &str) -> (String, bool) {
        use crate::review::visual;
        let review_id = app.review.id.clone();
        let Some(u) = app.review.unit_mut(unit_id) else {
            return (String::new(), false);
        };
        let title = visual::title(u);
        let Some(v) = u
            .dive
            .results
            .get_mut(&DiveKind::Visualize)
            .and_then(|r| r.visual.as_mut())
        else {
            return ("No picture yet.".into(), false);
        };
        let home = store::data_dir().join("visuals");
        let mut moved = false;
        if !(v.path.starts_with(&home) && v.path.is_file()) {
            let fresh = visual::page_path(&review_id, unit_id);
            moved = fresh != v.path;
            v.path = fresh;
            if let Err(e) = visual::write_page(v, &title) {
                return (format!("Couldn't write the picture: {e:#}"), moved);
            }
        }
        let status = if crate::approve::open_detached(&self.tools, v.path.as_os_str()) {
            format!("Opened {}", v.path.display())
        } else {
            format!(
                "Couldn't start {}. The picture is at {}",
                self.tools.opener.to_string_lossy(),
                v.path.display()
            )
        };
        (status, moved)
    }

    /// Carry out one effect. Returns true to quit.
    pub fn exec(&self, app: &mut App, effect: Effect) -> bool {
        match effect {
            Effect::Quit => return true,
            Effect::Save => {
                if let Err(e) = store::save(&app.review) {
                    app.status = format!("Couldn't save the review: {e:#}");
                }
            }
            Effect::Prepare(ids) => {
                let Some((adapter, session)) = self.agent(app) else {
                    return false;
                };
                let units = app.review.to_prepare(Some(&ids));
                let tx = self.tx.clone();
                let concurrency = app.cfg.concurrency;
                let second = app.cfg.second_opinion;
                let brief = Arc::new(app.review.brief());
                forward(tx, AppEvent::Prep, |ptx| {
                    prepare::prepare(adapter, session, units, concurrency, second, brief, ptx)
                });
            }
            Effect::Rerun => {
                let tx = self.tx.clone();
                let repo = app.review.repo_root.clone();
                let opts = self.build.clone();
                let pr = app.review.pr.clone();
                tokio::task::spawn_blocking(move || {
                    let r = crate::review::build_review(&repo, &opts)
                        .map(|mut r| {
                            r.pr = pr;
                            r
                        })
                        .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(AppEvent::Rerun(r.map(Box::new)));
                });
            }
            Effect::Dive {
                unit_id,
                kind: DiveKind::Tests,
                ..
            } => {
                // teer's own search of the test files: no agent.
                let tx = self.tx.clone();
                let Some(unit) = app.review.unit(&unit_id).cloned() else {
                    return false;
                };
                let root = app.review.repo_root.clone();
                let callers = crate::review::test_search::callers(&app.review, &unit);
                tokio::task::spawn_blocking(move || {
                    let ev = crate::review::dive::search_tests(&root, &unit, &callers);
                    let _ = tx.send(AppEvent::Dive(ev));
                });
            }
            Effect::InterpretTests { unit_id } => {
                let tx = self.tx.clone();
                let fail = |id: String, error: &str| {
                    let _ = tx.send(AppEvent::Dive(DiveEvent::Failed {
                        id,
                        kind: DiveKind::Tests,
                        error: error.into(),
                    }));
                };
                let Some((adapter, session)) = self.agent(app) else {
                    fail(unit_id, "no agent available");
                    return false;
                };
                let Some(unit) = app.review.unit(&unit_id).cloned() else {
                    return false;
                };
                let Some(file) = app.review.file_of(&unit).cloned() else {
                    fail(unit_id, "changed file not found");
                    return false;
                };
                let Some(search) = unit
                    .dive
                    .results
                    .get(&DiveKind::Tests)
                    .and_then(|r| r.coverage.as_ref())
                    .and_then(|c| c.search.clone())
                else {
                    fail(unit_id, "teer hasn't searched the tests yet");
                    return false;
                };
                let job = crate::review::dive::TestsJob {
                    target: crate::review::visual::target(&app.review, &unit),
                    root: app.review.repo_root.clone(),
                    unit,
                    file,
                    search,
                };
                forward(tx, AppEvent::Dive, |dtx| {
                    crate::review::dive::interpret_tests(adapter, session, job, dtx)
                });
            }
            Effect::Dive { unit_id, kind, web } => {
                let tx = self.tx.clone();
                let fail = |tx: &UnboundedSender<AppEvent>, id: String, error: &str| {
                    let _ = tx.send(AppEvent::Dive(DiveEvent::Failed {
                        id,
                        kind,
                        error: error.into(),
                    }));
                };
                let Some((adapter, session)) = self.agent(app) else {
                    fail(&tx, unit_id, "no agent available");
                    return false;
                };
                let Some(unit) = app.review.unit(&unit_id).cloned() else {
                    return false;
                };
                let Some(file) = app
                    .review
                    .files
                    .iter()
                    .find(|f| f.path == unit.file)
                    .cloned()
                else {
                    fail(&tx, unit_id, "changed file not found");
                    return false;
                };
                if kind == DiveKind::Behavior {
                    let job = crate::review::behavior::Job {
                        repo: app
                            .review
                            .pr
                            .as_ref()
                            .and_then(|p| p.repo_dir.clone())
                            .unwrap_or_else(|| app.review.repo_root.clone()),
                        base: app.review.base.clone(),
                        files: app.review.files.clone(),
                        review_id: app.review.id.clone(),
                        unit,
                        file,
                        timeout: Duration::from_secs(app.cfg.behavior_timeout.max(1)),
                    };
                    forward(tx, AppEvent::Dive, |dtx| {
                        crate::review::behavior::run(adapter, session, job, dtx)
                    });
                    return false;
                }
                // What a picture may cite.
                let target = (kind == DiveKind::Visualize)
                    .then(|| crate::review::visual::target(&app.review, &unit));
                forward(tx, AppEvent::Dive, |dtx| {
                    crate::review::dive::run_dive(
                        adapter, session, unit, file, kind, web, target, dtx,
                    )
                });
            }
            Effect::Ask { index } => {
                let tx = self.tx.clone();
                let Some((adapter, session)) = self.agent(app) else {
                    let _ = tx.send(AppEvent::Answer {
                        index,
                        result: Err("no agent available".into()),
                    });
                    return false;
                };
                // The code is read where the agent reads it: the repository,
                // or the PR's checkout.
                let job = crate::review::ask::Job::new(&app.review, index, session.cwd.clone());
                tokio::spawn(async move {
                    let result = crate::review::ask::answer(adapter, session, job).await;
                    let _ = tx.send(AppEvent::Answer { index, result });
                });
            }
            Effect::Second(ids) => {
                let tx = self.tx.clone();
                let units: Vec<_> = ids
                    .iter()
                    .filter_map(|id| app.review.unit(id))
                    .filter_map(|u| app.review.file_of(u).map(|f| (u.clone(), f.clone())))
                    .collect();
                let Some((adapter, session)) = self.agent(app) else {
                    for (u, _) in &units {
                        let _ = tx.send(AppEvent::Prep(prepare::PrepareEvent::Second {
                            id: u.id.clone(),
                            opinion: Err("no agent available".into()),
                        }));
                    }
                    return false;
                };
                forward(tx, AppEvent::Prep, |ptx| {
                    prepare::second_opinions(adapter, session, units, ptx)
                });
            }
            Effect::OpenVisual { unit_id } => {
                let (status, moved) = self.open_visual(app, &unit_id);
                app.status = status;
                if moved && let Err(e) = store::save(&app.review) {
                    app.status = format!("Couldn't save the review: {e:#}");
                }
            }
            Effect::PostReview {
                pr,
                verdict,
                body,
                inline,
                flat_body,
            } => {
                let tx = self.tx.clone();
                let tools = self.tools.clone();
                tokio::spawn(async move {
                    let result =
                        crate::approve::post(&tools, &pr, verdict, body, inline, flat_body).await;
                    let _ = tx.send(AppEvent::PrReviewed { verdict, result });
                });
            }
            Effect::Copy { text, what } => {
                app.status = match crate::clipboard::copy(&text) {
                    crate::clipboard::Via::Program(_) => format!("Copied {what} to the clipboard."),
                    crate::clipboard::Via::Terminal => {
                        format!("Sent {what} to the terminal's clipboard (OSC 52).")
                    }
                };
            }
            Effect::DraftCommitMessage(input) => {
                let tx = self.tx.clone();
                let Some((adapter, session)) = self.agent(app) else {
                    let _ = tx.send(AppEvent::CommitDrafted(Err("no agent available".into())));
                    return false;
                };
                tokio::spawn(async move {
                    let r = crate::commit::draft_message(
                        adapter,
                        session,
                        *input,
                        crate::commit::DRAFT_TIMEOUT,
                    )
                    .await;
                    let _ = tx.send(AppEvent::CommitDrafted(r));
                });
            }
            Effect::Summarize => {
                let Some((adapter, session)) = self.agent(app) else {
                    return false;
                };
                let prompt = crate::review::prompts::summary_prompt(&app.review);
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let schema = crate::review::prompts::summary_schema();
                    let r = match adapter.ask_prepare(&session, &prompt, &schema).await {
                        Ok(v) => crate::review::intent::clean_summary(&v)
                            .ok_or_else(|| "the agent wrote no summary".to_string()),
                        Err(e) => Err(format!("{e:#}")),
                    };
                    let _ = tx.send(AppEvent::Summary(r));
                });
            }
            // Need the terminal, so `run` handles them.
            Effect::Edit { .. }
            | Effect::EditComment { .. }
            | Effect::Commit { .. }
            | Effect::OpenAgent { .. } => {}
        }
        false
    }
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

/// Run a job on a task of its own and pass each event it sends on to the
/// app, as `wrap` makes it.
fn forward<E, F>(
    tx: UnboundedSender<AppEvent>,
    wrap: fn(E) -> AppEvent,
    job: impl FnOnce(UnboundedSender<E>) -> F,
) where
    E: Send + 'static,
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let (etx, mut erx) = unbounded_channel();
    let job = tokio::spawn(job(etx));
    tokio::spawn(async move {
        while let Some(ev) = erx.recv().await {
            if tx.send(wrap(ev)).is_err() {
                break;
            }
        }
        let _ = job.await;
    });
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

/// Run the agent CLI `cli` in `cwd` with `prompt` as its first message
/// (Claude Code resumes a fork of session `resume` when there is one, so a
/// session open elsewhere stays as it is), then take the terminal back.
/// Returns the status line to show afterwards; `rerun` is the key that
/// re-runs the review.
fn open_agent(
    terminal: &mut ratatui::DefaultTerminal,
    cli: crate::config::Provider,
    tools: &crate::approve::Tools,
    cwd: &std::path::Path,
    prompt: &str,
    resume: Option<&str>,
    rerun: &str,
) -> String {
    let program = match cli {
        crate::config::Provider::Codex => &tools.codex,
        _ => &tools.claude,
    };
    let result = with_terminal_released(terminal, || {
        let mut cmd = std::process::Command::new(program);
        if let Some(id) = resume {
            cmd.args(["--resume", id, "--fork-session"]);
        }
        cmd.arg(prompt).current_dir(cwd).status()
    });
    let name = cli.title();
    match result {
        Ok(_) => format!("Back from {name}. {rerun} re-runs the review on what it changed."),
        Err(e) => format!("Couldn't start {name} ({}): {e}", program.to_string_lossy()),
    }
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
