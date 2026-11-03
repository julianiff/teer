//! Phase 2, the start command, from the inside: what plain `teer` picks in
//! real git repos, the base chain, pull requests by URL and number, the
//! session that wrote the changes, and the first start (setup, then the
//! review) with the screens stood in for. gh and claude are fake scripts;
//! nothing reaches the network.

use crate::common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use clap::Parser;
use common::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::cli::{Args, NO_AGENT, Screens, run_on};
use teer::config::{Config, Provider};
use teer::git::{self, Target};
use teer::keymap::Keymap;
use teer::tui::app::App;
use teer::tui::setup::{Ended, Opened};

/// A repository on `main` with one commit.
fn repo() -> FixtureRepo {
    isolate_env();
    let r = FixtureRepo::new();
    r.write("shop.py", "def price(q):\n    return q * 10\n")
        .commit("init");
    r
}

/// [`repo`] with the branch `feature` two commits ahead, checked out.
fn feature() -> FixtureRepo {
    let r = repo();
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("shop.py", "def price(q):\n    return q * 12\n")
        .commit("raise prices");
    r.write("tax.py", "def tax(a):\n    return a * 0.2\n")
        .commit("add tax");
    r
}

fn sha(r: &FixtureRepo, rev: &str) -> String {
    r.git(&["rev-parse", rev]).trim().to_string()
}

fn short(r: &FixtureRepo, rev: &str) -> String {
    r.git(&["rev-parse", "--short", rev]).trim().to_string()
}

fn detect(r: &FixtureRepo) -> Target {
    git::detect(&r.path(), None, || None).unwrap()
}

fn branch(head: &str, base: &str) -> Target {
    Target::Branch {
        head: head.into(),
        base: base.into(),
    }
}

fn nothing(on: &str) -> Target {
    Target::Nothing { on: on.into() }
}

/// An executable script `name` in `dir`.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

/// The binary on `dir` with `args`, no config (first start), gh as given or none.
fn teer(dir: &Path, args: &[&str], gh: Option<&Path>) -> std::process::Output {
    let env = isolate_env();
    let mut c = Command::new(teer_bin());
    c.args(args)
        .arg("-C")
        .arg(dir)
        .env("TEER_DATA_DIR", env.path().join("data"))
        .env("TEER_CONFIG", env.path().join("never-written.toml"));
    if let Some(gh) = gh {
        c.env("TEER_GH", gh);
    }
    c.output().unwrap()
}

/// `teer --json --no-agent` on `dir`: the review's base commit.
fn json_base(dir: &Path, gh: Option<&Path>) -> String {
    let out = teer(dir, &["--json", "--no-agent"], gh);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["base"].as_str().unwrap().to_string()
}

// ---- detection ----

#[test]
fn staged_unstaged_or_untracked_changes_are_reviewed() {
    let r = feature();
    assert_eq!(detect(&r), branch("feature", "main"));

    r.write("shop.py", "def price(q):\n    return q * 13\n");
    assert_eq!(detect(&r), Target::Worktree, "unstaged");
    r.git(&["add", "shop.py"]);
    assert_eq!(detect(&r), Target::Worktree, "staged");
    r.git(&["commit", "-q", "-m", "more"]);
    r.write("new.py", "x = 1\n");
    assert_eq!(detect(&r), Target::Worktree, "untracked");

    // On the default branch too; an ignored file is no change.
    let r = repo();
    r.write(".gitignore", "*.log\n").commit("ignore logs");
    r.write("debug.log", "noise\n");
    assert_eq!(detect(&r), nothing("main"));
    r.write("shop.py", "def price(q):\n    return q\n");
    assert_eq!(detect(&r), Target::Worktree);
}

#[test]
fn a_clean_branch_is_reviewed_since_its_merge_base() {
    let r = feature();
    let fork = sha(&r, "main");
    // main moves on after the fork: the merge base stays where it was.
    r.git(&["checkout", "-q", "main"]);
    r.write("other.py", "y = 2\n").commit("main moves on");
    r.git(&["checkout", "-q", "feature"]);
    assert_eq!(detect(&r), branch("feature", "main"));

    let out = teer(&r.path(), &["--json", "--no-agent"], None);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["base"], fork.as_str());
    assert_eq!(v["head"], sha(&r, "HEAD").as_str());
    assert_eq!(v["intent"]["commits"].as_array().unwrap().len(), 2);
    let files: Vec<&str> = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["shop.py", "tax.py"]);
}

#[test]
fn a_clean_default_branch_has_nothing_to_review() {
    let r = repo();
    assert_eq!(detect(&r), nothing("main"));
    for args in [&[][..], &["--no-agent"], &["--json", "--no-agent"]] {
        let out = teer(&r.path(), args, None);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "Nothing to review: no changes, and you're on main.\n",
            "{args:?}"
        );
    }
    // No commit yet and nothing written.
    isolate_env();
    let empty = FixtureRepo::new();
    assert_eq!(detect(&empty), nothing("main"));
}

#[test]
fn a_detached_head_is_reviewed_only_when_the_base_lacks_it() {
    let r = feature();
    r.git(&["checkout", "-q", "--detach", "main"]);
    assert_eq!(detect(&r), nothing(&short(&r, "HEAD")));
    let out = teer(&r.path(), &["--no-agent"], None);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end(),
        format!(
            "Nothing to review: no changes, and you're on {}.",
            short(&r, "HEAD")
        )
    );
    // Behind the base: the base has it.
    r.git(&["checkout", "-q", "main"]);
    r.write("other.py", "y = 2\n").commit("main moves on");
    r.git(&["checkout", "-q", "--detach", "HEAD~1"]);
    assert_eq!(detect(&r), nothing(&short(&r, "HEAD")));

    r.git(&["checkout", "-q", "--detach", "feature"]);
    assert_eq!(detect(&r), branch(&short(&r, "feature"), "main"));
}

// ---- the base chain ----

#[test]
fn base_chain_flag_then_pr_base_then_origin_head_then_main_or_master() {
    let r = feature();
    let p = r.path();
    // --base wins, gh isn't asked, and it reviews the branch even with changes.
    r.write("scratch.py", "z = 3\n");
    let asked = |what| -> Option<String> { panic!("asked gh for {what}") };
    assert_eq!(
        git::detect(&p, Some("HEAD~1"), || asked("--base")).unwrap(),
        branch("feature", "HEAD~1")
    );
    r.remove("scratch.py");

    // The pull request's base, as found.
    assert_eq!(
        git::detect(&p, None, || Some("develop".into())).unwrap(),
        branch("feature", "develop")
    );
    // Else origin/HEAD, before the local main.
    r.git(&["update-ref", "refs/remotes/origin/trunk", "main"]);
    r.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/trunk",
    ]);
    assert_eq!(
        git::default_base(&p),
        Some(("origin/trunk".into(), "trunk".into()))
    );
    assert_eq!(detect(&r), branch("feature", "origin/trunk"));
    r.git(&["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"]);
    // Else main, else master.
    assert_eq!(detect(&r), branch("feature", "main"));
    r.git(&["branch", "-q", "-m", "main", "master"]);
    assert_eq!(detect(&r), branch("feature", "master"));
    // Else nothing to compare with: say how to name one.
    r.git(&["branch", "-q", "-m", "master", "dev"]);
    let e = git::detect(&p, None, || None).unwrap_err().to_string();
    assert!(e.contains("--base"), "{e}");
    // gh can still name it.
    assert_eq!(
        git::detect(&p, None, || Some("dev".into())).unwrap(),
        branch("feature", "dev")
    );
}

#[test]
fn the_pr_base_comes_from_gh_on_the_remote_it_uses() {
    let r = repo();
    // develop is one commit past main; feature branches off develop.
    r.git(&["checkout", "-q", "-b", "develop"]);
    r.write("dev.py", "d = 1\n").commit("dev work");
    let dev = sha(&r, "develop");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("shop.py", "def price(q):\n    return q * 12\n")
        .commit("raise prices");
    let init = sha(&r, "main");

    let d = tempfile::tempdir().unwrap();
    let gh = script(
        d.path(),
        "gh",
        "echo \"$@\" >> \"$0.args\"\necho '{\"baseRefName\":\"develop\"}'",
    );
    assert_eq!(json_base(&r.path(), Some(&gh)), dev);
    let args = std::fs::read_to_string(d.path().join("gh.args")).unwrap();
    assert_eq!(args.trim(), "pr view --json baseRefName");

    // gh on a remote: its copy of the branch is the base.
    r.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/shop.git",
    ]);
    r.git(&["update-ref", "refs/remotes/origin/develop", &init]);
    assert_eq!(json_base(&r.path(), Some(&gh)), init);
    r.git(&["update-ref", "-d", "refs/remotes/origin/develop"]);
    assert_eq!(json_base(&r.path(), Some(&gh)), dev);

    // gh failing, missing or naming a branch this clone lacks: main.
    let failing = script(
        d.path(),
        "gh-fails",
        "echo 'no pull requests found' >&2\nexit 1",
    );
    assert_eq!(json_base(&r.path(), Some(&failing)), init);
    assert_eq!(json_base(&r.path(), None), init);
    let unknown = script(
        d.path(),
        "gh-unknown",
        "echo '{\"baseRefName\":\"release/9\"}'",
    );
    assert_eq!(json_base(&r.path(), Some(&unknown)), init);
}

#[test]
fn a_slow_gh_is_not_waited_for() {
    let r = feature();
    let d = tempfile::tempdir().unwrap();
    let slow = script(d.path(), "gh", "sleep 5\necho '{\"baseRefName\":\"main\"}'");
    let t = Instant::now();
    assert_eq!(
        teer::gh::pr_base(slow.as_os_str(), &r.path(), Duration::from_millis(300)),
        None
    );
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    let quick = script(d.path(), "gh-quick", "echo '{\"baseRefName\":\" main \"}'");
    assert_eq!(
        teer::gh::pr_base(quick.as_os_str(), &r.path(), teer::gh::BASE_TIMEOUT).as_deref(),
        Some("main")
    );
    let junk = script(d.path(), "gh-junk", "echo main");
    assert_eq!(
        teer::gh::pr_base(junk.as_os_str(), &r.path(), teer::gh::BASE_TIMEOUT),
        None
    );
}

// ---- pull requests ----

#[test]
fn a_number_is_a_pull_request_of_the_github_remote() {
    let r = repo();
    let e = teer::pr::url_of_number(&r.path(), 482)
        .unwrap_err()
        .to_string();
    assert!(e.contains("needs a GitHub remote"), "{e}");
    let out = teer(&r.path(), &["482", "--no-agent"], None);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("pull request #482 needs a GitHub remote"),
        "{err}"
    );

    r.git(&["remote", "add", "origin", "git@github.com:acme/shop.git"]);
    assert_eq!(
        teer::pr::url_of_number(&r.path(), 482).unwrap(),
        "https://github.com/acme/shop/pull/482"
    );
}

#[test]
fn a_url_or_a_number_opens_the_pull_request_path() {
    let r = repo();
    // "GitHub" is a local bare repository without the pull request: the
    // fetch fails at once, after saying what it fetched.
    let bare = tempfile::tempdir().unwrap();
    let out = Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(bare.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    r.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/shop.git",
    ]);
    r.git(&[
        "config",
        &format!(
            "url.{}.insteadOf",
            bare.path().canonicalize().unwrap().display()
        ),
        "https://github.com/acme/shop.git",
    ]);
    for args in [
        &["7", "--no-agent"][..],
        &["https://github.com/acme/shop/pull/7", "--no-agent"],
        &["pr", "7", "--no-agent"],
        &["pr", "https://github.com/acme/shop/pull/7", "--no-agent"],
    ] {
        let out = teer(&r.path(), args, None);
        assert!(!out.status.success(), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("fetching PR #7 from origin"),
            "{args:?}: {err}"
        );
    }
    let out = teer(
        &r.path(),
        &["https://github.com/x/y/pull/1", "--no-agent"],
        None,
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("Run teer from a clone of x/y"));
    for args in [&["7", "--base", "main"][..], &["pr", "--base", "main"]] {
        let out = teer(&r.path(), args, None);
        assert!(!out.status.success(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("--base"),
            "{args:?}"
        );
    }
    let out = teer(&r.path(), &["notse"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("neither a command nor a pull request"));
}

#[test]
fn pr_without_a_url_is_the_picker_and_notes_stay_a_command() {
    let r = repo();
    r.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/shop.git",
    ]);
    let d = tempfile::tempdir().unwrap();
    let gh = script(d.path(), "gh", "echo \"$@\" >> \"$0.args\"\necho '[]'");
    // Not a terminal: the picker's lists are printed, as `teer pr list` does.
    let out = teer(&r.path(), &["pr"], Some(&gh));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("No open pull requests in acme/shop"));
    let calls = std::fs::read_to_string(d.path().join("gh.args")).unwrap();
    assert_eq!(calls.lines().count(), 3, "{calls}");
    assert!(
        calls
            .lines()
            .all(|l| l.starts_with("pr list --repo acme/shop"))
    );

    let out = teer(&r.path(), &["notes"], None);
    assert!(String::from_utf8_lossy(&out.stderr).contains("no saved review of this repository"));
}

// ---- the first start, and the session ----

/// The screens, stood in for: what was shown, and the review launched.
#[derive(Default)]
struct Fake {
    /// What setup writes as the config; `None` leaves without saving.
    saves: Option<&'static str>,
    /// Setup ends with ctrl+c.
    quits: bool,
    /// There is no terminal for setup.
    no_terminal: bool,
    shown: RefCell<Vec<String>>,
    launched: RefCell<Option<teer::tui::Launch>>,
}

#[async_trait::async_trait(?Send)]
impl Screens for Fake {
    async fn setup(
        &self,
        _cfg: Config,
        path: &Path,
        opened: Opened,
        _keys: Keymap,
    ) -> anyhow::Result<Option<Ended>> {
        if self.no_terminal {
            return Ok(None);
        }
        self.shown.borrow_mut().push(format!("setup {opened:?}"));
        if self.quits {
            return Ok(Some(Ended::Quit));
        }
        match self.saves {
            Some(toml) => {
                std::fs::write(path, toml)?;
                Ok(Some(Ended::Saved(Box::new(Config::load(path)?))))
            }
            None => Ok(Some(Ended::Left)),
        }
    }

    async fn review(&self, launch: teer::tui::Launch) -> anyhow::Result<()> {
        self.shown.borrow_mut().push("review".into());
        *self.launched.borrow_mut() = Some(launch);
        Ok(())
    }
}

impl Fake {
    fn shown(&self) -> Vec<String> {
        self.shown.borrow().clone()
    }

    fn launched(&self) -> teer::tui::Launch {
        self.launched
            .borrow_mut()
            .take()
            .expect("no review launched")
    }
}

async fn start(dir: &Path, args: &[&str], config: &Path, screens: &Fake) -> anyhow::Result<()> {
    let args = Args::try_parse_from(
        ["teer", "-C", dir.to_str().unwrap()]
            .into_iter()
            .chain(args.iter().copied()),
    )
    .unwrap();
    run_on(args, config, screens).await
}

/// The first line of `app` drawn 100 wide.
fn header(app: &App) -> String {
    let mut t = Terminal::new(TestBackend::new(100, 12)).unwrap();
    t.draw(|f| teer::tui::ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..100)
        .map(|x| buf[(x, 0)].symbol())
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[tokio::test]
async fn the_first_start_runs_setup_then_the_review() {
    let r = repo();
    r.write("shop.py", "def price(q):\n    return q * 11\n");
    let d = tempfile::tempdir().unwrap();
    let config = d.path().join("config.toml");

    let fake = Fake {
        saves: Some("provider = \"codex\"\n"),
        ..Default::default()
    };
    start(&r.path(), &[], &config, &fake).await.unwrap();
    assert_eq!(fake.shown(), ["setup First", "review"]);
    let l = fake.launched();
    assert_eq!(l.cfg.provider, Some(Provider::Codex));
    assert_eq!(l.review.files.len(), 1);
    assert!(config.exists());

    // The config is there now: straight into the review.
    let fake = Fake::default();
    start(&r.path(), &[], &config, &fake).await.unwrap();
    assert_eq!(fake.shown(), ["review"]);
}

#[tokio::test]
async fn setup_is_left_out_when_it_cant_or_shouldnt_ask() {
    let r = repo();
    r.write("shop.py", "def price(q):\n    return q * 11\n");
    let d = tempfile::tempdir().unwrap();
    let config = d.path().join("config.toml");

    // "Not now": the review all the same, and asked again next time.
    let fake = Fake::default();
    start(&r.path(), &[], &config, &fake).await.unwrap();
    assert_eq!(fake.shown(), ["setup First", "review"]);
    assert!(!config.exists());
    // ctrl+c in setup: no review.
    let fake = Fake {
        quits: true,
        ..Default::default()
    };
    start(&r.path(), &[], &config, &fake).await.unwrap();
    assert_eq!(fake.shown(), ["setup First"]);
    // No terminal, or no agent wanted: no setup.
    let fake = Fake {
        no_terminal: true,
        ..Default::default()
    };
    start(&r.path(), &[], &config, &fake).await.unwrap();
    assert_eq!(fake.shown(), ["review"]);
    let fake = Fake::default();
    start(&r.path(), &["--no-agent"], &config, &fake)
        .await
        .unwrap();
    assert_eq!(fake.shown(), ["review"]);
    // Nothing to review: not even setup.
    let clean = repo();
    let fake = Fake::default();
    start(&clean.path(), &[], &config, &fake).await.unwrap();
    assert!(fake.shown().is_empty());

    // `teer setup` is hidden but runs, and needs a terminal.
    let fake = Fake {
        saves: Some("provider = \"codex\"\n"),
        ..Default::default()
    };
    start(&r.path(), &["setup"], &config, &fake).await.unwrap();
    assert_eq!(fake.shown(), ["setup Command"]);
    let fake = Fake {
        no_terminal: true,
        ..Default::default()
    };
    let e = start(&r.path(), &["setup"], &config, &fake)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("needs a terminal"), "{e}");
}

#[tokio::test]
async fn the_header_says_what_is_reviewed_and_that_there_is_no_agent() {
    let r = feature();
    let d = tempfile::tempdir().unwrap();
    let config = d.path().join("config.toml");
    let fake = Fake::default();
    start(&r.path(), &["--no-agent"], &config, &fake)
        .await
        .unwrap();
    let l = fake.launched();
    assert_eq!(l.agent.as_ref().err().map(String::as_str), Some(NO_AGENT));
    let mut app = App::new(l.review, Config::default(), Keymap::default());
    app.agent_note = Some(NO_AGENT.into());
    let h = header(&app);
    assert!(h.starts_with(" feature since main, 2 commits"), "{h}");
    assert!(h.ends_with("no agent"), "{h}");

    // One commit: its subject follows.
    let fake = Fake::default();
    start(
        &r.path(),
        &["--no-agent", "--base", "HEAD~1"],
        &config,
        &fake,
    )
    .await
    .unwrap();
    let app = App::new(fake.launched().review, Config::default(), Keymap::default());
    assert_eq!(header(&app), " feature since HEAD~1, 1 commit · add tax");

    r.write("tax.py", "def tax(a):\n    return a * 0.3\n");
    let fake = Fake::default();
    start(&r.path(), &["--no-agent"], &config, &fake)
        .await
        .unwrap();
    let mut app = App::new(fake.launched().review, Config::default(), Keymap::default());
    app.has_agent = true;
    assert_eq!(header(&app), " your changes");
}

/// A fake `claude` first on the PATH and an empty Claude Code home, set
/// once for the test process ([`common::run::env`]): the projects folder.
fn claude_home() -> PathBuf {
    common::run::env()
}

/// Claude Code's folder name for sessions started in `dir`.
fn project_dir(dir: &Path) -> String {
    dir.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[tokio::test]
async fn the_session_that_wrote_the_changes_is_asked_and_session_overrides_it() {
    let projects = claude_home();
    let r = repo();
    r.write("shop.py", "def price(q):\n    return q * 11\n");
    let d = tempfile::tempdir().unwrap();
    let config = d.path().join("config.toml");
    std::fs::write(&config, "provider = \"claude\"\n").unwrap();

    // No session wrote it: a fresh agent.
    let fake = Fake::default();
    start(&r.path(), &[], &config, &fake).await.unwrap();
    assert_eq!(fake.launched().session_arg, None);

    let dir = projects.join(project_dir(&r.path()));
    std::fs::create_dir_all(&dir).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    let lines = [
        serde_json::json!({"type": "user", "cwd": r.path(), "message": {"content": "raise the price"}}),
        serde_json::json!({
            "type": "assistant", "cwd": r.path(), "timestamp": now,
            "message": {"content": [{"type": "tool_use", "name": "Edit",
                "input": {"file_path": r.path().join("shop.py")}}]}
        }),
    ];
    let body: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    std::fs::write(dir.join("s-wrote.jsonl"), body.join("\n") + "\n").unwrap();

    let fake = Fake::default();
    start(&r.path(), &[], &config, &fake).await.unwrap();
    let l = fake.launched();
    assert_eq!(l.session_arg.as_deref(), Some("s-wrote"));
    let agent = l.agent.expect("the fake claude is installed");
    let s = agent.sessions.iter().find(|s| s.id == "s-wrote").unwrap();
    assert_eq!(s.cwd, r.path());

    // --session wins; --no-agent asks no session.
    let fake = Fake::default();
    start(&r.path(), &["--session", "abc"], &config, &fake)
        .await
        .unwrap();
    assert_eq!(fake.launched().session_arg.as_deref(), Some("abc"));
    let fake = Fake::default();
    start(&r.path(), &["--no-agent"], &config, &fake)
        .await
        .unwrap();
    assert_eq!(fake.launched().session_arg, None);
}
