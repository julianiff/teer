//! Command line entry: argument parsing and the headless `--json` path.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::agent::api::ApiAdapter;
use crate::agent::claude::{ClaudeAdapter, is_installed};
use crate::agent::codex::CodexAdapter;
use crate::agent::{AgentAdapter, SessionInfo};
use crate::config::{ApiConfig, Config, Provider, Role};
use crate::review::model::{Review, SessionRef};
use crate::review::prepare;
use crate::review::{BuildOptions, build_review, store};

pub const TAGLINE: &str = "teer: the essence of every revision.";

#[derive(Debug, Parser)]
#[command(
    name = "teer",
    about = TAGLINE,
    long_about = "teer: the essence of every revision.\n\n\
Review code changes by what they mean instead of by their lines. An agent prepares the review \
underneath (Claude Code, Codex, or a model API with your key; `teer setup` picks it); you read it in the terminal \
and swipe through changes.",
    after_help = "Examples:\n  teer                 review uncommitted changes (the agent runs underneath)\n  teer setup           pick what prepares reviews: Claude Code, Codex, or a model API\n  teer --base main     review this branch against main\n  teer --session <id>  ask a fork of this Claude Code session, which knows why\n  teer --swipe         open straight into swipe mode\n  teer --json          print the prepared review as JSON and exit\n  teer pr <url>        review a GitHub pull request of this repository (fetched with git)\n  teer pr list         pick a pull request waiting for your review, assigned to you, or yours\n  teer notes           print your notes from the last review as Markdown, for an agent\n\nConfig: ~/.config/teer/config.toml   Data: ~/.local/share/teer/",
    disable_version_flag = true
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Review the branch against this revision (merge base) instead of the working tree.
    #[arg(long, value_name = "REV")]
    pub base: Option<String>,

    /// Claude Code session that made the changes. teer asks a throwaway fork of it,
    /// so the review knows why. Without it, teer runs
    /// a fresh headless agent in the repo. Claude Code only.
    #[arg(global = true, long, value_name = "ID")]
    pub session: Option<String>,

    /// Open straight into swipe mode.
    #[arg(global = true, long)]
    pub swipe: bool,

    /// Print the prepared review as JSON and exit (no TUI).
    #[arg(global = true, long)]
    pub json: bool,

    /// Don't ask an agent; show units without change statements.
    #[arg(global = true, long)]
    pub no_agent: bool,

    /// Run as if started in this directory.
    #[arg(global = true, short = 'C', value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Print version.
    #[arg(short = 'V', long)]
    pub version: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Review a pull request of the current repository, or list the open ones that need you.
    ///
    /// Fetches refs/pull/<n>/head (and /merge, to find the base) from the remote of this
    /// clone that matches the URL, using plain git. The head is checked out in a worktree
    /// teer keeps under its data dir; your working tree and branches are never touched.
    /// Decisions are saved per pull request. The code under review is untrusted: teer runs none of it.
    #[command(
        args_conflicts_with_subcommands = true,
        subcommand_negates_reqs = true,
        after_help = "Examples:\n  teer pr https://github.com/owner/repo/pull/123\n  teer pr github.com/owner/repo/pull/123/files --no-agent\n  teer pr list"
    )]
    Pr {
        #[command(subcommand)]
        action: Option<PrAction>,

        /// URL of the pull request: https://<host>/<owner>/<repo>/pull/<number>
        #[arg(value_name = "URL", required = true)]
        url: Option<String>,
    },
    /// Print the notes (`m` in the review) of this repository's last review as Markdown.
    ///
    /// Each note comes with the file, the lines, and the change it is about as a diff,
    /// so an agent can work through them: run `! teer notes` in Claude Code, or let
    /// the agent run it. Prints nothing when the review has no notes.
    #[command(
        after_help = "Examples:\n  teer notes\n  teer notes | pbcopy\n  teer notes https://github.com/owner/repo/pull/123"
    )]
    Notes {
        /// The pull request whose review's notes to print, instead of the repository's.
        #[arg(value_name = "URL")]
        url: Option<String>,
    },
    /// Pick what prepares reviews and its models; teer asks this on the first start.
    ///
    /// Claude Code (`claude -p`) or Codex (`codex exec`) headless, or a model API with a key:
    /// Anthropic, OpenAI, OpenRouter, Ollama, or any server that speaks OpenAI's chat
    /// completions. Then the fast model, which describes every change, and the second model,
    /// which checks each description. The choice goes to the config; an API key stays in
    /// your environment, the config only names its variable.
    Setup,
}

#[derive(Debug, Clone, Subcommand)]
pub enum PrAction {
    /// List the open pull requests of this repository that wait for your
    /// review, are assigned to you, or are yours.
    ///
    /// Uses the GitHub CLI (gh, logged in) on the repository behind this clone's remote:
    /// the one `gh repo set-default` chose, else `upstream`, else `origin`. On a terminal,
    /// pick one with ↑/↓ (or j/k) and Enter to review it as `teer pr <url>` would; --swipe,
    /// --no-agent and --session apply to that review. Piped, each pull request is printed
    /// with its URL. With --json, prints the lists as JSON.
    #[command(visible_alias = "ls")]
    List,
}

pub fn version_text() -> String {
    format!("{TAGLINE} (v{})", env!("CARGO_PKG_VERSION"))
}

/// Why there is no agent when the user asked for none; the review list
/// doesn't repeat it as a banner.
pub const NO_AGENT: &str = "running with --no-agent";

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
            let key = api_key(&cfg.api)?;
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

/// The API key from the variable `[api] key_env` names; `None` when it
/// names none (a local server).
pub fn api_key(api: &ApiConfig) -> std::result::Result<Option<String>, String> {
    let var = api.key_env.trim();
    if var.is_empty() {
        return Ok(None);
    }
    match std::env::var(var) {
        Ok(k) if !k.trim().is_empty() => Ok(Some(k.trim().to_string())),
        _ => Err(format!(
            "${var} isn't set: export your API key in it, or pick another agent with `teer setup`."
        )),
    }
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

/// Load a previous review for the same changes and carry its state over.
pub fn restore(review: &mut Review) {
    // A PR review only ever sees reviews of the same PR, never the repo's own.
    let pr = review.pr.as_ref();
    let prev = store::load(
        &review.repo_root,
        pr,
        &review.base,
        &review.head,
        &review.head_commit,
    )
    .or_else(|| store::load_latest(&review.repo_root, pr));
    if let Some(prev) = prev {
        store::merge_previous(review, &prev);
    }
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

pub async fn run(args: Args) -> Result<()> {
    if args.version {
        println!("{}", version_text());
        return Ok(());
    }
    let cwd = match &args.dir {
        Some(d) => d.clone(),
        None => std::env::current_dir().context("no current directory")?,
    };
    let path = store::config_path();
    let no_color = std::env::var_os("NO_COLOR");
    let mut cfg = Config::load(&path)?.with_no_color(no_color.as_deref());
    let keymap = cfg.keymap()?;
    if matches!(args.command, Some(Command::Setup)) {
        if !crate::tui::setup::can_run() {
            anyhow::bail!("teer setup needs a terminal");
        }
        if let crate::tui::setup::Ended::Saved(_) =
            crate::tui::setup::run(cfg, &path, false).await?
        {
            println!("teer: saved to {}", path.display());
        }
        return Ok(());
    }
    let needs_agent = !args.no_agent && !matches!(args.command, Some(Command::Notes { .. }));
    if cfg.provider.is_none() && needs_agent {
        // First start: ask in a terminal; elsewhere (scripts, --json) the
        // agent CLI that is there is used (`discover_agent`), and teer asks
        // next time.
        use crate::tui::setup::Ended;
        let asked = if !args.json && crate::tui::setup::can_run() {
            crate::tui::setup::run(cfg.clone(), &path, true).await?
        } else {
            Ended::Left
        };
        match asked {
            Ended::Saved(saved) => cfg = saved.with_no_color(no_color.as_deref()),
            Ended::Left => {}
            Ended::Quit => return Ok(()),
        }
    }
    if args.session.is_some() && cfg.provider() != Provider::Claude {
        anyhow::bail!(
            "--session works with Claude Code only (the provider is {})",
            cfg.provider().name()
        );
    }
    cfg.editor()?;
    // Bad model settings fail at start, not when the agent is first asked.
    if needs_agent {
        cfg.prepare_model()?;
        cfg.second_model()?;
    }
    match args.command.clone() {
        None => {
            let opts = BuildOptions {
                base: args.base.clone(),
                languages: cfg.languages.clone(),
            };
            let mut review = build_review(&cwd, &opts)?;
            restore(&mut review);
            run_review(args, review, cfg, keymap).await
        }
        Some(Command::Pr {
            action: Some(PrAction::List),
            ..
        }) => {
            if args.base.is_some() {
                anyhow::bail!("--base can't be combined with 'pr list'");
            }
            let picked = crate::pr_list::run(&cwd, args.json, &cfg, keymap.clone()).await?;
            let Some(pr) = picked else {
                return Ok(());
            };
            review_pr(args, cwd, pr.url, cfg, keymap).await
        }
        Some(Command::Notes { url }) => print_notes(&cwd, url.as_deref()),
        Some(Command::Setup) => unreachable!("handled above"),
        Some(Command::Pr { url, .. }) => {
            let url = url.context("teer pr needs the URL of a pull request")?;
            if args.base.is_some() {
                anyhow::bail!(
                    "--base can't be combined with 'pr': the pull request decides the base"
                );
            }
            review_pr(args, cwd, url, cfg, keymap).await
        }
    }
}

/// `teer notes`: the notes of the repository's last review, or of the pull
/// request at `url`, as Markdown on stdout.
fn print_notes(cwd: &Path, url: Option<&str>) -> Result<()> {
    let pr = url.map(crate::pr::parse_url).transpose()?;
    let repo = match &pr {
        // A pull request's reviews are kept by the pull request.
        Some(_) => cwd.to_path_buf(),
        None => crate::git::repo_root(cwd)?,
    };
    let Some(review) = store::load_latest(&repo, pr.as_ref()) else {
        match pr {
            Some(p) => anyhow::bail!("no saved review of PR #{}; run teer pr first", p.number),
            None => anyhow::bail!("no saved review of this repository; run teer first"),
        }
    };
    let notes = crate::notes::markdown(&review);
    if notes.is_empty() {
        eprintln!("teer: the last review has no notes (m writes one on the unit on show)");
    } else {
        print!("{notes}");
    }
    Ok(())
}

/// Fetch the pull request at `url` into teer's worktree and review it.
async fn review_pr(
    args: Args,
    cwd: PathBuf,
    url: String,
    cfg: Config,
    keymap: crate::config::KeyMap,
) -> Result<()> {
    // Fetching and checking out block; no TUI is up yet.
    let langs = cfg.languages.clone();
    let session =
        tokio::task::spawn_blocking(move || crate::pr::prepare(&cwd, &url, langs)).await??;
    // The kept checkout stays for the next run; a throwaway one must go.
    watch_signals(
        session.worktree.repo().to_path_buf(),
        (!session.worktree.is_kept()).then(|| session.worktree.path().to_path_buf()),
    );
    let crate::pr::PrSession {
        review,
        mut worktree,
    } = session;
    let result = run_review(args, review, cfg, keymap).await;
    // The terminal is restored by now; the worktree is released on every path.
    worktree.release();
    exit_if_signalled();
    result
}

/// The signal that asked teer to stop (0: none yet).
static SIGNAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// Wakes the TUI loop so it can save the review and leave the terminal cleanly.
pub static SHUTDOWN: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Exit status of a process ended by signal `sig`, by shell convention.
pub fn exit_code_for_signal(sig: i32) -> i32 {
    128 + sig
}

/// A signal (ctrl+c outside the TUI, SIGTERM, SIGHUP) must not leave a
/// throwaway PR worktree (`dir`) behind or the terminal in raw mode. While the
/// TUI is up it is asked to quit through [`SHUTDOWN`], which saves the review
/// and restores the terminal; if that does not happen soon, the terminal is
/// restored here. Either way `dir` is removed and the exit status is 128 +
/// signal. The kept checkout's lock goes with the process.
fn watch_signals(repo: PathBuf, dir: Option<PathBuf>) {
    #[cfg(unix)]
    use tokio::signal::unix::{SignalKind, signal};
    // Registered before this returns, so no signal finds the default action.
    #[cfg(unix)]
    let (Ok(mut term), Ok(mut hup), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::hangup()),
        signal(SignalKind::interrupt()),
    ) else {
        return;
    };
    tokio::spawn(async move {
        #[cfg(unix)]
        let sig = tokio::select! {
            _ = int.recv() => 2,
            _ = term.recv() => 15,
            _ = hup.recv() => 1,
        };
        #[cfg(not(unix))]
        let sig = {
            let _ = tokio::signal::ctrl_c().await;
            2
        };
        SIGNAL.store(sig, std::sync::atomic::Ordering::SeqCst);
        if crate::tui::is_active() {
            SHUTDOWN.notify_one();
            // The TUI saves and returns; the main task then exits with our code.
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            crate::tui::restore_terminal();
        }
        if let Some(dir) = dir {
            crate::pr::remove_worktree(&repo, &dir);
        }
        std::process::exit(exit_code_for_signal(sig));
    });
}

/// After the review ended: if a signal caused it, exit with its status.
fn exit_if_signalled() {
    let sig = SIGNAL.load(std::sync::atomic::Ordering::SeqCst);
    if sig != 0 {
        std::process::exit(exit_code_for_signal(sig));
    }
}

/// Prepare and show a built review: `--json`, or the TUI.
async fn run_review(
    args: Args,
    mut review: Review,
    mut cfg: Config,
    keymap: crate::config::KeyMap,
) -> Result<()> {
    let repo = review.repo_root.clone();
    let is_pr = review.pr.is_some();

    let agent = if args.no_agent {
        Err(NO_AGENT.to_string())
    } else {
        discover_agent(&repo, is_pr, &cfg)
    };
    if let Ok(a) = &agent {
        // Labels and session names follow the agent found.
        cfg.provider = Some(a.provider);
    }

    if args.json {
        match &agent {
            Ok(a) => {
                let s = match &args.session {
                    Some(id) => {
                        let mut s = session_by_id(id, &a.sessions, &repo);
                        if is_pr {
                            // Read the PR's code, not the checkout the session was made in.
                            s.cwd = repo.clone();
                        }
                        s
                    }
                    None => SessionRef::fresh(cfg.provider().name(), repo.clone()),
                };
                review.set_session(Some(s.clone()));
                prepare_all(&mut review, a.adapter.clone(), s, cfg.concurrency).await;
            }
            Err(why) if !args.no_agent => eprintln!("teer: {why}"),
            Err(_) => {}
        }
        if let Err(e) = store::save(&review) {
            eprintln!("teer: couldn't save review: {e:#}");
        }
        println!("{}", serde_json::to_string_pretty(&review)?);
        return Ok(());
    }

    if review.files.is_empty() && review.skipped.is_empty() {
        let what = if let Some(pr) = &review.pr {
            format!("changes in PR #{}", pr.number)
        } else if review.head == crate::git::WORKTREE {
            "uncommitted changes".to_string()
        } else {
            format!("changes since {}", args.base.as_deref().unwrap_or(""))
        };
        let place = match &review.pr {
            Some(pr) => format!("{}/{}", pr.owner, pr.repo),
            None => repo.display().to_string(),
        };
        println!("teer: no {what} to review in {place}");
        return Ok(());
    }

    crate::tui::run(crate::tui::Launch {
        review,
        agent,
        session_arg: args.session,
        swipe: args.swipe,
        cfg,
        keymap,
    })
    .await
}
