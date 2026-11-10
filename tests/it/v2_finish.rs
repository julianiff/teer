//! Phase 6, the `:w` sheet (F1–F9 of `docs/v2-map.md` §11, and the
//! acceptance scenarios "agent changes, nothing wrong", "PR, approve", "PR,
//! request changes" and "pending warning"): one default next step for the
//! decisions made, its alternatives, the notes, and the safety checks kept
//! from v1. Keys go in as Neovim notation; commits run real git in temp
//! repos (with the repo's own hooks), posts go through the runtime to a
//! recording `gh` script and a local bare "remote". No model is called: a
//! drafted commit message comes in as the event the runtime would send.
//!
//! Names the sheet's code may choose differently are kept in the helpers
//! right below, so they can be adapted in one place.

use crate::common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::sandbox::script;
use common::v2::{Driver, review, unit};
use common::{FixtureRepo, isolate_env};
use teer::approve::{EditOutcome, Tools, Verdict};
use teer::commit::{self, Io, Outcome};
use teer::config::{Config, Provider};
use teer::git::{ChangedFile, FileStatus};
use teer::review::model::{BranchNames, Decision, PrInfo, Range, Review, Unit};
use teer::review::{BuildOptions, build_review};
use teer::session::Runtime;
use teer::session::app::{AppEvent, Effect, Overlay, Screen};

use Decision::{Accepted as A, Investigate as F, Pending as P};

// ---- Guesses about names the sheet's code chooses ------------------------------

/// Is the `:w` sheet open?
fn sheet_open(d: &Driver) -> bool {
    matches!(d.overlay(), Overlay::Finish { .. })
}

/// What the sheet says first while units are pending.
fn units_pending(n: usize) -> String {
    format!("{n} units pending")
}

/// Answer the confirm a step asks for while units are pending.
fn confirm(d: &mut Driver) -> Vec<Effect> {
    d.keys("y")
}

/// The commit `effects` ask for: the paths and the message.
fn commit_of(effects: &[Effect]) -> Option<(Vec<PathBuf>, String)> {
    effects.iter().find_map(|e| match e {
        Effect::Commit { paths, message, .. } => Some((paths.clone(), message.clone())),
        _ => None,
    })
}

/// The review `effects` post: the verdict, the summary, the inline comments.
fn post_of(effects: &[Effect]) -> Option<(Verdict, String, usize)> {
    effects.iter().find_map(|e| match e {
        Effect::PostReview {
            verdict,
            body,
            inline,
            ..
        } => Some((*verdict, body.clone(), inline.len())),
        _ => None,
    })
}

/// What `effects` put on the clipboard.
fn copied(effects: &[Effect]) -> Option<String> {
    effects.iter().find_map(|e| match e {
        Effect::Copy { text, .. } => Some(text.clone()),
        _ => None,
    })
}

/// The agent CLI `effects` hand the notes to, and its first message.
fn handed_over(effects: &[Effect]) -> Option<(Provider, String)> {
    effects.iter().find_map(|e| match e {
        Effect::OpenAgent { cli, prompt, .. } => Some((*cli, prompt.clone())),
        _ => None,
    })
}

/// The text `effects` open in the configured editor (`i`, Q12).
fn editing(effects: &[Effect]) -> Option<String> {
    effects.iter().find_map(|e| match e {
        Effect::EditComment { text } => Some(text.clone()),
        _ => None,
    })
}

/// teer exits: the one line it leaves (the status as it quits), when
/// `effects` quit.
fn exit_line(d: &Driver, effects: &[Effect]) -> Option<String> {
    effects
        .contains(&Effect::Quit)
        .then(|| d.status().to_string())
}

/// Effects that do something outside the queue.
fn outward(effects: &[Effect]) -> Vec<Effect> {
    effects
        .iter()
        .filter(|e| {
            matches!(
                e,
                Effect::Commit { .. }
                    | Effect::PostReview { .. }
                    | Effect::OpenAgent { .. }
                    | Effect::Copy { .. }
                    | Effect::Quit
            )
        })
        .cloned()
        .collect()
}

// ---- Driving the sheet ------------------------------------------------------------

/// `:w`, unless deciding the last unit already opened the sheet. The
/// effects of opening it are in `d.effects` either way.
fn finish(d: &mut Driver) {
    if !sheet_open(d) {
        d.keys(":w<cr>");
    }
    assert!(sheet_open(d), "{:?} {}", d.overlay(), d.status());
}

/// Open the sheet, pick the step `n` below the default (`j`) and run it.
fn run_step(d: &mut Driver, n: usize) -> Vec<Effect> {
    finish(d);
    for _ in 0..n {
        d.keys("j");
    }
    d.keys("<cr>")
}

/// The agent's drafted commit message arrives, if one was asked for.
fn drafted(d: &mut Driver, message: &str) -> bool {
    let asked = d
        .effects
        .iter()
        .any(|e| matches!(e, Effect::DraftCommitMessage(_)));
    if asked {
        d.app
            .on_app_event(AppEvent::CommitDrafted(Ok(message.into())));
    }
    asked
}

// ---- Fixtures: your changes -------------------------------------------------------

const OLD_CALC: &str = "def total(a, b):\n    return a + b\n\n\ndef fee(x):\n    return x + 1\n";
const NEW_CALC: &str =
    "def total(a, b):\n    return a + b + fee(a)\n\n\ndef fee(x):\n    return x + 2\n";

/// A repo with `calc.py` changed (`total`, `fee`) and not committed, and
/// `other.txt` committed. Identity, signing and hooks are the repo's own.
fn work_repo() -> FixtureRepo {
    isolate_env();
    let r = FixtureRepo::new();
    r.git(&["config", "user.name", "Ada Lovelace"]);
    r.git(&["config", "user.email", "ada@example.com"]);
    r.git(&["config", "commit.gpgsign", "false"]);
    let hooks = r.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    r.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    r.write("calc.py", OLD_CALC)
        .write("other.txt", "other\n")
        .commit("init");
    r.write("calc.py", NEW_CALC);
    r
}

fn work(repo: &FixtureRepo, agent: bool) -> Driver {
    let r = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    assert_eq!(r.units.len(), 2, "{:?}", r.units);
    Driver::start(r, Config::default(), agent)
}

/// The repo's review with `total` flagged with a note and `fee` accepted.
fn flagged_work(repo: &FixtureRepo) -> Driver {
    let mut d = work(repo, false);
    by_symbol(&mut d, "total");
    d.keys("icharges the fee twice<cr>");
    d.keys("f");
    by_symbol(&mut d, "fee");
    d.keys("a");
    assert_eq!(counts(&d), (0, 1, 1));
    d
}

/// The repo's review with both units accepted, and a note on `total`.
fn accepted_work(repo: &FixtureRepo, agent: bool) -> Driver {
    let mut d = work(repo, agent);
    by_symbol(&mut d, "total");
    d.keys("i");
    d.keys("looks right<cr>");
    d.keys("2a");
    assert_eq!(counts(&d), (0, 0, 2));
    d
}

fn by_symbol(d: &mut Driver, name: &str) {
    let id = d
        .app
        .review
        .units
        .iter()
        .find(|u| u.symbol == name)
        .unwrap()
        .id
        .clone();
    d.select_id(&id);
}

/// (pending, flagged, accepted).
fn counts(d: &Driver) -> (usize, usize, usize) {
    let (a, f, p) = d.app.decision_counts();
    (p, f, a)
}

fn head_count(r: &FixtureRepo) -> usize {
    r.git(&["rev-list", "--count", "HEAD"])
        .trim()
        .parse()
        .unwrap()
}

// ---- Fixtures: a branch -------------------------------------------------------------

/// A review of the branch `feature` since `main` (nothing to commit), all
/// accepted, with a note.
fn accepted_branch() -> Driver {
    let mut units = common::v2::units(3);
    for u in &mut units {
        u.decision = A;
    }
    units[1].dive.note = "nice".into();
    let mut r = review(units);
    r.head = "c0ffee".into();
    r.branch = Some(BranchNames {
        head: "feature".into(),
        base: "main".into(),
    });
    Driver::open(r, Config::default())
}

// ---- Fixtures: a pull request -------------------------------------------------------

const OLD_A: &str = "def f():\n    return 1\n\n\ndef g():\n    return 2\n";
const NEW_A: &str = "def f():\n    return 10\n\n\ndef g():\n    return 20\n";
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn pr_unit(symbol: &str, lines: (u32, u32)) -> Unit {
    let mut u = unit(0, "a.py");
    u.id = format!("a.py::{symbol}");
    u.symbol = symbol.into();
    u.old_range = Some(Range::new(lines.0, lines.1));
    u.new_range = Some(Range::new(lines.0, lines.1));
    u
}

fn pr_info(number: u32, repo_dir: PathBuf, head: &str) -> PrInfo {
    PrInfo {
        host: "github.com".into(),
        owner: "acme".into(),
        repo: "shop".into(),
        number,
        remote: Some("origin".into()),
        head_sha: Some(head.into()),
        repo_dir: Some(repo_dir),
        ..Default::default()
    }
}

/// A review of PR `pr` changing `f` and `g` in `a.py`, both pending.
fn pr_review(pr: PrInfo) -> Review {
    let mut r = review(vec![pr_unit("f", (1, 2)), pr_unit("g", (5, 6))]);
    r.repo_root = PathBuf::from("/wt");
    r.head = pr.head_sha.clone().unwrap();
    r.head_commit = r.head.clone();
    r.pr = Some(pr);
    r.files = vec![ChangedFile {
        path: PathBuf::from("a.py"),
        old_path: None,
        status: FileStatus::Modified,
        old_content: Some(OLD_A.into()),
        new_content: Some(NEW_A.into()),
    }];
    r
}

fn pr_driver(pr: PrInfo) -> Driver {
    Driver::open(pr_review(pr), Config::default())
}

/// PR #7 with `f` flagged with a note and `g` accepted.
fn flagged_pr() -> Driver {
    let mut d = pr_driver(pr_info(7, PathBuf::from("/r"), SHA));
    d.select_id("a.py::f");
    d.keys("icheck the limit<cr>");
    d.keys("f");
    d.select_id("a.py::g");
    d.keys("a");
    assert_eq!(counts(&d), (0, 1, 1));
    d
}

/// PR #7 with both units accepted, and a note on `f`.
fn accepted_pr() -> Driver {
    let mut d = pr_driver(pr_info(7, PathBuf::from("/r"), SHA));
    d.select_id("a.py::f");
    d.keys("i");
    d.keys("nice catch<cr>");
    d.keys("2a");
    assert_eq!(counts(&d), (0, 0, 2));
    d
}

/// A PR's repo with a local bare "remote" that has the PR's head under
/// `refs/pull/<n>/head`.
struct Origin {
    repo: FixtureRepo,
    _bare: tempfile::TempDir,
    head: String,
    n: u32,
}

static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(900);

impl Origin {
    fn new() -> Origin {
        isolate_env();
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let repo = FixtureRepo::new();
        repo.write("a.py", OLD_A).commit("init");
        repo.write("a.py", NEW_A).commit("pr");
        let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        let bare = tempfile::tempdir().unwrap();
        let st = std::process::Command::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(bare.path())
            .status()
            .unwrap();
        assert!(st.success());
        let url = bare.path().canonicalize().unwrap();
        repo.git(&["remote", "add", "origin", url.to_str().unwrap()]);
        repo.git(&["push", "-q", "origin", "main"]);
        repo.git(&["push", "-q", "origin", &format!("HEAD:refs/pull/{n}/head")]);
        Origin {
            repo,
            _bare: bare,
            head,
            n,
        }
    }

    fn info(&self) -> PrInfo {
        pr_info(self.n, self.repo.path(), &self.head)
    }

    /// Push a new commit to the PR after the review.
    fn move_head(&self) {
        self.repo
            .write("a.py", "def f():\n    return 99\n")
            .commit("more");
        self.repo.git(&[
            "push",
            "-q",
            "-f",
            "origin",
            &format!("HEAD:refs/pull/{}/head", self.n),
        ]);
    }
}

/// `gh` records each call (`call_<k>`: argv, one per line; `stdin_<k>`) and
/// succeeds (`ok`) or fails (`fail`); the opener records what it opens.
struct Fakes {
    dir: tempfile::TempDir,
    tools: Tools,
}

fn fakes(mode: &str) -> Fakes {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("mode"), mode).unwrap();
    script(
        &d.join("gh"),
        r#"d=$(dirname "$0")
n=$(ls "$d" | grep -c '^call_')
n=$((n+1))
: > "$d/call_$n"
for a in "$@"; do printf '%s\n' "$a" >> "$d/call_$n"; done
cat > "$d/stdin_$n"
case "$(cat "$d/mode")" in
  ok) echo '{"id":1}';;
  fail) echo "HTTP 401: Bad credentials" >&2; exit 1;;
esac"#,
    );
    script(
        &d.join("opener"),
        r#"d=$(dirname "$0"); printf '%s\n' "$@" >> "$d/opened""#,
    );
    let tools = Tools {
        gh: d.join("gh").into(),
        opener: d.join("opener").into(),
        editor: d.join("no-editor").into(),
        claude: d.join("no-claude").into(),
        codex: d.join("no-codex").into(),
    };
    Fakes { dir, tools }
}

impl Fakes {
    fn calls(&self) -> usize {
        std::fs::read_dir(self.dir.path())
            .unwrap()
            .filter(|e| {
                let name = e.as_ref().unwrap().file_name();
                name.to_string_lossy().starts_with("call_")
            })
            .count()
    }

    fn stdin(&self, k: usize) -> serde_json::Value {
        let s = std::fs::read_to_string(self.dir.path().join(format!("stdin_{k}"))).unwrap();
        serde_json::from_str(&s).unwrap_or_else(|e| panic!("{e}: {s}"))
    }

    /// What the opener was asked to open (it runs detached: wait for it).
    async fn opened(&self) -> Vec<String> {
        let p = self.dir.path().join("opened");
        for _ in 0..100 {
            if let Ok(s) = std::fs::read_to_string(&p)
                && !s.is_empty()
            {
                return s.lines().map(String::from).collect();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Vec::new()
    }
}

/// Carry out `effects` as the runtime does, with the fakes, and hand back
/// what the app answers to the post's outcome.
async fn post_through_runtime(d: &mut Driver, fk: &Fakes, effects: Vec<Effect>) -> Vec<Effect> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let rt = Runtime {
        adapter: None,
        tx,
        build: BuildOptions {
            base: None,
            languages: None,
            ..Default::default()
        },
        tools: fk.tools.clone(),
    };
    for e in effects {
        if matches!(e, Effect::PostReview { .. } | Effect::Save) {
            rt.exec(&mut d.app, e);
        }
    }
    let ev = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("an answer")
        .unwrap();
    d.app.on_app_event(ev)
}

/// Commit what `effects` ask to, as the runtime does (git without a
/// terminal), and hand back what the app answers.
fn commit_through_git(d: &mut Driver, repo: &Path, effects: &[Effect]) -> Vec<Effect> {
    let (paths, message) = commit_of(effects).expect("a commit");
    let out = commit::run_commit(repo, &paths, &message, Io::Capture);
    assert!(matches!(out, Outcome::Committed { .. }), "{out:?}");
    d.app.commit_finished(out)
}

// ---- F1 ---------------------------------------------------------------------------

#[test]
fn enter_runs_the_default_step() {
    let repo = work_repo();
    let mut d = accepted_work(&repo, false);
    finish(&mut d);
    let effects = d.keys("<cr>");
    assert!(commit_of(&effects).is_some(), "{effects:?}");

    // `:w` works from the unit view too.
    let mut d = accepted_work(&repo, false);
    d.keys("l");
    assert_eq!(d.screen(), Screen::Unit);
    d.keys(":w<cr>");
    assert!(sheet_open(&d), "{:?}", d.overlay());
}

// ---- F2 ---------------------------------------------------------------------------

#[test]
fn default_and_alternatives_your_changes_flagged() {
    let repo = work_repo();
    // Default: the notes go to your agent (Claude Code here).
    let mut d = flagged_work(&repo);
    let effects = run_step(&mut d, 0);
    let (cli, prompt) = handed_over(&effects).expect("handed over");
    assert_eq!(cli, Provider::Claude);
    assert_eq!(prompt, teer::notes::agent_prompt(&d.app.review));
    assert!(prompt.contains("charges the fee twice"));
    // Yank the notes.
    let mut d = flagged_work(&repo);
    let effects = run_step(&mut d, 1);
    assert_eq!(copied(&effects), Some(teer::notes::markdown(&d.app.review)));
    // Commit anyway.
    let mut d = flagged_work(&repo);
    let effects = run_step(&mut d, 2);
    let (paths, _) = commit_of(&effects).expect("commits anyway");
    assert_eq!(paths, [PathBuf::from("calc.py")]);
}

#[test]
fn with_the_model_api_the_notes_are_copied() {
    // Q11: no agent CLI to send them to.
    let repo = work_repo();
    let mut d = flagged_work(&repo);
    d.app.cfg.provider = Some(Provider::Api);
    let effects = run_step(&mut d, 0);
    assert_eq!(handed_over(&effects), None);
    // What the agent would have been sent: its instructions, then the notes.
    let copied = copied(&effects).expect("the notes are copied");
    assert_eq!(copied, teer::notes::agent_prompt(&d.app.review));
    assert!(copied.ends_with(&teer::notes::markdown(&d.app.review)));
}

#[test]
fn default_and_alternatives_your_changes_all_accepted() {
    let repo = work_repo();
    let mut d = accepted_work(&repo, false);
    let effects = run_step(&mut d, 0);
    let (paths, message) = commit_of(&effects).expect("commits");
    assert_eq!(paths, [PathBuf::from("calc.py")]);
    assert!(!message.trim().is_empty());
    let mut d = accepted_work(&repo, false);
    let effects = run_step(&mut d, 1);
    assert_eq!(copied(&effects), Some(teer::notes::markdown(&d.app.review)));
    assert_eq!(commit_of(&effects), None);
}

#[test]
fn default_and_alternatives_branch_all_accepted() {
    // Q10: a branch has nothing to commit; "done" exits.
    let mut d = accepted_branch();
    let effects = run_step(&mut d, 0);
    assert!(effects.contains(&Effect::Quit), "{effects:?}");
    assert_eq!(commit_of(&effects), None);
    assert_eq!(post_of(&effects), None);
    let mut d = accepted_branch();
    let effects = run_step(&mut d, 1);
    assert_eq!(copied(&effects), Some(teer::notes::markdown(&d.app.review)));
    assert!(!effects.contains(&Effect::Quit));
}

#[test]
fn default_and_alternatives_pr_flagged() {
    // Request changes: the notes inline, and a summary.
    let mut d = flagged_pr();
    let effects = run_step(&mut d, 0);
    let (verdict, body, inline) = post_of(&effects).expect("posts");
    assert_eq!(verdict, Verdict::RequestChanges);
    assert_eq!(inline, 1, "the note on f, at its line");
    assert!(!body.trim().is_empty());
    // Comment only.
    let mut d = flagged_pr();
    let effects = run_step(&mut d, 1);
    assert_eq!(post_of(&effects).map(|p| p.0), Some(Verdict::Comment));
    // Yank the notes.
    let mut d = flagged_pr();
    let effects = run_step(&mut d, 2);
    assert_eq!(copied(&effects), Some(teer::notes::markdown(&d.app.review)));
    assert_eq!(post_of(&effects), None);
    // Never handed to an agent: a PR's code is untrusted.
    assert_eq!(handed_over(&d.effects), None);
}

#[test]
fn default_and_alternatives_pr_all_accepted() {
    let mut d = accepted_pr();
    let effects = run_step(&mut d, 0);
    assert_eq!(post_of(&effects).map(|p| p.0), Some(Verdict::Approve));
    // Approve with a comment: written first when the sheet asks for it.
    let mut d = accepted_pr();
    let mut effects = run_step(&mut d, 1);
    if editing(&effects).is_some() {
        d.app.comment_edited(EditOutcome::Edited("Ship it.".into()));
        effects = d.keys("<cr>");
    }
    let (verdict, body, _) = post_of(&effects).expect("approves");
    assert_eq!(verdict, Verdict::Approve);
    assert!(!body.trim().is_empty(), "with a comment");
    let mut d = accepted_pr();
    let effects = run_step(&mut d, 2);
    assert_eq!(copied(&effects), Some(teer::notes::markdown(&d.app.review)));
    assert_eq!(post_of(&effects), None);
}

// ---- F3 and the "pending warning" acceptance -----------------------------------

/// Six units, `u0` flagged with a note, `u1` accepted, four pending.
fn half_done() -> Driver {
    let mut d = Driver::open(review(common::v2::units(6)), Config::default());
    d.keys("ifirst note<cr>");
    d.keys("f");
    d.select_id("u1");
    d.keys("a");
    assert_eq!(counts(&d), (4, 1, 1));
    d.select_id("u5");
    d
}

#[test]
fn pending_warning() {
    // The count first; `n` jumps to the first pending unit.
    let mut d = half_done();
    finish(&mut d);
    let screen = d.render(120, 40);
    assert!(screen.contains(&units_pending(4)), "{screen}");
    d.keys("n");
    assert!(!sheet_open(&d));
    assert_eq!(d.screen(), Screen::Queue);
    assert_eq!(d.selected().as_deref(), Some("u2"));

    // The default is to keep reviewing: nothing happens.
    let mut d = half_done();
    let effects = run_step(&mut d, 0);
    assert_eq!(outward(&effects), Vec::new());
    assert!(!sheet_open(&d));
    assert_eq!(counts(&d), (4, 1, 1));

    // Every other step asks first.
    let mut d = half_done();
    let effects = run_step(&mut d, 1);
    assert_eq!(outward(&effects), Vec::new(), "asks before it does it");
    let effects = confirm(&mut d);
    assert!(handed_over(&effects).is_some(), "{effects:?}");
}

// ---- F4 ---------------------------------------------------------------------------

#[test]
fn sheet_lists_notes() {
    let mut units = common::v2::units(4);
    units[0].symbol = "charge_order".into();
    units[2].symbol = "refund_order".into();
    let mut d = Driver::open(review(units), Config::default());
    d.keys("iit charges twice<cr>");
    d.keys("f");
    d.select_id("u2");
    d.keys("irefunds the wrong amount<cr>");
    d.keys("f");
    d.keys("2a");
    finish(&mut d);
    let screen = d.render(140, 40);
    for text in [
        "charge_order",
        "it charges twice",
        "refund_order",
        "refunds the wrong amount",
    ] {
        assert!(screen.contains(text), "{text}:\n{screen}");
    }
}

// ---- F5 ---------------------------------------------------------------------------

#[test]
fn yank_format() {
    let repo = work_repo();
    let mut d = flagged_work(&repo);
    finish(&mut d);
    let effects = d.keys("y");
    let text = copied(&effects).expect("copied");
    assert_eq!(text, teer::notes::markdown(&d.app.review));
    assert!(text.starts_with("# Review notes"), "{text}");
    assert!(text.contains("## calc.py:"), "{text}");
    assert!(
        text.contains("```diff\n"),
        "each note with its diff: {text}"
    );
    assert!(text.contains("+    return a + b + fee(a)"), "{text}");
    assert!(text.contains("> charges the fee twice"), "{text}");
}

// ---- F6 ---------------------------------------------------------------------------

#[test]
fn i_edits_message_or_summary() {
    let repo = work_repo();
    // The commit message, in the configured editor.
    let mut d = accepted_work(&repo, false);
    finish(&mut d);
    let effects = d.keys("i");
    assert!(editing(&effects).is_some(), "{effects:?}");
    d.app
        .comment_edited(EditOutcome::Edited("Charge the fee once\n\nWhy.".into()));
    let effects = d.keys("<cr>");
    assert_eq!(
        commit_of(&effects).map(|c| c.1),
        Some("Charge the fee once\n\nWhy.".into())
    );

    // No editor: the subject on one line.
    let mut d = accepted_work(&repo, false);
    finish(&mut d);
    d.keys("i");
    d.app
        .comment_edited(EditOutcome::Unavailable("no editor".into()));
    d.keys("<c-u>");
    d.keys("One line subject<cr>");
    let effects = d.keys("<cr>");
    let (_, message) = commit_of(&effects).expect("commits");
    assert!(message.starts_with("One line subject"), "{message}");

    // The review summary of a PR.
    let mut d = flagged_pr();
    finish(&mut d);
    let effects = d.keys("i");
    assert!(editing(&effects).is_some(), "{effects:?}");
    d.app
        .comment_edited(EditOutcome::Edited("Please check the limit.".into()));
    let effects = d.keys("<cr>");
    let (_, body, _) = post_of(&effects).expect("posts");
    assert!(body.contains("Please check the limit."), "{body}");
}

// ---- F7 ---------------------------------------------------------------------------

#[test]
fn close_does_nothing() {
    for keys in ["<esc>", "q"] {
        let repo = work_repo();
        let mut d = flagged_work(&repo);
        let notes = teer::notes::markdown(&d.app.review);
        finish(&mut d);
        let effects = d.keys(keys);
        assert_eq!(outward(&effects), Vec::new(), "{keys}");
        assert!(!sheet_open(&d), "{keys}");
        assert_eq!(d.screen(), Screen::Queue, "{keys}");
        assert_eq!(counts(&d), (0, 1, 1), "{keys}");
        assert_eq!(teer::notes::markdown(&d.app.review), notes, "{keys}");
    }
}

// ---- F8 ---------------------------------------------------------------------------

#[test]
fn a_commit_takes_only_the_reviewed_files_and_runs_the_hooks() {
    let repo = work_repo();
    let hook = repo.path().join(".git").join("hooks").join("pre-commit");
    script(&hook, "touch \"$(git rev-parse --git-dir)/hook-ran\"");
    let mut d = accepted_work(&repo, false);
    // Changed after the review, not part of it.
    repo.write("other.txt", "other, changed\n");
    repo.write("stray.log", "untracked\n");
    let effects = run_step(&mut d, 0);
    let (paths, _) = commit_of(&effects).expect("commits");
    assert_eq!(paths, [PathBuf::from("calc.py")]);
    let before = head_count(&repo);
    commit_through_git(&mut d, &repo.path(), &effects);
    assert_eq!(head_count(&repo), before + 1);
    assert!(
        repo.path().join(".git").join("hook-ran").exists(),
        "the hook ran"
    );
    let files = repo.git(&["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(files.trim(), "calc.py");
    let status = repo.git(&["status", "--porcelain", "--untracked-files=all"]);
    assert!(status.contains(" M other.txt"), "{status}");
    assert!(status.contains("?? stray.log"), "{status}");
}

#[test]
fn a_commit_refuses_a_file_changed_since_the_review() {
    let repo = work_repo();
    let mut d = accepted_work(&repo, false);
    repo.write("calc.py", "def total(a, b):\n    return 0\n");
    let before = head_count(&repo);
    let effects = run_step(&mut d, 0);
    assert_eq!(commit_of(&effects), None, "{effects:?}");
    assert!(d.status().contains("calc.py"), "says which: {}", d.status());
    assert!(!effects.contains(&Effect::Quit));
    assert_eq!(head_count(&repo), before);
}

#[tokio::test]
async fn gh_runs_only_after_enter_and_never_on_a_moved_head() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut d = pr_driver(o.info());
    d.keys("2a");
    finish(&mut d);
    // Looking around the sheet posts nothing.
    let effects = d.keys("jkj");
    assert_eq!(post_of(&effects), None);
    d.keys("k");
    assert_eq!(fk.calls(), 0);

    o.move_head();
    let effects = d.keys("<cr>");
    assert!(post_of(&effects).is_some(), "{effects:?}");
    let after = post_through_runtime(&mut d, &fk, effects).await;
    assert!(
        d.status().contains("has new commits since you reviewed it"),
        "{}",
        d.status()
    );
    assert_eq!(fk.calls(), 0, "the head check comes first");
    assert!(!after.contains(&Effect::Quit));
    assert!(d.app.review.pr.as_ref().unwrap().approved_head.is_none());
}

#[tokio::test]
async fn a_failed_post_opens_the_pr_page() {
    let o = Origin::new();
    let fk = fakes("fail");
    let mut d = pr_driver(o.info());
    d.keys("2a");
    let effects = run_step(&mut d, 0);
    let after = post_through_runtime(&mut d, &fk, effects).await;
    assert!(d.status().starts_with("gh failed"), "{}", d.status());
    assert_eq!(fk.opened().await.len(), 1, "the PR page");
    assert!(!after.contains(&Effect::Quit), "teer stays: {after:?}");
}

// ---- F9 ---------------------------------------------------------------------------

#[test]
fn success_exits_with_one_line_after_a_commit() {
    let repo = work_repo();
    let mut d = accepted_work(&repo, false);
    let effects = run_step(&mut d, 0);
    let (_, message) = commit_of(&effects).unwrap();
    let after = commit_through_git(&mut d, &repo.path(), &effects);
    let line = exit_line(&d, &after).expect("exits");
    let subject = message.lines().next().unwrap();
    assert!(line.starts_with("Committed "), "{line}");
    assert!(line.contains(subject), "{line}");
    assert!(!line.contains('\n'), "one line: {line:?}");
}

#[tokio::test]
async fn success_exits_with_one_line_after_a_post() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut d = pr_driver(o.info());
    d.keys("2a");
    let effects = run_step(&mut d, 0);
    let after = post_through_runtime(&mut d, &fk, effects).await;
    let line = exit_line(&d, &after).expect("exits");
    assert_eq!(line, format!("PR #{} approved on GitHub.", o.n));
}

// ---- Acceptance --------------------------------------------------------------------

#[test]
fn agent_changes_nothing_wrong() {
    let repo = work_repo();
    let mut d = work(&repo, true);
    d.keys("a");
    d.keys("a");
    assert_eq!(counts(&d), (0, 0, 2));
    finish(&mut d);
    let message = "Charge the fee once\n\nDrafted by the agent.";
    assert!(drafted(&mut d, message), "the agent drafts the message");
    let effects = d.keys("<cr>");
    assert_eq!(commit_of(&effects).map(|c| c.1), Some(message.into()));
    let after = commit_through_git(&mut d, &repo.path(), &effects);
    assert_eq!(repo.git(&["log", "-1", "--format=%B"]).trim(), message);
    assert!(exit_line(&d, &after).is_some(), "{after:?}");
}

#[tokio::test]
async fn pr_approve() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut d = pr_driver(o.info());
    d.keys("aa");
    finish(&mut d);
    let effects = d.keys("<cr>");
    assert_eq!(post_of(&effects).map(|p| p.0), Some(Verdict::Approve));
    post_through_runtime(&mut d, &fk, effects).await;
    assert_eq!(fk.calls(), 1);
    let sent = fk.stdin(1);
    assert_eq!(sent["event"], "APPROVE");
    assert_eq!(sent["commit_id"], o.head.as_str());
}

#[tokio::test]
async fn pr_request_changes() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut d = pr_driver(o.info());
    d.keys("icheck the limit<cr>");
    d.keys("f");
    d.keys("iand this one<cr>");
    d.keys("f");
    assert_eq!(counts(&d), (0, 2, 0));
    finish(&mut d);
    let effects = d.keys("<cr>");
    let (verdict, _, inline) = post_of(&effects).expect("posts");
    assert_eq!((verdict, inline), (Verdict::RequestChanges, 2));
    post_through_runtime(&mut d, &fk, effects).await;
    assert_eq!(fk.calls(), 1);
    let sent = fk.stdin(1);
    assert_eq!(sent["event"], "REQUEST_CHANGES");
    assert_eq!(sent["comments"].as_array().unwrap().len(), 2);
    assert!(
        !sent["body"].as_str().unwrap().trim().is_empty(),
        "a summary"
    );
}

#[test]
fn decisions_reach_the_sheet_unchanged() {
    // Opening and closing the sheet never decides anything.
    let mut d = half_done();
    let before = d.decisions();
    finish(&mut d);
    d.keys("<esc>");
    assert_eq!(d.decisions(), before);
    assert_eq!(before.iter().filter(|x| **x == P).count(), 4);
    assert_eq!(before.iter().filter(|x| **x == F).count(), 1);
}
