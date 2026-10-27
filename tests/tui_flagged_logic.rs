//! Flagged screen logic: entry, navigation, requests, web gating, notes.

mod common;

use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use teer::config::{Config, KeyMap};
use teer::review::dive::DiveEvent;
use teer::review::model::{
    Decision, DiveKind, DiveResult, Prep, Resolution, Review, Unit, UnitKind, UnitStatus,
};
use teer::tui::app::{App, AppEvent, Effect, NOTE_MAX, Overlay, Pane, Screen};

/// `n` units; the first `flagged` are flagged (Investigate).
fn app_with(n: usize, flagged: usize, cfg: Config) -> App {
    common::isolate_env();
    let units = (0..n)
        .map(|i| Unit {
            id: format!("u{i}"),
            file: PathBuf::from("f.rs"),
            symbol: format!("s{i}"),
            kind: UnitKind::Function,
            status: UnitStatus::Changed,
            old_range: None,
            new_range: None,
            renamed_from: None,
            fingerprint: String::new(),
            changes: Vec::new(),
            prep: Prep::Skipped,
            callers: Vec::new(),
            callees: Vec::new(),
            decision: if i < flagged {
                Decision::Investigate
            } else {
                Decision::Pending
            },
            summary: None,
            nbc_reason: None,
            dive: Default::default(),
            dive_pending: Default::default(),
            checks: Default::default(),
        })
        .collect();
    let review = Review {
        id: "t".into(),
        repo_root: PathBuf::new(),
        base: String::new(),
        head: String::new(),
        head_commit: String::new(),
        agent_session: None,
        pr: None,
        intent: Default::default(),
        created_at: chrono::Utc::now(),
        files: Vec::new(),
        units,
        skipped: Vec::new(),
        graph: Default::default(),
        questions: Vec::new(),
    };
    App::new(review, cfg, KeyMap::default())
}

fn app(n: usize, flagged: usize) -> App {
    app_with(n, flagged, Config::default())
}

fn code(a: &mut App, c: KeyCode) -> Vec<Effect> {
    a.on_event(Event::Key(KeyEvent::new(c, KeyModifiers::NONE)))
}

fn key(a: &mut App, c: char) -> Vec<Effect> {
    code(a, KeyCode::Char(c))
}

fn dive(a: &mut App) {
    key(a, 'd');
    assert_eq!(a.screen, Screen::Flagged);
}

fn unit<'a>(a: &'a App, id: &str) -> &'a Unit {
    a.review.unit(id).unwrap()
}

fn ask(id: &str, kind: DiveKind, web: bool) -> Effect {
    Effect::Dive {
        unit_id: id.into(),
        kind,
        web,
    }
}

#[test]
fn entry_from_list_and_swipe_and_back() {
    for from in [Screen::Slim, Screen::Swipe] {
        let mut a = app(4, 2);
        match from {
            Screen::Swipe => a.enter_swipe(),
            s => a.screen = s,
        }
        assert_eq!(a.screen, from);
        dive(&mut a);
        assert_eq!(a.flagged.sel, 0);
        assert_eq!(a.flagged_unit_id().as_deref(), Some("u0"));
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.screen, from);
    }
}

#[test]
fn nothing_flagged_stays_with_a_message() {
    let mut a = app(3, 0);
    key(&mut a, 'd');
    assert_eq!(a.screen, Screen::Slim);
    assert!(!a.status.is_empty());
}

/// The tile in focus.
fn focus(a: &App) -> Pane {
    a.flagged_focus(a.flagged_unit().unwrap())
}

fn answered(a: &mut App, id: &str, kinds: &[DiveKind]) {
    let u = a.review.unit_mut(id).unwrap();
    for k in kinds {
        u.dive.results.insert(*k, Default::default());
        u.dive.asked.push(*k);
    }
}

#[test]
fn navigation_over_units_and_tiles() {
    let mut a = app(5, 3);
    answered(&mut a, "u0", &[DiveKind::Visualize, DiveKind::Tests]);
    dive(&mut a);
    // The latest answer has the focus; the tiles go in the order asked,
    // then the one that asks for more, and around.
    assert_eq!(focus(&a), Pane::Answer(DiveKind::Tests));
    code(&mut a, KeyCode::Right);
    assert_eq!(focus(&a), Pane::More);
    code(&mut a, KeyCode::Tab);
    assert_eq!(focus(&a), Pane::Answer(DiveKind::Visualize), "wraps");
    code(&mut a, KeyCode::Left);
    assert_eq!(focus(&a), Pane::More, "wraps back");
    code(&mut a, KeyCode::BackTab);
    assert_eq!(focus(&a), Pane::Answer(DiveKind::Tests));
    // z zooms the tile in focus; esc zooms out before it goes back.
    key(&mut a, 'z');
    assert!(a.flagged.zoom);
    code(&mut a, KeyCode::Esc);
    assert!(!a.flagged.zoom);
    assert_eq!(a.screen, Screen::Flagged);
    // Another unit, nothing asked: only the tile that asks for more.
    key(&mut a, 'j');
    assert_eq!(a.flagged.sel, 1);
    assert_eq!(focus(&a), Pane::More);
    code(&mut a, KeyCode::Right);
    assert_eq!(focus(&a), Pane::More);
    key(&mut a, 'j');
    key(&mut a, 'j');
    assert_eq!(a.flagged.sel, 2, "clamped to the last flagged unit");
    code(&mut a, KeyCode::Up);
    code(&mut a, KeyCode::Up);
    assert_eq!(a.flagged.sel, 0);
    assert_eq!(
        focus(&a),
        Pane::Answer(DiveKind::Tests),
        "its latest answer"
    );
    // The removed questions' keys do nothing here.
    assert!(key(&mut a, 'w').is_empty());
    // Moving around never asks anything.
    assert!(a.review.units.iter().all(|u| u.dive_pending.is_empty()));
}

#[test]
fn request_is_deduplicated_and_selecting_does_not_request() {
    let cfg = Config {
        dive_web: false,
        ..Config::default()
    };
    let mut a = app_with(2, 2, cfg);
    dive(&mut a);
    assert!(key(&mut a, 'j').is_empty());
    key(&mut a, 'k');
    assert_eq!(
        key(&mut a, 'v'),
        vec![ask("u0", DiveKind::Visualize, false)]
    );
    assert!(unit(&a, "u0").dive_pending.contains(&DiveKind::Visualize));
    assert!(key(&mut a, 'v').is_empty(), "in flight");
    // An answer arrives; asking again only gives its tile the focus.
    let fx = a.on_app_event(AppEvent::Dive(DiveEvent::Ready {
        id: "u0".into(),
        kind: DiveKind::Visualize,
        text: "t".into(),
        sources: vec![],
        visual: None,
        behavior: None,
        coverage: None,
    }));
    assert_eq!(fx, vec![Effect::Save]);
    code(&mut a, KeyCode::Right);
    assert_eq!(focus(&a), Pane::More);
    assert!(key(&mut a, 'v').is_empty());
    assert_eq!(focus(&a), Pane::Answer(DiveKind::Visualize));
    // Recommend is a different kind and goes out, in a tile after it.
    assert_eq!(
        key(&mut a, 'n'),
        vec![ask("u0", DiveKind::Recommend, false)]
    );
    assert_eq!(focus(&a), Pane::Answer(DiveKind::Recommend));
    assert_eq!(
        unit(&a, "u0").dive.asked,
        [DiveKind::Visualize, DiveKind::Recommend]
    );
}

#[test]
fn failed_dive_clears_pending_and_reports() {
    let mut a = app(1, 1);
    dive(&mut a);
    key(&mut a, 'v');
    let fx = a.on_app_event(AppEvent::Dive(DiveEvent::Failed {
        id: "u0".into(),
        kind: DiveKind::Visualize,
        error: "boom".into(),
    }));
    assert_eq!(fx, vec![Effect::Save]);
    assert!(unit(&a, "u0").dive_pending.is_empty());
    assert!(a.status.contains("boom"));
    // It can be asked again.
    assert_eq!(key(&mut a, 'v').len(), 1);
}

#[test]
fn rerun_clears_result_and_asks_again() {
    let mut a = app(1, 1);
    dive(&mut a);
    a.review.unit_mut("u0").unwrap().dive.results.insert(
        DiveKind::Visualize,
        DiveResult {
            text: "old".into(),
            sources: vec![],
            visual: None,
            behavior: None,
            coverage: None,
        },
    );
    // Answered already: `v` only opens the tab, `r` asks it again.
    assert!(key(&mut a, 'v').is_empty());
    assert_eq!(key(&mut a, 'r'), vec![ask("u0", DiveKind::Visualize, true)]);
    assert!(unit(&a, "u0").dive.results.is_empty());
    assert!(key(&mut a, 'r').is_empty(), "already in flight");
}

#[test]
fn resolution_toggles_and_saves() {
    let mut a = app(1, 1);
    dive(&mut a);
    assert_eq!(key(&mut a, 'o'), vec![Effect::Save]);
    assert_eq!(unit(&a, "u0").dive.resolution, Some(Resolution::Ok));
    assert_eq!(key(&mut a, 'f'), vec![Effect::Save]);
    assert_eq!(unit(&a, "u0").dive.resolution, Some(Resolution::NeedsFix));
    assert_eq!(key(&mut a, 'f'), vec![Effect::Save]);
    assert_eq!(unit(&a, "u0").dive.resolution, None);
}

#[test]
fn note_editor_types_edits_saves_and_cancels() {
    let mut a = app(1, 1);
    a.review.unit_mut("u0").unwrap().dive.note = "hi".into();
    dive(&mut a);
    key(&mut a, 'm');
    assert!(matches!(&a.overlay, Overlay::Note { text, .. } if text == "hi"));
    // Letters that are actions elsewhere are plain text here.
    key(&mut a, 'x');
    key(&mut a, 'q');
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
    assert_eq!(unit(&a, "u0").dive.note, "hixq");
    assert!(a.screen == Screen::Flagged);
    // Esc cancels without changing the note.
    key(&mut a, 'm');
    key(&mut a, 'z');
    assert!(code(&mut a, KeyCode::Esc).is_empty());
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(unit(&a, "u0").dive.note, "hixq");
    assert_eq!(a.screen, Screen::Flagged, "Esc only closed the editor");
}

#[test]
fn note_is_capped() {
    let mut a = app(1, 1);
    dive(&mut a);
    key(&mut a, 'm');
    for _ in 0..NOTE_MAX + 100 {
        key(&mut a, 'a');
    }
    code(&mut a, KeyCode::Enter);
    assert_eq!(unit(&a, "u0").dive.note.chars().count(), NOTE_MAX);
}

#[test]
fn first_web_request_confirms_then_remembers() {
    let mut a = app(2, 2);
    dive(&mut a);
    assert!(key(&mut a, 'n').is_empty());
    assert_eq!(a.overlay, Overlay::ConfirmWeb);
    assert!(unit(&a, "u0").dive_pending.contains(&DiveKind::Recommend));
    assert_eq!(
        code(&mut a, KeyCode::Enter),
        vec![ask("u0", DiveKind::Recommend, true)]
    );
    assert_eq!(a.overlay, Overlay::None);
    // Remembered for the session: no second prompt for the next unit.
    key(&mut a, 'j');
    assert_eq!(key(&mut a, 'n'), vec![ask("u1", DiveKind::Recommend, true)]);
    assert_eq!(a.overlay, Overlay::None);
}

#[test]
fn declining_web_clears_pending() {
    let mut a = app(1, 1);
    dive(&mut a);
    key(&mut a, 'n');
    assert_eq!(a.overlay, Overlay::ConfirmWeb);
    assert!(key(&mut a, 'x').is_empty());
    assert_eq!(a.overlay, Overlay::None);
    assert!(unit(&a, "u0").dive_pending.is_empty());
    // Asking again prompts again.
    key(&mut a, 'n');
    assert_eq!(a.overlay, Overlay::ConfirmWeb);
    assert_eq!(key(&mut a, 'y'), vec![ask("u0", DiveKind::Recommend, true)]);
}

#[test]
fn dive_web_false_skips_the_prompt_and_disables_web() {
    let cfg = Config {
        dive_web: false,
        ..Config::default()
    };
    let mut a = app_with(1, 1, cfg);
    dive(&mut a);
    assert_eq!(
        key(&mut a, 'n'),
        vec![ask("u0", DiveKind::Recommend, false)]
    );
    assert_eq!(a.overlay, Overlay::None);
}

#[test]
fn keys_quit_help_and_unrelated_keys() {
    let mut a = app(1, 1);
    dive(&mut a);
    key(&mut a, '?');
    assert_eq!(a.overlay, Overlay::Help);
    key(&mut a, 'a');
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(key(&mut a, 'q'), vec![Effect::Save, Effect::Quit]);
}
