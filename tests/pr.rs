//! `teer pr`: fetching a pull request from a (local, fake) remote, reviewing
//! it in the worktree teer keeps per repository (or a throwaway one while that
//! is busy), and keeping its decisions apart from the repository's own. No network and no real GitHub: the "GitHub" is a local
//! bare repository reached through `url.<path>.insteadOf`.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use common::*;
use serde_json::json;
use teer::agent::mock::MockAdapter;
use teer::pr::{self, PrWorktree, UNTRUSTED_NOTICE, UntrustedCode};
use teer::review::model::{Decision, Review, SessionRef};
use teer::review::{BuildOptions, build_review, store};

static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(100);
const SHOP_V0: &str =
    "def price(qty):\n    return qty * 10\n\n\ndef tax(amount):\n    return amount * 0.2\n";
const SHOP_PR: &str =
    "def price(qty):\n    return qty * 12\n\n\ndef tax(amount):\n    return amount * 0.2\n";

struct Fx {
    /// A PR number no other test uses: tests share one data dir.
    n: u32,
    url: String,
    repo: FixtureRepo,
    bare: tempfile::TempDir,
    /// Where the PR branched off.
    fork: String,
    /// Tip of main (it moved on after the fork).
    main_tip: String,
    head: String,
}

impl Fx {
    fn new(with_merge_ref: bool) -> Fx {
        isolate_env();
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let repo = FixtureRepo::new();
        repo.write("shop.py", SHOP_V0).commit("init");
        let fork = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.write("shop.py", SHOP_PR).commit("pr change");
        let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.git(&["checkout", "-q", "main"]);
        repo.write("other.py", "def other():\n    return 1\n")
            .commit("main moves on");
        let main_tip = repo.git(&["rev-parse", "HEAD"]).trim().to_string();

        let bare = tempfile::tempdir().unwrap();
        let out = Command::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(bare.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        let bare_path = bare.path().canonicalize().unwrap();
        let bare_s = bare_path.to_str().unwrap();
        repo.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/shop.git",
        ]);
        repo.git(&[
            "config",
            &format!("url.{bare_s}.insteadOf"),
            "https://github.com/acme/shop.git",
        ]);
        repo.git(&["push", "-q", "origin", "main"]);
        repo.git(&[
            "push",
            "-q",
            "origin",
            &format!("feature:refs/pull/{n}/head"),
        ]);
        if with_merge_ref {
            repo.git(&["checkout", "-q", "--detach", "main"]);
            repo.git(&["merge", "-q", "--no-ff", "feature", "-m", "merge"]);
            repo.git(&["push", "-q", "origin", &format!("HEAD:refs/pull/{n}/merge")]);
            repo.git(&["checkout", "-q", "main"]);
        }
        repo.git(&["branch", "-q", "-D", "feature"]);
        Fx {
            n,
            url: format!("https://github.com/acme/shop/pull/{n}"),
            repo,
            bare,
            fork,
            main_tip,
            head,
        }
    }

    fn path(&self) -> PathBuf {
        self.repo.path()
    }

    fn prepare(&self, url: &str) -> anyhow::Result<pr::PrSession> {
        pr::prepare(&self.path(), url, None)
    }

    /// Uncommitted work in the user's clone: the same edit as the PR to
    /// `shop.py`, and an untracked file.
    fn dirty(&self) {
        self.repo.write("shop.py", SHOP_PR);
        self.repo.write("scratch.txt", "mine\n");
    }

    /// The PR gets a new commit changing `shop.py` to `content`.
    fn push_new_head(&self, content: &str) -> String {
        let r = &self.repo;
        r.git(&["checkout", "-q", "-b", "feature2", &self.head]);
        r.write("shop.py", content).commit("pr update");
        let h = r.git(&["rev-parse", "HEAD"]).trim().to_string();
        r.git(&[
            "push",
            "-q",
            "-f",
            "origin",
            &format!("feature2:refs/pull/{}/head", self.n),
        ]);
        r.git(&["checkout", "-q", "main"]);
        r.git(&["branch", "-q", "-D", "feature2"]);
        h
    }

    fn worktrees(&self) -> usize {
        self.repo
            .git(&["worktree", "list", "--porcelain"])
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count()
    }
}

fn decision(r: &Review, symbol: &str) -> Decision {
    r.units
        .iter()
        .find(|u| u.symbol == symbol)
        .unwrap()
        .decision
}

fn decide(r: &mut Review, symbol: &str, d: Decision) {
    r.units
        .iter_mut()
        .find(|u| u.symbol == symbol)
        .unwrap()
        .decision = d;
}

#[test]
fn reviews_the_pr_range_in_a_worktree_and_keeps_it_for_the_next_run() {
    let fx = Fx::new(true);
    fx.dirty();
    let status_before = fx.repo.git(&["status", "--porcelain"]);
    let head_before = fx.repo.git(&["rev-parse", "HEAD"]);

    let wt_path;
    {
        let s = fx.prepare(&fx.url).unwrap();
        wt_path = s.worktree.path().to_path_buf();
        // Head is the PR head; base is the fork point, not the moved main tip.
        assert_eq!(s.review.head_commit, fx.head);
        assert_eq!(s.review.base, fx.fork);
        assert_ne!(s.review.base, fx.main_tip);
        assert_eq!(s.review.pr.as_ref().unwrap().base_assumed, None);
        assert_eq!(s.review.base, fx.fork);
        assert_eq!(s.review.head, fx.head);
        assert_eq!(s.review.pr.as_ref().unwrap().number, fx.n);
        // Only the PR's own change, not what main did meanwhile.
        let files: Vec<_> = s.review.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(files, vec![PathBuf::from("shop.py")]);
        assert_eq!(s.review.files[0].old_content.as_deref(), Some(SHOP_V0));
        assert_eq!(s.review.files[0].new_content.as_deref(), Some(SHOP_PR));
        assert!(s.review.units.iter().any(|u| u.symbol == "price"));
        // It was reviewed in the worktree, which holds the PR head.
        assert!(wt_path.join("shop.py").exists());
        assert_eq!(
            std::fs::read_to_string(wt_path.join("shop.py")).unwrap(),
            SHOP_PR
        );
        assert!(!wt_path.join("other.py").exists());
        assert!(s.worktree.is_kept());
        assert_eq!(fx.worktrees(), 2);
    }
    // Released: kept on disk and in git's list for the next run.
    assert!(wt_path.join("shop.py").exists());
    assert_eq!(fx.worktrees(), 2);
    // The user's clone is exactly as it was.
    assert_eq!(fx.repo.git(&["status", "--porcelain"]), status_before);
    assert_eq!(fx.repo.git(&["rev-parse", "HEAD"]), head_before);
    assert_eq!(fx.repo.git(&["branch", "--show-current"]).trim(), "main");
    assert_eq!(fx.repo.file("scratch.txt"), "mine\n");
    assert_eq!(fx.repo.file("shop.py"), SHOP_PR);
}

#[test]
fn accepts_url_variants_against_the_same_remote() {
    let fx = Fx::new(true);
    for u in [
        &format!("github.com/acme/shop/pull/{}", fx.n),
        &format!("http://github.com/acme/shop/pull/{}/files", fx.n),
        &format!(
            "https://GitHub.com/Acme/Shop/pull/{}/commits/abcd1234?x=1#y",
            fx.n
        ),
    ] {
        let s = fx.prepare(u).unwrap_or_else(|e| panic!("{u}: {e:#}"));
        assert_eq!(s.review.head_commit, fx.head);
    }
    assert_eq!(fx.worktrees(), 2);
}

#[test]
fn without_a_merge_ref_the_default_branch_is_assumed() {
    let fx = Fx::new(false);
    let s = fx.prepare(&fx.url).unwrap();
    assert_eq!(
        s.review.pr.as_ref().unwrap().base_assumed.as_deref(),
        Some("main")
    );
    assert_eq!(
        s.review.pr.as_ref().unwrap().remote.as_deref(),
        Some("origin")
    );
    assert_eq!(s.review.base, fx.fork);
    assert_eq!(s.review.head_commit, fx.head);
    assert_eq!(s.review.files.len(), 1);
}

#[test]
fn a_stale_merge_ref_from_an_earlier_run_is_not_used() {
    let fx = Fx::new(true);
    let first = fx.prepare(&fx.url).unwrap();
    assert_eq!(first.review.pr.as_ref().unwrap().base_assumed, None);
    drop(first);
    // The PR now conflicts: GitHub removes the merge ref.
    fx.repo.git(&[
        "push",
        "-q",
        "origin",
        &format!(":refs/pull/{}/merge", fx.n),
    ]);
    let second = fx.prepare(&fx.url).unwrap();
    assert_eq!(
        second.review.pr.as_ref().unwrap().base_assumed.as_deref(),
        Some("main")
    );
    assert_eq!(second.review.base, fx.fork);
}

#[test]
fn a_repo_that_is_not_the_prs_repo_is_refused() {
    let fx = Fx::new(true);
    let err = match fx.prepare("https://github.com/other/thing/pull/3") {
        Ok(_) => panic!("must fail"),
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("PR #3"), "{err}");
    assert!(err.contains("github.com/other/thing"), "{err}");
    assert!(err.contains("origin"), "{err}");
    assert!(err.contains("github.com/acme/shop.git"), "{err}");
    assert!(
        err.contains("Run teer from a clone of other/thing"),
        "{err}"
    );
    assert_eq!(fx.worktrees(), 1);

    // A repo with no remotes at all.
    let lone = FixtureRepo::new();
    lone.write("a.py", "def a():\n    pass\n").commit("x");
    let err = match pr::prepare(&lone.path(), "https://github.com/acme/shop/pull/7", None) {
        Ok(_) => panic!("must fail"),
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("no remotes"), "{err}");

    // Not a repository.
    let plain = tempfile::tempdir().unwrap();
    let err = match pr::prepare(plain.path(), "https://github.com/acme/shop/pull/7", None) {
        Ok(_) => panic!("must fail"),
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("not inside a git repository"), "{err}");
}

#[test]
fn remote_credentials_are_not_printed() {
    let fx = Fx::new(true);
    fx.repo.git(&[
        "remote",
        "add",
        "fork",
        "https://me:hunter2@github.com/me/shop.git",
    ]);
    let err = match fx.prepare("https://github.com/other/thing/pull/3") {
        Ok(_) => panic!("must fail"),
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("fork"), "{err}");
    assert!(!err.contains("hunter2"), "{err}");
}

#[test]
fn fetch_failures_are_explained_and_leave_no_worktree() {
    let fx = Fx::new(true);
    // PR that does not exist.
    let err = match fx.prepare("https://github.com/acme/shop/pull/99999") {
        Ok(_) => panic!("must fail"),
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("PR #99999 not found"), "{err}");
    assert_eq!(fx.worktrees(), 1);

    // Remote that cannot be reached.
    let gone = fx.bare.path().join("does-not-exist");
    fx.repo.git(&[
        "config",
        &format!("url.{}.insteadOf", gone.display()),
        "https://github.com/acme/shop.git",
    ]);
    // Later insteadOf entries with the same prefix win by length; make the broken one the only one.
    fx.repo.git(&[
        "config",
        "--unset-all",
        &format!(
            "url.{}.insteadOf",
            fx.bare.path().canonicalize().unwrap().display()
        ),
    ]);
    let err = match fx.prepare(&fx.url) {
        Ok(_) => panic!("must fail"),
        Err(e) => format!("{e:#}"),
    };
    assert!(
        err.contains(&format!("couldn't fetch PR #{} from 'origin'", fx.n)),
        "{err}"
    );
    assert_eq!(fx.worktrees(), 1);
}

#[test]
fn invalid_urls_never_reach_git() {
    let fx = Fx::new(true);
    for u in [
        "--upload-pack=x",
        "",
        "https://github.com/acme/shop/pull/0",
        "a b",
    ] {
        assert!(fx.prepare(u).is_err(), "{u}");
    }
    assert_eq!(fx.worktrees(), 1);
}

#[test]
fn a_throwaway_worktree_is_removed_when_something_fails_after_creating_it() {
    let fx = Fx::new(true);
    fn fails_after_creating(repo: &Path, head: &str) -> anyhow::Result<()> {
        let wt = PrWorktree::create_throwaway(repo, 7000, head, std::process::id())?;
        assert!(!wt.is_kept());
        assert!(wt.path().exists());
        anyhow::bail!("boom {}", wt.path().display());
    }
    let err = fails_after_creating(&fx.path(), &fx.head).unwrap_err();
    let msg = format!("{err:#}");
    let dir = PathBuf::from(msg.strip_prefix("boom ").unwrap());
    assert!(!dir.exists());
    assert_eq!(fx.worktrees(), 1);
}

/// A pid that certainly belongs to no running process.
fn dead_pid() -> u32 {
    let mut c = Command::new("true").spawn().unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

#[test]
fn a_worktree_left_by_a_dead_run_is_swept() {
    let fx = Fx::new(true);
    let first = PrWorktree::create_throwaway(&fx.path(), fx.n, &fx.head, dead_pid()).unwrap();
    let dir = first.path().to_path_buf();
    // Simulate a crash: the guard never runs, the directory stays (with edits).
    std::mem::forget(first);
    std::fs::write(dir.join("junk.txt"), "left over").unwrap();
    assert_eq!(fx.worktrees(), 2);

    let again = fx.prepare(&fx.url).unwrap();
    assert_ne!(again.worktree.path(), dir);
    assert!(!dir.exists(), "the dead run's directory is removed");
    assert_eq!(fx.worktrees(), 2);
    drop(again);
    assert_eq!(fx.worktrees(), 2, "the kept checkout stays");

    // Same when only the directory is gone but git still lists it.
    let s = PrWorktree::create_throwaway(&fx.path(), fx.n, &fx.head, dead_pid()).unwrap();
    let dir = s.path().to_path_buf();
    std::mem::forget(s);
    std::fs::remove_dir_all(&dir).unwrap();
    let s = fx.prepare(&fx.url).unwrap();
    assert!(s.worktree.path().join("shop.py").exists());
    assert_eq!(fx.worktrees(), 2);
}

#[test]
fn concurrent_runs_of_the_same_pr_do_not_share_a_worktree() {
    let fx = Fx::new(true);
    let a = fx.prepare(&fx.url).unwrap();
    // A second run (this very process stands in for another terminal; the
    // owner is alive, so the sweep leaves it alone) finds the kept checkout
    // locked and gets a throwaway one.
    let b = fx.prepare(&fx.url).unwrap();
    assert!(a.worktree.is_kept());
    assert!(!b.worktree.is_kept());
    assert_ne!(a.worktree.path(), b.worktree.path());
    assert!(b.worktree.path().join("shop.py").exists());
    assert_eq!(fx.worktrees(), 3);
    let (pa, pb) = (
        a.worktree.path().to_path_buf(),
        b.worktree.path().to_path_buf(),
    );
    drop(b);
    assert!(
        pa.join("shop.py").exists(),
        "run A's worktree survives run B's cleanup"
    );
    assert!(!pb.exists());
    assert_eq!(fx.worktrees(), 2);
    drop(a);
    assert_eq!(fx.worktrees(), 2);
    // Released: the next run gets the kept checkout again.
    let c = fx.prepare(&fx.url).unwrap();
    assert!(c.worktree.is_kept());
    assert_eq!(c.worktree.path(), pa);
}

#[test]
fn only_teer_worktree_directories_are_ever_removed() {
    let fx = Fx::new(true);
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(outside.path().join("pr-1-2-abcdef01")).unwrap();
    std::fs::write(outside.path().join("pr-1-2-abcdef01/keep.txt"), "x").unwrap();
    // The user's own clone, a temp dir, a `..` path into the data dir: all refused.
    let data = store::data_dir().join("worktrees");
    std::fs::create_dir_all(&data).unwrap();
    let tricky = data
        .join("..")
        .join("worktrees")
        .join("..")
        .join("config-ish");
    std::fs::create_dir_all(&tricky).ok();
    for target in [
        fx.path(),
        outside.path().to_path_buf(),
        outside.path().join("pr-1-2-abcdef01"),
        tricky.clone(),
        data.clone(),
        store::data_dir(),
    ] {
        pr::remove_worktree(&fx.path(), &target);
        assert!(target.exists(), "{} must not be removed", target.display());
    }
    assert!(fx.path().join("shop.py").exists());
    assert!(outside.path().join("pr-1-2-abcdef01/keep.txt").exists());
    // A symlink named like a run dir inside the data dir is not followed either.
    #[cfg(unix)]
    {
        let sub = data.join("some-hash");
        std::fs::create_dir_all(&sub).unwrap();
        for name in ["pr-9-1-abcdef01", "checkout"] {
            let link = sub.join(name);
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(outside.path(), &link).unwrap();
            pr::remove_worktree(&fx.path(), &link);
            assert!(outside.path().join("pr-1-2-abcdef01/keep.txt").exists());
            std::fs::remove_file(&link).unwrap();
        }
    }
}

fn executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Husky-style setup: the user's clone has `core.hooksPath=.husky/_` (relative)
/// with its own `post-checkout` hook, and the PR commits a `.husky/_/post-checkout`
/// of its own. Returns (pr head, marker of the user's hook, marker of the PR's hook).
fn hook_fixture(fx: &Fx) -> (String, PathBuf, PathBuf) {
    let user_marker = fx.bare.path().join("user-hook-ran");
    let pr_marker = fx.bare.path().join("pr-hook-ran");
    let hook = |m: &Path| format!("#!/bin/sh\necho ran > '{}'\n", m.display());
    let r = &fx.repo;
    r.git(&["checkout", "-q", "-b", "hooky", &fx.head]);
    r.write(".husky/_/post-checkout", &hook(&pr_marker));
    executable(&r.dir.path().join(".husky/_/post-checkout"));
    r.commit("pr adds a hook");
    let h = r.git(&["rev-parse", "HEAD"]).trim().to_string();
    r.git(&[
        "push",
        "-q",
        "-f",
        "origin",
        &format!("hooky:refs/pull/{}/head", fx.n),
    ]);
    r.git(&["checkout", "-q", "main"]);
    // main has no .husky; the user's own hook is an untracked file in their clone.
    r.write(".husky/_/post-checkout", &hook(&user_marker));
    executable(&r.dir.path().join(".husky/_/post-checkout"));
    r.git(&["config", "core.hooksPath", ".husky/_"]);
    (h, user_marker, pr_marker)
}

#[test]
fn hooks_never_run_when_the_pr_worktree_is_created() {
    let fx = Fx::new(true);
    let (hooky_head, user_marker, pr_marker) = hook_fixture(&fx);

    // Controls with plain git: the user's own relative hooksPath hook runs
    // during `worktree add`, and the PR's committed hook runs on a plain
    // checkout inside the worktree. So the assertions below would catch
    // an unhardened implementation.
    let control = fx.bare.path().join("control-wt");
    fx.repo.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        control.to_str().unwrap(),
        &hooky_head,
    ]);
    assert!(
        user_marker.exists(),
        "control: the user's hook should run on plain worktree add"
    );
    std::fs::remove_file(&user_marker).unwrap();
    fx.repo
        .git(&["worktree", "remove", "--force", control.to_str().unwrap()]);
    let control2 = fx.bare.path().join("control-wt2");
    fx.repo.git(&[
        "worktree",
        "add",
        "-q",
        "--no-checkout",
        "--detach",
        control2.to_str().unwrap(),
        &hooky_head,
    ]);
    Command::new("git")
        .arg("-C")
        .arg(&control2)
        .args(["checkout", "-q", "-f", &hooky_head, "--", "."])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        pr_marker.exists(),
        "control: the PR's committed hook should run on a plain checkout"
    );
    std::fs::remove_file(&pr_marker).unwrap();
    fx.repo
        .git(&["worktree", "remove", "--force", control2.to_str().unwrap()]);

    let s = fx.prepare(&fx.url).unwrap();
    assert_eq!(s.review.head_commit, hooky_head);
    assert!(
        s.worktree.path().join(".husky/_/post-checkout").exists(),
        "files are checked out"
    );
    assert!(s.worktree.path().join("shop.py").exists());
    assert!(!user_marker.exists(), "the user's hook ran with PR content");
    assert!(!pr_marker.exists(), "the PR's own hook ran");
}

#[test]
fn lfs_filters_of_the_users_config_do_not_run() {
    let fx = Fx::new(true);
    let marker = fx.bare.path().join("filter-ran");
    let r = &fx.repo;
    r.git(&["checkout", "-q", "-b", "attr", &fx.head]);
    r.write(".gitattributes", "*.py filter=lfs\n");
    r.commit("pr adds attributes");
    r.git(&[
        "push",
        "-q",
        "-f",
        "origin",
        &format!("attr:refs/pull/{}/head", fx.n),
    ]);
    r.git(&["checkout", "-q", "main"]);
    r.git(&[
        "config",
        "filter.lfs.smudge",
        &format!("sh -c 'echo ran >> {}; cat'", marker.display()),
    ]);
    r.git(&[
        "config",
        "filter.lfs.process",
        &format!("sh -c 'echo ran >> {}'", marker.display()),
    ]);
    // Control: plain git does run the configured filter on this checkout.
    let attr_head = r.git(&["rev-parse", "attr"]);
    let control = fx.bare.path().join("control-wt");
    r.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        control.to_str().unwrap(),
        attr_head.trim(),
    ]);
    assert!(
        marker.exists(),
        "control: the filter should run with plain git"
    );
    std::fs::remove_file(&marker).unwrap();
    r.git(&["worktree", "remove", "--force", control.to_str().unwrap()]);

    let s = fx.prepare(&fx.url).unwrap();
    assert!(s.worktree.path().join("shop.py").exists());
    assert!(!marker.exists(), "an LFS/smudge filter ran on PR content");
}

#[test]
fn hooks_never_run_when_the_kept_checkout_switches() {
    let fx = Fx::new(true);
    // The kept checkout exists, at the first head.
    drop(fx.prepare(&fx.url).unwrap());
    let (hooky_head, user_marker, pr_marker) = hook_fixture(&fx);
    let s = fx.prepare(&fx.url).unwrap();
    assert!(s.worktree.is_kept());
    assert_eq!(s.review.head_commit, hooky_head);
    assert!(s.worktree.path().join(".husky/_/post-checkout").exists());
    assert!(!user_marker.exists(), "the user's hook ran with PR content");
    assert!(!pr_marker.exists(), "the PR's own hook ran");
}

/// Git in `dir`, with the test's own environment.
fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn the_kept_checkout_switches_heads_and_drops_whatever_was_left_in_it() {
    let fx = Fx::new(true);
    let dir = {
        let s = fx.prepare(&fx.url).unwrap();
        s.worktree.path().to_path_buf()
    };
    // The PR gets a commit that adds a file.
    let r = &fx.repo;
    r.git(&["checkout", "-q", "-b", "grow", &fx.head]);
    r.write("extra.py", "def extra():\n    return 2\n")
        .commit("add extra");
    let grown = r.git(&["rev-parse", "HEAD"]).trim().to_string();
    let push = |sha: &str| {
        r.git(&[
            "push",
            "-q",
            "-f",
            "origin",
            &format!("{sha}:refs/pull/{}/head", fx.n),
        ]);
    };
    push(&grown);
    r.git(&["checkout", "-q", "main"]);
    let s = fx.prepare(&fx.url).unwrap();
    assert_eq!(s.worktree.path(), dir);
    assert_eq!(s.review.head_commit, grown);
    assert!(dir.join("extra.py").exists());
    assert!(
        s.review
            .files
            .iter()
            .any(|f| f.path == Path::new("extra.py"))
    );
    drop(s);

    // Leftovers of that run: an edit made with `e`, new files and folders, a
    // nested repository.
    std::fs::write(dir.join("shop.py"), "edited\n").unwrap();
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    std::fs::create_dir_all(dir.join("build/out")).unwrap();
    std::fs::write(dir.join("build/out/a.bin"), "x").unwrap();
    git_in(&dir, &["init", "-q", "nested"]);

    // The commit is dropped again (a force push).
    push(&fx.head);
    let s = fx.prepare(&fx.url).unwrap();
    assert_eq!(s.worktree.path(), dir);
    assert_eq!(s.review.head_commit, fx.head);
    assert_eq!(
        std::fs::read_to_string(dir.join("shop.py")).unwrap(),
        SHOP_PR
    );
    for gone in ["extra.py", "notes.txt", "build", "nested"] {
        assert!(!dir.join(gone).exists(), "{gone} is still there");
    }
    // Exactly the PR head, nothing else.
    assert_eq!(git_in(&dir, &["rev-parse", "HEAD"]).trim(), fx.head);
    assert_eq!(
        git_in(
            &dir,
            &[
                "status",
                "--porcelain",
                "--ignored",
                "--untracked-files=all"
            ]
        ),
        ""
    );
    assert_eq!(s.review.files.len(), 1);
    assert_eq!(fx.worktrees(), 2);
}

#[test]
fn a_broken_kept_checkout_is_rebuilt() {
    let fx = Fx::new(true);
    let dir = {
        let s = fx.prepare(&fx.url).unwrap();
        s.worktree.path().to_path_buf()
    };
    let check = |what: &str| {
        let s = fx
            .prepare(&fx.url)
            .unwrap_or_else(|e| panic!("{what}: {e:#}"));
        assert!(s.worktree.is_kept(), "{what}");
        assert_eq!(s.worktree.path(), dir, "{what}");
        assert_eq!(
            std::fs::read_to_string(dir.join("shop.py")).unwrap(),
            SHOP_PR,
            "{what}"
        );
        assert_eq!(fx.worktrees(), 2, "{what}");
    };

    // An interrupted run left git's index lock behind.
    let git_dir = PathBuf::from(git_in(&dir, &["rev-parse", "--absolute-git-dir"]).trim());
    std::fs::write(git_dir.join("index.lock"), "").unwrap();
    check("stale index lock");

    // The folder is gone but git still lists it.
    std::fs::remove_dir_all(&dir).unwrap();
    check("folder deleted");

    // A plain folder in its place that is not a worktree.
    fx.repo
        .git(&["worktree", "remove", "--force", dir.to_str().unwrap()]);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("junk.txt"), "x").unwrap();
    check("plain folder");
    assert!(!dir.join("junk.txt").exists());
}

#[tokio::test]
async fn agent_prompts_say_the_code_is_untrusted() {
    let fx = Fx::new(true);
    let mut s = fx.prepare(&fx.url).unwrap();
    let mock = MockAdapter::new(|_, _| Ok(json!({"units": []})));
    let prompts = mock.prompts.clone();
    let adapter = Arc::new(UntrustedCode(Arc::new(mock)));
    let session = SessionRef::fresh("mock", s.worktree.path().to_path_buf());
    teer::cli::prepare_all(&mut s.review, adapter, session, 2).await;
    let seen = prompts.lock().unwrap();
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|p| p.contains(UNTRUSTED_NOTICE)));
    assert!(seen.iter().all(|p| p.contains("def price")));
}

#[test]
fn decisions_persist_per_pr_and_carry_over_by_fingerprint() {
    let fx = Fx::new(true);
    let mut s = fx.prepare(&fx.url).unwrap();
    assert_eq!(decision(&s.review, "price"), Decision::Pending);
    decide(&mut s.review, "price", Decision::Investigate);
    store::save(&s.review).unwrap();
    drop(s);

    // Same PR again: restored.
    let s = fx.prepare(&fx.url).unwrap();
    assert_eq!(decision(&s.review, "price"), Decision::Investigate);
    drop(s);

    // The PR gets a new commit touching `tax` only: `price` carries over,
    // `tax` is new.
    let new_head = fx.push_new_head(&SHOP_PR.replace("0.2", "0.25"));
    let mut s = fx.prepare(&fx.url).unwrap();
    assert_eq!(s.review.head_commit, new_head);
    assert_ne!(new_head, fx.head);
    assert_eq!(decision(&s.review, "price"), Decision::Investigate);
    assert_eq!(decision(&s.review, "tax"), Decision::Pending);
    decide(&mut s.review, "tax", Decision::Accepted);
    store::save(&s.review).unwrap();
    drop(s);

    // `tax` changes again: that decision starts over, `price` still holds.
    let h2 = {
        let r = &fx.repo;
        r.git(&["checkout", "-q", "-b", "f3", &new_head]);
        r.write("shop.py", &SHOP_PR.replace("0.2", "0.3"))
            .commit("again");
        let h = r.git(&["rev-parse", "HEAD"]).trim().to_string();
        r.git(&[
            "push",
            "-q",
            "-f",
            "origin",
            &format!("f3:refs/pull/{}/head", fx.n),
        ]);
        r.git(&["checkout", "-q", "main"]);
        h
    };
    let s = fx.prepare(&fx.url).unwrap();
    assert_eq!(s.review.head_commit, h2);
    assert_eq!(decision(&s.review, "price"), Decision::Investigate);
    assert_eq!(decision(&s.review, "tax"), Decision::Pending);
}

#[test]
fn pr_and_local_reviews_never_see_each_other() {
    let fx = Fx::new(true);
    // The user's own uncommitted review of the same code, with a decision.
    fx.dirty();
    let mut local = build_review(&fx.path(), &BuildOptions::default()).unwrap();
    teer::cli::restore(&mut local);
    assert_eq!(decision(&local, "price"), Decision::Pending);
    decide(&mut local, "price", Decision::Investigate);
    store::save(&local).unwrap();

    // The PR has identical unit ids and fingerprints, yet must not inherit it.
    let mut s = fx.prepare(&fx.url).unwrap();
    assert_eq!(decision(&s.review, "price"), Decision::Pending);
    decide(&mut s.review, "price", Decision::Accepted);
    store::save(&s.review).unwrap();
    drop(s);

    // The local review still has its own decision, not the PR's.
    let mut local2 = build_review(&fx.path(), &BuildOptions::default()).unwrap();
    teer::cli::restore(&mut local2);
    assert_eq!(decision(&local2, "price"), Decision::Investigate);
    let latest = store::load_latest(&fx.path(), None).unwrap();
    assert!(latest.pr.is_none());
    assert_eq!(decision(&latest, "price"), Decision::Investigate);

    // And the PR has its own.
    let s = fx.prepare(&fx.url).unwrap();
    assert_eq!(decision(&s.review, "price"), Decision::Accepted);
}

#[test]
fn different_prs_have_different_stores() {
    let fx = Fx::new(true);
    let mut s = fx.prepare(&fx.url).unwrap();
    decide(&mut s.review, "price", Decision::Accepted);
    store::save(&s.review).unwrap();
    let info7 = s.review.pr.clone().unwrap();
    let mut info8 = info7.clone();
    info8.number = fx.n + 5000;
    assert_ne!(info7.store_key(), info8.store_key());
    assert!(store::load_latest(&fx.path(), Some(&info8)).is_none());
    assert!(store::load_latest(&fx.path(), Some(&info7)).is_some());
    // The PR's store does not depend on where the worktree is.
    let mut elsewhere = s.review.clone();
    elsewhere.repo_root = PathBuf::from("/somewhere/else");
    assert_eq!(
        store::review_path(&elsewhere.repo_root, Some(&info7), "b", "h", "c"),
        store::review_path(&s.review.repo_root, Some(&info7), "b", "h", "c"),
    );
}

// ---- the binary ----

fn teer(fx: &Fx, args: &[&str]) -> std::process::Output {
    let env = isolate_env();
    Command::new(teer_bin())
        .args(args)
        .arg("-C")
        .arg(fx.path())
        .env("TEER_DATA_DIR", env.path().join("data"))
        .env("TEER_CONFIG", env.path().join("config.toml"))
        .output()
        .unwrap()
}

#[test]
fn cli_json_review_of_a_pr() {
    let fx = Fx::new(true);
    fx.dirty();
    let out = teer(&fx, &["pr", &fx.url, "--json", "--no-agent"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    for step in [
        format!("fetching PR #{} from origin…", fx.n),
        format!("checking out PR #{} (1 file)…", fx.n),
        "reading the changes…".to_string(),
    ] {
        assert!(err.contains(&step), "{step}: {err}");
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["pr"]["number"], fx.n);
    assert_eq!(v["pr"]["owner"], "acme");
    assert_eq!(v["base"], fx.fork.as_str());
    assert_eq!(v["head"], fx.head.as_str());
    assert_eq!(v["files"].as_array().unwrap().len(), 1);
    assert_eq!(fx.worktrees(), 2);
    assert_eq!(fx.repo.file("scratch.txt"), "mine\n");

    // The next run switches the kept checkout instead of writing a new one.
    let out = teer(&fx, &["pr", &fx.url, "--json", "--no-agent"]);
    assert!(out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!("switching the checkout to PR #{}…", fx.n)),
        "{err}"
    );
    assert!(!err.contains("checking out"), "{err}");
    assert_eq!(fx.worktrees(), 2);
}

#[test]
fn cli_json_has_the_commits_and_the_title_gh_gives() {
    let fx = Fx::new(true);
    // gh missing (tests point TEER_GH at nothing): the commits alone.
    let out = teer(&fx, &["pr", &fx.url, "--json", "--no-agent"]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["intent"]["pr"].is_null(), "{}", v["intent"]);
    assert_eq!(v["intent"]["commits"][0]["subject"], "pr change");
    assert_eq!(v["intent"]["commits"][0]["sha"], fx.head.as_str());

    let d = tempfile::tempdir().unwrap();
    let gh = d.path().join("gh");
    std::fs::write(
        &gh,
        "#!/bin/sh\necho '{\"title\":\"Shop: charge in one place\",\"author\":{\"login\":\"jdoe\"},\"body\":\"Moves charging.\",\"additions\":3,\"deletions\":1,\"changedFiles\":1}'\n",
    )
    .unwrap();
    executable(&gh);
    let env = isolate_env();
    let out = Command::new(teer_bin())
        .args(["pr", &fx.url, "--json", "--no-agent", "-C"])
        .arg(fx.path())
        .env("TEER_DATA_DIR", env.path().join("data"))
        .env("TEER_CONFIG", env.path().join("config.toml"))
        .env("TEER_GH", &gh)
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["intent"]["pr"]["title"], "Shop: charge in one place");
    assert_eq!(v["intent"]["pr"]["author"], "jdoe");
    assert_eq!(v["intent"]["pr"]["size"]["additions"], 3);
}

#[test]
fn cli_errors_and_help() {
    let fx = Fx::new(true);
    let out = Command::new(teer_bin()).args(["pr"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Usage"));

    let out = Command::new(teer_bin()).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("pr "), "{help}");

    let out = Command::new(teer_bin())
        .args(["pr", "--upload-pack=x"])
        .output()
        .unwrap();
    assert!(!out.status.success());

    let out = teer(
        &fx,
        &["pr", "https://github.com/acme/shop/issues/7", "--no-agent"],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a pull request URL"));

    let out = teer(&fx, &["pr", "https://github.com/x/y/pull/1", "--no-agent"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Run teer from a clone of x/y"));

    let out = teer(&fx, &["pr", &fx.url, "--base", "main", "--no-agent"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--base"));
}

fn header_of(r: &Review) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use teer::config::{Config, KeyMap};
    use teer::tui::{app::App, ui};
    let app = App::new(r.clone(), Config::default(), KeyMap::default());
    let mut t = Terminal::new(TestBackend::new(120, 20)).unwrap();
    t.draw(|f| ui::draw(f, &app)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..120).map(|x| buf[(x, 0)].symbol()).collect::<String>()
}

#[test]
fn the_header_marks_an_assumed_base() {
    // With a merge ref the base is the merge base, shown as a short sha.
    let fx = Fx::new(true);
    let s = fx.prepare(&fx.url).unwrap();
    assert_eq!(s.review.pr.as_ref().unwrap().base_assumed, None);
    let h = header_of(&s.review);
    // The PR's only commit names it while gh isn't there.
    assert!(
        h.contains(&format!("PR #{} · pr change · teer · +", fx.n)),
        "{h}"
    );
    assert!(
        h.contains(&format!(
            "· acme/shop · {}..{}",
            &fx.fork[..7],
            &fx.head[..7]
        )),
        "{h}"
    );
    assert!(!h.contains("main*"), "{h}");
    drop(s);

    // Without one the default branch is assumed and the header says so.
    let fx = Fx::new(false);
    let s = fx.prepare(&fx.url).unwrap();
    assert_eq!(
        s.review.pr.as_ref().unwrap().base_assumed.as_deref(),
        Some("main")
    );
    let h = header_of(&s.review);
    assert!(
        h.contains(&format!("· acme/shop · main*..{}", &fx.head[..7])),
        "{h}"
    );
    // It does not change the PR's identity (store key).
    let mut plain = s.review.pr.clone().unwrap();
    plain.base_assumed = None;
    assert_eq!(plain.store_key(), s.review.pr.as_ref().unwrap().store_key());
}

#[test]
fn the_port_is_part_of_the_prs_identity_but_not_of_old_keys() {
    let a = pr::parse_url("https://git.corp.example/team/svc/pull/5").unwrap();
    let b = pr::parse_url("https://git.corp.example:8443/team/svc/pull/5").unwrap();
    let c = pr::parse_url("https://git.corp.example:9443/team/svc/pull/5").unwrap();
    assert_eq!(a.port, None);
    assert_ne!(a.store_key(), b.store_key());
    assert_ne!(b.store_key(), c.store_key());
    // Remote matching stays host-based (an ssh remote has another port).
    assert!(pr::remote_url_matches(
        "git@git.corp.example:team/svc.git",
        &b
    ));
}

#[tokio::test]
async fn untrusted_reviews_never_get_web_tools() {
    use teer::agent::AgentAdapter;
    let mock = MockAdapter::new(|_, _| Ok(json!({"text": "t", "sources": []})));
    let web_calls = mock.web_calls.clone();
    let adapter = UntrustedCode(Arc::new(mock));
    let s = SessionRef::fresh("mock", PathBuf::from("/x"));
    adapter
        .ask_dive(&s, "DIVE:Visualize\n\nq", &json!({}), true)
        .await
        .unwrap();
    adapter
        .ask_dive(&s, "DIVE:Recommend\n\nq", &json!({}), true)
        .await
        .unwrap();
    assert_eq!(*web_calls.lock().unwrap(), [false, false]);
}

#[cfg(unix)]
fn fake_claude_dir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let bin = d.path().join("claude");
    std::fs::write(
        &bin,
        "#!/bin/sh\ntouch \"$(dirname \"$0\")/started\"\nif [ \"$1\" = \"--version\" ]; then echo 9.9.9; exit 0; fi\ncat >/dev/null\nexec sleep 25\n",
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    d
}

/// Starts `teer pr --json` with a "claude" that never answers, waits until it
/// sits in the agent phase (claude was started, so the signal handlers are
/// in place) with `worktrees` worktrees listed, sends `sig` and returns its
/// exit code.
#[cfg(unix)]
fn signal_teer(fx: &Fx, sig: &str, worktrees: usize) -> Option<i32> {
    let fake = fake_claude_dir();
    let env = isolate_env();
    let path = format!(
        "{}:{}",
        fake.path().display(),
        std::env::var("PATH").unwrap()
    );
    let mut child = Command::new(teer_bin())
        .args(["pr", &fx.url, "--json", "-C"])
        .arg(fx.path())
        .env("PATH", path)
        .env("TEER_DATA_DIR", env.path().join("data"))
        .env("TEER_CONFIG", env.path().join("config.toml"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    while fx.worktrees() < worktrees || !fake.path().join("started").exists() {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(30),
            "teer never reached the agent"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Command::new("kill")
        .arg(format!("-{sig}"))
        .arg(child.id().to_string())
        .status()
        .unwrap();
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s.code();
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(40),
            "{sig}: teer did not exit"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[cfg(unix)]
#[test]
fn signals_remove_a_throwaway_worktree_and_exit_with_128_plus_the_signal() {
    for (sig, code) in [("TERM", 143), ("HUP", 129), ("INT", 130)] {
        let fx = Fx::new(true);
        // This process holds the kept checkout, so teer gets a throwaway one.
        let held = fx.prepare(&fx.url).unwrap();
        assert_eq!(signal_teer(&fx, sig, 3), Some(code), "{sig}");
        assert_eq!(fx.worktrees(), 2, "{sig}: throwaway worktree left behind");
        assert!(held.worktree.path().join("shop.py").exists());
    }
}

#[cfg(unix)]
#[test]
fn a_signal_leaves_the_kept_checkout_and_releases_its_lock() {
    let fx = Fx::new(true);
    assert_eq!(signal_teer(&fx, "INT", 2), Some(130));
    assert_eq!(fx.worktrees(), 2);
    let s = fx.prepare(&fx.url).unwrap();
    assert!(s.worktree.is_kept(), "the lock died with the process");
    assert_eq!(fx.worktrees(), 2);
}

#[test]
fn exit_codes_follow_the_shell_convention() {
    assert_eq!(teer::cli::exit_code_for_signal(15), 143);
    assert_eq!(teer::cli::exit_code_for_signal(1), 129);
    assert_eq!(teer::cli::exit_code_for_signal(2), 130);
}

#[cfg(unix)]
#[test]
fn a_stalled_fetch_times_out_and_leaves_nothing_behind() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new(true);
    let script = fx.bare.path().join("slow-upload-pack.sh");
    std::fs::write(&script, "#!/bin/sh\nexec sleep 20\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    fx.repo.git(&[
        "config",
        "remote.origin.uploadpack",
        script.to_str().unwrap(),
    ]);
    let env = isolate_env();
    let start = std::time::Instant::now();
    let out = Command::new(teer_bin())
        .args(["pr", &fx.url, "--json", "--no-agent", "-C"])
        .arg(fx.path())
        .env("TEER_FETCH_TIMEOUT", "2")
        .env("TEER_DATA_DIR", env.path().join("data"))
        .env("TEER_CONFIG", env.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(
        start.elapsed() < std::time::Duration::from_secs(15),
        "took {:?}",
        start.elapsed()
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("timed out after 2s"), "{err}");
    assert_eq!(fx.worktrees(), 1);
}
