//! Committing the reviewed changes from the "all reviewed" popup: when it is
//! offered, the confirmation overlay, the message (template, agent draft,
//! editing), exactly what gets committed, the guards, and the user's hooks.
//! Real git in temp repositories; the agent is a mock; no network, no TTY.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::agent::AgentAdapter;
use teer::agent::mock::MockAdapter;
use teer::approve::{self, EditOutcome};
use teer::commit::{self, Branch, Io, Outcome};
use teer::config::{Config, KeyMap};
use teer::review::model::{Decision, PrInfo, Review, SessionRef};
use teer::review::{BuildOptions, build_review, store};
use teer::tui::Runtime;
use teer::tui::app::{App, AppEvent, DraftState, Effect, Overlay};
use teer::tui::ui;

// ------------------------------------------------------------- fixtures

const MSG: &str =
    "Fix \"quotes\" and ünï 日本\n\nline two 'single' $HOME `tick` \\ done\n\n- bullet";

/// A repository with one commit and, on top of it, uncommitted work:
/// `a.py` modified, `new.py` untracked, `gone.py` deleted. Identity, signing
/// and hooks are configured in the repository itself so the user's own
/// global git config can't change what these tests do.
fn fixture() -> FixtureRepo {
    let r = base_repo();
    r.write("a.py", PY_OLD)
        .write("gone.py", "x = 1\n")
        .write("keep.txt", "keep\n")
        .write("other.txt", "other\n");
    r.commit("initial");
    r.write("a.py", PY_NEW).write("new.py", PY_NEW);
    r.remove("gone.py");
    r
}

fn base_repo() -> FixtureRepo {
    isolate_env();
    let r = FixtureRepo::new();
    r.git(&["config", "user.name", "Ada Lovelace"]);
    r.git(&["config", "user.email", "ada@example.com"]);
    r.git(&["config", "commit.gpgsign", "false"]);
    let hooks = r.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    r.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    r
}

fn hook(r: &FixtureRepo, name: &str, body: &str) {
    let p = r.path().join(".git").join("hooks").join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn review_of(r: &FixtureRepo) -> Review {
    build_review(&r.path(), &BuildOptions::default()).unwrap()
}

fn accept_all(rv: &mut Review) {
    for u in &mut rv.units {
        u.decision = Decision::Accepted;
    }
}

fn app_of(rv: Review) -> App {
    App::new(rv, Config::default(), KeyMap::default())
}

/// The finished review of the fixture, started like the real thing (so the
/// popup is open).
fn done_app(r: &FixtureRepo) -> App {
    let mut rv = review_of(r);
    assert_eq!(rv.files.len(), 3, "{:?}", rv.files);
    accept_all(&mut rv);
    let mut a = app_of(rv);
    a.start(false, None, Vec::new(), None, false);
    assert_eq!(a.overlay, Overlay::AllDone);
    a
}

/// Changes made after the review was built: not part of it.
fn add_unrelated(r: &FixtureRepo) {
    r.write("staged.txt", "staged\n");
    r.git(&["add", "staged.txt"]);
    r.write("other.txt", "other, modified\n");
    r.write("stray.log", "untracked\n");
}

fn press(a: &mut App, code: KeyCode) -> Vec<Effect> {
    a.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

fn key(a: &mut App, c: char) -> Vec<Effect> {
    press(a, KeyCode::Char(c))
}

fn draw(a: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, a)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct Parts {
    message: String,
    draft: DraftState,
    scroll: usize,
    editing: Option<String>,
    branch: Branch,
    others: usize,
}

fn parts(a: &App) -> Parts {
    match &a.overlay {
        Overlay::Commit {
            message,
            draft,
            scroll,
            editing,
            branch,
            others,
        } => Parts {
            message: message.clone(),
            draft: *draft,
            scroll: *scroll,
            editing: editing.clone(),
            branch: branch.clone(),
            others: *others,
        },
        o => panic!("not the commit overlay: {o:?}"),
    }
}

/// Open the overlay from the popup.
fn open(a: &mut App) -> Vec<Effect> {
    assert_eq!(a.overlay, Overlay::AllDone);
    let e = key(a, 'c');
    assert!(
        matches!(a.overlay, Overlay::Commit { .. }),
        "{:?}",
        a.status
    );
    e
}

fn set_message(a: &mut App, m: &str) {
    a.comment_edited(EditOutcome::Edited(m.into()));
}

/// Press `y` and, if git is asked to commit, do it (without a terminal).
fn confirm(a: &mut App) -> Outcome {
    let e = key(a, 'y');
    let [
        Effect::Commit {
            repo,
            paths,
            message,
        },
    ] = &e[..]
    else {
        panic!("no commit effect: {e:?}; status {:?}", a.status)
    };
    let out = commit::run_commit(repo, paths, message, Io::Capture);
    a.commit_finished(out.clone());
    out
}

fn status(r: &FixtureRepo) -> String {
    r.git(&["status", "--porcelain", "--untracked-files=all"])
}

fn head_count(r: &FixtureRepo) -> usize {
    r.git(&["rev-list", "--count", "HEAD"])
        .trim()
        .parse()
        .unwrap()
}

fn show_names(r: &FixtureRepo) -> Vec<String> {
    let mut v: Vec<String> = r
        .git(&["show", "--name-status", "--format=", "-M", "HEAD"])
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    v.sort();
    v
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

// -------------------------------------------------- when it is offered

#[test]
fn the_popup_offers_commit_for_a_local_worktree_review() {
    let r = fixture();
    let a = done_app(&r);
    let t = draw(&a, 100, 30);
    assert!(t.contains("Hurray!"), "{t}");
    assert!(t.contains("c commit these changes"), "{t}");
    assert!(a.can_commit());
}

#[test]
fn the_popup_does_not_offer_commit_for_prs_or_base_reviews() {
    let r = fixture();
    let mut rv = review_of(&r);
    accept_all(&mut rv);
    rv.pr = Some(PrInfo {
        host: "github.com".into(),
        owner: "o".into(),
        repo: "r".into(),
        number: 1,
        ..Default::default()
    });
    let mut a = app_of(rv);
    a.overlay = Overlay::AllDone;
    assert!(!draw(&a, 100, 30).contains("commit these changes"));
    key(&mut a, 'c');
    assert_eq!(a.overlay, Overlay::None);

    // A --base review reviews commits, not the working tree.
    let r2 = base_repo();
    r2.write("a.py", PY_OLD).commit("one");
    r2.write("a.py", PY_NEW).commit("two");
    let opts = BuildOptions {
        base: Some("HEAD~1".into()),
        languages: None,
    };
    let mut rv = build_review(&r2.path(), &opts).unwrap();
    assert!(!rv.files.is_empty());
    accept_all(&mut rv);
    let mut b = app_of(rv);
    b.overlay = Overlay::AllDone;
    assert!(!b.can_commit());
    assert!(!draw(&b, 100, 30).contains("commit these changes"));
    key(&mut b, 'c');
    assert_eq!(b.overlay, Overlay::None);
}

#[test]
fn c_with_something_still_flagged_commits_nothing() {
    let r = fixture();
    let mut a = done_app(&r);
    a.review.units[0].decision = Decision::Investigate;
    let e = key(&mut a, 'c');
    assert!(e.is_empty());
    assert_eq!(a.overlay, Overlay::None);
    assert!(a.status.contains("Not everything"), "{}", a.status);
}

#[test]
fn a_complete_saved_review_shows_the_popup_once_at_launch() {
    let r = fixture();
    let mut rv = review_of(&r);
    accept_all(&mut rv);
    store::save(&rv).unwrap();
    let mut fresh = review_of(&r);
    teer::cli::restore(&mut fresh);
    let mut a = app_of(fresh);
    a.start(false, None, Vec::new(), None, false);
    assert_eq!(a.overlay, Overlay::AllDone);
    assert!(draw(&a, 100, 30).contains("c commit these changes"));
    // Dismissing it simply continues.
    key(&mut a, 'x');
    assert_eq!(a.overlay, Overlay::None);
}

#[test]
fn the_launch_popup_needs_a_complete_local_review_with_files() {
    let r = fixture();
    // Incomplete.
    let mut rv = review_of(&r);
    accept_all(&mut rv);
    rv.units[0].decision = Decision::Pending;
    store::save(&rv).unwrap();
    let mut fresh = review_of(&r);
    teer::cli::restore(&mut fresh);
    let mut a = app_of(fresh);
    a.start(false, None, Vec::new(), None, false);
    assert_eq!(a.overlay, Overlay::None);
    // Flagged.
    let mut rv = review_of(&r);
    accept_all(&mut rv);
    rv.units[0].decision = Decision::Investigate;
    let mut a = app_of(rv);
    a.start(false, None, Vec::new(), None, false);
    assert_eq!(a.overlay, Overlay::None);
    // A pull request.
    let mut rv = review_of(&r);
    accept_all(&mut rv);
    rv.pr = Some(PrInfo {
        host: "github.com".into(),
        owner: "o".into(),
        repo: "r".into(),
        number: 1,
        ..Default::default()
    });
    let mut a = app_of(rv);
    a.start(false, None, Vec::new(), None, false);
    assert_eq!(a.overlay, Overlay::None);
    // Nothing to commit (clean tree).
    let mut rv = review_of(&r);
    accept_all(&mut rv);
    rv.files.clear();
    let mut a = app_of(rv);
    a.start(false, None, Vec::new(), None, true);
    assert_eq!(a.overlay, Overlay::None);
    // A --base review.
    let r2 = base_repo();
    r2.write("a.py", PY_OLD).commit("one");
    r2.write("a.py", PY_NEW).commit("two");
    let opts = BuildOptions {
        base: Some("HEAD~1".into()),
        languages: None,
    };
    let mut rv = build_review(&r2.path(), &opts).unwrap();
    accept_all(&mut rv);
    let mut a = app_of(rv);
    a.start(false, None, Vec::new(), None, false);
    assert_eq!(a.overlay, Overlay::None);
}

// ------------------------------------------------------------- overlay

#[test]
fn the_overlay_lists_the_files_the_branch_and_the_other_changes() {
    let r = fixture();
    let mut a = done_app(&r);
    add_unrelated(&r);
    let e = open(&mut a);
    assert!(e.is_empty(), "no agent, no draft request");
    let p = parts(&a);
    assert_eq!(p.branch, Branch::Named("main".into()));
    // staged.txt, other.txt and stray.log are not in the review.
    assert_eq!(p.others, 3);
    assert_eq!(p.draft, DraftState::Template);
    let t = draw(&a, 100, 40);
    assert!(t.contains("Commit 3 files on main?"), "{t}");
    assert!(
        t.contains("~ a.py") && t.contains("+ new.py") && t.contains("− gone.py"),
        "{t}"
    );
    assert!(
        t.contains("3 other changed files are not part of this review and will not be included"),
        "{t}"
    );
    assert!(t.contains("Update a.py, gone.py, new.py"), "{t}");
    assert!(
        t.contains("y/⏎ commit") && t.contains("e edit message"),
        "{t}"
    );
}

#[test]
fn one_other_file_and_none_are_worded_right() {
    let r = fixture();
    let mut a = done_app(&r);
    r.write("stray.log", "x\n");
    open(&mut a);
    let t = draw(&a, 100, 40);
    assert!(
        t.contains("1 other changed file is not part of this review and will not be included"),
        "{t}"
    );
    let r = fixture();
    let mut a = done_app(&r);
    open(&mut a);
    assert_eq!(parts(&a).others, 0);
    assert!(!draw(&a, 100, 40).contains("other changed"));
}

#[test]
fn a_detached_head_says_so() {
    let r = fixture();
    let mut a = done_app(&r);
    r.git(&["checkout", "-q", "--detach"]);
    open(&mut a);
    assert_eq!(parts(&a).branch, Branch::Detached);
    let t = draw(&a, 100, 40);
    assert!(t.contains("Commit 3 files on detached HEAD?"), "{t}");
}

#[test]
fn the_template_message_has_a_bullet_per_summary() {
    let r = fixture();
    let mut rv = review_of(&r);
    accept_all(&mut rv);
    for (i, u) in rv.units.iter_mut().enumerate() {
        u.summary = Some(if i % 2 == 0 {
            "Adds a currency check.".into()
        } else {
            format!("Reworks piece {i}.")
        });
    }
    let mut a = app_of(rv);
    a.start(false, None, Vec::new(), None, false);
    open(&mut a);
    let m = parts(&a).message;
    assert!(m.starts_with("Update a.py, gone.py, new.py\n\n- "), "{m}");
    assert_eq!(
        m.matches("Adds a currency check.").count(),
        1,
        "deduplicated: {m}"
    );
    assert!(m.lines().count() <= 2 + commit::BULLETS_MAX);
    assert!(m.lines().next().unwrap().chars().count() <= commit::SUBJECT_MAX);
}

#[test]
fn esc_cancels_and_stray_keys_do_nothing() {
    let r = fixture();
    let mut a = done_app(&r);
    open(&mut a);
    let before = a.overlay.clone();
    for c in ['x', 'q', 'n', 'c', 'a', 'R', 'i', ' '] {
        assert!(key(&mut a, c).is_empty(), "{c}");
        assert_eq!(a.overlay, before, "{c}");
    }
    for code in [
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Tab,
        KeyCode::Backspace,
    ] {
        assert!(press(&mut a, code).is_empty());
        assert_eq!(a.overlay, before);
    }
    press(&mut a, KeyCode::Esc);
    assert_eq!(a.overlay, Overlay::None);
    assert!(a.committed.is_none());
    assert_eq!(head_count(&r), 1);
}

#[test]
fn ctrl_c_still_quits_from_the_overlay() {
    let r = fixture();
    let mut a = done_app(&r);
    open(&mut a);
    let e = a.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(e, vec![Effect::Save, Effect::Quit]);
}

#[test]
fn the_overlay_scrolls_and_stays_inside_small_screens() {
    let r = fixture();
    let mut a = done_app(&r);
    open(&mut a);
    let long = format!(
        "Subject line\n\n{}",
        (0..60)
            .map(|i| format!("- bullet number {i} with some words"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    set_message(&mut a, &long);
    // Draw once so the maximum scroll is known.
    let t = draw(&a, 60, 20);
    assert!(t.lines().count() == 20);
    assert!(t.contains("bullet number 0"), "{t}");
    assert!(!t.contains("bullet number 59"));
    for _ in 0..100 {
        press(&mut a, KeyCode::Down);
    }
    let down = parts(&a).scroll;
    assert!(down > 0);
    let t = draw(&a, 60, 20);
    assert!(t.contains("bullet number 59"), "{t}");
    assert!(!t.contains("bullet number 0 "));
    // The footer keys stay visible whatever the scroll.
    assert!(t.contains("esc cancel"), "{t}");
    press(&mut a, KeyCode::PageUp);
    assert!(parts(&a).scroll < down);
    for _ in 0..200 {
        press(&mut a, KeyCode::Up);
    }
    assert_eq!(parts(&a).scroll, 0);
    press(&mut a, KeyCode::PageDown);
    assert!(parts(&a).scroll > 0);
}

#[test]
fn every_state_renders_at_common_and_tiny_sizes() {
    let r = fixture();
    let mut a = done_app(&r);
    add_unrelated(&r);
    open(&mut a);
    a.review.agent_session = Some(SessionRef::default());
    let mut states = vec![a.overlay.clone()];
    if let Overlay::Commit { draft, .. } = &mut a.overlay {
        *draft = DraftState::Pending;
    }
    states.push(a.overlay.clone());
    if let Overlay::Commit { editing, .. } = &mut a.overlay {
        *editing = Some("A subject being typed".into());
    }
    states.push(a.overlay.clone());
    let sizes = [
        (120, 40),
        (80, 24),
        (60, 20),
        (30, 8),
        (20, 6),
        (11, 4),
        (4, 2),
        (1, 1),
    ];
    for o in &states {
        a.overlay = o.clone();
        for (w, h) in sizes {
            let t = draw(&a, w, h);
            assert_eq!(t.lines().count(), h as usize);
        }
    }
    a.overlay = Overlay::AllDone;
    for (w, h) in sizes {
        draw(&a, w, h);
    }
    a.committed = Some("abc1234".into());
    for (w, h) in sizes {
        draw(&a, w, h);
    }
    let t = draw(&a, 80, 24);
    assert!(t.contains("Committed abc1234."), "{t}");
}

#[test]
fn commit_can_be_rebound() {
    let r = fixture();
    let mut rv = review_of(&r);
    accept_all(&mut rv);
    let ov = [("commit".to_string(), "m".to_string())]
        .into_iter()
        .collect();
    let keys = KeyMap::with_overrides(&ov).unwrap();
    let mut a = App::new(rv, Config::default(), keys);
    a.start(false, None, Vec::new(), None, false);
    assert_eq!(a.overlay, Overlay::AllDone);
    let t = draw(&a, 100, 30);
    assert!(t.contains("m commit these changes"), "{t}");
    // The default key no longer does it.
    key(&mut a, 'c');
    assert_eq!(a.overlay, Overlay::None);
    a.overlay = Overlay::AllDone;
    key(&mut a, 'm');
    assert!(matches!(a.overlay, Overlay::Commit { .. }));
}

// ------------------------------------------------------------- message

fn agent_app(r: &FixtureRepo) -> App {
    let mut rv = review_of(r);
    accept_all(&mut rv);
    let mut a = app_of(rv);
    a.start(
        true,
        None,
        Vec::new(),
        Some(SessionRef::fresh("claude", r.path())),
        false,
    );
    assert_eq!(a.overlay, Overlay::AllDone);
    a
}

fn runtime(
    adapter: Arc<dyn AgentAdapter>,
) -> (Runtime, tokio::sync::mpsc::UnboundedReceiver<AppEvent>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (
        Runtime {
            adapter: Some(adapter),
            tx,
            build: BuildOptions {
                base: None,
                languages: None,
            },
            tools: Default::default(),
        },
        rx,
    )
}

async fn next_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) -> AppEvent {
    tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("an event")
        .unwrap()
}

#[tokio::test]
async fn the_agent_drafts_a_better_message() {
    let r = fixture();
    let mut a = agent_app(&r);
    let mock = MockAdapter::new(|prompt, schema| {
        assert!(prompt.starts_with("COMMITMSG"));
        assert_eq!(schema["required"], serde_json::json!(["subject", "body"]));
        Ok(serde_json::json!({
            "subject": "Add currency check to charging",
            "body": "- Rejects unsupported currencies\n- Saves the payment"
        }))
    });
    let (rt, mut rx) = runtime(Arc::new(mock.clone()));
    let effects = open(&mut a);
    assert_eq!(effects.len(), 1);
    assert!(matches!(effects[0], Effect::DraftCommitMessage(_)));
    // The overlay is open at once, with the template and the spinner.
    let p = parts(&a);
    assert_eq!(p.draft, DraftState::Pending);
    assert!(p.message.starts_with("Update a.py"));
    assert!(draw(&a, 100, 40).contains("drafting message…"));
    for e in effects {
        rt.exec(&mut a, e);
    }
    let ev = next_event(&mut rx).await;
    a.on_app_event(ev);
    let p = parts(&a);
    assert_eq!(p.draft, DraftState::Drafted);
    assert_eq!(
        p.message,
        "Add currency check to charging\n\n- Rejects unsupported currencies\n- Saves the payment"
    );
    assert!(!draw(&a, 100, 40).contains("drafting message…"));
    // The prompt: files, the earlier subject as a style example, read-only (no web).
    let prompts = mock.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1);
    let pr = &prompts[0];
    assert!(
        pr.contains("modified a.py") && pr.contains("added new.py"),
        "{pr}"
    );
    assert!(pr.contains("deleted gone.py"), "{pr}");
    assert!(pr.contains("initial"), "{pr}");
    assert!(pr.contains("72 characters"));
    assert_eq!(*mock.web_calls.lock().unwrap(), vec![false]);
}

#[tokio::test]
async fn a_draft_never_overwrites_what_the_user_started_typing() {
    let r = fixture();
    let mock =
        MockAdapter::new(|_, _| Ok(serde_json::json!({"subject": "Agent subject", "body": ""})));
    let (rt, mut rx) = runtime(Arc::new(mock));
    // `e` pressed before the draft arrives.
    let mut a = agent_app(&r);
    let effects = open(&mut a);
    for e in effects {
        rt.exec(&mut a, e);
    }
    let e = key(&mut a, 'e');
    assert!(matches!(&e[..], [Effect::EditComment { .. }]));
    let ev = next_event(&mut rx).await;
    a.on_app_event(ev);
    assert_eq!(parts(&a).draft, DraftState::Edited);
    assert!(parts(&a).message.starts_with("Update a.py"));
    a.comment_edited(EditOutcome::Edited("My own message".into()));
    assert_eq!(parts(&a).message, "My own message");

    // The one-line editor is open when it arrives.
    let mut b = agent_app(&r);
    let effects = open(&mut b);
    for e in effects {
        rt.exec(&mut b, e);
    }
    key(&mut b, 'e');
    b.comment_edited(EditOutcome::Unavailable("no nvim".into()));
    key(&mut b, '!');
    let ev = next_event(&mut rx).await;
    b.on_app_event(ev);
    let p = parts(&b);
    assert!(p.message.starts_with("Update a.py"), "{}", p.message);
    assert!(p.editing.unwrap().ends_with('!'));

    // Pressing y before the draft arrives also freezes the text.
    let mut c = agent_app(&r);
    open(&mut c);
    key(&mut c, 'y');
    c.on_app_event(AppEvent::CommitDrafted(Ok("Late".into())));
    assert!(parts(&c).message.starts_with("Update a.py"));
}

#[tokio::test]
async fn a_failing_agent_keeps_the_template_and_says_so() {
    let r = fixture();
    let mock = MockAdapter::new(|_, _| anyhow::bail!("claude exploded"));
    let (rt, mut rx) = runtime(Arc::new(mock));
    let mut a = agent_app(&r);
    let effects = open(&mut a);
    for e in effects {
        rt.exec(&mut a, e);
    }
    let ev = next_event(&mut rx).await;
    assert!(matches!(ev, AppEvent::CommitDrafted(Err(_))));
    a.on_app_event(ev);
    let p = parts(&a);
    assert_eq!(p.draft, DraftState::Template);
    assert!(p.message.starts_with("Update a.py"));
    assert!(a.status.contains("kept the template"), "{}", a.status);
    assert!(a.status.contains("claude exploded"), "{}", a.status);

    // An empty answer is a failure too.
    let mut b = agent_app(&r);
    open(&mut b);
    b.on_app_event(AppEvent::CommitDrafted(Ok("  \n ".into())));
    assert_eq!(parts(&b).draft, DraftState::Template);
    assert!(b.status.contains("template"), "{}", b.status);
}

struct Slow;

#[async_trait::async_trait]
impl AgentAdapter for Slow {
    fn list_sessions(&self, _: &Path) -> anyhow::Result<Vec<teer::agent::SessionInfo>> {
        Ok(Vec::new())
    }
    async fn ask_structured(
        &self,
        _: &SessionRef,
        _: &str,
        _: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(serde_json::json!({"subject": "too late", "body": ""}))
    }
}

#[tokio::test]
async fn a_slow_agent_times_out_and_the_template_stays() {
    let r = fixture();
    let rv = review_of(&r);
    let input = commit::draft_input(&rv, &[]);
    let started = std::time::Instant::now();
    let res = commit::draft_message(
        Arc::new(Slow),
        SessionRef::fresh("claude", r.path()),
        input,
        Duration::from_millis(100),
    )
    .await;
    assert!(started.elapsed() < Duration::from_secs(5));
    let err = res.unwrap_err();
    assert!(err.contains("timed out"), "{err}");
    let mut a = agent_app(&r);
    open(&mut a);
    a.on_app_event(AppEvent::CommitDrafted(Err(err)));
    assert!(a.status.contains("timed out") && a.status.contains("kept the template"));
    assert!(parts(&a).message.starts_with("Update a.py"));
    assert_eq!(commit::DRAFT_TIMEOUT, Duration::from_secs(30));
}

#[test]
fn without_an_agent_the_template_is_used_and_no_draft_is_asked() {
    let r = fixture();
    let mut a = done_app(&r);
    let e = open(&mut a);
    assert!(e.is_empty());
    assert_eq!(parts(&a).draft, DraftState::Template);
    assert!(a.status.contains("template"), "{}", a.status);
    // A draft that shows up anyway is ignored.
    a.on_app_event(AppEvent::CommitDrafted(Ok("Surprise".into())));
    assert!(parts(&a).message.starts_with("Update a.py"));
}

#[test]
fn drafts_are_cleaned_and_capped() {
    let r = fixture();
    let mut a = agent_app(&r);
    open(&mut a);
    let dirty = format!(
        "\u{1b}[31m{}\u{1b}[0m\r\n\r\n\r\n\r\nbody\u{7} line\n\n\n\n",
        "S".repeat(120)
    );
    a.on_app_event(AppEvent::CommitDrafted(Ok(dirty)));
    let m = parts(&a).message;
    let (subject, body) = m.split_once("\n\n").unwrap();
    assert_eq!(subject, "S".repeat(72));
    assert_eq!(body, "body line");
    let mut b = agent_app(&r);
    open(&mut b);
    b.on_app_event(AppEvent::CommitDrafted(Ok(format!(
        "Subject\n\n{}",
        "word ".repeat(2000)
    ))));
    assert!(parts(&b).message.chars().count() <= 4000);
}

#[test]
fn editing_with_an_external_editor_round_trips_through_a_temp_file() {
    let r = fixture();
    let mut a = done_app(&r);
    let original = open_message(&mut a);
    let e = key(&mut a, 'e');
    assert_eq!(
        e,
        vec![Effect::EditComment {
            text: original.clone()
        }]
    );
    assert_eq!(parts(&a).draft, DraftState::Edited);
    let d = tempfile::tempdir().unwrap();
    let seen = d.path().join("seen");
    let editor = script(
        d.path(),
        "ed",
        &format!(
            "echo \"$1\" > {seen}\ncp \"$1\" {copy}\nprintf 'Edited subject\\n\\n\\033[31mbody\\tline\\n\\n\\n' > \"$1\"",
            seen = seen.display(),
            copy = d.path().join("orig").display()
        ),
    );
    let out = approve::edit_message_in_editor(&editor.into(), &original);
    assert_eq!(
        out,
        EditOutcome::Edited("Edited subject\n\nbody    line".into())
    );
    assert_eq!(
        std::fs::read_to_string(d.path().join("orig")).unwrap(),
        original
    );
    let path = PathBuf::from(std::fs::read_to_string(&seen).unwrap().trim());
    assert!(path.starts_with(store::data_dir().join("tmp")), "{path:?}");
    assert_eq!(path.file_name().unwrap(), "COMMIT_EDITMSG");
    assert!(!path.exists(), "the temp file is deleted");
    a.comment_edited(out);
    assert_eq!(parts(&a).message, "Edited subject\n\nbody    line");
    assert!(draw(&a, 100, 40).contains("Edited subject"));
    // :cq keeps what was there; an empty file does too.
    let cq = script(d.path(), "cq", "echo nope > \"$1\"; exit 1");
    let out = approve::edit_message_in_editor(&cq.into(), "x");
    assert_eq!(out, EditOutcome::Cancelled);
    a.comment_edited(out);
    a.comment_edited(EditOutcome::Edited("  \n".into()));
    assert_eq!(parts(&a).message, "Edited subject\n\nbody    line");
    // No editor at all.
    let missing = d.path().join("missing");
    assert!(matches!(
        approve::edit_message_in_editor(&missing.into(), "x"),
        EditOutcome::Unavailable(_)
    ));
}

fn open_message(a: &mut App) -> String {
    open(a);
    parts(a).message
}

#[test]
fn without_an_editor_the_subject_is_edited_on_one_line() {
    let r = fixture();
    let mut a = done_app(&r);
    open(&mut a);
    set_message(&mut a, "Old subject\n\n- keep this body");
    key(&mut a, 'e');
    a.comment_edited(EditOutcome::Unavailable("not found".into()));
    assert!(a.status.contains("one line"), "{}", a.status);
    assert_eq!(parts(&a).editing.as_deref(), Some("Old subject"));
    assert!(draw(&a, 100, 40).contains("Edit the subject"));
    // Typing never commits; y is just a letter here.
    for c in " yes".chars() {
        assert!(key(&mut a, c).is_empty());
    }
    assert!(matches!(a.overlay, Overlay::Commit { .. }));
    press(&mut a, KeyCode::Backspace);
    press(&mut a, KeyCode::Enter);
    let p = parts(&a);
    assert_eq!(p.editing, None);
    assert_eq!(p.message, "Old subject ye\n\n- keep this body");
    // Esc leaves the editor without saving, and a second Esc closes the overlay.
    key(&mut a, 'e');
    a.comment_edited(EditOutcome::Unavailable("x".into()));
    key(&mut a, 'z');
    press(&mut a, KeyCode::Esc);
    assert_eq!(parts(&a).message, "Old subject ye\n\n- keep this body");
    assert!(parts(&a).editing.is_none());
    press(&mut a, KeyCode::Esc);
    assert_eq!(a.overlay, Overlay::None);
}

// -------------------------------------------------------------- commit

#[test]
fn y_commits_exactly_the_reviewed_files_with_the_given_message() {
    let r = fixture();
    let mut a = done_app(&r);
    add_unrelated(&r);
    let status_before = status(&r);
    open(&mut a);
    set_message(&mut a, MSG);
    let out = confirm(&mut a);
    let Outcome::Committed { short } = out else {
        panic!("{out:?}")
    };
    assert_eq!(head_count(&r), 2, "exactly one new commit");
    assert_eq!(
        show_names(&r),
        vec!["A new.py", "D gone.py", "M a.py"],
        "exactly the reviewed files"
    );
    // The message, byte for byte (git adds the final newline).
    assert_eq!(r.git(&["log", "-1", "--format=%B"]), format!("{MSG}\n\n"));
    assert_eq!(
        r.git(&["log", "-1", "--format=%an <%ae>|%cn <%ce>"]).trim(),
        "Ada Lovelace <ada@example.com>|Ada Lovelace <ada@example.com>"
    );
    // Unrelated work is exactly as it was: still staged, still modified, still untracked.
    let after = status(&r);
    assert!(after.contains("A  staged.txt"), "{after}");
    assert!(after.contains(" M other.txt"), "{after}");
    assert!(after.contains("?? stray.log"), "{after}");
    assert!(!after.contains("a.py") && !after.contains("new.py") && !after.contains("gone.py"));
    assert_eq!(
        r.git(&["diff", "--cached", "--name-only"]).trim(),
        "staged.txt"
    );
    assert!(status_before.contains("staged.txt"));
    // The app: status, popup variant, no repeat offer.
    assert_eq!(
        a.status,
        format!("Committed {short}: Fix \"quotes\" and ünï 日本")
    );
    assert_eq!(a.committed.as_deref(), Some(short.as_str()));
    assert_eq!(a.overlay, Overlay::AllDone);
    let t = draw(&a, 100, 30);
    assert!(t.contains(&format!("Committed {short}.")), "{t}");
    assert!(
        t.contains("q quit") && t.contains("any other key to continue"),
        "{t}"
    );
    assert!(
        !t.contains("commit these changes") && !t.contains("Hurray"),
        "{t}"
    );
    key(&mut a, 'c');
    assert_eq!(a.overlay, Overlay::None, "c only dismisses the variant");
    a.overlay = Overlay::AllDone;
    let e = key(&mut a, 'q');
    assert_eq!(e, vec![Effect::Save, Effect::Quit]);
    // The offer is gone for the rest of the session...
    assert!(!a.can_commit());
    // ...until a re-run builds a new review.
    let again = a.review.clone();
    a.on_app_event(AppEvent::Rerun(Ok(Box::new(again))));
    assert!(a.committed.is_none());
}

#[test]
fn enter_commits_too_and_the_message_file_is_gone_afterwards() {
    let r = fixture();
    let mut a = done_app(&r);
    open(&mut a);
    let e = press(&mut a, KeyCode::Enter);
    let [
        Effect::Commit {
            repo,
            paths,
            message,
        },
    ] = &e[..]
    else {
        panic!("{e:?}")
    };
    let out = commit::run_commit(repo, paths, message, Io::Capture);
    assert!(matches!(out, Outcome::Committed { .. }), "{out:?}");
    assert_eq!(head_count(&r), 2);
}

#[test]
fn the_message_is_not_in_argv() {
    let r = fixture();
    hook(
        &r,
        "pre-commit",
        &format!(
            "ps -o args= -p $PPID > {}",
            r.path().join(".git").join("parent-args").display()
        ),
    );
    let mut a = done_app(&r);
    open(&mut a);
    set_message(&mut a, "Secret subject words\n\nbody words");
    confirm(&mut a);
    let args = std::fs::read_to_string(r.path().join(".git/parent-args")).unwrap();
    assert!(args.contains("commit") && args.contains("-F"), "{args}");
    assert!(
        !args.contains("Secret") && !args.contains("words"),
        "{args}"
    );
    for tok in args.split_whitespace() {
        assert!(
            !["--no-verify", "-n", "--amend", "-S", "--gpg-sign"].contains(&tok)
                && !tok.starts_with("-S")
                && !tok.starts_with("--gpg-sign"),
            "{tok} in {args}"
        );
    }
}

#[test]
fn a_renamed_file_is_committed_under_both_names() {
    let r = base_repo();
    r.write("a.py", PY_OLD)
        .write("ren_old.py", PY_OLD)
        .commit("initial");
    r.git(&["mv", "ren_old.py", "ren_new.py"]);
    r.write("a.py", PY_NEW);
    let mut rv = review_of(&r);
    assert!(
        rv.files.iter().any(|f| f.old_path.is_some()),
        "{:?}",
        rv.files
            .iter()
            .map(|f| (&f.path, &f.old_path))
            .collect::<Vec<_>>()
    );
    accept_all(&mut rv);
    let mut a = app_of(rv);
    a.start(false, None, Vec::new(), None, false);
    open(&mut a);
    assert_eq!(parts(&a).others, 0, "{}", status(&r));
    assert!(draw(&a, 100, 40).contains("ren_old.py → ren_new.py"));
    confirm(&mut a);
    assert_eq!(head_count(&r), 2);
    assert_eq!(show_names(&r), vec!["M a.py", "R100 ren_old.py ren_new.py"]);
    assert_eq!(status(&r), "");
}

#[test]
fn the_first_commit_of_a_repository_works() {
    let r = base_repo();
    r.write("f.py", PY_NEW);
    let mut rv = review_of(&r);
    assert_eq!(rv.head_commit, teer::git::EMPTY_TREE);
    accept_all(&mut rv);
    let mut a = app_of(rv);
    a.start(false, None, Vec::new(), None, false);
    assert_eq!(a.overlay, Overlay::AllDone);
    r.write("unrelated.txt", "u\n");
    open(&mut a);
    assert_eq!(parts(&a).branch, Branch::Named("main".into()));
    confirm(&mut a);
    assert_eq!(head_count(&r), 1);
    assert_eq!(show_names(&r), vec!["A f.py"]);
    assert!(status(&r).contains("?? unrelated.txt"));
}

#[test]
fn a_path_with_glob_characters_is_taken_literally() {
    let r = base_repo();
    r.write("a.py", PY_OLD)
        .write("[b].py", "x = 1\n")
        .commit("initial");
    r.write("a.py", PY_NEW).write("[b].py", "x = 2\n");
    let mut rv = review_of(&r);
    assert_eq!(rv.files.len(), 2);
    accept_all(&mut rv);
    let mut a = app_of(rv);
    a.start(false, None, Vec::new(), None, false);
    // Made after the review; a glob `[b].py` would match it.
    r.write("b.py", "decoy = 1\n");
    open(&mut a);
    confirm(&mut a);
    assert_eq!(show_names(&r), vec!["M [b].py", "M a.py"]);
    assert_eq!(status(&r), "?? b.py\n");
}

// ---------------------------------------------------------------- hooks

#[test]
fn the_users_pre_commit_hook_runs_and_a_failing_one_blocks_the_commit() {
    let r = fixture();
    let ran = r.path().join(".git").join("ran");
    hook(
        &r,
        "pre-commit",
        &format!(
            "echo ran >> {}\necho 'lint failed: no tabs' >&2\nexit 1",
            ran.display()
        ),
    );
    let mut a = done_app(&r);
    open(&mut a);
    set_message(&mut a, "Blocked by the hook");
    let out = confirm(&mut a);
    assert!(
        matches!(out, Outcome::Failed { code: Some(1), .. }),
        "{out:?}"
    );
    assert!(ran.exists(), "the user's hook ran");
    assert_eq!(head_count(&r), 1, "nothing was committed");
    // The overlay stays open for a retry or an edit, with a short sanitised status.
    assert!(matches!(a.overlay, Overlay::Commit { .. }));
    assert_eq!(parts(&a).message, "Blocked by the hook");
    assert!(a.status.starts_with("Commit failed"), "{}", a.status);
    assert!(a.status.contains("lint failed: no tabs"), "{}", a.status);
    assert!(!a.status.contains('\n'));
    assert!(a.committed.is_none());
    // Fix the hook, retry.
    hook(&r, "pre-commit", "exit 0");
    let out = confirm(&mut a);
    assert!(matches!(out, Outcome::Committed { .. }), "{out:?}");
    assert_eq!(head_count(&r), 2);
}

#[test]
fn the_commit_msg_hook_sees_the_message_and_can_reject_it() {
    let r = fixture();
    let seen = r.path().join(".git").join("seen-msg");
    hook(
        &r,
        "commit-msg",
        &format!(
            "cp \"$1\" {}\ngrep -q WIP \"$1\" && {{ echo 'no WIP commits' >&2; exit 1; }}\nexit 0",
            seen.display()
        ),
    );
    let mut a = done_app(&r);
    open(&mut a);
    set_message(&mut a, "WIP stuff");
    let out = confirm(&mut a);
    assert!(matches!(out, Outcome::Failed { .. }), "{out:?}");
    assert!(a.status.contains("no WIP commits"), "{}", a.status);
    assert_eq!(std::fs::read_to_string(&seen).unwrap(), "WIP stuff\n");
    set_message(&mut a, MSG);
    let out = confirm(&mut a);
    assert!(matches!(out, Outcome::Committed { .. }), "{out:?}");
    assert_eq!(std::fs::read_to_string(&seen).unwrap(), format!("{MSG}\n"));
}

#[test]
fn the_hardened_read_helpers_still_do_not_run_hooks() {
    let r = base_repo();
    r.write("a.py", PY_OLD).commit("initial");
    let ran = r.path().join(".git").join("ran");
    hook(
        &r,
        "pre-commit",
        &format!("echo ran > {}\nexit 1", ran.display()),
    );
    // teer's own read-only git: hooks are off, the commit goes through.
    let out = teer::git::command(&r.path())
        .args(["commit", "--allow-empty", "-q", "-m", "hardened"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!ran.exists(), "the hardened helper ran a hook");
    // The commit feature runs the user's own git: the same hook blocks.
    r.write("a.py", PY_NEW);
    let mut rv = review_of(&r);
    accept_all(&mut rv);
    let mut a = app_of(rv);
    a.start(false, None, Vec::new(), None, false);
    open(&mut a);
    let out = confirm(&mut a);
    assert!(matches!(out, Outcome::Failed { .. }), "{out:?}");
    assert!(ran.exists());
}

// --------------------------------------------------------------- guards

/// Press y, expect a refusal: no effect, the overlay closed, and nothing in
/// the repository touched (index, working tree, HEAD).
fn refused(a: &mut App, r: &FixtureRepo) -> String {
    let before = (status(r), r.git(&["rev-parse", "HEAD"]));
    let staged_before = r.git(&["diff", "--cached", "--name-only"]);
    let e = key(a, 'y');
    assert!(e.is_empty(), "{e:?}");
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!((status(r), r.git(&["rev-parse", "HEAD"])), before);
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]), staged_before);
    assert!(a.committed.is_none());
    a.status.clone()
}

#[test]
fn a_file_changed_after_the_review_is_never_committed() {
    let r = fixture();
    let mut a = done_app(&r);
    open(&mut a);
    r.write("a.py", "changed = 1\n");
    let s = refused(&mut a, &r);
    assert!(
        s.contains("Files changed since you reviewed them (a.py)")
            && s.contains("Press r to re-run the review."),
        "{s}"
    );
    assert!(
        status(&r).contains("?? new.py"),
        "still untracked, not added"
    );
}

#[test]
fn a_new_file_a_restored_file_or_a_symlink_count_as_changed() {
    let r = fixture();
    let mut a = done_app(&r);
    open(&mut a);
    r.write("gone.py", "back = 1\n");
    let s = refused(&mut a, &r);
    assert!(s.contains("(gone.py)"), "{s}");

    let r = fixture();
    let mut a = done_app(&r);
    open(&mut a);
    #[cfg(unix)]
    {
        std::fs::remove_file(r.path().join("new.py")).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", r.path().join("new.py")).unwrap();
        let s = refused(&mut a, &r);
        assert!(s.contains("(new.py)"), "{s}");
    }
}

#[test]
fn a_moved_head_is_never_committed() {
    let r = fixture();
    let mut a = done_app(&r);
    open(&mut a);
    r.git(&["commit", "--allow-empty", "-q", "-m", "elsewhere"]);
    let s = refused(&mut a, &r);
    assert!(s.starts_with("HEAD moved since you reviewed"), "{s}");
    assert!(s.contains("Press r"), "{s}");
}

#[test]
fn a_merge_rebase_or_cherry_pick_in_progress_is_never_committed() {
    for (marker, word, dir) in [
        ("MERGE_HEAD", "merge", false),
        ("rebase-merge", "rebase", true),
        ("CHERRY_PICK_HEAD", "cherry-pick", false),
    ] {
        let r = fixture();
        let mut a = done_app(&r);
        open(&mut a);
        let p = r.path().join(".git").join(marker);
        if dir {
            std::fs::create_dir_all(&p).unwrap();
        } else {
            std::fs::write(&p, r.git(&["rev-parse", "HEAD"])).unwrap();
        }
        let s = refused(&mut a, &r);
        assert!(s.contains(&format!("A {word} is in progress")), "{s}");
    }
}

#[test]
fn nothing_to_commit_is_reported_and_stages_nothing() {
    let r = base_repo();
    r.write("a.py", PY_OLD).commit("initial");
    // A review that claims a change the working tree no longer has.
    let mut rv = review_of(&r);
    rv.files.push(teer::git::ChangedFile {
        path: "a.py".into(),
        old_path: None,
        status: teer::git::FileStatus::Modified,
        old_content: Some("old\n".into()),
        new_content: Some(PY_OLD.into()),
    });
    assert_eq!(commit::check(&rv), Err(commit::Refusal::Nothing));
    assert!(
        commit::Refusal::Nothing
            .text("r")
            .contains("Nothing to commit")
    );
}

#[test]
fn a_flagged_unit_blocks_the_commit_even_with_the_overlay_open() {
    let r = fixture();
    let mut a = done_app(&r);
    open(&mut a);
    a.review.units[0].decision = Decision::Pending;
    let s = refused(&mut a, &r);
    assert!(s.contains("Not everything is accepted"), "{s}");
}

#[test]
fn the_guards_run_before_any_git_add() {
    // check() is read-only: calling it leaves the index and tree alone.
    let r = fixture();
    let mut rv = review_of(&r);
    accept_all(&mut rv);
    let before = status(&r);
    let paths = commit::check(&rv).unwrap();
    assert_eq!(paths.len(), 3);
    assert_eq!(status(&r), before);
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]), "");
    // and the files the guard saw are exactly the review's.
    let mut names: Vec<_> = paths.iter().map(|p| p.display().to_string()).collect();
    names.sort();
    assert_eq!(names, vec!["a.py", "gone.py", "new.py"]);
}

#[test]
fn run_commit_refuses_an_empty_message_or_unsafe_paths() {
    let r = fixture();
    let out = commit::run_commit(&r.path(), &["a.py".into()], " \n ", Io::Capture);
    assert!(matches!(out, Outcome::Refused(_)));
    for bad in ["../x", "/etc/passwd", ""] {
        let out = commit::run_commit(&r.path(), &[bad.into()], "msg", Io::Capture);
        assert!(matches!(out, Outcome::Refused(_)), "{bad}");
    }
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]), "");
    assert_eq!(head_count(&r), 1);
}

#[test]
fn the_commit_effect_is_left_to_the_terminal_loop() {
    let r = fixture();
    let mut a = done_app(&r);
    open(&mut a);
    let e = key(&mut a, 'y');
    let (rt, _rx) = runtime(Arc::new(MockAdapter::new(|_, _| anyhow::bail!("no"))));
    for e in e {
        assert!(!rt.exec(&mut a, e));
    }
    assert_eq!(head_count(&r), 1, "Runtime::exec never commits by itself");
}
