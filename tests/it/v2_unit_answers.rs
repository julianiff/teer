//! The agent's answers about a unit in the unit view, with no agent
//! running: what to do about it and the tools' panels as they draw
//! (sources, the Behavior table, what is on its way, untrusted text, any
//! size), a question going out once, a failure, the web, the note line,
//! and the answers and the note surviving a save and a re-run.

use crate::common;

use std::path::PathBuf;

use common::v2::{press, review, screen};
use common::{FixtureRepo, PY_NEW, PY_OLD, isolate_env};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use teer::config::Config;
use teer::keymap::Keymap;
use teer::review::dive::DiveEvent;
use teer::review::model::{
    Decision, Dive, DiveKind, DiveResult, PrInfo, Prep, Range, Review, SessionRef, Unit, UnitKind,
    UnitStatus,
};
use teer::review::{BuildOptions, build_review, store};
use teer::session::app::{AppEvent, Effect, NOTE_MAX, Overlay, Screen};
use teer::session::unit::Panel;
use teer::tui::app::App;

/// Unit `u{i}`, a described function `func_{i}` in `src/f{i}.rs`, pending.
fn unit(i: usize) -> Unit {
    Unit {
        id: format!("u{i}"),
        file: PathBuf::from(format!("src/f{i}.rs")),
        symbol: format!("func_{i}"),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: None,
        new_range: Some(Range::new(4 + 10 * i as u32, 8 + 10 * i as u32)),
        renamed_from: None,
        fingerprint: format!("fp{i}"),
        changes: Vec::new(),
        prep: Prep::Unavailable,
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Pending,
        summary: Some("Changes how the thing is computed".into()),
        nbc_reason: None,
        dive: Default::default(),
        accepted_by: None,
        reasons: Vec::new(),
        dive_pending: Default::default(),
        checks: Default::default(),
        security: Vec::new(),
    }
}

/// `n` units under `cfg`, on the queue, with an agent to ask.
fn app_with(n: usize, cfg: Config) -> App {
    isolate_env();
    let mut r = review((0..n).map(unit).collect());
    r.agent_session = Some(SessionRef::fresh("mock", r.repo_root.clone()));
    let mut app = App::new(r, cfg, Keymap::default());
    app.has_agent = true;
    app
}

fn app(n: usize) -> App {
    app_with(n, Config::default())
}

/// Open the unit view on `id` (`l` on its row); what opening it asked.
fn open(app: &mut App, id: &str) -> Vec<Effect> {
    app.sel = app
        .order
        .iter()
        .position(|&i| app.review.units[i].id == id)
        .unwrap();
    let effects = press(app, "l");
    assert_eq!(app.screen, Screen::Unit);
    assert_eq!(app.unit.id, id);
    effects
}

fn ask(id: &str, kind: DiveKind, web: bool) -> Effect {
    Effect::Dive {
        unit_id: id.into(),
        kind,
        web,
    }
}

/// The questions about units `effects` sends out: (unit, kind, web).
fn asked(effects: &[Effect]) -> Vec<(String, DiveKind, bool)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Dive { unit_id, kind, web } => Some((unit_id.clone(), *kind, *web)),
            _ => None,
        })
        .collect()
}

fn result(text: &str) -> DiveResult {
    DiveResult {
        text: text.into(),
        sources: Vec::new(),
        coverage: None,
        audit: None,
        proposal: None,
    }
}

fn answer(app: &mut App, id: &str, kind: DiveKind, res: DiveResult) {
    app.review
        .unit_mut(id)
        .unwrap()
        .dive
        .results
        .insert(kind, res);
}

fn code(a: &mut App, c: KeyCode) -> Vec<Effect> {
    a.on_event(Event::Key(KeyEvent::new(c, KeyModifiers::NONE)))
}

// ---- Advice ---------------------------------------------------------------------

#[test]
fn the_advice_shows_with_its_sources_and_who_wrote_it() {
    let mut a = app(3);
    answer(
        &mut a,
        "u0",
        DiveKind::Recommend,
        DiveResult {
            sources: vec!["https://example.com/rounding".into()],
            ..result("It rounds the total before tax.")
        },
    );
    // Opening asks nothing.
    let effects = open(&mut a, "u0");
    assert!(asked(&effects).is_empty(), "{effects:?}");
    let out = screen(&a, 120, 40);
    let at = |s: &str| out.find(s).unwrap_or_else(|| panic!("{s} missing:\n{out}"));
    // In the ask tile, open from the start, under its heading; questions
    // about a unit run on Claude Code's own model, which the heading names.
    assert_eq!(a.unit.tiles, [Panel::Ask]);
    assert!(at("Advice  agent (Claude Code's default model)") < at("It rounds"));
    assert!(at("It rounds the total before tax.") < at("Sources"));
    assert!(out.contains("example.com/rounding"), "{out}");
}

#[test]
fn what_is_on_its_way_says_so() {
    let mut a = app(2);
    // Opening asks nothing; `<space>a` does, and its tile waits for it.
    let effects = open(&mut a, "u0");
    assert!(asked(&effects).is_empty(), "{effects:?}");
    let effects = press(&mut a, "K<cr>");
    assert!(asked(&effects).contains(&("u0".into(), DiveKind::Recommend, false)));
    let out = screen(&a, 120, 40);
    assert!(out.contains("asking the agent…"), "{out}");
}

#[test]
fn untrusted_text_is_sanitized() {
    let mut a = app(1);
    answer(
        &mut a,
        "u0",
        DiveKind::Recommend,
        DiveResult {
            sources: vec!["src\x1b]0;x".into()],
            ..result("bad\x1b[31m text\x07")
        },
    );
    a.review.unit_mut("u0").unwrap().dive.note = "n\x1b[2Jote".into();
    open(&mut a, "u0");
    let out = screen(&a, 100, 30);
    assert!(!out.contains('\x1b'), "{out:?}");
    assert!(!out.contains('\x07'), "{out:?}");
}

#[test]
fn tiny_sizes_never_panic() {
    let mut a = app(3);
    answer(
        &mut a,
        "u0",
        DiveKind::Recommend,
        DiveResult {
            sources: vec!["s".repeat(200)],
            ..result(&"word ".repeat(100))
        },
    );
    open(&mut a, "u0");
    let sizes = [
        (1, 1),
        (2, 2),
        (5, 3),
        (10, 4),
        (20, 6),
        (30, 8),
        (89, 10),
        (90, 10),
        (40, 3),
    ];
    // No tile, the picture's, the ask tile, and three tiles at once.
    for panel in [None, Some("<space>x"), Some("K"), Some("<space>x<space>tK")] {
        if let Some(keys) = panel {
            press(&mut a, keys);
        }
        for (w, h) in sizes {
            screen(&a, w, h);
            for overlay in [
                Overlay::Note {
                    id: "u0".into(),
                    text: "hello".into(),
                    back: None,
                },
                Overlay::ConfirmWeb,
                Overlay::Help,
            ] {
                a.overlay = overlay;
                screen(&a, w, h);
            }
            a.overlay = Overlay::None;
        }
        // esc stops typing, then closes one tile at a time, down to what
        // to do about it, open from the start.
        for _ in 0..4 {
            if a.unit.tiles == [Panel::Ask] {
                break;
            }
            press(&mut a, "<esc>");
        }
        assert_eq!(a.screen, Screen::Unit);
    }
    // Nothing left to show.
    a.review.units.clear();
    for (w, h) in [(1, 1), (60, 20)] {
        let out = screen(&a, w, h);
        if w > 1 {
            assert!(out.contains("Nothing to show."), "{out}");
        }
    }
}

#[test]
fn the_note_line_and_the_web_question_draw_in_the_unit_view() {
    let mut a = app(1);
    open(&mut a, "u0");
    a.overlay = Overlay::Note {
        id: "u0".into(),
        text: "remember to check rounding".into(),
        back: None,
    };
    let out = screen(&a, 80, 24);
    let bar = out.lines().last().unwrap();
    assert!(
        bar.contains("note on func_0: remember to check rounding"),
        "{bar}"
    );
    assert!(bar.contains("⏎ save · esc cancel"), "{bar}");
    // A long note keeps its end in view.
    a.overlay = Overlay::Note {
        id: "u0".into(),
        text: format!("{}end", "x".repeat(300)),
        back: None,
    };
    let out = screen(&a, 40, 12);
    let bar = out.lines().last().unwrap();
    assert!(bar.contains("…") && bar.contains("xend"), "{bar}");
    a.overlay = Overlay::ConfirmWeb;
    let out = screen(&a, 80, 24);
    assert!(out.contains("never your code"), "{out}");
    assert!(out.contains("y/⏎ yes · esc no"), "{out}");
}

// ---- Asking -------------------------------------------------------------------

#[test]
fn a_failed_answer_clears_the_spinner_and_can_be_asked_again() {
    let mut a = app(2);
    open(&mut a, "u0");
    press(&mut a, "K<cr><space>x");
    for kind in [DiveKind::Security, DiveKind::Recommend] {
        let fingerprint = a.review.unit("u0").unwrap().fingerprint.clone();
        let fx = a.on_app_event(AppEvent::Dive(DiveEvent::Failed {
            id: "u0".into(),
            fingerprint,
            kind,
            error: "boom".into(),
        }));
        assert_eq!(fx, vec![Effect::Save]);
        assert!(a.status.contains("boom"), "{}", a.status);
    }
    assert!(
        !a.review
            .unit("u0")
            .unwrap()
            .dive_pending
            .contains(&DiveKind::Security)
    );
    assert!(
        !a.review
            .unit("u0")
            .unwrap()
            .dive_pending
            .contains(&DiveKind::Recommend)
    );
    let out = screen(&a, 120, 50);
    assert!(
        out.contains("␣x asks the agent how this change can be abused."),
        "{out}"
    );
    assert!(
        out.contains("1 What should I do about this change"),
        "{out}"
    );
    assert!(!out.contains("asking the agent"), "{out}");
    // The tool's key asks again, and so does the advice's.
    assert_eq!(
        press(&mut a, "<space>x"),
        [ask("u0", DiveKind::Security, false)]
    );
    press(&mut a, "<esc>h");
    assert_eq!(
        asked(&press(&mut a, "K<cr>")),
        [("u0".into(), DiveKind::Recommend, false)]
    );
}

#[test]
fn the_web_goes_to_the_advice_once_allowed_and_never_to_the_tools() {
    // Before the reviewer said yes to the ask panel's question: no web.
    let mut a = app(6);
    open(&mut a, "u0");
    let effects = press(&mut a, "K<cr>");
    assert_eq!(asked(&effects), [("u0".into(), DiveKind::Recommend, false)]);
    press(&mut a, "Kwhy?<cr>");
    assert_eq!(a.overlay, Overlay::ConfirmWeb);
    press(&mut a, "y");
    assert!(a.web_allowed());
    // Then the advice uses it; the picture never does.
    press(&mut a, "<esc>");
    let effects = press(&mut a, "nK<cr>");
    let recommend: Vec<_> = asked(&effects)
        .into_iter()
        .filter(|(_, kind, _)| *kind == DiveKind::Recommend)
        .collect();
    assert!(!recommend.is_empty(), "{effects:?}");
    assert!(recommend.iter().all(|(_, _, web)| *web), "{effects:?}");
    let effects = press(&mut a, "<space>x");
    assert!(
        asked(&effects).iter().all(|(_, _, web)| !web),
        "{effects:?}"
    );

    // `dive_web = false`: never asked, never used.
    let cfg = Config {
        dive_web: false,
        ..Config::default()
    };
    let mut a = app_with(2, cfg);
    open(&mut a, "u0");
    let effects = press(&mut a, "Kwhy?<cr>");
    assert_eq!(a.overlay, Overlay::None);
    assert!(effects.contains(&Effect::Ask { index: 0 }), "{effects:?}");
    assert!(!a.web_allowed());

    // A pull request: the same, whatever the config says.
    let mut a = app(2);
    a.review.pr = Some(PrInfo {
        host: "github.com".into(),
        owner: "acme".into(),
        repo: "shop".into(),
        number: 7,
        ..Default::default()
    });
    let effects = open(&mut a, "u0");
    assert!(
        asked(&effects).iter().all(|(_, _, web)| !web),
        "{effects:?}"
    );
    press(&mut a, "Kwhy?<cr>");
    assert_eq!(a.overlay, Overlay::None, "no web question for a PR");
    assert!(!a.web_allowed());
}

// ---- The note -----------------------------------------------------------------

#[test]
fn the_note_line_types_edits_saves_and_cancels() {
    let mut a = app(2);
    a.review.unit_mut("u0").unwrap().dive.note = "hi".into();
    open(&mut a, "u0");
    press(&mut a, "i");
    assert!(matches!(&a.overlay, Overlay::Note { text, .. } if text == "hi"));
    // Letters that are actions elsewhere are plain text here.
    press(&mut a, "xq");
    code(&mut a, KeyCode::Char('é'));
    assert!(matches!(&a.overlay, Overlay::Note { text, .. } if text == "hixqé"));
    code(&mut a, KeyCode::Backspace);
    assert!(matches!(&a.overlay, Overlay::Note { text, .. } if text == "hixq"));
    // Control characters and ctrl chords are rejected.
    code(&mut a, KeyCode::Char('\u{1b}'));
    code(&mut a, KeyCode::Char('\n'));
    a.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('z'),
        KeyModifiers::CONTROL,
    )));
    assert!(matches!(&a.overlay, Overlay::Note { text, .. } if text == "hixq"));
    assert_eq!(code(&mut a, KeyCode::Enter), vec![Effect::Save]);
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(a.review.unit("u0").unwrap().dive.note, "hixq");
    assert_eq!((a.screen, a.unit.id.as_str()), (Screen::Unit, "u0"));
    // Esc cancels without changing the note.
    press(&mut a, "iz");
    assert!(code(&mut a, KeyCode::Esc).is_empty());
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(a.review.unit("u0").unwrap().dive.note, "hixq");
    assert_eq!(a.screen, Screen::Unit, "esc only closed the note line");
}

#[test]
fn a_note_is_capped() {
    let mut a = app(1);
    open(&mut a, "u0");
    press(&mut a, "i");
    for _ in 0..NOTE_MAX + 100 {
        code(&mut a, KeyCode::Char('a'));
    }
    code(&mut a, KeyCode::Enter);
    assert_eq!(
        a.review.unit("u0").unwrap().dive.note.chars().count(),
        NOTE_MAX
    );
}

#[test]
fn any_key_closes_the_help_and_q_leaves_the_unit_view_then_quits() {
    let mut a = app(1);
    open(&mut a, "u0");
    press(&mut a, "?");
    assert_eq!(a.overlay, Overlay::Help);
    press(&mut a, "a");
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(a.screen, Screen::Unit, "the key only closed the help");
    assert_eq!(a.review.unit("u0").unwrap().decision, Decision::Pending);
    // `q` closes the unit view; in the queue it quits.
    assert!(!press(&mut a, "q").contains(&Effect::Quit));
    assert_eq!(a.screen, Screen::Queue);
    assert_eq!(press(&mut a, "q"), vec![Effect::Save, Effect::Quit]);
}

// ---- Saved --------------------------------------------------------------------

/// Unit `id` with fingerprint `fp`, as a saved review has it.
fn saved_unit(id: &str, fp: &str) -> Unit {
    serde_json::from_value(serde_json::json!({
        "id": id, "file": "a.rs", "symbol": id, "kind": "Function",
        "status": "Changed", "old_range": null, "new_range": null,
        "fingerprint": fp, "changes": [], "callers": [], "callees": [],
        "decision": "Pending"
    }))
    .unwrap()
}

fn saved_review(tag: &str, units: Vec<Unit>) -> Review {
    Review {
        id: "r".into(),
        repo_root: PathBuf::from(format!("/work/dive-store-{tag}")),
        head: "WORKTREE".into(),
        head_commit: "abcdef123456".into(),
        ..review(units)
    }
}

/// A flagged unit with two answers and a note.
fn answered(id: &str, fp: &str) -> Unit {
    let mut u = saved_unit(id, fp);
    u.decision = Decision::Investigate;
    u.dive.results.insert(
        DiveKind::Recommend,
        DiveResult {
            sources: vec!["a.rs:3".into()],
            ..result("because")
        },
    );
    u.dive.results.insert(
        DiveKind::Tests,
        DiveResult {
            sources: vec!["https://owasp.org".into()],
            ..result("OWASP says so")
        },
    );
    u.dive.note = "ask Sam".into();
    u
}

#[test]
fn answers_and_the_note_survive_save_and_reload() {
    let _env = isolate_env();
    let r = saved_review("roundtrip", vec![answered("a", "fp")]);
    store::save(&r).unwrap();
    let back = store::load(&r.repo_root, None, &r.base, &r.head, &r.head_commit).unwrap();
    assert_eq!(back.units[0].dive, r.units[0].dive);
    assert_eq!(back.units[0].decision, Decision::Investigate);
    let latest = store::load_latest(&r.repo_root, None).unwrap();
    assert_eq!(latest.units[0].dive.note, "ask Sam");
}

#[test]
fn questions_on_their_way_never_persist() {
    let _env = isolate_env();
    let mut r = saved_review("pending", vec![answered("a", "fp")]);
    r.units[0].dive_pending.insert(DiveKind::Recommend);
    let path = store::save(&r).unwrap();
    let text = std::fs::read_to_string(path).unwrap();
    assert!(!text.contains("dive_pending"), "{text}");
    let back = store::load(&r.repo_root, None, &r.base, &r.head, &r.head_commit).unwrap();
    assert!(back.units[0].dive_pending.is_empty());
}

#[test]
fn changed_code_drops_the_answers_and_the_decision_but_keeps_the_note() {
    let old = saved_review(
        "fp",
        vec![answered("keep", "same"), answered("edit", "before")],
    );
    let mut new = saved_review(
        "fp",
        vec![saved_unit("keep", "same"), saved_unit("edit", "after")],
    );
    store::merge_previous(&mut new, &old);
    assert_eq!(new.units[0].decision, Decision::Investigate);
    assert_eq!(new.units[0].dive, old.units[0].dive);
    // Only the reviewer's note is left: it may be what changed the code.
    let note_only = Dive {
        note: old.units[1].dive.note.clone(),
        ..Dive::default()
    };
    assert_eq!(new.units[1].dive, note_only);
    assert_eq!(new.units[1].decision, Decision::Pending);
}

#[test]
fn rerunning_a_review_keeps_the_answers_for_unchanged_units() {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD).commit("init");
    repo.write("checkout.py", PY_NEW);
    let mut first = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    assert!(!first.units.is_empty());
    let id = first.units[0].id.clone();
    first.units[0].decision = Decision::Investigate;
    first.units[0].dive = answered(&id, "x").dive;
    let want = first.units[0].dive.clone();

    let mut app = App::new(first, Config::default(), Keymap::default());
    let again = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    app.on_app_event(AppEvent::Rerun(Ok(Box::new(again))));

    let u = app.review.units.iter().find(|u| u.id == id).unwrap();
    assert_eq!(u.decision, Decision::Investigate);
    assert_eq!(u.dive, want);
    assert!(u.dive_pending.is_empty());
}
