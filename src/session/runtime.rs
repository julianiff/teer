//! Carries out the [`Effect`]s an [`App`] returns that need no screen:
//! agent calls, re-runs, saving, posting, the clipboard. What each finds
//! comes back to the app as an [`AppEvent`] on the runtime's channel. The
//! effects that take over the screen (the editor, the agent CLI, git's
//! commit, the setup screen) are the front end's.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::app::{App, AppEvent, Effect};
use crate::agent::AgentAdapter;
use crate::config::Config;
use crate::review::dive::DiveEvent;
use crate::review::model::DiveKind;
use crate::review::{BuildOptions, prepare, store};

/// Carries out effects. Public so tests can drive it with a mock agent.
/// The front end owns it and hands it every effect it doesn't carry out
/// itself.
pub struct Runtime {
    pub adapter: Option<Arc<dyn AgentAdapter>>,
    pub tx: UnboundedSender<AppEvent>,
    pub build: BuildOptions,
    /// The external programs (gh, browser opener, editor).
    pub tools: crate::approve::Tools,
}

impl Runtime {
    /// Setup saved `saved` (`<space>s`): the app takes it (see
    /// [`settings_saved`]), and `gf` and the texts written in the editor
    /// open the editor it names from now on. The status line to show.
    pub fn settings_saved(&mut self, app: &mut App, saved: Config) -> String {
        let mut status = settings_saved(app, saved);
        match app.cfg.editor() {
            Ok(editor) => self.tools.editor = editor,
            Err(e) => status.push_str(&format!(
                " The editor stays {}: {e:#}",
                self.tools.editor.name()
            )),
        }
        status
    }

    /// The agent CLI the notes were handed to ([`Effect::OpenAgent`], run
    /// by the front end) is done, `ran` being how: its exit is the signal
    /// that the changes are in, so the review re-runs now
    /// ([`App::agent_returned`]); one that couldn't start says why. The
    /// front end calls this once the CLI has exited. Returns true to quit.
    pub fn agent_exited(
        &self,
        app: &mut App,
        cli: crate::config::Provider,
        ran: std::io::Result<std::process::ExitStatus>,
    ) -> bool {
        match ran {
            Ok(_) => {
                let mut quit = false;
                for follow in app.agent_returned(cli) {
                    quit |= self.exec(app, follow);
                }
                quit
            }
            Err(e) => {
                app.status = agent_failed(&self.tools, cli, &e);
                false
            }
        }
    }

    /// While the app waits for the changes to copied notes, look at the
    /// working tree when a look is due ([`App::watch_due`]), off the UI
    /// thread; what it saw comes back as [`AppEvent::Tree`].
    pub fn watch(&self, app: &mut App, now: std::time::Instant) {
        let Some(repo) = app.watch_due(now) else {
            return;
        };
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let seen = crate::watch::snapshot(&repo).map_err(|e| format!("{e:#}"));
            let at = std::time::Instant::now();
            let _ = tx.send(AppEvent::Tree { seen, at });
        });
    }

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
            Effect::SaveCursor => {
                if let Err(e) = store::save_cursor(&app.review) {
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
                    let r = match pr {
                        // A pull request is fetched again: a pushed fix
                        // shows up (Q14). Its SARIF files and code-scanning
                        // alerts are read again too.
                        Some(pr) if pr.repo_dir.is_some() => crate::pr::refresh(&repo, &pr, &opts),
                        pr => crate::review::build_review(&repo, &opts).map(|mut r| {
                            r.pr = pr;
                            r
                        }),
                    };
                    let r = r.map_err(|e| format!("{e:#}"));
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
            Effect::ProveFindings { unit_id } => {
                let tx = self.tx.clone();
                let Some(unit) = app.review.unit(&unit_id).cloned() else {
                    return false;
                };
                // The findings not run yet, by index into the audit's.
                let findings: Vec<(usize, crate::security::Finding)> = app
                    .review
                    .audit_mut(&unit_id)
                    .map(|a| {
                        a.findings
                            .iter()
                            .enumerate()
                            .filter(|(_, f)| f.proof.is_none())
                            .map(|(i, f)| (i, f.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                // Whatever stops it here, every finding is told why, so
                // `proving` clears.
                let fail = |why: &str| {
                    let _ = tx.send(AppEvent::Dive(DiveEvent::Proved {
                        id: unit.id.clone(),
                        fingerprint: unit.fingerprint.clone(),
                        proofs: crate::security::prove::unproven(&findings, why),
                    }));
                };
                let Some((adapter, session)) = self.agent(app) else {
                    fail("no agent available");
                    return false;
                };
                let Some(file) = app.review.file_of(&unit).cloned() else {
                    fail("changed file not found");
                    return false;
                };
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
                    crate::security::prove::run(adapter, session, job, findings, dtx)
                });
            }
            Effect::InterpretTests { unit_id } => {
                let tx = self.tx.clone();
                let fail = |u: &crate::review::model::Unit, error: &str| {
                    let _ = tx.send(AppEvent::Dive(DiveEvent::Failed {
                        id: u.id.clone(),
                        fingerprint: u.fingerprint.clone(),
                        kind: DiveKind::Tests,
                        error: error.into(),
                    }));
                };
                let Some(unit) = app.review.unit(&unit_id).cloned() else {
                    return false;
                };
                let Some((adapter, session)) = self.agent(app) else {
                    fail(&unit, "no agent available");
                    return false;
                };
                let Some(file) = app.review.file_of(&unit).cloned() else {
                    fail(&unit, "changed file not found");
                    return false;
                };
                let Some(search) = unit
                    .dive
                    .results
                    .get(&DiveKind::Tests)
                    .and_then(|r| r.coverage.as_ref())
                    .and_then(|c| c.search.clone())
                else {
                    fail(&unit, "teer hasn't searched the tests yet");
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
                let fail = |u: &crate::review::model::Unit, error: &str| {
                    let _ = tx.send(AppEvent::Dive(DiveEvent::Failed {
                        id: u.id.clone(),
                        fingerprint: u.fingerprint.clone(),
                        kind,
                        error: error.into(),
                    }));
                };
                let Some(unit) = app.review.unit(&unit_id).cloned() else {
                    return false;
                };
                let Some((adapter, session)) = self.agent(app) else {
                    fail(&unit, "no agent available");
                    return false;
                };
                let Some(file) = app
                    .review
                    .files
                    .iter()
                    .find(|f| f.path == unit.file)
                    .cloned()
                else {
                    fail(&unit, "changed file not found");
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
                if kind == DiveKind::Security {
                    // Cited lines are read where the agent reads them.
                    let job = crate::security::audit::Job::new(
                        &app.review,
                        unit,
                        file,
                        session.cwd.clone(),
                        crate::security::SecurityConfig::load(),
                        app.cfg.second_opinion,
                    );
                    forward(tx, AppEvent::Dive, |dtx| {
                        crate::security::audit::run(adapter, session, job, dtx)
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
                let job = crate::review::ask::Job::new(&app.review, index, session.cwd.clone())
                    .with_web(app.web_allowed());
                let ptx = tx.clone();
                let progress = move |progress| {
                    let _ = ptx.send(AppEvent::Asking { index, progress });
                };
                tokio::spawn(async move {
                    let result = crate::review::ask::answer(adapter, session, job, progress).await;
                    let _ = tx.send(AppEvent::Answer { index, result });
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
                let via = crate::clipboard::copy(&text);
                app.status = if app.waiting.is_some() {
                    // Copied to be fixed elsewhere: short, so the line
                    // that teer waits, and how to stop, stays in view.
                    let copied = match via {
                        crate::clipboard::Via::Program(_) => "Copied to the clipboard.",
                        crate::clipboard::Via::Terminal => "Sent to the terminal's clipboard.",
                    };
                    format!("{copied} {}", app.waiting_line())
                } else {
                    match via {
                        crate::clipboard::Via::Program(_) => {
                            format!("Copied {what} to the clipboard.")
                        }
                        crate::clipboard::Via::Terminal => {
                            format!("Sent {what} to the terminal's clipboard (OSC 52).")
                        }
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
            // They need the front end's screen (the terminal, an editor, a
            // window): it carries them out itself.
            Effect::Edit { .. }
            | Effect::EditComment { .. }
            | Effect::Commit { .. }
            | Effect::OpenAgent { .. }
            | Effect::Settings
            | Effect::OpenApp => {}
        }
        false
    }
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

/// The program that runs agent CLI `cli`.
pub fn agent_program(
    tools: &crate::approve::Tools,
    cli: crate::config::Provider,
) -> &std::ffi::OsStr {
    match cli {
        crate::config::Provider::Codex => &tools.codex,
        _ => &tools.claude,
    }
}

/// The status line when the agent CLI couldn't start.
fn agent_failed(
    tools: &crate::approve::Tools,
    cli: crate::config::Provider,
    e: &std::io::Error,
) -> String {
    format!(
        "Couldn't start {} ({}): {e}",
        cli.title(),
        agent_program(tools, cli).to_string_lossy()
    )
}

/// The settings that made the running agent: changing them takes a new
/// start.
fn agent_settings(c: &Config) -> impl PartialEq + use<> {
    (
        c.provider,
        c.api.clone(),
        c.prepare_model.clone(),
        c.prepare_effort.clone(),
        c.second_model.clone(),
        c.second_effort.clone(),
    )
}

/// Take the config setup saved: the app reads it, and its keys, from now
/// on. The agent keeps what it was started with until teer starts again,
/// and so do the names the screens give its models. The status line to
/// show.
pub fn settings_saved(app: &mut App, saved: Config) -> String {
    let mut cfg = saved.with_no_color(std::env::var_os("NO_COLOR").as_deref());
    let mut status = "Settings saved.".to_string();
    if agent_settings(&cfg) != agent_settings(&app.cfg) {
        let old = &app.cfg;
        cfg.provider = old.provider;
        cfg.api = old.api.clone();
        cfg.prepare_model = old.prepare_model.clone();
        cfg.prepare_effort = old.prepare_effort.clone();
        cfg.second_model = old.second_model.clone();
        cfg.second_effort = old.second_effort.clone();
        status.push_str(" The new agent and models take over when teer starts again.");
    }
    match cfg.keymap() {
        Ok((keys, _)) => app.keys = keys,
        Err(e) => status.push_str(&format!(" The keys stay as they were: {e:#}")),
    }
    app.cfg = cfg;
    status
}

#[cfg(test)]
mod tests {
    fn app(cfg: crate::config::Config) -> super::App {
        let review = crate::review::model::Review {
            id: "t".into(),
            repo_root: Default::default(),
            base: String::new(),
            head: String::new(),
            head_commit: String::new(),
            agent_session: None,
            pr: None,
            intent: Default::default(),
            created_at: chrono::Utc::now(),
            files: Vec::new(),
            units: Vec::new(),
            skipped: Vec::new(),
            graph: Default::default(),
            questions: Vec::new(),
            version: 0,
            history: Default::default(),
            branch: None,
            cursor: None,
        };
        super::App::new(review, cfg, Default::default())
    }

    /// `<space>s` saved: the keys apply at once, the agent's models only
    /// from the next start, and the screens keep naming the ones running.
    #[test]
    fn saved_settings_bring_their_keys_and_keep_the_running_agent() {
        use crate::config::{Config, Provider};
        use crate::keymap::{Action, KeySpec};
        let cfg = Config {
            provider: Some(Provider::Claude),
            prepare_model: Some("haiku".into()),
            ..Config::default()
        };
        let mut app = app(cfg.clone());
        let mut saved = cfg.clone();
        saved
            .keys
            .insert("decide.accept".into(), KeySpec::One("y".into()));
        assert_eq!(
            super::settings_saved(&mut app, saved.clone()),
            "Settings saved."
        );
        assert_eq!(app.keys.label(Action::DecideAccept), "y");

        saved.prepare_model = Some("sonnet".into());
        saved.second_opinion = false;
        let status = super::settings_saved(&mut app, saved.clone());
        assert!(status.contains("when teer starts again"), "{status}");
        assert_eq!(app.cfg.prepare_model.as_deref(), Some("haiku"));
        // What doesn't make the agent applies now.
        assert!(!app.cfg.second_opinion);

        // Keys that don't load leave the ones there are.
        saved.prepare_model = Some("haiku".into());
        saved
            .keys
            .insert("move.down".into(), KeySpec::One("y".into()));
        let status = super::settings_saved(&mut app, saved);
        assert!(status.contains("The keys stay as they were"), "{status}");
        assert_eq!(app.keys.label(Action::DecideAccept), "y");
    }
}
