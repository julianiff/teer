//! Phase 7, the fix loop, against real git in temp repos: notes handed to a
//! fake agent CLI that edits the code and exits (the signal), notes copied
//! and the working tree going quiet, `q` while waiting, what a re-run keeps
//! and what it brings back, notes on changed code, `:e` and `:e!` (on a
//! pull request, fetched again).

use crate::common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::v2::{press, screen};
use common::{FixtureRepo, isolate_env};
use teer::approve::Tools;
use teer::config::{Config, Provider};
use teer::keymap::Keymap;
use teer::review::model::{Decision, PrInfo};
use teer::review::{BuildOptions, build_review};
use teer::tui::Runtime;
use teer::tui::app::{App, AppEvent, Effect, Overlay};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const OLD: &str = "def total(items):
    return sum(items)


def fee(amount):
    return amount * 2


def tax(x):
    return x * 0.2


def shape(a, b):
    return {'a': a, 'b': b}
";

/// The change under review: three functions changed, one reformatted.
const NEW: &str = "def total(items):
    return sum(items) + fee(sum(items))


def fee(amount):
    return amount * 3


def tax(x):
    return x * 0.25


def shape(a, b):
    return {
        'a': a,
        'b': b,
    }
";

/// What the agent makes of the note on `fee`: only `fee` changes.
const FIXED: &str = "def total(items):
    return sum(items) + fee(sum(items))


def fee(amount):
    return amount * 2 + 1


def tax(x):
    return x * 0.25


def shape(a, b):
    return {
        'a': a,
        'b': b,
    }
";

const TOTAL: &str = "calc.py::total";
const FEE: &str = "calc.py::fee";
const TAX: &str = "calc.py::tax";
const SHAPE: &str = "calc.py::shape";

/// A repository with `NEW` uncommitted over `OLD`.
fn repo() -> FixtureRepo {
    isolate_env();
    let r = FixtureRepo::new();
    r.write("calc.py", OLD).commit("init");
    r.write("calc.py", NEW);
    r
}

/// `teer` on `dir` (no `--base`), opened as without an agent.
fn open(dir: &Path, base: Option<String>, cfg: Config) -> App {
    let opts = BuildOptions {
        base,
        languages: None,
    };
    let review = build_review(dir, &opts).unwrap();
    let mut a = App::new(review, cfg, Keymap::default());
    a.start(false, None, Vec::new(), None);
    a
}

/// The runtime that carries out the effects, and what it sends back.
fn runtime(tools: Tools, base: Option<String>) -> (Runtime, UnboundedReceiver<AppEvent>) {
    let (tx, rx) = unbounded_channel();
    let build = BuildOptions {
        base,
        languages: None,
    };
    let rt = Runtime {
        adapter: None,
        tx,
        build,
        tools,
    };
    (rt, rx)
}

/// Carry out the re-run `effects` ask for and hand the new review to the
/// app; what it returns then.
async fn rerun(
    a: &mut App,
    rt: &Runtime,
    rx: &mut UnboundedReceiver<AppEvent>,
    effects: Vec<Effect>,
) -> Vec<Effect> {
    assert!(effects.contains(&Effect::Rerun), "{effects:?}");
    rt.exec(a, Effect::Rerun);
    reran(a, rx).await
}

/// Hand the re-run's new review, once it comes, to the app (and anything
/// that comes before it); what the app returns then.
async fn reran(a: &mut App, rx: &mut UnboundedReceiver<AppEvent>) -> Vec<Effect> {
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("the re-run finishes")
            .unwrap();
        let rerun = matches!(ev, AppEvent::Rerun(_));
        let effects = a.on_app_event(ev);
        if rerun {
            return effects;
        }
    }
}

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

/// A fake Claude Code: it writes its arguments to `args` next to it, puts
/// `FIXED` in `calc.py` where it runs, and exits.
fn fake_claude(dir: &Path) -> PathBuf {
    let args = dir.join("args");
    script(
        dir,
        "claude",
        &format!(
            "printf '%s\\n' \"$@\" > '{}'\ncat > calc.py <<'EOF'\n{FIXED}EOF",
            args.display()
        ),
    )
}

fn select(a: &mut App, id: &str) {
    a.sel = a
        .order
        .iter()
        .position(|&i| a.review.units[i].id == id)
        .unwrap_or_else(|| panic!("{id} is listed"));
}

fn decision(a: &App, id: &str) -> Decision {
    a.review.unit(id).expect("unit exists").decision
}

/// `fee` flagged with a note, `total` and `tax` accepted.
fn reviewed(a: &mut App) {
    for id in [TOTAL, TAX] {
        select(a, id);
        press(a, "a");
    }
    select(a, FEE);
    press(a, "iadd one instead of doubling<cr>f");
    assert_eq!(decision(a, FEE), Decision::Investigate);
}

// ---- L1, L3: the hand-off, its exit the signal ---------------------------------

#[tokio::test]
async fn the_agent_exiting_reruns_the_review_on_what_it_changed() {
    let r = repo();
    let mut a = open(&r.path(), None, Config::default());
    reviewed(&mut a);
    let e = press(&mut a, ":w<cr><cr>");
    let [
        Effect::OpenAgent {
            cli,
            cwd,
            prompt,
            resume,
        },
    ] = &e[..]
    else {
        panic!("{e:?}")
    };
    assert_eq!(*cli, Provider::Claude);
    let d = tempfile::tempdir().unwrap();
    let tools = Tools {
        claude: fake_claude(d.path()).into(),
        ..Default::default()
    };
    let exit = teer::tui::run_agent(&tools, *cli, cwd, prompt, resume.as_deref()).unwrap();
    assert!(exit.success());
    let args = std::fs::read_to_string(d.path().join("args")).unwrap();
    assert!(args.contains("> add one instead of doubling"), "{args}");
    assert_eq!(r.file("calc.py"), FIXED);

    // Back in teer, as the run loop does: no key, the review re-runs.
    let (rt, mut rx) = runtime(tools, None);
    let quit = rt.agent_exited(&mut a, *cli, Ok(exit));
    assert!(!quit);
    assert_eq!(a.status, "Back from Claude Code. Re-running the review…");
    let after = reran(&mut a, &mut rx).await;
    assert!(after.contains(&Effect::Save), "{after:?}");
    assert_eq!(a.status, "1 unit changed by Claude Code.");
    assert_eq!(a.selected_id().as_deref(), Some(FEE));
    let fee = a.review.unit(FEE).unwrap();
    assert_eq!(fee.decision, Decision::Pending);
    assert_eq!(fee.dive.note, "add one instead of doubling");
    assert!(fee.note_is_stale());
    assert_eq!(decision(&a, TOTAL), Decision::Accepted);
    assert_eq!(decision(&a, TAX), Decision::Accepted);
    // The queue shows the note as written on other code.
    let s = screen(&a, 140, 40);
    assert!(s.contains("the code changed since"), "{s}");
    assert!(s.contains("1 unit changed by Claude Code."), "{s}");
}

/// `NEW` committed on `feature`, the tree clean: plain `teer` reviews the
/// branch since `main`.
fn branch_repo() -> FixtureRepo {
    isolate_env();
    let r = FixtureRepo::new();
    r.write("calc.py", OLD).commit("init");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("calc.py", NEW).commit("charge a fee");
    r
}

/// A branch review covers what isn't committed too: the agent fixes the
/// note without committing, and the re-run (as `tui::run` builds it, from
/// the review's base) shows the fix, its unit pending again, on the same
/// review.
#[tokio::test]
async fn a_branch_review_reruns_on_fixes_that_are_not_committed() {
    let r = branch_repo();
    let mut a = open(&r.path(), Some("main".into()), Config::default());
    assert_ne!(a.review.head, teer::git::WORKTREE);
    let fee_before = a.review.unit(FEE).unwrap().fingerprint.clone();
    let key = |a: &App| {
        let r = &a.review;
        (
            r.id.clone(),
            r.base.clone(),
            r.head.clone(),
            r.head_commit.clone(),
        )
    };
    let before = key(&a);
    reviewed(&mut a);
    let e = press(&mut a, ":w<cr><cr>");
    let [
        Effect::OpenAgent {
            cli,
            cwd,
            prompt,
            resume,
        },
    ] = &e[..]
    else {
        panic!("{e:?}")
    };
    let d = tempfile::tempdir().unwrap();
    let tools = Tools {
        claude: fake_claude(d.path()).into(),
        ..Default::default()
    };
    let exit = teer::tui::run_agent(&tools, *cli, cwd, prompt, resume.as_deref()).unwrap();
    assert_eq!(r.file("calc.py"), FIXED);
    assert!(
        r.git(&["status", "--porcelain"]).contains("calc.py"),
        "not committed"
    );

    let (rt, mut rx) = runtime(tools, Some(a.review.base.clone()));
    assert!(!rt.agent_exited(&mut a, *cli, Ok(exit)));
    reran(&mut a, &mut rx).await;
    assert_eq!(a.status, "1 unit changed by Claude Code.");
    assert_eq!(key(&a), before, "the same review, saved where it was");
    let file = &a.review.files[0];
    assert_eq!(file.new_content.as_deref(), Some(FIXED));
    let fee = a.review.unit(FEE).unwrap();
    assert_ne!(fee.fingerprint, fee_before);
    assert_eq!(fee.decision, Decision::Pending);
    assert!(fee.note_is_stale());
    assert_eq!(a.selected_id().as_deref(), Some(FEE));
    assert_eq!(decision(&a, TOTAL), Decision::Accepted);
    assert_eq!(decision(&a, TAX), Decision::Accepted);
    // The header still names the branch's commits.
    assert_eq!(a.review.intent.commits.len(), 1);
}

#[tokio::test]
async fn the_agent_resumes_a_fork_of_the_found_session() {
    let r = repo();
    let mut a = open(&r.path(), None, Config::default());
    a.review.agent_session = Some(teer::review::model::SessionRef {
        agent: "claude".into(),
        id: "abc123".into(),
        cwd: r.path(),
    });
    reviewed(&mut a);
    let e = press(&mut a, ":w<cr><cr>");
    let [
        Effect::OpenAgent {
            cli,
            cwd,
            prompt,
            resume,
        },
    ] = &e[..]
    else {
        panic!("{e:?}")
    };
    assert_eq!(resume.as_deref(), Some("abc123"));
    let d = tempfile::tempdir().unwrap();
    let tools = Tools {
        claude: fake_claude(d.path()).into(),
        ..Default::default()
    };
    teer::tui::run_agent(&tools, *cli, cwd, prompt, resume.as_deref()).unwrap();
    let args = std::fs::read_to_string(d.path().join("args")).unwrap();
    assert!(
        args.starts_with("--resume\nabc123\n--fork-session\n"),
        "{args}"
    );
}

#[tokio::test]
async fn an_agent_that_cannot_start_reruns_nothing_and_says_why() {
    let r = repo();
    let mut a = open(&r.path(), None, Config::default());
    reviewed(&mut a);
    let e = press(&mut a, ":w<cr><cr>");
    let [
        Effect::OpenAgent {
            cli, cwd, prompt, ..
        },
    ] = &e[..]
    else {
        panic!("{e:?}")
    };
    let tools = Tools {
        claude: r.path().join("no-such-claude").into(),
        ..Default::default()
    };
    let ran = teer::tui::run_agent(&tools, *cli, cwd, prompt, None);
    assert!(ran.is_err());
    let (rt, mut rx) = runtime(tools, None);
    assert!(!rt.agent_exited(&mut a, *cli, ran));
    assert!(
        a.status.starts_with("Couldn't start Claude Code"),
        "{}",
        a.status
    );
    let none = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
    assert!(none.is_err(), "nothing re-runs");
    assert_eq!(decision(&a, FEE), Decision::Investigate);
}

// ---- L2, L3: copied notes, the files going quiet, `q` --------------------------

/// Look at the tree while the app waits, as the runtime's tick does, until
/// it asks for a re-run; the effects that ask.
async fn wait_for_quiet(
    a: &mut App,
    rt: &Runtime,
    rx: &mut UnboundedReceiver<AppEvent>,
    edit: impl FnOnce(),
) -> Vec<Effect> {
    let mut edit = Some(edit);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut looks = 0;
    while Instant::now() < deadline {
        rt.watch(a, Instant::now());
        if let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
            looks += 1;
            let effects = a.on_app_event(ev);
            if effects.contains(&Effect::Rerun) {
                assert!(looks >= 3, "a baseline, the change, then quiet");
                return effects;
            }
            // The agent edits once teer has had its first look.
            if let Some(edit) = edit.take() {
                edit();
            }
        }
    }
    panic!("the files never went quiet");
}

#[tokio::test]
async fn copied_notes_wait_for_the_files_to_go_quiet_then_rerun() {
    let r = repo();
    let mut cfg = Config::default();
    cfg.fix_loop.quiet_seconds = 0.3;
    let mut a = open(&r.path(), None, cfg);
    reviewed(&mut a);
    // `y` on the sheet: copied, the sheet closed, teer waiting on the queue.
    let e = press(&mut a, ":w<cr>y");
    assert!(matches!(&e[..], [Effect::Copy { .. }]), "{e:?}");
    assert_eq!(a.overlay, Overlay::None);
    assert!(a.waiting.is_some());
    let s = screen(&a, 140, 40);
    assert!(
        s.contains("Waiting for the changes to the note… (q to stop waiting)"),
        "{s}"
    );
    // Read-only meanwhile.
    press(&mut a, "a");
    assert_eq!(decision(&a, FEE), Decision::Investigate);

    let (rt, mut rx) = runtime(Tools::default(), None);
    let path = r.path();
    let effects = wait_for_quiet(&mut a, &rt, &mut rx, || {
        std::fs::write(path.join("calc.py"), FIXED).unwrap();
    })
    .await;
    assert!(a.waiting.is_none());
    rerun(&mut a, &rt, &mut rx, effects).await;
    assert_eq!(a.status, "1 unit changed since the notes were copied.");
    assert_eq!(a.selected_id().as_deref(), Some(FEE));
    assert_eq!(decision(&a, FEE), Decision::Pending);
}

#[tokio::test]
async fn the_model_api_copies_the_notes_and_waits_too() {
    let r = repo();
    let cfg = Config {
        provider: Some(Provider::Api),
        ..Config::default()
    };
    let mut a = open(&r.path(), None, cfg);
    reviewed(&mut a);
    press(&mut a, ":w<cr>");
    let s = screen(&a, 120, 40);
    assert!(
        s.contains("copy the notes for your agent and wait for the changes"),
        "{s}"
    );
    let e = press(&mut a, "<cr>");
    let [Effect::Copy { text, .. }] = &e[..] else {
        panic!("{e:?}")
    };
    assert!(text.starts_with("These are my review notes"), "{text}");
    assert!(a.waiting.is_some());
}

#[tokio::test]
async fn q_stops_waiting_nothing_reruns_and_colon_e_does_later() {
    let r = repo();
    let mut a = open(&r.path(), None, Config::default());
    reviewed(&mut a);
    press(&mut a, ":w<cr>j<cr>");
    assert!(a.waiting.is_some());
    let e = press(&mut a, "q");
    assert!(!e.contains(&Effect::Quit), "{e:?}");
    assert!(a.waiting.is_none());
    assert_eq!(a.watch_due(Instant::now() + Duration::from_secs(60)), None);
    // The fix comes in; `:e` picks it up, still the copied notes' changes.
    r.write("calc.py", FIXED);
    let e = press(&mut a, ":e<cr>");
    let (rt, mut rx) = runtime(Tools::default(), None);
    rerun(&mut a, &rt, &mut rx, e).await;
    assert_eq!(a.status, "1 unit changed since the notes were copied.");
}

#[tokio::test]
async fn a_pull_request_copies_without_waiting() {
    let (o, mut a) = pr_review();
    select(&mut a, FEE);
    press(&mut a, "iwhy three?<cr>f");
    let e = press(&mut a, ":w<cr>y");
    assert!(matches!(&e[..], [Effect::Copy { .. }]), "{e:?}");
    assert!(a.waiting.is_none());
    assert!(matches!(a.overlay, Overlay::Finish { .. }));
    drop(o);
}

// ---- L4, L6: what a re-run keeps; notes on changed code -------------------------

#[tokio::test]
async fn notes_on_changed_code_stay_until_acted_on() {
    let r = repo();
    let mut a = open(&r.path(), None, Config::default());
    reviewed(&mut a);
    select(&mut a, TOTAL);
    press(&mut a, "ishould total include the fee?<cr>");
    assert_eq!(decision(&a, TOTAL), Decision::Accepted);
    // The agent changes `fee` and `total`.
    r.write(
        "calc.py",
        &FIXED.replace("sum(items) + fee", "sum(items) + 2 * fee"),
    );
    let e = press(&mut a, ":e<cr>");
    let (rt, mut rx) = runtime(Tools::default(), None);
    rerun(&mut a, &rt, &mut rx, e).await;
    assert_eq!(a.status, "2 units changed since the last run.");
    for id in [FEE, TOTAL] {
        let u = a.review.unit(id).unwrap();
        assert_eq!(u.decision, Decision::Pending, "{id}");
        assert!(u.note_is_stale(), "{id}");
    }
    // Both lead their level.
    let first: Vec<&str> = a.order[..2]
        .iter()
        .map(|&i| a.review.units[i].id.as_str())
        .collect();
    assert!(first.contains(&FEE) && first.contains(&TOTAL), "{first:?}");
    // The notes go out again, saying the code changed.
    let md = teer::notes::markdown(&a.review);
    assert!(md.contains("_The code changed after this note"), "{md}");

    // `a` accepts and puts the note away: kept on the unit, out of the notes.
    select(&mut a, FEE);
    press(&mut a, "a");
    let fee = a.review.unit(FEE).unwrap();
    assert_eq!(fee.decision, Decision::Accepted);
    assert!(fee.dive.note.is_empty());
    assert_eq!(fee.dive.archived[0].text, "add one instead of doubling");
    assert!(!teer::notes::markdown(&a.review).contains("add one"));
    // `u` brings it back.
    press(&mut a, "u");
    let fee = a.review.unit(FEE).unwrap();
    assert_eq!(fee.dive.note, "add one instead of doubling");
    assert!(fee.dive.archived.is_empty());
    // Saved and loaded, the archive stays with the unit.
    press(&mut a, "<c-r>");
    let json = serde_json::to_string(&a.review).unwrap();
    let back: teer::review::model::Review = serde_json::from_str(&json).unwrap();
    assert_eq!(back.unit(FEE).unwrap().dive.archived.len(), 1);

    // `i` edits the note on `total` without deciding.
    select(&mut a, TOTAL);
    press(&mut a, "i<c-u>fine now<cr>");
    let total = a.review.unit(TOTAL).unwrap();
    assert_eq!(total.decision, Decision::Pending);
    assert_eq!(total.dive.note, "fine now");
    assert!(!total.note_is_stale());
    // `f` flags it and leaves the note as it is.
    press(&mut a, "f");
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(decision(&a, TOTAL), Decision::Investigate);
    assert_eq!(a.review.unit(TOTAL).unwrap().dive.note, "fine now");
}

// ---- L7: `:e` and `:e!` -------------------------------------------------------------

#[tokio::test]
async fn colon_e_bang_asks_then_resets_everything_and_reruns() {
    let r = repo();
    let mut a = open(&r.path(), None, Config::default());
    reviewed(&mut a);
    // The reformatted unit was accepted for you.
    assert!(a.review.unit(SHAPE).unwrap().auto_accepted());
    let n = a.review.units.len();
    press(&mut a, ":e!<cr>");
    assert_eq!(a.overlay, Overlay::ConfirmReset);
    let s = screen(&a, 100, 30);
    assert!(s.contains(&format!("Reset all {n} decisions? y/n")), "{s}");
    let e = press(&mut a, "y");
    assert_eq!(e, vec![Effect::Save, Effect::Rerun]);
    let (rt, mut rx) = runtime(Tools::default(), None);
    rerun(&mut a, &rt, &mut rx, e).await;
    for id in [TOTAL, FEE, TAX] {
        assert_eq!(decision(&a, id), Decision::Pending, "{id}");
    }
    // Weighed again on the code as it is: still no behavior change.
    assert!(a.review.unit(SHAPE).unwrap().auto_accepted());
    assert_eq!(a.status, "Nothing changed since the last run.");
    // `u` brings the decisions back.
    press(&mut a, "u");
    assert_eq!(decision(&a, TOTAL), Decision::Accepted);
    assert_eq!(decision(&a, FEE), Decision::Investigate);
}

/// A pull request #7 of a clone with a bare `origin`, checked out detached
/// in a worktree of its own as teer keeps it, and reviewed there.
struct Pr {
    clone: FixtureRepo,
    _origin: tempfile::TempDir,
    checkout: tempfile::TempDir,
}

const PR: u32 = 7;

impl Pr {
    fn head(&self) -> String {
        self.clone
            .git(&["rev-parse", "refs/heads/pr"])
            .trim()
            .to_string()
    }

    /// The author pushes `text` to the PR.
    fn push(&self, text: &str) {
        self.clone.git(&["checkout", "-q", "pr"]);
        self.clone.write("calc.py", text).commit("more");
        self.clone.git(&[
            "push",
            "-q",
            "-f",
            "origin",
            &format!("pr:refs/pull/{PR}/head"),
        ]);
        self.clone.git(&["checkout", "-q", "main"]);
    }
}

fn pr_review() -> (Pr, App) {
    isolate_env();
    let clone = FixtureRepo::new();
    clone.write("calc.py", OLD).commit("init");
    let origin = tempfile::tempdir().unwrap();
    let url = origin.path().canonicalize().unwrap();
    let url = url.to_str().unwrap();
    clone.git(&["init", "-q", "--bare", "-b", "main", url]);
    clone.git(&["remote", "add", "origin", url]);
    clone.git(&["push", "-q", "origin", "main"]);
    clone.git(&["checkout", "-q", "-b", "pr"]);
    clone.write("calc.py", NEW).commit("pr");
    clone.git(&["push", "-q", "origin", &format!("pr:refs/pull/{PR}/head")]);
    clone.git(&["checkout", "-q", "main"]);
    let checkout = tempfile::tempdir().unwrap();
    let dir = checkout.path().join("checkout");
    let pr = Pr {
        clone,
        _origin: origin,
        checkout,
    };
    let head = pr.head();
    pr.clone.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        dir.to_str().unwrap(),
        &head,
    ]);
    let base = pr
        .clone
        .git(&["merge-base", "main", &head])
        .trim()
        .to_string();
    let mut a = open(&dir, Some(base), Config::default());
    a.review.pr = Some(PrInfo {
        host: "github.com".into(),
        owner: "acme".into(),
        repo: "shop".into(),
        number: PR,
        remote: Some("origin".into()),
        head_sha: Some(head),
        repo_dir: Some(pr.clone.path()),
        ..Default::default()
    });
    (pr, a)
}

#[tokio::test]
async fn colon_e_on_a_pull_request_fetches_the_head_it_has_now() {
    let (pr, mut a) = pr_review();
    let first = pr.head();
    for id in [TOTAL, FEE, TAX] {
        select(&mut a, id);
        press(&mut a, "a");
    }
    // The author pushes a fix to `fee`.
    pr.push(FIXED);
    let pushed = pr.head();
    assert_ne!(pushed, first);
    let e = press(&mut a, ":e<cr>");
    assert_eq!(e, vec![Effect::Rerun]);
    assert_eq!(
        a.status,
        format!("Fetching PR #{PR} and re-running the review…")
    );
    let (rt, mut rx) = runtime(Tools::default(), None);
    rerun(&mut a, &rt, &mut rx, e).await;
    let info = a.review.pr.as_ref().unwrap();
    assert_eq!(info.head_sha.as_deref(), Some(pushed.as_str()));
    assert_eq!(info.repo_dir.as_deref(), Some(pr.clone.path().as_path()));
    // The checkout moved to it, and only `fee` comes back.
    let dir = pr.checkout.path().join("checkout");
    let at = std::process::Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&at.stdout).trim(), pushed);
    assert_eq!(a.status, "1 unit changed since the last run.");
    assert_eq!(decision(&a, FEE), Decision::Pending);
    assert_eq!(decision(&a, TOTAL), Decision::Accepted);
    assert_eq!(a.selected_id().as_deref(), Some(FEE));
}
