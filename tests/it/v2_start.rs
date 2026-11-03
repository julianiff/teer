//! v2 Phase 2, one start command (S1–S4, S6–S8, S10 of docs/v2-map.md §11):
//! what plain `teer` reviews (a dirty tree, a branch since its base, nothing
//! on the default branch), the base chain, `teer <url>` and `teer <number>`,
//! `teer pr`, the subcommands that stay, the first run, and the flags.
//!
//! Runs the real binary in temp git repos, each run in a `Sandbox`
//! (tests/it/common/sandbox.rs): its own `HOME`, a `PATH` with git and the test's
//! fake programs only (never the real claude, codex or gh), a fake gh through
//! `TEER_GH`, its own config file and the test process's data dir. Where only
//! a terminal shows the result (the TUI, the setup screen, the PR picker), the
//! test checks what the binary prints without one, or the library part, and
//! says so.

use crate::common;

use std::process::Command;

use common::sandbox::*;
use common::*;
use serde_json::Value;
use teer::config::Config;
use teer::keymap::{Keymap, parse_keys};
use teer::tui::setup::{Effect as SetupEffect, Opened, Setup};

// ---- The surface these tests expect (Phase 2) ------------------------------

/// What plain `teer` prints, and all it prints, on a clean default branch.
fn nothing_to_review(branch: &str) -> String {
    format!("Nothing to review: no changes, and you're on {branch}.")
}

/// Flags `teer --help` shows after Phase 2, and the ones it hides but takes.
const VISIBLE_FLAGS: [&str; 3] = ["--base", "--no-agent", "--json"];
const HIDDEN_FLAGS: [&str; 1] = ["--session"];

// ---- Fixtures --------------------------------------------------------------

const SHOP_V0: &str =
    "def price(qty):\n    return qty * 10\n\n\ndef tax(amount):\n    return amount * 0.2\n";
const SHOP_V1: &str =
    "def price(qty):\n    return qty * 12\n\n\ndef tax(amount):\n    return amount * 0.2\n";
const NEW_PY: &str = "def fresh(x):\n    return x + 1\n";

fn sha(repo: &FixtureRepo, rev: &str) -> String {
    repo.git(&["rev-parse", rev]).trim().to_string()
}

/// One commit on main with `shop.py`.
fn one_commit() -> FixtureRepo {
    let repo = FixtureRepo::new();
    repo.write("shop.py", SHOP_V0).commit("init");
    repo
}

/// A bare repo standing in for GitHub: `origin` is
/// `https://github.com/acme/shop.git`, which git rewrites to it.
fn github_origin(repo: &FixtureRepo) -> tempfile::TempDir {
    let bare = tempfile::tempdir().unwrap();
    let out = Command::new(real_git())
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(bare.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success());
    let path = bare.path().canonicalize().unwrap();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/shop.git",
    ]);
    repo.git(&[
        "config",
        &format!("url.{}.insteadOf", path.display()),
        "https://github.com/acme/shop.git",
    ]);
    bare
}

/// Branches whose merge bases with `feature` all differ:
///
/// ```text
/// main     A ─ M          (M adds other.py)
///           \
/// develop    D            (adds dev.py)
///             \
/// release      R          (adds rel.py)
///               \
/// feature        F        (changes shop.py), checked out, clean
/// ```
struct Chain {
    repo: FixtureRepo,
    a: String,
    d: String,
    r: String,
    f: String,
}

impl Chain {
    fn new() -> Chain {
        let repo = one_commit();
        let a = sha(&repo, "HEAD");
        repo.git(&["checkout", "-q", "-b", "develop"]);
        repo.write("dev.py", "def dev():\n    return 1\n")
            .commit("develop");
        let d = sha(&repo, "HEAD");
        repo.git(&["checkout", "-q", "-b", "release"]);
        repo.write("rel.py", "def rel():\n    return 2\n")
            .commit("release");
        let r = sha(&repo, "HEAD");
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.write("shop.py", SHOP_V1).commit("price up");
        let f = sha(&repo, "HEAD");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("other.py", "def other():\n    return 3\n")
            .commit("main moves on");
        repo.git(&["checkout", "-q", "feature"]);
        Chain { repo, a, d, r, f }
    }
}

/// A gh that knows the pull request of the current branch: its base is
/// `develop`. Answers `--jq`/`--template` with the name alone, `gh api` and
/// `gh pr list` with a list, anything else with `gh pr view`'s object.
const GH_PR_BASE_DEVELOP: &str = r#"echo "$@" >> "$(dirname "$0")/gh.log"
case " $* " in
  *" --jq "*|*" -q "*|*" --template "*|*" -t "*) echo develop ;;
  *" api "*) echo '[{"number":7,"base":{"ref":"develop"},"head":{"ref":"feature"},"state":"open"}]' ;;
  *" list "*) echo '[{"number":7,"baseRefName":"develop","headRefName":"feature","state":"OPEN","url":"https://github.com/acme/shop/pull/7"}]' ;;
  *) echo '{"number":7,"baseRefName":"develop","headRefName":"feature","state":"OPEN","url":"https://github.com/acme/shop/pull/7"}' ;;
esac"#;

/// A gh that knows no pull request for the branch.
const GH_NO_PR: &str = r#"echo "$@" >> "$(dirname "$0")/gh.log"
echo 'no pull requests found for branch "feature"' >&2
exit 1"#;

// ---- S1: a dirty tree ------------------------------------------------------

#[test]
fn dirty_tree_reviews_the_changes() {
    for kind in ["staged", "unstaged", "untracked"] {
        let sb = Sandbox::new();
        let repo = one_commit();
        let head = sha(&repo, "HEAD");
        let want = match kind {
            "staged" => {
                repo.write("shop.py", SHOP_V1);
                repo.git(&["add", "shop.py"]);
                "shop.py"
            }
            "unstaged" => {
                repo.write("shop.py", SHOP_V1);
                "shop.py"
            }
            _ => {
                repo.write("new.py", NEW_PY);
                "new.py"
            }
        };
        let v = sb.json(&repo.path(), &["--json", "--no-agent"]);
        assert_eq!(v["head"], teer::git::WORKTREE, "{kind}");
        assert_eq!(v["base"], head.as_str(), "{kind}: against HEAD");
        assert_eq!(files(&v), vec![want], "{kind}");
        assert!(
            !v["units"].as_array().unwrap().is_empty(),
            "{kind}: units to review"
        );
    }
}

#[test]
fn a_dirty_tree_on_a_feature_branch_reviews_only_the_uncommitted_changes() {
    let sb = Sandbox::new();
    let c = Chain::new();
    c.repo.write("new.py", NEW_PY);
    let v = sb.json(&c.repo.path(), &["--json", "--no-agent"]);
    assert_eq!(v["head"], teer::git::WORKTREE);
    assert_eq!(v["base"], c.f.as_str());
    assert_eq!(files(&v), vec!["new.py"], "not the branch's commits");
}

// ---- S2: a clean feature branch --------------------------------------------

#[test]
fn clean_branch_reviews_since_base() {
    let sb = Sandbox::new();
    let repo = one_commit();
    let fork = sha(&repo, "HEAD");
    repo.git(&["checkout", "-q", "-b", "feature/refunds"]);
    repo.write("shop.py", SHOP_V1).commit("price up");
    repo.write("new.py", NEW_PY).commit("add fresh");
    let head = sha(&repo, "HEAD");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("other.py", "def other():\n    return 3\n")
        .commit("main moves on");
    repo.git(&["checkout", "-q", "feature/refunds"]);
    let status = repo.git(&["status", "--porcelain"]);
    assert_eq!(status, "", "clean");

    let v = sb.json(&repo.path(), &["--json", "--no-agent"]);
    assert_eq!(v["base"], fork.as_str(), "the merge base with main");
    assert_eq!(v["head"], head.as_str());
    assert_eq!(v["head_commit"], head.as_str());
    assert_eq!(
        files(&v),
        vec!["new.py", "shop.py"],
        "the branch's changes, not what main did since"
    );
    // Reviewing touched nothing.
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    assert_eq!(
        repo.git(&["branch", "--show-current"]).trim(),
        "feature/refunds"
    );
}

// ---- S3: a clean default branch --------------------------------------------

#[test]
fn clean_default_branch_exits_zero() {
    let sb = Sandbox::new();
    // A config with a provider: no first-run question gets in the way.
    sb.config("provider = \"claude\"\n");

    let main = one_commit();
    let out = sb.run(&main.path(), &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let all = format!("{}{}", stdout(&out), stderr(&out));
    assert_eq!(all.trim(), nothing_to_review("main"));

    let master = one_commit();
    master.git(&["branch", "-m", "master"]);
    let out = sb.run(&master.path(), &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let all = format!("{}{}", stdout(&out), stderr(&out));
    assert_eq!(all.trim(), nothing_to_review("master"));
}

#[test]
fn the_default_branch_is_the_one_origin_head_names() {
    let sb = Sandbox::new();
    sb.config("provider = \"claude\"\n");
    let c = Chain::new();
    let _bare = github_origin(&c.repo);
    c.repo
        .git(&["push", "-q", "origin", "main", "develop", "release"]);
    c.repo.git(&["remote", "set-head", "origin", "develop"]);
    c.repo.git(&["checkout", "-q", "develop"]);
    let out = sb.run(&c.repo.path(), &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let all = format!("{}{}", stdout(&out), stderr(&out));
    assert_eq!(all.trim(), nothing_to_review("develop"));
}

// ---- S4: the base chain ----------------------------------------------------

#[test]
fn base_fallback_chain() {
    let sb = Sandbox::new();
    let c = Chain::new();
    let dir = c.repo.path();
    let _bare = github_origin(&c.repo);
    c.repo
        .git(&["push", "-q", "origin", "main", "develop", "release"]);
    c.repo.git(&["push", "-q", "-u", "origin", "feature"]);
    c.repo.git(&["remote", "set-head", "origin", "release"]);
    sb.gh(GH_PR_BASE_DEVELOP);

    // 1. `--base` wins over gh, origin/HEAD and main.
    let v = sb.json(&dir, &["--base", "release", "--json", "--no-agent"]);
    assert_eq!(v["base"], c.r.as_str(), "--base release");
    assert_eq!(v["head"], c.f.as_str());
    assert_eq!(files(&v), vec!["shop.py"]);

    // 2. Without it: the base of the branch's pull request, as gh knows it.
    let v = sb.json(&dir, &["--json", "--no-agent"]);
    assert_eq!(
        v["base"],
        c.d.as_str(),
        "gh's PR base (develop); gh was asked: {}",
        sb.gh_calls()
    );
    assert_eq!(files(&v), vec!["rel.py", "shop.py"]);

    // 3. gh knows no pull request: origin/HEAD (release).
    sb.gh(GH_NO_PR);
    let v = sb.json(&dir, &["--json", "--no-agent"]);
    assert_eq!(v["base"], c.r.as_str(), "origin/HEAD");
    assert_eq!(files(&v), vec!["shop.py"]);

    // 4. No origin/HEAD either: main.
    c.repo.git(&["remote", "set-head", "origin", "-d"]);
    let v = sb.json(&dir, &["--json", "--no-agent"]);
    assert_eq!(v["base"], c.a.as_str(), "main");
    assert_eq!(files(&v), vec!["dev.py", "rel.py", "shop.py"]);
    assert_eq!(v["head"], c.f.as_str());
}

#[test]
fn base_falls_back_to_main_or_master_without_a_remote_or_gh() {
    let sb = Sandbox::new();
    let c = Chain::new();
    let v = sb.json(&c.repo.path(), &["--json", "--no-agent"]);
    assert_eq!(v["base"], c.a.as_str(), "main");
    assert_eq!(files(&v), vec!["dev.py", "rel.py", "shop.py"]);

    let repo = one_commit();
    repo.git(&["branch", "-m", "master"]);
    let fork = sha(&repo, "HEAD");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("shop.py", SHOP_V1).commit("price up");
    repo.git(&["checkout", "-q", "master"]);
    repo.write("other.py", "def other():\n    return 3\n")
        .commit("master moves on");
    repo.git(&["checkout", "-q", "feature"]);
    let v = sb.json(&repo.path(), &["--json", "--no-agent"]);
    assert_eq!(v["base"], fork.as_str(), "master");
    assert_eq!(files(&v), vec!["shop.py"]);
}

// ---- S6: `teer <url>` and `teer <number>` ----------------------------------

static NEXT_PR: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(480);

/// A clone of "GitHub" (see [`github_origin`]) with pull request `n` at
/// `refs/pull/<n>/head` and `/merge`, which changes `shop.py`; the clone is
/// on main with uncommitted work of its own.
struct Pr {
    n: u32,
    repo: FixtureRepo,
    _bare: tempfile::TempDir,
    fork: String,
    head: String,
}

impl Pr {
    fn new() -> Pr {
        let n = NEXT_PR.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let repo = one_commit();
        let fork = sha(&repo, "HEAD");
        repo.git(&["checkout", "-q", "-b", "pr"]);
        repo.write("shop.py", SHOP_V1).commit("pr change");
        let head = sha(&repo, "HEAD");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("other.py", "def other():\n    return 3\n")
            .commit("main moves on");
        let bare = github_origin(&repo);
        repo.git(&["push", "-q", "origin", "main"]);
        repo.git(&["push", "-q", "origin", &format!("pr:refs/pull/{n}/head")]);
        repo.git(&["checkout", "-q", "--detach", "main"]);
        repo.git(&["merge", "-q", "--no-ff", "pr", "-m", "merge"]);
        repo.git(&["push", "-q", "origin", &format!("HEAD:refs/pull/{n}/merge")]);
        repo.git(&["checkout", "-q", "main"]);
        repo.git(&["branch", "-q", "-D", "pr"]);
        repo.write("scratch.txt", "mine\n");
        Pr {
            n,
            repo,
            _bare: bare,
            fork,
            head,
        }
    }

    fn url(&self) -> String {
        format!("https://github.com/acme/shop/pull/{}", self.n)
    }
}

#[test]
fn url_and_number_open_the_pr() {
    let sb = Sandbox::new();
    let pr = Pr::new();
    let dir = pr.repo.path();
    let status = pr.repo.git(&["status", "--porcelain"]);
    let url = pr.url();
    let number = pr.n.to_string();
    for target in [url.as_str(), number.as_str()] {
        let v = sb.json(&dir, &[target, "--json", "--no-agent"]);
        assert_eq!(v["pr"]["number"], pr.n, "teer {target}");
        assert_eq!(v["pr"]["owner"], "acme", "teer {target}");
        assert_eq!(v["head"], pr.head.as_str(), "teer {target}");
        assert_eq!(v["base"], pr.fork.as_str(), "teer {target}");
        assert_eq!(files(&v), vec!["shop.py"], "teer {target}");
        // The user's clone is as it was.
        assert_eq!(pr.repo.git(&["status", "--porcelain"]), status);
        assert_eq!(pr.repo.git(&["branch", "--show-current"]).trim(), "main");
        assert_eq!(pr.repo.file("scratch.txt"), "mine\n");
    }
    // `teer pr <url>` still works.
    let v = sb.json(&dir, &["pr", &url, "--json", "--no-agent"]);
    assert_eq!(v["pr"]["number"], pr.n);
}

#[test]
fn a_pr_number_without_a_github_remote_is_a_clear_error() {
    let sb = Sandbox::new();
    let none = one_commit();
    let local = one_commit();
    local.git(&["remote", "add", "origin", "/srv/git/shop.git"]);
    for repo in [&none, &local] {
        let out = sb.run(&repo.path(), &["482", "--no-agent"]);
        let err = stderr(&out);
        assert!(!out.status.success(), "{err}");
        assert!(err.to_lowercase().contains("github"), "says why: {err}");
        assert!(!err.contains("Usage"), "not a usage error: {err}");
    }
}

// ---- S7: `teer pr` ---------------------------------------------------------

/// gh's `pr list` answer: one pull request waiting for a review.
const GH_LIST: &str = r#"echo "$@" >> "$(dirname "$0")/gh.log"
echo '[{"number":7,"title":"Charge in one place","url":"https://github.com/acme/shop/pull/7","author":{"login":"ana"},"isDraft":false,"reviewDecision":"REVIEW_REQUIRED","updatedAt":"2026-11-01T10:00:00Z","headRefName":"refunds"}]'"#;

#[test]
fn pr_alone_opens_the_picker() {
    // On a terminal `teer pr` opens the picker (`pr_list::run`); without one
    // the picker prints its list instead, which is what is checked here:
    // `pr` is the command, never a target named "pr".
    let sb = Sandbox::new();
    sb.gh(GH_LIST);
    let repo = one_commit();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/shop.git",
    ]);
    let status = repo.git(&["status", "--porcelain"]);

    let out = sb.run(&repo.path(), &["pr"]);
    let (o, e) = (stdout(&out), stderr(&out));
    assert!(out.status.success(), "{e}");
    assert!(o.contains("Charge in one place"), "{o}");
    assert!(
        !e.contains("Usage") && !e.contains("not a pull request"),
        "{e}"
    );
    assert!(sb.gh_calls().contains("pr list"), "{}", sb.gh_calls());

    let picked = sb.run(&repo.path(), &["pr", "--json"]);
    assert!(picked.status.success(), "{}", stderr(&picked));
    let listed = sb.run(&repo.path(), &["pr", "list", "--json"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let v: Value = serde_json::from_slice(&picked.stdout).unwrap();
    assert!(v.to_string().contains("Charge in one place"), "{v}");
    assert_eq!(stdout(&picked), stdout(&listed), "teer pr is teer pr list");
    assert_eq!(repo.git(&["status", "--porcelain"]), status, "untouched");
}

// ---- S8: subcommands that stay, and the first run ---------------------------

#[test]
fn notes_and_setup_are_still_subcommands() {
    let sb = Sandbox::new();
    let repo = one_commit();

    // `notes` reads this repository's last review; there is none yet.
    let out = sb.run(&repo.path(), &["notes"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no saved review of this repository"),
        "{}",
        stderr(&out)
    );
    repo.write("shop.py", SHOP_V1);
    sb.json(&repo.path(), &["--json", "--no-agent"]);
    let out = sb.run(&repo.path(), &["notes"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("no notes"), "{}", stderr(&out));

    // `setup` is the setup screen, which needs a terminal.
    let out = sb.run(&repo.path(), &["setup"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("needs a terminal"),
        "{}",
        stderr(&out)
    );

    // ... and hidden from the help's commands.
    let out = sb.run(&repo.path(), &["--help"]);
    let help = stdout(&out);
    assert!(
        !help.lines().any(|l| l.trim_start().starts_with("setup")),
        "{help}"
    );
    assert!(
        help.lines().any(|l| l.trim_start().starts_with("notes")),
        "{help}"
    );
}

#[test]
fn first_run_without_a_terminal_goes_straight_to_the_review() {
    // No config and no agent CLI: nothing to ask without a terminal, and the
    // review runs (it says why there is no agent).
    let sb = Sandbox::new();
    let repo = one_commit();
    repo.write("shop.py", SHOP_V1);
    assert!(!sb.config_path().exists());
    let v = sb.json(&repo.path(), &["--json"]);
    assert_eq!(files(&v), vec!["shop.py"]);
    assert!(!v["units"].as_array().unwrap().is_empty());
}

#[test]
fn first_run_setup_then_review() {
    // Assumption: on a terminal with no config, `cli::run` opens
    // `tui::setup::run(.., Opened::First, ..)` and goes on to the review
    // when it ends `Saved` or `Left`, and stops only on `Quit`. Without a
    // pty that path can't be driven from here, so this checks the screen's
    // side of it: on the first start, leaving is "not now" (on to the
    // review), never "quit teer"; only ctrl+c quits.
    let keys = |s: &mut Setup, k: &str| {
        let mut last = None;
        for k in parse_keys(k).unwrap() {
            last = s.on_key(k.event());
        }
        last
    };
    let mut s = Setup::new(Config::default(), Opened::First, Keymap::default());
    assert_eq!(keys(&mut s, "q"), Some(SetupEffect::Leave));
    let mut s = Setup::new(Config::default(), Opened::First, Keymap::default());
    assert_eq!(keys(&mut s, "<esc>"), Some(SetupEffect::Leave));
    let mut s = Setup::new(Config::default(), Opened::First, Keymap::default());
    assert_eq!(keys(&mut s, "<c-c>"), Some(SetupEffect::Quit));
}

// ---- S10: flags ------------------------------------------------------------

#[test]
fn visible_and_hidden_flags() {
    let sb = Sandbox::new();
    let repo = one_commit();
    let out = sb.run(&repo.path(), &["--help"]);
    assert!(out.status.success());
    let help = stdout(&out);
    for f in VISIBLE_FLAGS {
        assert!(help.contains(f), "{f} shown: {help}");
    }
    for f in HIDDEN_FLAGS {
        assert!(!help.contains(f), "{f} hidden: {help}");
    }

    // Hidden, still taken.
    repo.write("shop.py", SHOP_V1);
    let args = ["--session", "0f3c2a", "--json", "--no-agent"];
    let v = sb.json(&repo.path(), &args);
    assert_eq!(files(&v), vec!["shop.py"], "{args:?}");
    // Gone with the v1 screens.
    let out = sb.run(&repo.path(), &["--swipe", "--json", "--no-agent"]);
    assert!(!out.status.success());
}

#[test]
fn no_agent_reviews_without_an_agent() {
    let sb = Sandbox::new();
    sb.config("provider = \"claude\"\n");
    let ran = sb.root.path().join("agent-ran");
    let body = format!("touch '{}'\nexit 1", ran.display());
    sb.program("claude", &body).program("codex", &body);
    let repo = one_commit();
    repo.write("shop.py", SHOP_V1);

    let v = sb.json(&repo.path(), &["--json", "--no-agent"]);
    assert!(!ran.exists(), "no agent was started");
    let units = v["units"].as_array().unwrap();
    assert!(!units.is_empty());
    for u in units {
        assert_ne!(u["prep"], "Ready", "{u}");
        assert!(u["summary"].is_null(), "{u}");
    }
}
