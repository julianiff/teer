//! Approving a reviewed PR on GitHub from the "all reviewed" popup: the
//! popup line, the confirmation overlay, the comments that would be posted,
//! the head check, and the `gh api` call. No network and no real GitHub: `gh`,
//! the browser opener and the editor are scripts in a temp dir, and the
//! "remote" is a local bare repository.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::approve::{self, EditOutcome, Tools, Verdict};
use teer::config::{Config, KeyMap};
use teer::git::{ChangedFile, FileStatus};
use teer::review::BuildOptions;
use teer::review::model::{Decision, PrInfo, Prep, Range, Review, Unit, UnitKind, UnitStatus};
use teer::tui::app::{App, AppEvent, Effect, Overlay};
use teer::tui::{Runtime, ui};

const OLD: &str = "def f():\n    return 1\n\n\ndef g():\n    return 2\n";
const NEW: &str = "def f():\n    return 10\n\n\ndef g():\n    return 20\n";
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

// ------------------------------------------------------------- fixtures

fn unit(symbol: &str, file: &str, old: (u32, u32), new: (u32, u32), note: &str) -> Unit {
    Unit {
        id: format!("{file}::{symbol}"),
        file: PathBuf::from(file),
        symbol: symbol.into(),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: Some(Range::new(old.0, old.1)),
        new_range: Some(Range::new(new.0, new.1)),
        renamed_from: None,
        fingerprint: String::new(),
        changes: Vec::new(),
        prep: Prep::Skipped,
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Accepted,
        summary: None,
        nbc_reason: None,
        dive: teer::review::model::Dive {
            note: note.into(),
            ..Default::default()
        },
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

fn review(units: Vec<Unit>, pr: Option<PrInfo>) -> Review {
    Review {
        id: "t".into(),
        repo_root: PathBuf::from("/wt"),
        base: "b".into(),
        head: SHA.into(),
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
        units,
        skipped: Vec::new(),
        graph: Default::default(),
        intent: Default::default(),
        questions: Vec::new(),
    }
}

fn notes_units() -> Vec<Unit> {
    vec![
        unit("f", "a.py", (1, 2), (1, 2), "check the \"limit\""),
        unit("g", "a.py", (5, 6), (5, 6), ""),
        // File not in the review: no inline target.
        unit("h", "b.py", (1, 2), (1, 2), "why this?"),
    ]
}

fn app_for(units: Vec<Unit>, pr: Option<PrInfo>, keys: KeyMap) -> App {
    App::new(review(units, pr), Config::default(), keys)
}

fn pr_app() -> App {
    let mut a = app_for(notes_units(), Some(pr_info()), KeyMap::default());
    a.overlay = Overlay::AllDone;
    a
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

fn overlay_parts(a: &App) -> (Vec<approve::NoteEntry>, String, bool) {
    match &a.overlay {
        Overlay::PostReview {
            notes,
            intro,
            inline_on,
            ..
        } => (notes.clone(), intro.clone(), *inline_on),
        o => panic!("not the approve overlay: {o:?}"),
    }
}

// ---------------------------------------------------------- popup and keys

#[test]
fn the_popup_offers_approval_only_for_pr_reviews() {
    let a = pr_app();
    let t = draw(&a, 100, 30);
    assert!(t.contains("a approve PR #7 on GitHub"), "{t}");

    let mut plain = app_for(notes_units(), None, KeyMap::default());
    plain.overlay = Overlay::AllDone;
    let t = draw(&plain, 100, 30);
    assert!(!t.contains("approve"), "{t}");
    // `a` just closes it: no overlay, no effect.
    assert!(key(&mut plain, 'a').is_empty());
    assert_eq!(plain.overlay, Overlay::None);
}

#[test]
fn a_opens_the_overlay_with_the_notes() {
    let mut a = pr_app();
    assert!(key(&mut a, 'a').is_empty());
    let (notes, intro, inline_on) = overlay_parts(&a);
    assert_eq!(intro, "Reviewed with teer.");
    assert!(inline_on);
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].target, Some((2, teer::review::model::Side::New)));
    assert_eq!(notes[1].target, None);
    let t = draw(&a, 100, 30);
    assert!(t.contains("Approve PR #7 (acme/shop) on GitHub?"), "{t}");
    assert!(t.contains("1 inline comment"), "{t}");
    assert!(t.contains("Reviewed with teer."), "{t}");
    assert!(t.contains("- h (b.py:1): why this?"), "{t}");
    assert!(t.contains("a.py:2 check the \"limit\""), "{t}");
    assert!(t.contains("send") && t.contains("edit comment") && t.contains("cancel"));
}

#[test]
fn without_notes_the_comment_is_just_the_intro() {
    let units = vec![unit("f", "a.py", (1, 2), (1, 2), "")];
    let mut a = app_for(units, Some(pr_info()), KeyMap::default());
    a.overlay = Overlay::AllDone;
    key(&mut a, 'a');
    let (notes, intro, _) = overlay_parts(&a);
    assert!(notes.is_empty());
    let plan = approve::build_plan(&notes, &intro, true);
    assert_eq!(plan.body, "Reviewed with teer.");
    assert!(plan.inline.is_empty());
    assert!(draw(&a, 100, 30).contains("0 inline comments"));
}

#[test]
fn control_characters_are_stripped_and_sizes_capped() {
    let mut units = vec![
        unit(
            "f",
            "a.py",
            (1, 2),
            (1, 2),
            "bad\u{1b}[31m\u{7}note\ttab\nline",
        ),
        unit("g", "a.py", (5, 6), (5, 6), &"x".repeat(5000)),
    ];
    for i in 0..80 {
        units.push(unit(&format!("n{i}"), "b.py", (1, 1), (1, 1), "later"));
    }
    let mut a = app_for(units, Some(pr_info()), KeyMap::default());
    a.overlay = Overlay::AllDone;
    key(&mut a, 'a');
    let (notes, intro, _) = overlay_parts(&a);
    let plan = approve::build_plan(&notes, &intro, true);
    for c in &plan.inline {
        assert!(
            !c.body.chars().any(|ch| ch.is_control() && ch != '\n'),
            "{c:?}"
        );
        assert!(c.body.chars().count() <= approve::INLINE_MAX_CHARS);
    }
    assert!(plan.inline[0].body.contains("note tab line"));
    assert!(plan.body.lines().count() <= approve::BODY_MAX_LINES);
    assert!(plan.body.chars().count() <= approve::BODY_MAX_CHARS);
    assert!(plan.body.contains("more notes"));
    // Text from an editor is cleaned the same way.
    assert_eq!(approve::clamp_text("a\r\nb\u{1b}c\t", 100), "a\nb c");
    assert_eq!(approve::clamp_text(&"y".repeat(9000), 4000).len(), 4000);
}

#[test]
fn esc_cancels_and_stray_keys_do_nothing() {
    let mut a = pr_app();
    key(&mut a, 'a');
    let before = a.overlay.clone();
    for c in ['x', 'a', 'q', 'n', 'R', ' '] {
        assert!(key(&mut a, c).is_empty(), "{c}");
        assert_eq!(a.overlay, before, "{c} must not change or close it");
    }
    assert!(press(&mut a, KeyCode::Tab).is_empty());
    assert_eq!(a.overlay, before);
    assert!(press(&mut a, KeyCode::Esc).is_empty());
    assert_eq!(a.overlay, Overlay::None);
    assert!(!a.approving);
}

#[test]
fn ctrl_c_still_quits_from_the_overlay() {
    let mut a = pr_app();
    key(&mut a, 'a');
    let e = a.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    assert!(e.contains(&Effect::Quit));
}

#[test]
fn the_inline_toggle_moves_notes_into_the_summary() {
    let mut a = pr_app();
    key(&mut a, 'a');
    key(&mut a, 'i');
    assert!(!overlay_parts(&a).2);
    let t = draw(&a, 100, 30);
    assert!(
        t.contains("0 inline comments") && t.contains("Inline comments are off"),
        "{t}"
    );
    assert!(t.contains("- f (a.py:1): check the \"limit\""), "{t}");
    let e = key(&mut a, 'y');
    let Effect::PostReview {
        inline,
        body,
        flat_body,
        ..
    } = &e[0]
    else {
        panic!("{e:?}")
    };
    assert!(inline.is_empty());
    assert!(body.contains("- f (a.py:1)") && body.contains("- h (b.py:1)"));
    assert_eq!(body, flat_body);
}

#[test]
fn a_guessed_base_never_gets_inline_comments() {
    let mut pr = pr_info();
    pr.base_assumed = Some("main".into());
    let mut a = app_for(notes_units(), Some(pr), KeyMap::default());
    a.overlay = Overlay::AllDone;
    key(&mut a, 'a');
    let e = key(&mut a, 'y');
    let Effect::PostReview { inline, body, .. } = &e[0] else {
        panic!("{e:?}")
    };
    assert!(inline.is_empty());
    assert!(body.contains("- f (a.py:1)"));
}

#[test]
fn y_and_enter_send_once_and_close_the_overlay() {
    for send in [KeyCode::Char('y'), KeyCode::Enter] {
        let mut a = pr_app();
        key(&mut a, 'a');
        let e = press(&mut a, send);
        assert_eq!(e.len(), 1, "{e:?}");
        let Effect::PostReview {
            pr, body, inline, ..
        } = &e[0]
        else {
            panic!("{e:?}")
        };
        assert_eq!(pr.number, 7);
        assert_eq!(body, "Reviewed with teer.\n\n- h (b.py:1): why this?");
        assert_eq!(inline.len(), 1);
        assert_eq!((inline[0].path.as_str(), inline[0].line), ("a.py", 2));
        assert_eq!(a.overlay, Overlay::None);
        assert!(a.status.contains("Approving PR #7"));
        // While it is in flight another popup cannot send again.
        a.overlay = Overlay::AllDone;
        key(&mut a, 'a');
        assert_eq!(a.overlay, Overlay::None);
    }
}

#[test]
fn the_overlay_scrolls_and_never_overflows_the_screen() {
    let mut units = Vec::new();
    for i in 0..30 {
        units.push(unit(
            &format!("n{i}"),
            "b.py",
            (1, 1),
            (1, 1),
            "a fairly long note about this unit",
        ));
    }
    let mut a = app_for(units, Some(pr_info()), KeyMap::default());
    a.overlay = Overlay::AllDone;
    key(&mut a, 'a');
    for (w, h) in [
        (120, 40),
        (80, 24),
        (60, 20),
        (40, 12),
        (20, 8),
        (12, 5),
        (8, 3),
        (1, 1),
        (3, 30),
        (200, 4),
    ] {
        let t = draw(&a, w, h);
        assert_eq!(t.lines().count(), h as usize);
        for _ in 0..3 {
            press(&mut a, KeyCode::Down);
            draw(&a, w, h);
        }
        press(&mut a, KeyCode::PageUp);
        press(&mut a, KeyCode::PageDown);
        draw(&a, w, h);
        // Up many times returns to the top: the scroll never runs past the end.
        for _ in 0..200 {
            press(&mut a, KeyCode::Down);
        }
        draw(&a, w, h);
        for _ in 0..200 {
            press(&mut a, KeyCode::Up);
        }
        let _ = draw(&a, w, h);
    }
    let t = draw(&a, 80, 24);
    assert!(t.contains("scroll"), "{t}");
}

#[test]
fn rendering_with_inline_list_and_editor_fits_common_sizes() {
    let mut a = pr_app();
    key(&mut a, 'a');
    for (w, h) in [(60, 20), (80, 24), (120, 40), (30, 10), (10, 4)] {
        let t = draw(&a, w, h);
        assert_eq!(t.lines().count(), h as usize);
        if w >= 60 {
            assert!(t.contains("a.py:2"), "{w}x{h}\n{t}");
        }
    }
    a.comment_edited(EditOutcome::Unavailable("no nvim".into()));
    for (w, h) in [(60, 20), (80, 24), (120, 40), (10, 4), (1, 1)] {
        draw(&a, w, h);
    }
}

#[test]
fn approve_pr_can_be_rebound() {
    let mut keys = std::collections::HashMap::new();
    keys.insert("approve_pr".to_string(), "p".to_string());
    let km = KeyMap::with_overrides(&keys).unwrap();
    let mut a = app_for(notes_units(), Some(pr_info()), km.clone());
    a.overlay = Overlay::AllDone;
    assert!(draw(&a, 100, 30).contains("p approve PR #7"));
    key(&mut a, 'a');
    assert_eq!(
        a.overlay,
        Overlay::None,
        "the default key no longer approves"
    );
    a.overlay = Overlay::AllDone;
    key(&mut a, 'p');
    assert!(matches!(a.overlay, Overlay::PostReview { .. }));
    assert_eq!(a.keys.label(teer::config::Action::ApprovePr), "p");
}

// ------------------------------------------------------ approval outcome

#[test]
fn success_is_remembered_and_the_popup_says_approved() {
    let mut a = pr_app();
    key(&mut a, 'a');
    key(&mut a, 'y');
    let e = a.on_app_event(AppEvent::PrReviewed {
        verdict: Verdict::Approve,
        result: Ok("PR #7 approved on GitHub.".into()),
    });
    assert_eq!(e, vec![Effect::Save]);
    assert_eq!(a.status, "PR #7 approved on GitHub.");
    assert_eq!(
        a.review.pr.as_ref().unwrap().approved_head.as_deref(),
        Some(SHA)
    );
    assert!(!a.approving);
    a.overlay = Overlay::AllDone;
    let t = draw(&a, 100, 30);
    assert!(
        t.contains("PR #7 approved") && !t.contains("a approve PR"),
        "{t}"
    );
    // `a` is inert now.
    key(&mut a, 'a');
    assert_eq!(a.overlay, Overlay::None);
}

#[test]
fn failure_keeps_it_unapproved() {
    let mut a = pr_app();
    key(&mut a, 'a');
    key(&mut a, 'y');
    a.on_app_event(AppEvent::PrReviewed {
        verdict: Verdict::Approve,
        result: Err("gh failed: nope".into()),
    });
    assert_eq!(a.status, "gh failed: nope");
    assert!(a.review.pr.as_ref().unwrap().approved_head.is_none());
    a.overlay = Overlay::AllDone;
    key(&mut a, 'a');
    assert!(
        matches!(a.overlay, Overlay::PostReview { .. }),
        "can try again"
    );
}

#[test]
fn approval_belongs_to_one_head() {
    isolate_env();
    let mut old = review(notes_units(), Some(pr_info()));
    old.pr.as_mut().unwrap().approved_head = Some(SHA.into());
    // Same head: carried over.
    let mut same = review(notes_units(), Some(pr_info()));
    teer::review::store::merge_previous(&mut same, &old);
    assert!(same.pr.as_ref().unwrap().is_approved());
    // New head: starts unapproved.
    let mut p = pr_info();
    p.head_sha = Some("b".repeat(40));
    let mut newer = review(notes_units(), Some(p));
    teer::review::store::merge_previous(&mut newer, &old);
    assert!(!newer.pr.as_ref().unwrap().is_approved());
    assert!(newer.pr.as_ref().unwrap().approved_head.is_none());
    // It survives save and load; the local checkout path does not.
    let json = serde_json::to_string(&old).unwrap();
    let back: Review = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.pr.as_ref().unwrap().approved_head.as_deref(),
        Some(SHA)
    );
    assert!(back.pr.as_ref().unwrap().repo_dir.is_none());
    // Old files without the new fields load.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    for k in ["remote", "head_sha", "approved_head"] {
        v["pr"].as_object_mut().unwrap().remove(k);
    }
    let old_style: Review = serde_json::from_value(v).unwrap();
    assert!(!old_style.pr.unwrap().is_approved());
}

// ---------------------------------------------------------------- editing

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

#[test]
fn editing_round_trip_uses_a_temp_file_and_deletes_it() {
    isolate_env();
    let d = tempfile::tempdir().unwrap();
    let seen = d.path().join("seen");
    let editor = script(
        d.path(),
        "ed",
        &format!(
            "echo \"$1\" > {seen}\ncp \"$1\" {copy}\nprintf 'Edited by hand\\n\\033[31mred\\tx\\n' > \"$1\"",
            seen = seen.display(),
            copy = d.path().join("orig").display()
        ),
    );
    let out = approve::edit_in_editor(editor.as_os_str(), "Reviewed with teer.");
    assert_eq!(
        out,
        EditOutcome::Edited("Edited by hand\n [31mred x".into())
    );
    assert_eq!(
        std::fs::read_to_string(d.path().join("orig")).unwrap(),
        "Reviewed with teer."
    );
    let path = PathBuf::from(std::fs::read_to_string(&seen).unwrap().trim());
    assert!(
        path.starts_with(teer::review::store::data_dir()),
        "{path:?}"
    );
    assert!(!path.exists(), "the temp file is deleted");

    // A non-zero exit (:cq) cancels.
    let cq = script(d.path(), "cq", "echo changed > \"$1\"; exit 1");
    assert_eq!(
        approve::edit_in_editor(cq.as_os_str(), "x"),
        EditOutcome::Cancelled
    );
    // No editor: unavailable.
    let none = d.path().join("missing");
    assert!(matches!(
        approve::edit_in_editor(none.as_os_str(), "x"),
        EditOutcome::Unavailable(_)
    ));
}

#[test]
fn an_edited_summary_is_used_and_cancel_keeps_the_old_one() {
    let mut a = pr_app();
    key(&mut a, 'a');
    let e = key(&mut a, 'e');
    assert_eq!(
        e,
        vec![Effect::EditComment {
            text: "Reviewed with teer.".into()
        }]
    );
    a.comment_edited(EditOutcome::Edited("Looks good to me".into()));
    assert_eq!(overlay_parts(&a).1, "Looks good to me");
    assert!(draw(&a, 100, 30).contains("Looks good to me"));
    a.comment_edited(EditOutcome::Cancelled);
    assert_eq!(overlay_parts(&a).1, "Looks good to me");
    a.comment_edited(EditOutcome::Edited("  \n".into()));
    assert_eq!(
        overlay_parts(&a).1,
        "Looks good to me",
        "an empty result keeps the old text"
    );
    let e = key(&mut a, 'y');
    let Effect::PostReview { body, .. } = &e[0] else {
        panic!()
    };
    assert!(body.starts_with("Looks good to me\n\n- h"));
}

#[test]
fn without_an_editor_a_one_line_editor_takes_over() {
    let mut a = pr_app();
    key(&mut a, 'a');
    key(&mut a, 'e');
    a.comment_edited(EditOutcome::Unavailable("not found".into()));
    assert!(a.status.contains("editing on one line"), "{}", a.status);
    let Overlay::PostReview {
        editing: Some(t), ..
    } = &a.overlay
    else {
        panic!("{:?}", a.overlay)
    };
    assert_eq!(t, "Reviewed with teer.");
    // Stray keys type; they don't send or close.
    for c in " ok".chars() {
        assert!(key(&mut a, c).is_empty());
    }
    key(&mut a, 'y');
    assert!(
        matches!(&a.overlay, Overlay::PostReview { editing: Some(t), .. } if t == "Reviewed with teer. oky")
    );
    press(&mut a, KeyCode::Backspace);
    press(&mut a, KeyCode::Backspace);
    press(&mut a, KeyCode::Enter);
    let (_, intro, _) = overlay_parts(&a);
    assert_eq!(intro, "Reviewed with teer. o");
    // Esc in the editor goes back without saving.
    a.comment_edited(EditOutcome::Unavailable("x".into()));
    key(&mut a, '!');
    press(&mut a, KeyCode::Esc);
    assert_eq!(overlay_parts(&a).1, "Reviewed with teer. o");
    assert!(matches!(
        &a.overlay,
        Overlay::PostReview { editing: None, .. }
    ));
}

// ----------------------------------------------- gh, opener, remote head

struct Origin {
    repo: FixtureRepo,
    _bare: tempfile::TempDir,
    head: String,
    n: u32,
}

static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(500);

impl Origin {
    fn new() -> Origin {
        isolate_env();
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let repo = FixtureRepo::new();
        repo.write("a.py", OLD).commit("init");
        repo.write("a.py", NEW).commit("pr");
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

    fn move_head(&self) -> String {
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
        self.repo.git(&["rev-parse", "HEAD"]).trim().to_string()
    }
}

struct Fakes {
    dir: tempfile::TempDir,
    tools: Tools,
}

/// `gh` records its calls (`call_<k>` holds argv, one per line; `stdin_<k>`
/// its input, `cwd_<k>` its directory) and answers according to `mode`:
/// ok | fail | reject_lines | reject_always | own_pr.
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
pwd > "$d/cwd_$n"
cat > "$d/stdin_$n"
case "$(cat "$d/mode")" in
  ok) echo '{"id":1}';;
  fail) echo "HTTP 401: Bad credentials https://me:pw@github.com/x ghp_secret123" >&2; exit 1;;
  reject_lines) if [ "$n" = 1 ]; then echo "gh: Unprocessable Entity (HTTP 422)" >&2; echo "Pull request review thread line must be part of the diff" >&2; exit 1; fi; echo '{"id":2}';;
  reject_always) echo "gh: Unprocessable Entity (HTTP 422): line must be part of the diff" >&2; exit 1;;
  own_pr) echo "gh: Unprocessable Entity (HTTP 422): Can not approve your own pull request" >&2; exit 1;;
esac"#,
    );
    let opener = script(
        d,
        "opener",
        r#"d=$(dirname "$0"); printf '%s\n' "$@" >> "$d/opened""#,
    );
    let editor = d.join("no-editor");
    let claude = d.join("no-claude");
    Fakes {
        dir,
        tools: Tools {
            gh: gh.into(),
            opener: opener.into(),
            editor: editor.into(),
            claude: claude.into(),
        },
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
    fn argv(&self, k: usize) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join(format!("call_{k}")))
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
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
    fn nothing_opened(&self) -> bool {
        !self.dir.path().join("opened").exists()
    }
}

fn plan_for(a: &App) -> (String, Vec<approve::Inline>, String) {
    let notes = approve::collect_notes(&a.review, &a.order);
    let p = approve::build_plan(&notes, approve::DEFAULT_INTRO, true);
    let flat = approve::build_plan(&notes, approve::DEFAULT_INTRO, false).body;
    (p.body, p.inline, flat)
}

/// An app whose review is the fixture (a.py f/g/h notes) of PR `o`.
fn app_of(o: &Origin) -> App {
    app_for(notes_units(), Some(o.info()), KeyMap::default())
}

#[tokio::test]
async fn gh_is_called_once_with_the_right_argv_and_json_on_stdin() {
    let o = Origin::new();
    let fk = fakes("ok");
    let a = app_of(&o);
    let (body, inline, flat) = plan_for(&a);
    let r = approve::post(
        &fk.tools,
        &o.info(),
        Verdict::Approve,
        body.clone(),
        inline,
        flat,
    )
    .await;
    assert_eq!(r, Ok(format!("PR #{} approved on GitHub.", o.n)));
    assert_eq!(fk.calls(), 1);
    assert_eq!(
        fk.argv(1),
        [
            "api".to_string(),
            "--method".into(),
            "POST".into(),
            format!("repos/acme/shop/pulls/{}/reviews", o.n),
            "--input".into(),
            "-".into()
        ]
    );
    let v = fk.stdin(1);
    assert_eq!(v["commit_id"], o.head.as_str());
    assert_eq!(v["event"], "APPROVE");
    assert_eq!(v["body"], body.as_str());
    assert_eq!(
        v["comments"],
        serde_json::json!([{"path": "a.py", "line": 2, "side": "RIGHT", "body": "check the \"limit\""}])
    );
    // Never in argv; run from the user's repo.
    assert!(
        fk.argv(1)
            .iter()
            .all(|x| !x.contains("Reviewed") && !x.contains("limit"))
    );
    let cwd = std::fs::read_to_string(fk.dir.path().join("cwd_1")).unwrap();
    assert_eq!(Path::new(cwd.trim()).canonicalize().unwrap(), o.repo.path());
    assert!(fk.nothing_opened());
}

#[tokio::test]
async fn other_hosts_get_a_hostname_flag() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut info = o.info();
    info.host = "git.corp.example".into();
    info.port = Some(8443);
    let r = approve::post(
        &fk.tools,
        &info,
        Verdict::Approve,
        "b".into(),
        vec![],
        "b".into(),
    )
    .await;
    assert!(r.is_ok(), "{r:?}");
    let argv = fk.argv(1);
    let at = argv.iter().position(|x| x == "--hostname").unwrap();
    assert_eq!(argv[at + 1], "git.corp.example:8443");
    assert_eq!(&argv[..3], ["api", "--method", "POST"]);
    // Nothing to inline: no comments key at all.
    assert!(fk.stdin(1).get("comments").is_none());
}

#[tokio::test]
async fn json_escaping_survives_quotes_newlines_and_unicode() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut u = notes_units();
    u[0].dive.note = "say \"hi\" \\ ✓ 日本 `code`".into();
    let a = app_for(u, Some(o.info()), KeyMap::default());
    let notes = approve::collect_notes(&a.review, &a.order);
    let p = approve::build_plan(&notes, "Intro \"q\"\nsecond line ✓", true);
    approve::post(
        &fk.tools,
        &o.info(),
        Verdict::Approve,
        p.body.clone(),
        p.inline.clone(),
        p.body,
    )
    .await
    .unwrap();
    let v = fk.stdin(1);
    assert_eq!(v["comments"][0]["body"], "say \"hi\" \\ ✓ 日本 `code`");
    assert!(
        v["body"]
            .as_str()
            .unwrap()
            .starts_with("Intro \"q\"\nsecond line ✓")
    );
}

#[tokio::test]
async fn gh_failure_opens_the_pr_page_and_reports_the_trimmed_message() {
    let o = Origin::new();
    let fk = fakes("fail");
    let r = approve::post(
        &fk.tools,
        &o.info(),
        Verdict::Approve,
        "b".into(),
        vec![],
        "b".into(),
    )
    .await;
    let msg = r.unwrap_err();
    assert!(
        msg.starts_with("gh failed: HTTP 401: Bad credentials"),
        "{msg}"
    );
    assert!(msg.ends_with("opened the PR page instead"), "{msg}");
    assert!(
        !msg.contains("me:pw") && !msg.contains("ghp_secret"),
        "{msg}"
    );
    assert_eq!(fk.calls(), 1, "no automatic retry");
    assert_eq!(
        fk.opened().await,
        [format!("https://github.com/acme/shop/pull/{}", o.n)]
    );
}

#[tokio::test]
async fn a_missing_gh_falls_back_to_the_browser() {
    let o = Origin::new();
    let fk = fakes("ok");
    let tools = Tools {
        gh: fk.dir.path().join("not-installed").into(),
        ..fk.tools.clone()
    };
    let msg = approve::post(
        &tools,
        &o.info(),
        Verdict::Approve,
        "b".into(),
        vec![],
        "b".into(),
    )
    .await
    .unwrap_err();
    assert!(msg.contains("gh failed: gh is not installed"), "{msg}");
    assert!(msg.ends_with("opened the PR page instead"), "{msg}");
    assert_eq!(fk.opened().await.len(), 1);
    // And with no opener either, it says so.
    let tools = Tools {
        gh: fk.dir.path().join("not-installed").into(),
        opener: fk.dir.path().join("no-opener").into(),
        ..fk.tools.clone()
    };
    let msg = approve::post(
        &tools,
        &o.info(),
        Verdict::Approve,
        "b".into(),
        vec![],
        "b".into(),
    )
    .await
    .unwrap_err();
    assert!(msg.contains("couldn't open the PR page either"), "{msg}");
}

#[tokio::test]
async fn a_moved_head_is_never_approved() {
    let o = Origin::new();
    let fk = fakes("ok");
    let new_head = o.move_head();
    let msg = approve::post(
        &fk.tools,
        &o.info(),
        Verdict::Approve,
        "b".into(),
        vec![],
        "b".into(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        msg,
        format!(
            "PR #{} has new commits since you reviewed it (reviewed {}, now {}). Run teer pr again.",
            o.n,
            &o.head[..7],
            &new_head[..7]
        )
    );
    assert_eq!(fk.calls(), 0, "gh must not run");
    assert!(fk.nothing_opened());
}

#[tokio::test]
async fn an_unreadable_remote_is_never_approved() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut info = o.info();
    info.remote = Some("nowhere".into());
    let msg = approve::post(
        &fk.tools,
        &info,
        Verdict::Approve,
        "b".into(),
        vec![],
        "b".into(),
    )
    .await
    .unwrap_err();
    assert!(msg.contains("Could not verify the PR head"), "{msg}");
    assert_eq!(fk.calls(), 0);
    // A PR that no longer exists on the remote.
    let mut info = o.info();
    info.number += 1000;
    let msg = approve::post(
        &fk.tools,
        &info,
        Verdict::Approve,
        "b".into(),
        vec![],
        "b".into(),
    )
    .await
    .unwrap_err();
    assert!(msg.contains("Could not verify the PR head"), "{msg}");
    assert_eq!(fk.calls(), 0);
    // Invalid values never reach a command.
    let mut info = o.info();
    info.owner = "a b".into();
    assert!(
        approve::post(
            &fk.tools,
            &info,
            Verdict::Approve,
            "b".into(),
            vec![],
            "b".into()
        )
        .await
        .is_err()
    );
    let mut info = o.info();
    info.remote = Some("-oops".into());
    assert!(
        approve::post(
            &fk.tools,
            &info,
            Verdict::Approve,
            "b".into(),
            vec![],
            "b".into()
        )
        .await
        .is_err()
    );
    let mut info = o.info();
    info.head_sha = Some("abc".into());
    assert!(
        approve::post(
            &fk.tools,
            &info,
            Verdict::Approve,
            "b".into(),
            vec![],
            "b".into()
        )
        .await
        .is_err()
    );
    assert_eq!(fk.calls(), 0);
}

#[tokio::test]
async fn rejected_inline_comments_are_retried_once_as_a_summary() {
    let o = Origin::new();
    let fk = fakes("reject_lines");
    let a = app_of(&o);
    let (body, inline, flat) = plan_for(&a);
    assert_eq!(inline.len(), 1);
    let r = approve::post(
        &fk.tools,
        &o.info(),
        Verdict::Approve,
        body,
        inline,
        flat.clone(),
    )
    .await
    .unwrap();
    assert!(
        r.contains(
            "inline comments were rejected by GitHub; posted with a summary comment instead"
        ),
        "{r}"
    );
    assert_eq!(fk.calls(), 2);
    assert!(fk.stdin(1)["comments"].is_array());
    let second = fk.stdin(2);
    assert!(second.get("comments").is_none());
    assert_eq!(second["body"], flat.as_str());
    assert!(flat.contains("- f (a.py:1): check the \"limit\""));
    assert_eq!(second["commit_id"], o.head.as_str());
    assert!(fk.nothing_opened());

    // Rejected again: one retry only, then the browser.
    let fk = fakes("reject_always");
    let (body, inline, flat) = plan_for(&a);
    let msg = approve::post(&fk.tools, &o.info(), Verdict::Approve, body, inline, flat)
        .await
        .unwrap_err();
    assert_eq!(fk.calls(), 2);
    assert!(msg.ends_with("opened the PR page instead"), "{msg}");

    // A 422 that is not about comment lines is not retried.
    let fk = fakes("own_pr");
    let (body, inline, flat) = plan_for(&a);
    let msg = approve::post(&fk.tools, &o.info(), Verdict::Approve, body, inline, flat)
        .await
        .unwrap_err();
    assert_eq!(fk.calls(), 1);
    assert!(msg.contains("own pull request"), "{msg}");
}

#[tokio::test]
async fn a_body_only_review_is_never_retried() {
    let o = Origin::new();
    let fk = fakes("reject_always");
    let msg = approve::post(
        &fk.tools,
        &o.info(),
        Verdict::Approve,
        "b".into(),
        vec![],
        "b".into(),
    )
    .await
    .unwrap_err();
    assert_eq!(fk.calls(), 1);
    assert!(msg.starts_with("gh failed:"), "{msg}");
}

// ------------------------------------------------------- full app + runtime

fn runtime(fk: &Fakes) -> (Runtime, tokio::sync::mpsc::UnboundedReceiver<AppEvent>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (
        Runtime {
            adapter: None,
            tx,
            build: BuildOptions {
                base: None,
                languages: None,
            },
            tools: fk.tools.clone(),
        },
        rx,
    )
}

async fn send_through_runtime(app: &mut App, fk: &Fakes) -> Vec<Effect> {
    let (rt, mut rx) = runtime(fk);
    app.overlay = Overlay::AllDone;
    key(app, 'a');
    let effects = key(app, 'y');
    assert!(
        matches!(effects[0], Effect::PostReview { .. }),
        "{effects:?}"
    );
    for e in effects {
        rt.exec(app, e);
    }
    let ev = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("an answer")
        .unwrap();
    app.on_app_event(ev)
}

#[tokio::test]
async fn the_whole_flow_approves_through_the_runtime() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut app = app_of(&o);
    let after = send_through_runtime(&mut app, &fk).await;
    assert_eq!(after, vec![Effect::Save]);
    assert_eq!(app.status, format!("PR #{} approved on GitHub.", o.n));
    assert_eq!(
        app.review.pr.as_ref().unwrap().approved_head.as_deref(),
        Some(o.head.as_str())
    );
    assert_eq!(fk.calls(), 1);
    // The popup now says approved and `a` does nothing.
    app.overlay = Overlay::AllDone;
    assert!(draw(&app, 100, 30).contains(&format!("PR #{} approved", o.n)));
    key(&mut app, 'a');
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(fk.calls(), 1);
}

#[tokio::test]
async fn the_whole_flow_with_a_failing_gh_shows_the_message_and_stays_unapproved() {
    let o = Origin::new();
    let fk = fakes("fail");
    let mut app = app_of(&o);
    send_through_runtime(&mut app, &fk).await;
    assert!(
        app.status.starts_with("gh failed: HTTP 401"),
        "{}",
        app.status
    );
    assert!(app.status.ends_with("opened the PR page instead"));
    assert!(app.review.pr.as_ref().unwrap().approved_head.is_none());
    assert_eq!(fk.opened().await.len(), 1);
}

#[tokio::test]
async fn the_whole_flow_with_a_moved_head_does_not_call_gh() {
    let o = Origin::new();
    let fk = fakes("ok");
    let mut app = app_of(&o);
    o.move_head();
    send_through_runtime(&mut app, &fk).await;
    assert!(
        app.status.contains("has new commits since you reviewed it"),
        "{}",
        app.status
    );
    assert_eq!(fk.calls(), 0);
    assert!(app.review.pr.as_ref().unwrap().approved_head.is_none());
}

#[tokio::test]
async fn a_non_pr_review_never_reaches_the_effect() {
    let fk = fakes("ok");
    let mut app = app_for(notes_units(), None, KeyMap::default());
    app.overlay = Overlay::AllDone;
    assert!(key(&mut app, 'a').is_empty());
    assert!(key(&mut app, 'y').is_empty());
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(fk.calls(), 0);
}
