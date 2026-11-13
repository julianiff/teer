//! Starting a review, whatever front end shows it: what to review (the
//! repository's changes, its branch, a pull request), the agent that
//! prepares it, and the session it works in.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::agent::api::ApiAdapter;
use crate::agent::claude::{ClaudeAdapter, is_installed};
use crate::agent::codex::CodexAdapter;
use crate::agent::{AgentAdapter, SessionInfo};
use crate::config::{Config, Provider, Role};
use crate::git;
use crate::review::model::{Review, SessionRef};
use crate::review::prepare;
use crate::util::sanitize;

pub struct Agent {
    pub adapter: Arc<dyn AgentAdapter>,
    pub sessions: Vec<SessionInfo>,
    /// The provider it is: the configured one, or the one found.
    pub provider: Provider,
}

/// Find the agent `cfg` names, and its sessions for this repo (Claude
/// Code's only); with none set up, the first agent CLI on the PATH, as teer
/// did before it asked. `Err` explains why there is no agent, for the user.
/// `untrusted` is for code from a pull request: the agent loads none of the
/// repo's own instructions and every prompt says the code is untrusted data.
pub fn discover_agent(
    repo: &Path,
    untrusted: bool,
    cfg: &Config,
) -> std::result::Result<Agent, String> {
    // A CLI found here is known to be installed.
    let (provider, found) = match cfg.provider {
        Some(p) => (p, false),
        None => detect_provider().map_or((Provider::Claude, false), |p| (p, true)),
    };
    let cfg = &Config {
        provider: Some(provider),
        ..cfg.clone()
    };
    let installed = |bin: &str| found || is_installed(bin);
    let model = |role| cfg.model(role).map_err(|e| format!("{e:#}"));
    let (prepare, second) = (model(Role::Fast)?, model(Role::Second)?);
    let missing = |what: &str, url: &str| {
        format!(
            "{what} isn't on your PATH. Install it ({url}), pick another agent with `teer setup`, \
or run with --no-agent to review without change statements."
        )
    };
    let (adapter, sessions): (Arc<dyn AgentAdapter>, _) = match provider {
        Provider::Claude => {
            let adapter = ClaudeAdapter {
                untrusted,
                prepare,
                second,
                ..ClaudeAdapter::default()
            };
            if !installed(&adapter.bin) {
                return Err(missing(
                    "Claude Code ('claude')",
                    "https://claude.com/claude-code",
                ));
            }
            let sessions = adapter
                .list_sessions(repo)
                .map_err(|e| format!("couldn't list Claude Code sessions: {e:#}"))?;
            (Arc::new(adapter), sessions)
        }
        Provider::Codex => {
            let adapter = CodexAdapter {
                untrusted,
                prepare,
                second,
                ..CodexAdapter::default()
            };
            if !installed(&adapter.bin) {
                return Err(missing(
                    "Codex ('codex')",
                    "https://developers.openai.com/codex",
                ));
            }
            (Arc::new(adapter), Vec::new())
        }
        Provider::Api => {
            let key = cfg.api.key()?;
            let deep = model(Role::Deep)?;
            let adapter =
                ApiAdapter::new(cfg.api.format, &cfg.api.url(), key, prepare, second, deep)
                    .map_err(|e| format!("{e:#}"))?;
            (Arc::new(adapter), Vec::new())
        }
    };
    let adapter: Arc<dyn AgentAdapter> = if untrusted {
        Arc::new(crate::pr::UntrustedCode(adapter))
    } else {
        adapter
    };
    Ok(Agent {
        adapter,
        sessions,
        provider,
    })
}

/// The first agent CLI on the PATH: Claude Code, then Codex.
pub fn detect_provider() -> Option<Provider> {
    [Provider::Claude, Provider::Codex]
        .into_iter()
        .find(|p| p.program().is_some_and(is_installed))
}

/// Resolve `--session <id>` (a prefix is enough) against the known sessions.
pub fn session_by_id(id: &str, sessions: &[SessionInfo], repo: &Path) -> SessionRef {
    sessions
        .iter()
        .find(|s| s.id == id)
        .or_else(|| sessions.iter().find(|s| s.id.starts_with(id)))
        .map(|s| s.to_ref())
        .unwrap_or_else(|| SessionRef {
            agent: Provider::Claude.name().into(),
            id: id.into(),
            cwd: repo.to_path_buf(),
        })
}

/// Prepare every unit that still needs the agent, waiting for all of them.
/// No second opinion here: `--json` output stays what it was.
pub async fn prepare_all(
    review: &mut Review,
    adapter: Arc<dyn AgentAdapter>,
    session: SessionRef,
    concurrency: usize,
) {
    let todo = review.to_prepare(None);
    if todo.is_empty() {
        return;
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(prepare::prepare(
        adapter,
        session,
        todo,
        concurrency,
        false,
        Arc::new(review.brief()),
        tx,
    ));
    let mut dropped = 0;
    while let Some(ev) = rx.recv().await {
        if let prepare::PrepareEvent::Ready { dropped: d, .. } = &ev {
            dropped += d.len();
        }
        review.apply(ev);
    }
    let _ = task.await;
    if dropped > 0 {
        eprintln!(
            "teer: dropped {dropped} claim(s) without valid evidence (see {})",
            crate::log::path().display()
        );
    }
}

/// What plain `teer` prints when there is nothing to review: a clean tree
/// on the default branch (`on`), or on a detached HEAD the base has.
pub fn nothing_to_review(on: &str) -> String {
    format!(
        "Nothing to review: no changes, and you're on {}.",
        sanitize(on)
    )
}

/// The URL of the pull request `input` names: its URL, or its number in the
/// repository behind this clone's GitHub remote.
pub fn pr_url(cwd: &Path, input: &str) -> Result<String> {
    match crate::pr::parse_target(input)? {
        crate::pr::Target::Url(url) => Ok(url),
        crate::pr::Target::Number(n) => {
            let repo = git::repo_root(cwd)
                .with_context(|| format!("teer {n} must run inside a clone of the repository"))?;
            crate::pr::url_of_number(&repo, n)
        }
    }
}

/// The base of the current branch's pull request as a ref of this clone:
/// the branch on the remote gh uses (`origin/main`), else the local branch.
/// `None` when gh doesn't say within [`crate::gh::BASE_TIMEOUT`] or names
/// a branch this clone doesn't have.
pub fn pr_base_ref(repo: &Path) -> Option<String> {
    let name = crate::gh::pr_base(&crate::gh::program(), repo, crate::gh::BASE_TIMEOUT)?;
    if !crate::pr::valid_branch_name(&name) {
        return None;
    }
    let remote = crate::pr_list::target_repo(repo).ok().map(|r| r.remote);
    remote
        .map(|r| format!("{r}/{name}"))
        .into_iter()
        .chain([name])
        .find(|r| git::commit_of(repo, r).is_some())
}

/// The Claude Code session that wrote `review`'s changes in the last day
/// ([`crate::agent::sessions::wrote_changes`]), if there is one.
pub fn writer_session(review: &Review) -> Option<SessionInfo> {
    let root = crate::agent::sessions::projects_root()?;
    let changed: Vec<PathBuf> = review
        .files
        .iter()
        .flat_map(|f| std::iter::once(&f.path).chain(&f.old_path))
        .chain(review.skipped.iter().map(|f| &f.path))
        .cloned()
        .collect();
    crate::agent::sessions::wrote_changes(
        &root,
        &review.repo_root,
        &changed,
        std::time::SystemTime::now(),
    )
}

/// A review ready to show: the review, the agent found (or why there is
/// none), the session asked for (`--session`, or the Claude Code session
/// that wrote the changes), the config and the keys, and the SARIF files
/// (`--sarif`) every re-run reads again.
pub struct Launch {
    pub review: Review,
    pub agent: std::result::Result<Agent, String>,
    pub session_arg: Option<String>,
    pub cfg: Config,
    pub keymap: crate::keymap::Keymap,
    pub sarif: Vec<PathBuf>,
    /// The mode named at start (`teer security`); `None` goes by the mode
    /// kept with the review, else the config's, else what is reviewed.
    pub mode: Option<crate::mode::Mode>,
}

/// A review begun: the app on its first screen, the runtime that carries
/// out its effects, the channel their answers come back on, the effects
/// to carry out first, and the lock that keeps the review open here only
/// (dropped after the last save).
pub struct Begun {
    pub app: crate::session::App,
    pub runtime: crate::session::Runtime,
    pub events: tokio::sync::mpsc::UnboundedReceiver<crate::session::AppEvent>,
    pub initial: Vec<crate::session::Effect>,
    pub lock: crate::review::store::Lock,
}

/// The mode `review` opens in: the one the reviewer chose for it (at
/// start or with `<space>m`), else the config's, else `review`.
pub fn mode_for(review: &Review, cfg: &Config) -> crate::mode::Mode {
    review
        .mode
        .or(cfg.mode)
        .unwrap_or_else(|| crate::mode::Mode::for_review(review))
}

/// Begin `l` as every front end does: the app on the queue, started with
/// the agent and the session (a fresh one in the repo when none was asked
/// for), and the runtime with the configured editor. `by` names the front
/// end for the lock: a review open elsewhere (`teer` in a terminal,
/// Teer.app) isn't begun again.
pub fn begin(l: Launch, by: &str) -> Result<Begun> {
    let lock = crate::review::store::lock(&l.review, by)
        .map_err(|held| anyhow::anyhow!("this review is open in {held}: leave it there first"))?;
    let base = (l.review.head != git::WORKTREE).then(|| l.review.base.clone());
    let build = crate::review::BuildOptions {
        base,
        languages: l.cfg.languages.clone(),
        sarif: l.sarif,
    };
    let editor = l.cfg.editor()?;
    let mut review = l.review;
    // A mode named at start is the reviewer's choice: kept with the review.
    if l.mode.is_some() {
        review.mode = l.mode;
    }
    let mode = mode_for(&review, &l.cfg);
    let mut app = crate::session::App::new(review, l.cfg, l.keymap);
    app.set_mode(mode);
    let repo = app.review.repo_root.clone();
    let (adapter, sessions, note) = match l.agent {
        Ok(a) => (Some(a.adapter), a.sessions, None),
        Err(why) => (None, Vec::new(), Some(why)),
    };
    let session = match l.session_arg {
        Some(id) => {
            let mut s = session_by_id(&id, &sessions, &repo);
            if app.review.pr.is_some() {
                // Read the PR's code, not the checkout the session was made in.
                s.cwd = repo.clone();
            }
            s
        }
        None => SessionRef::fresh(app.cfg.provider().name(), repo),
    };
    let initial = app.start(adapter.is_some(), note, sessions, Some(session));
    if let Some(b) = app.review.pr.as_ref().and_then(|p| p.base_assumed.clone()) {
        app.status = format!("base assumed: {} (PR has conflicts)", sanitize(&b));
    }
    let (tx, events) = tokio::sync::mpsc::unbounded_channel();
    let runtime = crate::session::Runtime {
        adapter,
        tx,
        build,
        tools: crate::approve::Tools {
            editor,
            ..Default::default()
        },
    };
    Ok(Begun {
        app,
        runtime,
        events,
        initial,
        lock,
    })
}

/// What to review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    /// The repository's uncommitted changes, or with none its branch since
    /// its base; `base` picks the base (`--base`).
    Repo { base: Option<String> },
    /// A pull request of the repository: its URL or number.
    Pr(String),
}

/// What [`open`] found.
pub enum Opened {
    /// A review to show, and the pull request's checkout to release once
    /// it is over.
    Review {
        launch: Box<Launch>,
        worktree: Option<crate::pr::PrWorktree>,
    },
    /// Nothing to review, and why.
    Nothing(String),
}

/// Open `what` in the repository at `cwd` as plain `teer` does, without
/// asking anything: build the review (a pull request is fetched and
/// checked out first), restore earlier decisions and find the agent
/// (`no_agent`: none). Blocks on git, gh and the agent's sessions.
pub fn open(cwd: &Path, what: &What, mut cfg: Config, no_agent: bool) -> Result<Opened> {
    let keymap = cfg
        .keymap()
        .map(|(k, _)| k)
        .context("invalid [keys] in the config")?;
    cfg.editor()?;
    if !no_agent {
        cfg.prepare_model()?;
        cfg.second_model()?;
    }
    let (review, worktree) = match what {
        What::Repo { base } => {
            let repo = git::repo_root(cwd)?;
            let target = git::detect(&repo, base.as_deref(), || pr_base_ref(&repo))?;
            if let git::Target::Nothing { on } = &target {
                return Ok(Opened::Nothing(nothing_to_review(on)));
            }
            let opts = crate::review::BuildOptions {
                base: target.base().map(String::from),
                languages: cfg.languages.clone(),
                ..Default::default()
            };
            let mut review = crate::review::build_review(cwd, &opts)?;
            if let git::Target::Branch { head, base } = &target {
                review.branch = Some(crate::review::model::BranchNames {
                    head: head.clone(),
                    base: base.clone(),
                });
            }
            crate::review::store::restore(&mut review);
            (review, None)
        }
        What::Pr(input) => {
            let url = pr_url(cwd, input)?;
            let s = crate::pr::prepare(cwd, &url, cfg.languages.clone())?;
            (s.review, Some(s.worktree))
        }
    };
    if review.files.is_empty() && review.skipped.is_empty() {
        return Ok(Opened::Nothing(nothing_to_show(&review)));
    }
    let repo = review.repo_root.clone();
    let is_pr = review.pr.is_some();
    let mut agent = if no_agent {
        Err(crate::session::app::NO_AGENT.to_string())
    } else {
        discover_agent(&repo, is_pr, &cfg)
    };
    if let Ok(a) = &agent {
        // Labels and session names follow the agent found.
        cfg.provider = Some(a.provider);
    }
    let session_arg = match &mut agent {
        Ok(a) if a.provider == Provider::Claude && !is_pr => writer_session(&review).map(|found| {
            let id = found.id.clone();
            if !a.sessions.iter().any(|s| s.id == id) {
                a.sessions.push(found);
            }
            id
        }),
        _ => None,
    };
    Ok(Opened::Review {
        launch: Box::new(Launch {
            review,
            agent,
            session_arg,
            cfg,
            keymap,
            // Teer.app opens a review without other tools' SARIF files; a
            // PR's code-scanning alerts come along all the same.
            sarif: Vec::new(),
            mode: None,
        }),
        worktree,
    })
}

/// What a review with no files says: there is nothing in it, and where.
pub fn nothing_to_show(review: &Review) -> String {
    let what = if let Some(pr) = &review.pr {
        format!("changes in PR #{}", pr.number)
    } else if review.head == git::WORKTREE {
        "uncommitted changes".to_string()
    } else {
        let base = review.branch.as_ref().map(|b| b.base.clone());
        let base = base.unwrap_or_else(|| review.base.chars().take(7).collect());
        format!("changes since {}", sanitize(&base))
    };
    let place = match &review.pr {
        Some(pr) => format!("{}/{}", pr.owner, pr.repo),
        None => review.repo_root.display().to_string(),
    };
    format!("teer: no {what} to review in {place}")
}
