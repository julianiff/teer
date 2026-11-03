//! Notes: `i` writes one on the unit on show anywhere, `␣n` lists them,
//! the `:w` sheet takes them out of teer (clipboard, Claude Code or Codex,
//! the PR), `teer notes` prints them.

mod common;

use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::approve::Verdict;
use teer::config::{Config, Provider};
use teer::git::{ChangedFile, FileStatus};
use teer::keymap::Keymap;
use teer::review::model::{
    Decision, PrInfo, Prep, Range, Review, SessionRef, Unit, UnitKind, UnitStatus,
};
use teer::tui::app::{App, AppEvent, Effect, Overlay, Screen};
use teer::tui::finish::Step;
use teer::tui::ui;

const OLD: &str = "def f():\n    return 1\n\n\ndef g():\n    return 2\n";
const NEW: &str = "def f():\n    return 10\n\n\ndef g():\n    return 20\n";
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn unit(symbol: &str, old: (u32, u32), new: (u32, u32), note: &str) -> Unit {
    Unit {
        id: format!("a.py::{symbol}"),
        file: PathBuf::from("a.py"),
        symbol: symbol.into(),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: Some(Range::new(old.0, old.1)),
        new_range: Some(Range::new(new.0, new.1)),
        renamed_from: None,
        fingerprint: format!("fp-{symbol}"),
        changes: Vec::new(),
        prep: Prep::Skipped,
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Pending,
        summary: None,
        nbc_reason: None,
        dive: teer::review::model::Dive {
            note: note.into(),
            ..Default::default()
        },
        accepted_by: None,
        reasons: Vec::new(),
        dive_pending: Default::default(),
        checks: Default::default(),
    }
}

fn pr_info() -> PrInfo {
    PrInfo {
        host: "github.com".into(),
        owner: "acme".into(),
        repo: "shop".into(),
        number: 7,
        remote: Some("origin".into()),
        head_sha: Some(SHA.into()),
        repo_dir: Some(PathBuf::from("/r")),
        ..Default::default()
    }
}

fn app(notes: [&str; 2], pr: Option<PrInfo>) -> App {
    common::isolate_env();
    let review = Review {
        id: "t".into(),
        repo_root: PathBuf::from("/repo"),
        base: "HEAD".into(),
        head: teer::git::WORKTREE.into(),
        head_commit: SHA.into(),
        agent_session: None,
        pr,
        created_at: chrono::Utc::now(),
        files: vec![ChangedFile {
            path: PathBuf::from("a.py"),
            old_path: None,
            status: FileStatus::Modified,
            old_content: Some(OLD.into()),
            new_content: Some(NEW.into()),
        }],
        units: vec![
            unit("f", (1, 2), (1, 2), notes[0]),
            unit("g", (5, 6), (5, 6), notes[1]),
        ],
        skipped: Vec::new(),
        graph: Default::default(),
        intent: Default::default(),
        questions: Vec::new(),
        version: 0,
        history: Default::default(),
        branch: None,
        cursor: None,
    };
    App::new(review, Config::default(), Keymap::default())
}

fn code(a: &mut App, c: KeyCode) -> Vec<Effect> {
    a.on_event(Event::Key(KeyEvent::new(c, KeyModifiers::NONE)))
}

fn key(a: &mut App, c: char) -> Vec<Effect> {
    code(a, KeyCode::Char(c))
}

/// Type `k` in Neovim notation (`<space>n`).
fn keys(a: &mut App, k: &str) -> Vec<Effect> {
    common::v2::press(a, k)
}

fn typed(a: &mut App, text: &str) {
    for c in text.chars() {
        key(a, c);
    }
}

fn note<'a>(a: &'a App, symbol: &str) -> &'a str {
    &a.review.unit(&format!("a.py::{symbol}")).unwrap().dive.note
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

// ---------------------------------------------------------------- writing

#[test]
fn i_writes_a_note_in_the_queue_and_in_the_unit_view() {
    let mut a = app(["", ""], None);
    let id = a.selected().unwrap().id.clone();
    key(&mut a, 'i');
    assert!(matches!(&a.overlay, Overlay::Note { id: i, back: None, .. } if *i == id));
    // Letters that are actions elsewhere are text here.
    typed(&mut a, "zero amounts?");
    assert_eq!(a.screen, Screen::Queue);
    assert_eq!(code(&mut a, KeyCode::Enter), vec![Effect::Save]);
    let u = a.review.unit(&id).unwrap();
    assert_eq!(u.dive.note, "zero amounts?");
    // Written on the code as it is now.
    assert_eq!(u.dive.note_on.as_deref(), Some(u.fingerprint.as_str()));
    assert!(!u.note_is_stale());

    keys(&mut a, "jl");
    assert_eq!(a.screen, Screen::Unit);
    let other = a.unit.id.clone();
    assert_ne!(other, id);
    key(&mut a, 'i');
    assert!(matches!(&a.overlay, Overlay::Note { id: i, .. } if *i == other));
    typed(&mut a, "and here");
    code(&mut a, KeyCode::Enter);
    assert_eq!(a.review.unit(&other).unwrap().dive.note, "and here");
}

#[test]
fn an_emptied_note_is_no_note() {
    let mut a = app(["fix", ""], None);
    a.review.units[0].dive.note_on = Some("fp-f".into());
    key(&mut a, 'i');
    for _ in 0..3 {
        code(&mut a, KeyCode::Backspace);
    }
    code(&mut a, KeyCode::Enter);
    assert_eq!(note(&a, "f"), "");
    assert_eq!(a.review.units[0].dive.note_on, None);
}

#[test]
fn the_note_shows_under_the_code_and_marks_the_row() {
    let mut a = app(["zero hits the gateway", ""], None);
    let out = draw(&a, 120, 30);
    assert!(out.contains("✎ you"), "{out}");
    assert!(out.contains("zero hits the gateway"), "{out}");
    let row = out.lines().find(|l| l.contains("▌")).unwrap();
    assert!(row.contains('✎'), "{row}");
    // Written on other code: the card says so.
    a.review.units[0].dive.note_on = Some("older".into());
    let out = draw(&a, 120, 30);
    assert!(out.contains("the code changed since"), "{out}");
    // In the unit view too.
    key(&mut a, 'l');
    assert_eq!(a.screen, Screen::Unit);
    let out = draw(&a, 120, 40);
    assert!(out.contains("zero hits the gateway"), "{out}");
}

// ------------------------------------------------------------- notes view

#[test]
fn leader_n_without_notes_says_how_to_write_one() {
    let mut a = app(["", ""], None);
    keys(&mut a, "<space>n");
    assert_eq!(a.overlay, Overlay::None);
    assert!(a.status.contains("No notes yet"), "{}", a.status);
}

#[test]
fn the_notes_view_lists_moves_and_goes_to_a_unit() {
    let mut a = app(["first", "second"], None);
    keys(&mut a, "<space>n");
    assert_eq!(a.overlay, Overlay::Notes { sel: 0 });
    let out = draw(&a, 100, 30);
    assert!(out.contains("Notes · 2 notes"), "{out}");
    assert!(out.contains("first") && out.contains("second"), "{out}");
    assert!(out.contains(":w finish"), "{out}");
    assert!(!out.contains("post on the PR"), "{out}");
    key(&mut a, 'j');
    assert_eq!(a.overlay, Overlay::Notes { sel: 1 });
    key(&mut a, 'j');
    assert_eq!(a.overlay, Overlay::Notes { sel: 1 }, "stops at the last");
    code(&mut a, KeyCode::Enter);
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(a.selected().unwrap().symbol, "g");
    // Opening it again starts on the note of the unit on show.
    keys(&mut a, "<space>n");
    assert_eq!(a.overlay, Overlay::Notes { sel: 1 });
    code(&mut a, KeyCode::Esc);
    assert_eq!(a.overlay, Overlay::None);
}

#[test]
fn editing_from_the_view_goes_back_to_it() {
    let mut a = app(["first", "second"], None);
    keys(&mut a, "<space>n");
    key(&mut a, 'j');
    key(&mut a, 'i');
    assert!(matches!(&a.overlay, Overlay::Note { back: Some(1), text, .. } if text == "second"));
    typed(&mut a, "!");
    code(&mut a, KeyCode::Enter);
    assert_eq!(a.overlay, Overlay::Notes { sel: 1 });
    assert_eq!(note(&a, "g"), "second!");
    key(&mut a, 'i');
    code(&mut a, KeyCode::Esc);
    assert_eq!(a.overlay, Overlay::Notes { sel: 1 });
}

#[test]
fn d_deletes_a_note_after_asking() {
    let mut a = app(["first", "second"], None);
    keys(&mut a, "<space>n");
    key(&mut a, 'd');
    assert!(matches!(
        &a.overlay,
        Overlay::ConfirmDeleteNote { sel: 0, .. }
    ));
    assert!(draw(&a, 100, 30).contains("Delete the note on f?"));
    key(&mut a, 'n');
    assert_eq!(a.overlay, Overlay::Notes { sel: 0 });
    assert_eq!(note(&a, "f"), "first");
    key(&mut a, 'd');
    assert_eq!(key(&mut a, 'y'), vec![Effect::Save]);
    assert_eq!(note(&a, "f"), "");
    assert_eq!(a.overlay, Overlay::Notes { sel: 0 });
    // The last one closes the view.
    key(&mut a, 'd');
    key(&mut a, 'y');
    assert_eq!(a.overlay, Overlay::None);
}

#[test]
fn y_copies_every_note_as_markdown() {
    let mut a = app(["first", "second"], None);
    keys(&mut a, "<space>n");
    let e = key(&mut a, 'y');
    let [Effect::Copy { text, what }] = &e[..] else {
        panic!("{e:?}")
    };
    assert_eq!(what, "2 notes as Markdown");
    assert!(text.starts_with("# Review notes\n\n2 notes on the uncommitted changes in `repo`."));
    assert!(text.contains("## a.py:1-2 · `f`"), "{text}");
    assert!(text.contains("-    return 1\n+    return 10"), "{text}");
    assert!(text.contains("> first\n"), "{text}");
    assert_eq!(a.overlay, Overlay::Notes { sel: 0 }, "the view stays open");
}

/// Flag unit `symbol`.
fn flag(a: &mut App, symbol: &str) {
    a.review
        .unit_mut(&format!("a.py::{symbol}"))
        .unwrap()
        .decision = Decision::Investigate;
}

/// Accept unit `symbol`.
fn accept(a: &mut App, symbol: &str) {
    a.review
        .unit_mut(&format!("a.py::{symbol}"))
        .unwrap()
        .decision = Decision::Accepted;
}

#[test]
fn the_sheet_hands_the_notes_to_claude_code() {
    let mut a = app(["first", ""], None);
    flag(&mut a, "f");
    accept(&mut a, "g");
    keys(&mut a, ":w<cr>");
    assert_eq!(a.finish_step(), Some(Step::SendNotes));
    let e = code(&mut a, KeyCode::Enter);
    let [
        Effect::OpenAgent {
            cli: Provider::Claude,
            cwd,
            prompt,
            resume,
        },
    ] = &e[..]
    else {
        panic!("{e:?}")
    };
    assert_eq!(cwd, &PathBuf::from("/repo"));
    assert_eq!(*resume, None);
    assert!(prompt.starts_with("These are my review notes"), "{prompt}");
    assert!(prompt.contains("> first"), "{prompt}");
    assert_eq!(a.overlay, Overlay::None);

    // With the session that made the change, a fork of it, where it was
    // started. `:w` in the notes view opens the sheet on it.
    a.review.agent_session = Some(SessionRef {
        agent: "claude".into(),
        id: "abc-123".into(),
        cwd: PathBuf::from("/repo/sub"),
    });
    keys(&mut a, "<space>n");
    // `c` was v1's way to hand them over: nothing now.
    key(&mut a, 'c');
    assert!(matches!(a.overlay, Overlay::Notes { .. }));
    keys(&mut a, ":w<cr>");
    assert_eq!(a.finish_step(), Some(Step::SendNotes));
    let e = code(&mut a, KeyCode::Enter);
    let [Effect::OpenAgent { cwd, resume, .. }] = &e[..] else {
        panic!("{e:?}")
    };
    assert_eq!(cwd, &PathBuf::from("/repo/sub"));
    assert_eq!(resume.as_deref(), Some("abc-123"));
}

#[test]
fn the_sheet_hands_the_notes_to_codex_or_copies_them_for_an_api() {
    let mut a = app(["first", ""], None);
    flag(&mut a, "f");
    accept(&mut a, "g");
    a.review.agent_session = Some(SessionRef {
        agent: "claude".into(),
        id: "abc-123".into(),
        cwd: PathBuf::from("/repo/sub"),
    });
    a.cfg.provider = Some(Provider::Codex);
    keys(&mut a, ":w<cr>");
    let e = code(&mut a, KeyCode::Enter);
    let [
        Effect::OpenAgent {
            cli: Provider::Codex,
            cwd,
            resume: None,
            ..
        },
    ] = &e[..]
    else {
        panic!("{e:?}")
    };
    assert_eq!(cwd, &PathBuf::from("/repo"));

    // The model API has no session to take them: they are copied, with
    // what to do with them.
    a.cfg.provider = Some(Provider::Api);
    keys(&mut a, ":w<cr>");
    assert!(draw(&a, 100, 30).contains("copy the notes for your agent"));
    let e = code(&mut a, KeyCode::Enter);
    let [Effect::Copy { text, what }] = &e[..] else {
        panic!("{e:?}")
    };
    assert!(text.starts_with("These are my review notes"), "{text}");
    assert!(what.contains("model API"), "{what}");
}

// ------------------------------------------------------------ pull request

#[test]
fn a_pull_request_posts_the_notes_and_never_hands_them_to_claude() {
    let mut a = app(["first", ""], Some(pr_info()));
    flag(&mut a, "f");
    accept(&mut a, "g");
    keys(&mut a, "<space>n");
    assert!(draw(&a, 100, 30).contains(":w finish"));
    // `p` was v1's way to post them: nothing now.
    key(&mut a, 'p');
    assert!(matches!(a.overlay, Overlay::Notes { .. }));
    // `:w` opens the sheet on requesting changes; nothing hands to Claude.
    keys(&mut a, ":w<cr>");
    assert_eq!(
        a.finish_steps(),
        [Step::RequestChanges, Step::CommentOnly, Step::Yank]
    );
    assert_eq!(a.finish_step(), Some(Step::RequestChanges));
    assert!(draw(&a, 100, 30).contains("request changes on PR #7"));
    // Comment only, then back.
    key(&mut a, 'j');
    assert_eq!(a.finish_step(), Some(Step::CommentOnly));
    key(&mut a, 'k');
    let e = code(&mut a, KeyCode::Enter);
    let [
        Effect::PostReview {
            verdict, inline, ..
        },
    ] = &e[..]
    else {
        panic!("{e:?}")
    };
    assert_eq!(*verdict, Verdict::RequestChanges);
    assert_eq!(inline.len(), 1);
    assert!(
        a.status.contains("Requesting changes on PR #7"),
        "{}",
        a.status
    );
    // Done: still not approved, and teer ends with the line.
    let e = a.on_app_event(AppEvent::PrReviewed {
        verdict: Verdict::RequestChanges,
        result: Ok("Changes requested on PR #7 on GitHub.".into()),
    });
    assert_eq!(e, vec![Effect::Save, Effect::Quit]);
    assert!(!a.review.pr.as_ref().unwrap().is_approved());
    assert_eq!(a.status, "Changes requested on PR #7 on GitHub.");
}

#[test]
fn approving_is_on_offer_once_everything_was_accepted() {
    let mut a = app(["first", ""], Some(pr_info()));
    flag(&mut a, "f");
    accept(&mut a, "g");
    keys(&mut a, ":w<cr>");
    assert!(!a.finish_steps().contains(&Step::Approve));
    keys(&mut a, "<esc>");
    accept(&mut a, "f");
    keys(&mut a, ":w<cr>");
    assert_eq!(
        a.finish_steps(),
        [Step::Approve, Step::ApproveWithComment, Step::Yank]
    );
}

// --------------------------------------------------------------- teer notes

#[test]
fn teer_notes_prints_the_last_reviews_notes() {
    let env = common::isolate_env();
    let repo = common::FixtureRepo::new();
    repo.write("a.py", OLD).commit("init");
    let run = || {
        std::process::Command::new(common::teer_bin())
            .args(["notes", "-C"])
            .arg(repo.path())
            .env("TEER_DATA_DIR", env.path().join("data"))
            .output()
            .unwrap()
    };
    let out = run();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no saved review"));

    repo.write("a.py", NEW);
    let mut review = teer::review::build_review(&repo.path(), &Default::default()).unwrap();
    teer::review::store::save(&review).unwrap();
    let out = run();
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no notes"));

    let f = review.units.iter_mut().find(|u| u.symbol == "f").unwrap();
    f.dive.note = "ten?".into();
    teer::review::store::save(&review).unwrap();
    let out = run();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with("# Review notes\n\n1 note on the uncommitted changes"),
        "{text}"
    );
    assert!(text.contains("## a.py:1-2 · `f`"), "{text}");
    assert!(text.contains("+    return 10"), "{text}");
    assert!(text.contains("> ten?"), "{text}");
}
