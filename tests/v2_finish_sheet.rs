//! v2 Phase 6: the `:w` sheet, end to end through the keys. One default
//! step per case (your changes, a branch, a pull request; something flagged
//! or everything accepted), its alternatives, the pending warning, `y`,
//! `i` on the commit message and on the review summary, and every safety
//! check refusing. Real git in temp repositories; `gh`, the browser opener
//! and the editor are scripts; no agent, no network.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use teer::approve::{self, EditOutcome, Tools};
use teer::commit::{self, Io, Outcome};
use teer::config::{AutoAccept, Config, Provider};
use teer::git::{ChangedFile, FileStatus};
use teer::keymap::Keymap;
use teer::review::model::{
    Decision, Dive, PrInfo, Prep, Range, Review, Unit, UnitKind, UnitStatus,
};
use teer::review::{BuildOptions, build_review};
use teer::tui::Runtime;
use teer::tui::app::{App, AppEvent, DraftState, Effect, Overlay};
use teer::tui::finish::Step;

// ---- Local changes and branches ---------------------------------------------

/// A repository with one commit, `a.py` changed on top of it, and its own
/// identity and hooks, so the user's git config can't change what happens.
fn repo() -> FixtureRepo {
    isolate_env();
    let r = FixtureRepo::new();
    r.git(&["config", "user.name", "Ada Lovelace"]);
    r.git(&["config", "user.email", "ada@example.com"]);
    r.git(&["config", "commit.gpgsign", "false"]);
    let hooks = r.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    r.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    r.write("a.py", PY_OLD)
        .write("keep.txt", "keep\n")
        .commit("initial");
    r.write("a.py", PY_NEW);
    r
}

/// The same change committed on a branch: a branch review since `main`.
fn branch_repo() -> FixtureRepo {
    let r = repo();
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit("change a.py");
    r
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

fn app_with(rv: Review, cfg: Config) -> App {
    let mut a = App::new(rv, cfg, Keymap::default());
    a.start(false, None, Vec::new(), None);
    a
}

/// The review of `r`'s uncommitted changes, with every unit `d`.
fn changes(r: &FixtureRepo, d: Decision) -> App {
    let mut rv = build_review(&r.path(), &BuildOptions::default()).unwrap();
    for u in &mut rv.units {
        u.decision = d;
    }
    app_with(rv, Config::default())
}

/// The review of `r`'s branch since `main`, with every unit `d`.
fn branch(r: &FixtureRepo, d: Decision) -> App {
    let opts = BuildOptions {
        base: Some("main".into()),
        languages: None,
    };
    let mut rv = build_review(&r.path(), &opts).unwrap();
    assert!(!rv.units.is_empty());
    for u in &mut rv.units {
        u.decision = d;
    }
    app_with(rv, Config::default())
}

/// Flag the first unit with `note`.
fn flag_first(a: &mut App, note: &str) -> String {
    let u = &mut a.review.units[0];
    u.decision = Decision::Investigate;
    u.dive.note = note.into();
    u.symbol.clone()
}

fn keys(a: &mut App, k: &str) -> Vec<Effect> {
    v2::press(a, k)
}

fn head_count(r: &FixtureRepo) -> usize {
    r.git(&["rev-list", "--count", "HEAD"])
        .trim()
        .parse()
        .unwrap()
}

fn status(r: &FixtureRepo) -> String {
    r.git(&["status", "--porcelain", "--untracked-files=all"])
}

/// Run the commit the sheet asked for (without a terminal) and hand the
/// outcome back, as the terminal loop does; the effects that follow.
fn run(a: &mut App, e: Vec<Effect>) -> (Outcome, Vec<Effect>) {
    let [
        Effect::Commit {
            repo,
            paths,
            message,
        },
    ] = &e[..]
    else {
        panic!("no commit: {e:?}; status {:?}", a.status)
    };
    let out = commit::run_commit(repo, paths, message, Io::Capture);
    let after = a.commit_finished(out.clone());
    (out, after)
}

/// ⏎ must refuse: no effect, the sheet closed, nothing in the repository
/// touched. The status line.
fn refused(a: &mut App, r: &FixtureRepo) -> String {
    let before = (status(r), r.git(&["rev-parse", "HEAD"]));
    let e = keys(a, "<cr>");
    assert!(e.is_empty(), "{e:?}");
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!((status(r), r.git(&["rev-parse", "HEAD"])), before);
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]), "");
    a.status.clone()
}

#[test]
fn your_changes_all_accepted_enter_commits_and_teer_ends_with_one_line() {
    let r = repo();
    let mut a = changes(&r, Decision::Accepted);
    assert!(keys(&mut a, ":w<cr>").is_empty(), "no agent: no draft");
    assert_eq!(a.finish_steps(), [Step::Commit, Step::Yank]);
    assert_eq!(a.finish.draft, Some(DraftState::Template));
    let e = keys(&mut a, "<cr>");
    let (out, after) = run(&mut a, e);
    let Outcome::Committed { short } = out else {
        panic!("{out:?}")
    };
    assert_eq!(after, vec![Effect::Save, Effect::Quit]);
    assert_eq!(
        a.finish.exit_line.as_deref(),
        Some(format!("Committed {short}: Update a.py").as_str())
    );
    assert_eq!(head_count(&r), 2);
    assert_eq!(r.git(&["log", "-1", "--format=%s"]).trim(), "Update a.py");
}

#[test]
fn your_changes_flagged_send_the_notes_or_yank_them_or_commit_anyway() {
    let r = repo();
    let mut a = changes(&r, Decision::Accepted);
    flag_first(&mut a, "the currency check runs after pay()");
    keys(&mut a, ":w<cr>");
    assert_eq!(
        a.finish_steps(),
        [Step::SendNotes, Step::Yank, Step::Commit]
    );
    // ⏎: the notes go to Claude Code, in the repository.
    let e = keys(&mut a, "<cr>");
    let [
        Effect::OpenAgent {
            cli: Provider::Claude,
            cwd,
            prompt,
            resume: None,
        },
    ] = &e[..]
    else {
        panic!("{e:?}")
    };
    assert_eq!(cwd, &r.path());
    assert!(
        prompt.contains("> the currency check runs after pay()"),
        "{prompt}"
    );
    assert_eq!(a.overlay, Overlay::None);
    // `j`: yank the notes.
    keys(&mut a, ":w<cr>j");
    let e = keys(&mut a, "<cr>");
    let [Effect::Copy { text, what }] = &e[..] else {
        panic!("{e:?}")
    };
    assert_eq!(what, "1 note as Markdown");
    assert_eq!(*text, teer::notes::markdown(&a.review));
    // teer waits for the changes now (Phase 7); `q` stops waiting.
    assert!(a.waiting.is_some());
    keys(&mut a, "q");
    // `jj`: commit anyway, the reviewed files only.
    keys(&mut a, ":w<cr>jj");
    let e = keys(&mut a, "<cr>");
    let (out, _) = run(&mut a, e);
    assert!(matches!(out, Outcome::Committed { .. }), "{out:?}");
    assert_eq!(head_count(&r), 2);
    assert_eq!(status(&r), "");
}

#[test]
fn with_the_model_api_the_notes_are_copied_for_the_agent() {
    let r = repo();
    let mut a = changes(&r, Decision::Accepted);
    a.cfg.provider = Some(Provider::Api);
    flag_first(&mut a, "why?");
    keys(&mut a, ":w<cr>");
    let e = keys(&mut a, "<cr>");
    let [Effect::Copy { text, what }] = &e[..] else {
        panic!("{e:?}")
    };
    assert_eq!(*text, teer::notes::agent_prompt(&a.review));
    assert_eq!(
        what,
        "1 note for your agent (teer can't hand them to the model API)"
    );
}

#[test]
fn a_branch_all_accepted_is_done_and_flagged_sends_the_notes() {
    let r = branch_repo();
    let mut a = branch(&r, Decision::Accepted);
    keys(&mut a, ":w<cr>");
    assert_eq!(a.finish_steps(), [Step::Done, Step::Yank]);
    assert_eq!(keys(&mut a, "<cr>"), vec![Effect::Save, Effect::Quit]);
    let line = a.finish.exit_line.clone().unwrap();
    assert!(line.starts_with("Reviewed since "), "{line}");
    assert!(line.ends_with("units accepted."), "{line}");
    assert_eq!(head_count(&r), 2, "nothing committed");

    let mut b = branch(&r, Decision::Accepted);
    flag_first(&mut b, "rename it");
    keys(&mut b, ":w<cr>");
    assert_eq!(b.finish_steps(), [Step::SendNotes, Step::Yank]);
    let e = keys(&mut b, "<cr>");
    assert!(matches!(&e[..], [Effect::OpenAgent { .. }]), "{e:?}");
}

#[test]
fn pending_units_come_first_n_goes_there_and_the_other_steps_ask() {
    let r = repo();
    let mut a = changes(&r, Decision::Accepted);
    assert!(a.review.units.len() > 2);
    for i in [1, 2] {
        a.review.units[i].decision = Decision::Pending;
    }
    a.sel = 0;
    keys(&mut a, ":w<cr>");
    assert_eq!(
        a.finish_steps(),
        [Step::KeepReviewing, Step::Commit, Step::Yank]
    );
    let screen = v2::screen(&a, 100, 40);
    assert!(
        screen.contains("2 units pending · n goes to the first"),
        "{screen}"
    );
    // `n`: the sheet closes on the first pending unit.
    keys(&mut a, "n");
    assert_eq!(a.overlay, Overlay::None);
    let sel = a.selected().unwrap();
    assert_eq!(sel.decision, Decision::Pending);
    // Commit asks first; no is no.
    keys(&mut a, ":w<cr>j");
    assert!(keys(&mut a, "<cr>").is_empty());
    let screen = v2::screen(&a, 100, 40);
    assert!(
        screen.contains("2 units still pending. Commit anyway?"),
        "{screen}"
    );
    assert!(keys(&mut a, "n").is_empty());
    assert_eq!(head_count(&r), 1);
    // Yes commits.
    keys(&mut a, "<cr>");
    let e = keys(&mut a, "y");
    let (out, _) = run(&mut a, e);
    assert!(matches!(out, Outcome::Committed { .. }), "{out:?}");
}

#[test]
fn y_copies_the_notes_as_markdown_each_with_its_diff() {
    let r = repo();
    let mut a = changes(&r, Decision::Accepted);
    let name = flag_first(&mut a, "check \"limit\"");
    a.review.units[1].decision = Decision::Investigate;
    keys(&mut a, ":w<cr>");
    // The sheet lists both.
    let screen = v2::screen(&a, 100, 50);
    assert!(screen.contains("check \"limit\""), "{screen}");
    assert!(screen.contains("no note"), "{screen}");
    let e = keys(&mut a, "y");
    let [Effect::Copy { text, what }] = &e[..] else {
        panic!("{e:?}")
    };
    assert_eq!(what, "1 note and 1 flagged unit without one as Markdown");
    let repo_name = r.path().file_name().unwrap().to_string_lossy().to_string();
    assert!(
        text.starts_with(&format!(
            "# Review notes\n\n1 note and 1 flagged unit without one on the uncommitted changes in `{repo_name}`.\n"
        )),
        "{text}"
    );
    assert!(text.contains(&format!("· `{name}`")), "{text}");
    assert!(text.contains("```diff\n"), "{text}");
    assert!(text.contains("> check \"limit\"\n"), "{text}");
    assert!(
        text.contains("_Flagged in the review, without a note._"),
        "{text}"
    );
    // Notes to fix went out: the sheet closes and teer waits for the
    // changes (Phase 7).
    assert_eq!(a.overlay, Overlay::None);
    assert!(a.waiting.is_some());
}

#[test]
fn i_edits_the_commit_message_in_the_configured_editor() {
    let r = repo();
    let mut a = changes(&r, Decision::Accepted);
    keys(&mut a, ":w<cr>");
    let e = keys(&mut a, "i");
    let [Effect::EditComment { text }] = &e[..] else {
        panic!("{e:?}")
    };
    assert!(a.edits_commit_message());
    let d = tempfile::tempdir().unwrap();
    let ed = script(
        d.path(),
        "ed",
        "printf 'Check the currency first\\n\\nBefore paying.\\n' > \"$1\"",
    );
    a.comment_edited(approve::edit_message_in_editor(&ed.into(), text));
    assert_eq!(
        a.finish.message,
        "Check the currency first\n\nBefore paying."
    );
    let e = keys(&mut a, "<cr>");
    run(&mut a, e);
    assert_eq!(
        r.git(&["log", "-1", "--format=%B"]).trim(),
        "Check the currency first\n\nBefore paying."
    );
}

#[test]
fn esc_and_q_close_without_doing_anything() {
    let r = repo();
    let mut a = changes(&r, Decision::Accepted);
    for close in ["<esc>", "q", ":q<cr>"] {
        keys(&mut a, ":w<cr>");
        assert!(keys(&mut a, close).is_empty(), "{close}");
        assert_eq!(a.overlay, Overlay::None, "{close}");
    }
    assert_eq!(head_count(&r), 1);
    assert!(a.finish.exit_line.is_none());
}

// ---- Safety: commits --------------------------------------------------------

#[test]
fn only_the_reviewed_files_are_committed_with_the_users_hooks() {
    let r = repo();
    let mut a = changes(&r, Decision::Accepted);
    let ran = r.path().join(".git").join("ran");
    let hook = r.path().join(".git").join("hooks").join("pre-commit");
    std::fs::write(&hook, format!("#!/bin/sh\necho ran > {}\n", ran.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // Made after the review: not part of it.
    r.write("stray.txt", "later\n");
    keys(&mut a, ":w<cr>");
    let e = keys(&mut a, "<cr>");
    let (out, _) = run(&mut a, e);
    assert!(matches!(out, Outcome::Committed { .. }), "{out:?}");
    assert!(ran.exists(), "the user's hook ran");
    assert_eq!(
        r.git(&["show", "--name-only", "--format=", "HEAD"]).trim(),
        "a.py"
    );
    assert_eq!(status(&r), "?? stray.txt\n");
}

#[test]
fn a_file_changed_since_the_review_is_refused() {
    let r = repo();
    let mut a = changes(&r, Decision::Accepted);
    keys(&mut a, ":w<cr>");
    r.write("a.py", "changed = 1\n");
    let s = refused(&mut a, &r);
    assert!(
        s.contains("Files changed since you reviewed them (a.py)"),
        "{s}"
    );
    // Commit anyway checks the files the same way.
    let mut b = changes(&r, Decision::Accepted);
    b.review.units[0].decision = Decision::Investigate;
    r.write("a.py", "changed again = 1\n");
    keys(&mut b, ":w<cr>jj");
    assert_eq!(b.finish_step(), Some(Step::Commit));
    let s = refused(&mut b, &r);
    assert!(s.contains("Files changed since you reviewed them"), "{s}");
}

#[test]
fn a_moved_head_is_refused() {
    let r = repo();
    let mut a = changes(&r, Decision::Accepted);
    keys(&mut a, ":w<cr>");
    r.git(&["commit", "--allow-empty", "-q", "-m", "elsewhere"]);
    let s = refused(&mut a, &r);
    assert!(s.starts_with("HEAD moved since you reviewed"), "{s}");
}

#[test]
fn a_pending_no_behavior_change_unit_holds_the_commit_back() {
    let r = repo();
    let cfg = Config {
        auto_accept: AutoAccept::None,
        ..Config::default()
    };
    let rv = build_review(&r.path(), &BuildOptions::default()).unwrap();
    let mut a = app_with(rv, cfg);
    let quiet: Vec<usize> = (0..a.review.units.len())
        .filter(|&i| a.review.units[i].status == UnitStatus::NoBehaviorChange)
        .collect();
    assert!(
        !quiet.is_empty(),
        "the fixture has a unit with no behavior change"
    );
    for (i, u) in a.review.units.iter_mut().enumerate() {
        if !quiet.contains(&i) {
            u.decision = Decision::Accepted;
        }
    }
    // Not accepted for you: it waits, and the commit with it.
    assert_eq!(a.review.units[quiet[0]].decision, Decision::Pending);
    assert_eq!(commit::check(&a.review), Err(commit::Refusal::NotReviewed));
    keys(&mut a, ":w<cr>");
    assert_eq!(a.finish_step(), Some(Step::KeepReviewing));
}

// ---- Pull requests ----------------------------------------------------------

const OLD: &str = "def f():\n    return 1\n\n\ndef g():\n    return 2\n";
const NEW: &str = "def f():\n    return 10\n\n\ndef g():\n    return 20\n";

static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(700);

/// A PR on a local "remote": its head is pushed to `refs/pull/<n>/head`.
struct Origin {
    repo: FixtureRepo,
    _bare: tempfile::TempDir,
    head: String,
    n: u32,
}

impl Origin {
    fn new() -> Origin {
        isolate_env();
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let repo = FixtureRepo::new();
        repo.write("a.py", OLD).commit("init");
        repo.write("a.py", NEW).commit("pr");
        let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        let bare = tempfile::tempdir().unwrap();
        let url = bare.path().canonicalize().unwrap();
        repo.git(&["init", "-q", "--bare", "-b", "main", url.to_str().unwrap()]);
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
        PrInfo {
            host: "github.com".into(),
            owner: "acme".into(),
            repo: "shop".into(),
            number: self.n,
            remote: Some("origin".into()),
            head_sha: Some(self.head.clone()),
            repo_dir: Some(self.repo.path()),
            ..Default::default()
        }
    }

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

fn unit(symbol: &str, lines: (u32, u32), d: Decision, note: &str) -> Unit {
    Unit {
        id: format!("a.py::{symbol}"),
        file: PathBuf::from("a.py"),
        symbol: symbol.into(),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: Some(Range::new(lines.0, lines.1)),
        new_range: Some(Range::new(lines.0, lines.1)),
        renamed_from: None,
        fingerprint: format!("fp-{symbol}"),
        changes: Vec::new(),
        prep: Prep::Skipped,
        callers: Vec::new(),
        callees: Vec::new(),
        decision: d,
        summary: None,
        nbc_reason: None,
        dive: Dive {
            note: note.into(),
            ..Default::default()
        },
        accepted_by: None,
        reasons: Vec::new(),
        dive_pending: Default::default(),
        checks: Default::default(),
    }
}

/// The review of PR `o`: `f` is `d` with `note`, `g` is accepted.
fn pr_app(o: &Origin, d: Decision, note: &str) -> App {
    let rv = Review {
        id: format!("pr{}", o.n),
        repo_root: o.repo.path(),
        base: "b".into(),
        head: o.head.clone(),
        head_commit: o.head.clone(),
        agent_session: None,
        pr: Some(o.info()),
        intent: Default::default(),
        created_at: chrono::Utc::now(),
        files: vec![ChangedFile {
            path: PathBuf::from("a.py"),
            old_path: None,
            status: FileStatus::Modified,
            old_content: Some(OLD.into()),
            new_content: Some(NEW.into()),
        }],
        units: vec![
            unit("f", (1, 2), d, note),
            unit("g", (5, 6), Decision::Accepted, ""),
        ],
        skipped: Vec::new(),
        graph: Default::default(),
        questions: Vec::new(),
        version: 0,
        history: Default::default(),
        branch: None,
        cursor: None,
    };
    App::new(rv, Config::default(), Keymap::default())
}

struct Fakes {
    dir: tempfile::TempDir,
    tools: Tools,
}

/// `gh` records each call's argv and stdin, and answers `ok` or `fail`;
/// the opener records what it opened.
fn fakes(mode: &str) -> Fakes {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("mode"), mode).unwrap();
    let gh = script(
        d,
        "gh",
        r#"d=$(dirname "$0")
n=$(ls "$d" | grep -c '^call_')
n=$((n+1))
: > "$d/call_$n"
for a in "$@"; do printf '%s\n' "$a" >> "$d/call_$n"; done
cat > "$d/stdin_$n"
case "$(cat "$d/mode")" in
  ok) echo '{"id":1}';;
  fail) echo "HTTP 500: boom" >&2; exit 1;;
esac"#,
    );
    let opener = script(
        d,
        "opener",
        r#"d=$(dirname "$0"); printf '%s\n' "$@" >> "$d/opened""#,
    );
    Fakes {
        tools: Tools {
            gh: gh.into(),
            opener: opener.into(),
            editor: d.join("no-editor").into(),
            claude: d.join("no-claude").into(),
            codex: d.join("no-codex").into(),
        },
        dir,
    }
}

impl Fakes {
    fn calls(&self) -> usize {
        std::fs::read_dir(self.dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("call_")
            })
            .count()
    }

    fn stdin(&self, k: usize) -> serde_json::Value {
        let s = std::fs::read_to_string(self.dir.path().join(format!("stdin_{k}"))).unwrap();
        serde_json::from_str(&s).unwrap_or_else(|e| panic!("{e}: {s}"))
    }

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

/// Carry out `effects` (a post) with the fakes, and hand the answer back;
/// the effects that follow it.
async fn post(a: &mut App, fk: &Fakes, effects: Vec<Effect>) -> Vec<Effect> {
    assert!(
        matches!(&effects[..], [Effect::PostReview { .. }]),
        "{effects:?}"
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let rt = Runtime {
        adapter: None,
        tx,
        build: BuildOptions {
            base: None,
            languages: None,
        },
        tools: fk.tools.clone(),
    };
    for e in effects {
        rt.exec(a, e);
    }
    let ev = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("an answer")
        .unwrap();
    assert!(matches!(ev, AppEvent::PrReviewed { .. }));
    a.on_app_event(ev)
}

#[tokio::test]
async fn a_pr_with_something_flagged_requests_changes_with_the_notes_inline() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut a = pr_app(&o, Decision::Investigate, "off by one");
    keys(&mut a, ":w<cr>");
    assert_eq!(
        a.finish_steps(),
        [Step::RequestChanges, Step::CommentOnly, Step::Yank]
    );
    // Nothing reaches gh before ⏎.
    keys(&mut a, "jki");
    keys(&mut a, "y");
    assert_eq!(fk.calls(), 0);
    let e = keys(&mut a, "<cr>");
    let after = post(&mut a, &fk, e).await;
    assert_eq!(after, vec![Effect::Save, Effect::Quit]);
    assert_eq!(fk.calls(), 1);
    let v = fk.stdin(1);
    assert_eq!(v["event"], "REQUEST_CHANGES");
    assert_eq!(v["commit_id"], o.head.as_str());
    assert_eq!(v["body"], approve::DEFAULT_INTRO);
    assert_eq!(v["comments"][0]["body"], "off by one");
    assert_eq!(v["comments"][0]["path"], "a.py");
    assert_eq!(
        a.finish.exit_line.as_deref(),
        Some(format!("Changes requested on PR #{} on GitHub.", o.n).as_str())
    );
}

#[tokio::test]
async fn comment_only_posts_a_comment_and_yank_posts_nothing() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut a = pr_app(&o, Decision::Investigate, "why?");
    keys(&mut a, ":w<cr>jj");
    let e = keys(&mut a, "<cr>");
    assert!(matches!(&e[..], [Effect::Copy { .. }]), "{e:?}");
    assert_eq!(fk.calls(), 0);
    keys(&mut a, ":w<cr>j");
    let e = keys(&mut a, "<cr>");
    post(&mut a, &fk, e).await;
    assert_eq!(fk.stdin(1)["event"], "COMMENT");
}

#[tokio::test]
async fn a_pr_all_accepted_approves_or_approves_with_the_written_comment() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut a = pr_app(&o, Decision::Accepted, "");
    keys(&mut a, ":w<cr>");
    assert_eq!(
        a.finish_steps(),
        [Step::Approve, Step::ApproveWithComment, Step::Yank]
    );
    let e = keys(&mut a, "<cr>");
    let after = post(&mut a, &fk, e).await;
    assert_eq!(after, vec![Effect::Save, Effect::Quit]);
    assert_eq!(fk.stdin(1)["event"], "APPROVE");
    assert!(a.review.pr.as_ref().unwrap().is_approved());

    // With a comment: `i` writes it (the configured editor), ⏎ approves with it.
    let o = Origin::new();
    let fk = fakes("ok");
    let mut b = pr_app(&o, Decision::Accepted, "");
    keys(&mut b, ":w<cr>j");
    let e = keys(&mut b, "i");
    let [Effect::EditComment { text }] = &e[..] else {
        panic!("{e:?}")
    };
    assert_eq!(text, approve::DEFAULT_INTRO);
    let d = tempfile::tempdir().unwrap();
    let ed = script(d.path(), "ed", "printf 'Nice tests.\\n' > \"$1\"");
    b.comment_edited(approve::edit_in_editor(&ed.into(), text));
    assert_eq!(b.finish_summary(), "Nice tests.");
    let e = keys(&mut b, "<cr>");
    post(&mut b, &fk, e).await;
    let v = fk.stdin(1);
    assert_eq!(v["event"], "APPROVE");
    assert_eq!(v["body"], "Nice tests.");
}

#[test]
fn without_an_editor_the_summary_is_written_on_one_line() {
    let o = Origin::new();
    let mut a = pr_app(&o, Decision::Investigate, "x");
    keys(&mut a, ":w<cr>i");
    a.comment_edited(EditOutcome::Unavailable("no editor".into()));
    keys(&mut a, "<c-u>Please fix the bound.<cr>");
    assert_eq!(a.finish_summary(), "Please fix the bound.");
    assert!(v2::screen(&a, 100, 40).contains("Please fix the bound."));
}

#[tokio::test]
async fn a_moved_pr_head_is_refused_and_gh_never_runs() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut a = pr_app(&o, Decision::Accepted, "");
    o.move_head();
    keys(&mut a, ":w<cr>");
    let e = keys(&mut a, "<cr>");
    let after = post(&mut a, &fk, e).await;
    assert!(after.is_empty(), "teer stays: {after:?}");
    assert!(
        a.status.contains("has new commits since you reviewed it"),
        "{}",
        a.status
    );
    assert_eq!(fk.calls(), 0);
    assert!(!a.review.pr.as_ref().unwrap().is_approved());
    assert!(a.finish.exit_line.is_none());
}

#[tokio::test]
async fn a_failed_post_opens_the_pr_page_and_teer_stays() {
    let o = Origin::new();
    let fk = fakes("fail");
    let mut a = pr_app(&o, Decision::Investigate, "x");
    keys(&mut a, ":w<cr>");
    let e = keys(&mut a, "<cr>");
    let after = post(&mut a, &fk, e).await;
    assert!(after.is_empty());
    assert!(
        a.status.ends_with("opened the PR page instead"),
        "{}",
        a.status
    );
    assert_eq!(
        fk.opened().await,
        [format!("https://github.com/acme/shop/pull/{}", o.n)]
    );
    // It can be tried again.
    keys(&mut a, ":w<cr>");
    let e = keys(&mut a, "<cr>");
    assert!(matches!(&e[..], [Effect::PostReview { .. }]), "{e:?}");
}
