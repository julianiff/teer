//! Command line entry: argument parsing, working out what to review, and
//! the headless `--json` path.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::config::{Config, Provider};
use crate::git::{self, Target};
use crate::keymap::{Action, Keymap};
use crate::review::model::{BranchNames, Review, SessionRef};
use crate::review::{BuildOptions, build_review, store};
use crate::start::{
    discover_agent, nothing_to_review, pr_base_ref, pr_url, prepare_all, session_by_id,
    writer_session,
};
use crate::tui::setup::{Ended, Opened};
use crate::util::sanitize;

pub const TAGLINE: &str = "teer: the essence of every revision.";

#[derive(Debug, Parser)]
#[command(
    name = "teer",
    about = TAGLINE,
    long_about = "teer: the essence of every revision.\n\n\
Review code changes by what they mean instead of by their lines. An agent prepares the review \
underneath (Claude Code, Codex, or a model API with your key; teer asks which on the first start, \
`teer setup` changes it); you read it in the terminal.\n\n\
Plain `teer` works out what to review: your uncommitted changes (staged, unstaged or untracked); \
with none, this branch since its base; on the default branch with none, there is nothing to review.",
    after_help = "Examples:\n  teer                 review your changes, or this branch since its base\n  teer --base main     review this branch since main\n  teer <url>           review a GitHub pull request of this repository (fetched with git)\n  teer 482             review pull request #482 of this repository\n  teer pr              pick a pull request waiting for your review, assigned to you, or yours\n  teer --no-agent      review without an agent: no change statements, no questions\n  teer --json          print the prepared review as JSON and exit\n  teer notes           print your notes from the last review as Markdown, for an agent\n\nConfig: ~/.config/teer/config.toml   Data: ~/.local/share/teer/",
    override_usage = "teer [--base <REF>] [--no-agent] [--json]\n       teer <URL>\n       teer pr\n       teer notes",
    disable_version_flag = true,
    disable_help_subcommand = true
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// A pull request to review: its URL, or its number in this repository.
    #[arg(value_name = "URL")]
    pub target: Option<String>,

    /// Review this branch since its merge base with REF, instead of the base
    /// teer finds (the pull request's, else origin/HEAD, else main or master).
    #[arg(long, value_name = "REF")]
    pub base: Option<String>,

    /// The Claude Code session to ask (a throwaway fork of it) and hand the
    /// notes to, instead of the one teer finds: the newest session in this
    /// repository that edited the changed files in the last 24 hours.
    #[arg(global = true, long, value_name = "ID", hide = true)]
    pub session: Option<String>,

    /// Print the prepared review as JSON and exit (no TUI).
    #[arg(global = true, long)]
    pub json: bool,

    /// Review without an agent: no change statements, no questions.
    #[arg(global = true, long)]
    pub no_agent: bool,

    /// Run as if started in this directory. Hidden: the help lists only
    /// the three flags a review takes.
    #[arg(global = true, short = 'C', value_name = "DIR", hide = true)]
    pub dir: Option<PathBuf>,

    /// Print version. Hidden, as `-C` is.
    #[arg(short = 'V', long, hide = true)]
    pub version: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Pick a pull request of the current repository to review.
    ///
    /// Lists the open pull requests that wait for your review, are assigned to you, or are
    /// yours, using the GitHub CLI (gh, logged in) on the repository behind this clone's
    /// remote: the one `gh repo set-default` chose, else `upstream`, else `origin`. On a
    /// terminal, pick one with ↑/↓ (or j/k) and Enter to review it as `teer <url>` would;
    /// --no-agent applies to that review. Piped, each pull request is printed with its URL.
    /// With --json, prints the lists as JSON. The pull request is checked out in a worktree
    /// teer keeps under its data dir; your working tree and branches are never touched. The
    /// code under review is untrusted: teer runs none of it.
    #[command(
        args_conflicts_with_subcommands = true,
        override_usage = "teer pr [--no-agent] [--json]",
        after_help = "Examples:\n  teer pr\n  teer pr --no-agent\n  teer pr --json"
    )]
    Pr {
        /// `teer pr list`, v1's picker. Hidden.
        #[command(subcommand)]
        action: Option<PrAction>,

        /// The pull request, as v1 took it (`teer pr <url>`); `teer <url>` now.
        /// Hidden, and kept for one release with a hint ([`old_form_hint`]).
        #[arg(value_name = "URL", hide = true)]
        url: Option<String>,
    },
    /// Print the notes of this repository's last review as Markdown.
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
    #[command(hide = true)]
    Setup,
    /// Print the help, or a command's (`teer help pr`). Hidden: `--help`
    /// says it.
    #[command(hide = true)]
    Help {
        #[arg(value_name = "COMMAND")]
        command: Option<String>,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum PrAction {
    /// v1's picker, now `teer pr`. Hidden, and kept for one release with a
    /// hint ([`old_form_hint`]).
    #[command(hide = true, alias = "ls")]
    List,
}

/// The one-line hint for a form v1 had and v2 replaced, which still works
/// for one release: `teer pr list` (now `teer pr`), `teer pr <url>` (now
/// `teer <url>`). `None` for every other form.
pub fn old_form_hint(args: &Args) -> Option<String> {
    let (old, new) = match &args.command {
        Some(Command::Pr {
            action: Some(PrAction::List),
            ..
        }) => ("teer pr list", "teer pr".to_string()),
        Some(Command::Pr { url: Some(u), .. }) => {
            ("teer pr <url>", format!("teer {}", sanitize(u)))
        }
        _ => return None,
    };
    Some(format!(
        "`{old}` is now `{new}`; the old form goes in the next release."
    ))
}

/// `teer help [COMMAND]`: the help `--help` prints, of teer or of one of
/// its commands, hidden ones included.
fn print_help(command: Option<&str>) -> Result<()> {
    use clap::CommandFactory;
    let mut cmd = Args::command();
    cmd.build();
    match command {
        None => cmd.print_long_help()?,
        Some(name) => match cmd.find_subcommand_mut(name) {
            Some(sub) => sub.print_long_help()?,
            None => bail!("no command '{}': see teer --help", sanitize(name)),
        },
    }
    Ok(())
}

pub fn version_text() -> String {
    format!("{TAGLINE} (v{})", env!("CARGO_PKG_VERSION"))
}

/// The screens teer opens on the terminal: setup and the review. [`Terminal`]
/// draws them; tests stand in for the user.
#[async_trait::async_trait(?Send)]
pub trait Screens {
    /// The setup screen, opened from `opened`, until it is left; `None` when
    /// there is no terminal to show it on.
    async fn setup(
        &self,
        cfg: Config,
        path: &Path,
        opened: Opened,
        keys: Keymap,
    ) -> Result<Option<Ended>>;

    /// The review, until the user leaves it.
    async fn review(&self, launch: crate::start::Launch) -> Result<()>;
}

/// The screens on the terminal teer runs in.
pub struct Terminal;

#[async_trait::async_trait(?Send)]
impl Screens for Terminal {
    async fn setup(
        &self,
        cfg: Config,
        path: &Path,
        opened: Opened,
        keys: Keymap,
    ) -> Result<Option<Ended>> {
        if !crate::tui::setup::can_run() {
            return Ok(None);
        }
        crate::tui::setup::run(cfg, path, opened, keys)
            .await
            .map(Some)
    }

    async fn review(&self, launch: crate::start::Launch) -> Result<()> {
        crate::tui::run(launch).await
    }
}

/// What a start reviews, worked out before anything is asked or built.
enum Start {
    /// The repository: its changes, or its branch ([`git::detect`]).
    Repo(Target),
    /// The pull request at this URL.
    Pr(String),
    /// A pull request the user picks (`teer pr`, `teer pr list`).
    Pick,
}

pub async fn run(args: Args) -> Result<()> {
    run_on(args, &store::config_path(), &Terminal).await
}

/// [`run`] with the config at `path`, opening `screens`.
pub async fn run_on(args: Args, path: &Path, screens: &dyn Screens) -> Result<()> {
    if args.version {
        println!("{}", version_text());
        return Ok(());
    }
    if let Some(Command::Help { command }) = &args.command {
        return print_help(command.as_deref());
    }
    if let Some(hint) = old_form_hint(&args) {
        eprintln!("teer: {hint}");
    }
    let cwd = match &args.dir {
        Some(d) => d.clone(),
        None => std::env::current_dir().context("no current directory")?,
    };
    let no_color = std::env::var_os("NO_COLOR");
    let mut cfg = Config::load(path)?.with_no_color(no_color.as_deref());
    let keys = cfg
        .keymap()
        .with_context(|| format!("invalid [keys] in {}", path.display()));
    let warn = |warnings: &[String]| {
        for w in warnings {
            eprintln!("teer: {w} (in {})", path.display());
        }
    };
    if matches!(args.command, Some(Command::Setup)) {
        // Keys that don't load don't keep setup from running.
        let keymap = match keys {
            Ok((keymap, warnings)) => {
                warn(&warnings);
                keymap
            }
            Err(e) => {
                eprintln!("teer: {e:#}; setup uses the default keys");
                Keymap::default()
            }
        };
        match screens.setup(cfg, path, Opened::Command, keymap).await? {
            None => bail!("teer setup needs a terminal"),
            Some(Ended::Saved(_)) => println!("teer: saved to {}", path.display()),
            Some(Ended::Left | Ended::Quit) => {}
        }
        return Ok(());
    }
    let (keymap, key_warnings) = keys?;
    warn(&key_warnings);
    let start = match (args.command.clone(), args.target.clone()) {
        (Some(_), Some(t)) => bail!(
            "give either a pull request ('{}') or a command, not both",
            sanitize(&t)
        ),
        (Some(Command::Notes { url }), None) => {
            return print_notes(&cwd, url.as_deref(), &keymap);
        }
        (Some(Command::Setup | Command::Help { .. }), None) => unreachable!("handled above"),
        // `teer pr`, `teer pr list` (clap takes no URL with `list`).
        (Some(Command::Pr { url: None, .. }), None) => Start::Pick,
        (Some(Command::Pr { url: Some(u), .. }), None) | (None, Some(u)) => {
            Start::Pr(pr_url(&cwd, &u)?)
        }
        (None, None) => {
            let repo = git::repo_root(&cwd)?;
            if let Some(b) = args.base.as_deref()
                && let Some((r, name)) = git::default_base(&repo)
                && (b == r || b == name)
            {
                // v1's way to review a branch: plain `teer` finds this base now.
                eprintln!(
                    "teer: plain `teer` finds the base ({name}) on its own; --base picks another"
                );
            }
            Start::Repo(git::detect(&repo, args.base.as_deref(), || {
                pr_base_ref(&repo)
            })?)
        }
    };
    match &start {
        Start::Repo(Target::Nothing { on }) => {
            println!("{}", nothing_to_review(on));
            return Ok(());
        }
        Start::Pr(_) | Start::Pick if args.base.is_some() => {
            bail!("--base can't be combined with a pull request: the pull request decides the base")
        }
        _ => {}
    }
    let needs_agent = !args.no_agent;
    if needs_agent && !args.json && !path.exists() {
        // The first start: setup, then on into the review. Without a
        // terminal (scripts) the agent CLI that is there is used
        // (`discover_agent`), and teer asks next time.
        match screens
            .setup(cfg.clone(), path, Opened::First, keymap.clone())
            .await?
        {
            Some(Ended::Saved(saved)) => cfg = saved.with_no_color(no_color.as_deref()),
            Some(Ended::Quit) => return Ok(()),
            Some(Ended::Left) | None => {}
        }
    }
    if args.session.is_some() && cfg.provider() != Provider::Claude {
        bail!(
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
    match start {
        Start::Repo(target) => {
            let opts = BuildOptions {
                base: target.base().map(String::from),
                languages: cfg.languages.clone(),
            };
            let mut review = build_review(&cwd, &opts)?;
            if let Target::Branch { head, base } = &target {
                review.branch = Some(BranchNames {
                    head: head.clone(),
                    base: base.clone(),
                });
            }
            store::restore(&mut review);
            run_review(args, review, cfg, keymap, screens).await
        }
        Start::Pick => {
            let picked = pr_list(&cwd, args.json, &cfg, keymap.clone()).await?;
            let Some(pr) = picked else {
                return Ok(());
            };
            review_pr(args, cwd, pr.url, cfg, keymap, screens).await
        }
        Start::Pr(url) => review_pr(args, cwd, url, cfg, keymap, screens).await,
    }
}

/// `teer pr list` for the repository at `cwd`. On a terminal the pull
/// requests open in a picker and the one picked is returned, to be reviewed;
/// piped or with `json`, they are printed.
async fn pr_list(
    cwd: &Path,
    json: bool,
    cfg: &Config,
    keys: Keymap,
) -> Result<Option<crate::pr_list::ListedPr>> {
    use crate::pr_list::{fetch, render, target_repo};
    use crate::tui::pr_pick::Picked;
    use std::io::IsTerminal;
    let repo = crate::git::repo_root(cwd)
        .context("teer pr must run inside a clone of a GitHub repository")?;
    let target = {
        let repo = repo.clone();
        tokio::task::spawn_blocking(move || target_repo(&repo)).await??
    };
    let gh = crate::gh::program();
    if !json && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        let theme = crate::tui::theme::Theme::new(cfg.theme);
        let listing = fetch(&gh, &repo, target.clone());
        return match crate::tui::pr_pick::pick(target, listing, theme, keys).await? {
            Picked::Pr(p) => Ok(Some(p)),
            Picked::Quit => Ok(None),
            Picked::Empty(l) => {
                print!("{}", render(&l, chrono::Utc::now(), None, false));
                Ok(None)
            }
        };
    }
    let listing = fetch(&gh, &repo, target).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&listing)?);
        return Ok(None);
    }
    let tty = std::io::stdout().is_terminal();
    let width = tty
        .then(|| crossterm::terminal::size().ok().map(|(w, _)| w as usize))
        .flatten();
    let color = tty && std::env::var_os("NO_COLOR").is_none();
    print!("{}", render(&listing, chrono::Utc::now(), width, color));
    Ok(None)
}

/// `teer notes`: the notes of the repository's last review, or of the pull
/// request at `url`, as Markdown on stdout. `keys` name the key that writes
/// a note.
fn print_notes(cwd: &Path, url: Option<&str>, keys: &Keymap) -> Result<()> {
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
        eprintln!(
            "teer: the last review has no notes ({} writes one on the unit on show)",
            keys.label(Action::NoteEdit)
        );
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
    keymap: Keymap,
    screens: &dyn Screens,
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
    let result = run_review(args, review, cfg, keymap, screens).await;
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
    keymap: Keymap,
    screens: &dyn Screens,
) -> Result<()> {
    let repo = review.repo_root.clone();
    let is_pr = review.pr.is_some();

    let agent = if args.no_agent {
        Err(crate::session::app::NO_AGENT.to_string())
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
        // Levels, reasons and what is accepted for you, as the queue would have them.
        review.settle(cfg.auto_accept);
        if let Err(e) = store::save(&review) {
            eprintln!("teer: couldn't save review: {e:#}");
        }
        println!("{}", serde_json::to_string_pretty(&review)?);
        return Ok(());
    }

    if review.files.is_empty() && review.skipped.is_empty() {
        println!("{}", crate::start::nothing_to_show(&review));
        return Ok(());
    }

    // The session asked and handed the notes: --session, else the Claude
    // Code session that wrote these changes, else a fresh agent.
    let mut agent = agent;
    let session_arg = match (&args.session, &mut agent) {
        (Some(id), _) => Some(id.clone()),
        (None, Ok(a)) if a.provider == Provider::Claude && !is_pr => {
            writer_session(&review).map(|found| {
                let id = found.id.clone();
                if !a.sessions.iter().any(|s| s.id == id) {
                    a.sessions.push(found);
                }
                id
            })
        }
        _ => None,
    };
    screens
        .review(crate::start::Launch {
            review,
            agent,
            session_arg,
            cfg,
            keymap,
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    fn parse(args: &[&str]) -> Args {
        Args::try_parse_from(std::iter::once("teer").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn commands_stay_commands_and_anything_else_is_a_pull_request() {
        assert!(matches!(
            parse(&["notes"]).command,
            Some(Command::Notes { url: None })
        ));
        assert!(matches!(parse(&["setup"]).command, Some(Command::Setup)));
        assert!(matches!(
            parse(&["pr"]).command,
            Some(Command::Pr {
                action: None,
                url: None
            })
        ));
        assert!(matches!(
            parse(&["--no-agent", "pr", "list"]).command,
            Some(Command::Pr {
                action: Some(PrAction::List),
                url: None
            })
        ));
        assert!(matches!(
            parse(&["-C", "/x", "notes"]).command,
            Some(Command::Notes { .. })
        ));
        for (input, target) in [
            (vec!["482"], "482"),
            (
                vec!["https://github.com/a/b/pull/7"],
                "https://github.com/a/b/pull/7",
            ),
            (vec!["--no-agent", "482", "--json"], "482"),
        ] {
            let a = parse(&input);
            assert!(a.command.is_none(), "{input:?}");
            assert_eq!(a.target.as_deref(), Some(target), "{input:?}");
        }
        assert!(parse(&["482", "--json"]).json);
        let plain = parse(&[]);
        assert!(plain.target.is_none() && plain.command.is_none());
        // Still there, only hidden.
        assert_eq!(parse(&["--session", "abc"]).session.as_deref(), Some("abc"));
        // Gone with the v1 screens.
        assert!(Args::try_parse_from(["teer", "--swipe"]).is_err());
        assert_eq!(parse(&["--base", "main"]).base.as_deref(), Some("main"));
    }

    #[test]
    fn help_shows_the_v2_flags_and_hides_the_rest() {
        let help = Args::command().render_long_help().to_string();
        for shown in [
            "--base",
            "--no-agent",
            "--json",
            "[URL]",
            "  pr ",
            "  notes ",
        ] {
            assert!(help.contains(shown), "{shown} missing:\n{help}");
        }
        for hidden in [
            "--session",
            "  setup ",
            "  help ",
            "-C",
            "--version",
            "list",
        ] {
            assert!(!help.contains(hidden), "{hidden} shown:\n{help}");
        }
    }

    #[test]
    fn old_forms_still_parse_and_get_a_hint() {
        let hint = |args: &[&str]| old_form_hint(&parse(args));
        assert_eq!(
            hint(&["pr", "list"]).as_deref(),
            Some("`teer pr list` is now `teer pr`; the old form goes in the next release.")
        );
        assert_eq!(hint(&["pr", "ls"]), hint(&["pr", "list"]));
        assert_eq!(
            hint(&["pr", "https://github.com/a/b/pull/7"]).as_deref(),
            Some(
                "`teer pr <url>` is now `teer https://github.com/a/b/pull/7`; the old form goes in the next release."
            )
        );
        for v2 in [
            &[][..],
            &["pr"],
            &["482"],
            &["notes"],
            &["--base", "main"],
            &["setup"],
        ] {
            assert_eq!(hint(v2), None, "{v2:?}");
        }
        assert!(matches!(
            parse(&["help", "pr"]).command,
            Some(Command::Help { command: Some(ref c) }) if c == "pr"
        ));
    }

    #[test]
    fn the_nothing_message_is_exact() {
        assert_eq!(
            nothing_to_review("main"),
            "Nothing to review: no changes, and you're on main."
        );
    }
}
