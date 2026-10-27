//! The "all reviewed" popup: when it opens, what its keys do, how it draws.

mod common;

use std::path::PathBuf;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use teer::config::{Config, KeyMap};
use teer::review::model::{Decision, Prep, Range, Review, Unit, UnitKind, UnitStatus};
use teer::tui::app::{App, AppEvent, Effect, Overlay, Screen};
use teer::tui::ui;

/// Unit `i` lands in a group by `i % 4`: routine (0), worth a look (1),
/// close look (2), no behavior change (3). With 12 units the list is three
/// boxes of three: positions 0-2 close look, 3-5 worth a look, 6-8 routine.
fn unit(i: usize) -> Unit {
    let mut u = Unit {
        id: format!("file{i:03}.rs::sym{i:03}"),
        file: PathBuf::from(format!("src/file{i:03}.rs")),
        symbol: format!("sym{i:03}"),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: None,
        new_range: Some(Range::new(10 + i as u32, 20 + i as u32)),
        renamed_from: None,
        fingerprint: String::new(),
        changes: Vec::new(),
        prep: Prep::Skipped,
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Pending,
        summary: Some("summary".into()),
        nbc_reason: None,
        dive: Default::default(),
        dive_pending: Default::default(),
        checks: Default::default(),
    };
    match i % 4 {
        1 => u.prep = Prep::Unavailable,
        2 => {
            u.status = UnitStatus::Removed;
            u.callers = vec!["somebody".into()];
        }
        3 => {
            u.status = UnitStatus::NoBehaviorChange;
            u.nbc_reason = Some("formatting only".into());
        }
        _ => {}
    }
    u
}

fn app_with(n: usize, cfg: Config, keys: KeyMap) -> App {
    common::isolate_env();
    let review = Review {
        id: "t".into(),
        repo_root: PathBuf::from("/r"),
        base: "b".into(),
        head: "h".into(),
        head_commit: "h".into(),
        agent_session: None,
        pr: None,
        intent: Default::default(),
        created_at: chrono::Utc::now(),
        files: Vec::new(),
        units: (0..n).map(unit).collect(),
        skipped: Vec::new(),
        graph: Default::default(),
        questions: Vec::new(),
    };
    App::new(review, cfg, keys)
}

fn app(n: usize) -> App {
    app_with(n, Config::default(), KeyMap::default())
}

fn draw(app: &App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    t.backend().buffer().clone()
}

fn rows(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

fn text(app: &App, w: u16, h: u16) -> String {
    rows(&draw(app, w, h)).join("\n")
}

fn press(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Vec<Effect> {
    app.on_event(Event::Key(KeyEvent::new(code, mods)))
}

fn key(app: &mut App, c: char) -> Vec<Effect> {
    press(app, KeyCode::Char(c), KeyModifiers::NONE)
}

fn shift_a(app: &mut App) -> Vec<Effect> {
    press(app, KeyCode::Char('A'), KeyModifiers::SHIFT)
}

fn decisions(a: &App) -> Vec<Decision> {
    a.review.units.iter().map(|u| u.decision).collect()
}

fn mouse_up(a: &mut App) -> Vec<Effect> {
    a.on_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 5,
        row: 5,
        modifiers: KeyModifiers::NONE,
    }))
}

/// Three units that need a look (no behavior-change one), list screen.
fn three() -> App {
    app(3)
}

/// Units 0..=n-1 accepted except the last, which is pending.
fn all_but_last() -> App {
    let mut a = three();
    for u in a.review.units.iter_mut().take(2) {
        u.decision = Decision::Accepted;
    }
    a
}

/// Accept the one pending unit through the list.
fn accept_last(a: &mut App) {
    let pos = a
        .order
        .iter()
        .position(|&i| a.review.units[i].decision == Decision::Pending)
        .unwrap();
    a.sel = pos;
    key(a, 'l');
}

fn done() -> App {
    let mut a = all_but_last();
    accept_last(&mut a);
    assert_eq!(a.overlay, Overlay::AllDone);
    a
}

// ---- Opening ---------------------------------------------------------

#[test]
fn accepting_the_last_pending_unit_in_the_list_opens_it() {
    let mut a = three();
    for _ in 0..2 {
        key(&mut a, 'l');
        assert_eq!(a.overlay, Overlay::None);
    }
    key(&mut a, 'l');
    assert_eq!(a.overlay, Overlay::AllDone);
}

#[test]
fn a_no_behavior_change_unit_does_not_hold_it_back() {
    let mut a = app(4);
    assert_eq!(a.review.units[3].decision, Decision::Pending);
    for u in a.review.units.iter_mut().take(2) {
        u.decision = Decision::Accepted;
    }
    accept_last(&mut a);
    assert_eq!(a.overlay, Overlay::AllDone);
}

#[test]
fn the_last_swipe_card_opens_it_over_the_summary() {
    let mut a = three();
    key(&mut a, 's');
    assert_eq!(a.screen, Screen::Swipe);
    key(&mut a, 'l');
    key(&mut a, 'l');
    assert_eq!(a.overlay, Overlay::None);
    key(&mut a, 'l');
    assert_eq!(a.overlay, Overlay::AllDone);
    assert!(a.swipe_current().is_none());
    let t = text(&a, 90, 24);
    assert!(t.contains("Hurray!"));
    assert_eq!(a.screen, Screen::Swipe);
}

#[test]
fn right_arrow_and_drag_accept_open_it_too() {
    let mut a = all_but_last();
    key(&mut a, 's');
    press(&mut a, KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(a.overlay, Overlay::AllDone);
    let mut b = all_but_last();
    key(&mut b, 's');
    b.swipe_dir(teer::tui::swipe::Dir::Right);
    assert_eq!(b.overlay, Overlay::AllDone);
}

#[test]
fn accept_file_that_completes_everything_opens_it_after_the_confirm() {
    let mut a = all_but_last();
    // The remaining unit's file.
    a.sel = a
        .order
        .iter()
        .position(|&i| a.review.units[i].decision == Decision::Pending)
        .unwrap();
    shift_a(&mut a);
    assert!(matches!(a.overlay, Overlay::ConfirmAcceptFile { .. }));
    key(&mut a, 'y');
    assert_eq!(a.overlay, Overlay::AllDone);
}

#[test]
fn not_shown_while_a_unit_is_flagged() {
    let mut a = all_but_last();
    a.review.units[0].decision = Decision::Investigate;
    accept_last(&mut a);
    assert_eq!(a.overlay, Overlay::None);
    // Accepting the flagged one too finishes it.
    a.sel = a
        .order
        .iter()
        .position(|&i| a.review.units[i].decision == Decision::Investigate)
        .unwrap();
    key(&mut a, 'l');
    assert_eq!(a.overlay, Overlay::AllDone);
}

#[test]
fn not_shown_for_an_already_accepted_review() {
    let mut a = three();
    for u in a.review.units.iter_mut() {
        u.decision = Decision::Accepted;
    }
    a.start(false, None, Vec::new(), None, false);
    assert_eq!(a.overlay, Overlay::None);
    let mut b = three();
    for u in b.review.units.iter_mut() {
        u.decision = Decision::Accepted;
    }
    b.start(false, None, Vec::new(), None, true);
    assert_eq!(b.overlay, Overlay::None);
}

#[test]
fn not_shown_after_undo_reset_or_rerun() {
    let mut a = done();
    key(&mut a, 'x');
    assert_eq!(a.overlay, Overlay::None);
    // Undo: back to something left, no popup.
    key(&mut a, 'u');
    assert_eq!(a.overlay, Overlay::None);
    assert!(!a.all_reviewed());
    // Undoing further and re-accepting is fine; undo itself never opens it.
    let mut b = done();
    key(&mut b, 'x');
    key(&mut b, 'R');
    key(&mut b, 'y');
    assert_eq!(b.overlay, Overlay::None);
    assert!(
        b.review
            .units
            .iter()
            .all(|u| u.decision == Decision::Pending)
    );
    // Rerun of an all-accepted review.
    let mut c = done();
    key(&mut c, 'x');
    let new = Review {
        units: (0..3).map(unit).collect(),
        ..c.review.clone()
    };
    c.on_app_event(AppEvent::Rerun(Ok(Box::new(new))));
    assert_eq!(c.overlay, Overlay::None);
}

#[test]
fn not_repeated_but_shown_again_after_undo_and_accept() {
    let mut a = done();
    key(&mut a, 'x');
    assert_eq!(a.overlay, Overlay::None);
    key(&mut a, 'j');
    assert_eq!(a.overlay, Overlay::None);
    key(&mut a, 'u');
    assert_eq!(a.overlay, Overlay::None);
    accept_last(&mut a);
    assert_eq!(a.overlay, Overlay::AllDone);
}

// ---- Keys ------------------------------------------------------------

#[test]
fn q_saves_and_quits() {
    let mut a = done();
    assert_eq!(key(&mut a, 'q'), vec![Effect::Save, Effect::Quit]);
    assert_eq!(a.overlay, Overlay::None);
}

#[test]
fn ctrl_c_still_quits() {
    let mut a = done();
    assert_eq!(
        press(&mut a, KeyCode::Char('c'), KeyModifiers::CONTROL),
        vec![Effect::Save, Effect::Quit]
    );
}

#[test]
fn r_opens_the_reset_confirm_and_y_resets() {
    let mut a = done();
    assert!(key(&mut a, 'R').is_empty());
    assert_eq!(a.overlay, Overlay::ConfirmReset);
    assert!(
        a.review
            .units
            .iter()
            .all(|u| u.decision == Decision::Accepted)
    );
    assert_eq!(key(&mut a, 'y'), vec![Effect::Save]);
    assert_eq!(a.overlay, Overlay::None);
    assert!(
        a.review
            .units
            .iter()
            .all(|u| u.decision == Decision::Pending)
    );
}

#[test]
fn other_keys_only_dismiss() {
    for code in [
        KeyCode::Esc,
        KeyCode::Enter,
        KeyCode::Char('j'),
        KeyCode::Char('x'),
    ] {
        let mut a = done();
        let sel = a.sel;
        let before = decisions(&a);
        assert!(press(&mut a, code, KeyModifiers::NONE).is_empty());
        assert_eq!(a.overlay, Overlay::None, "{code:?}");
        assert_eq!(a.sel, sel, "{code:?} must not also move");
        assert_eq!(decisions(&a), before);
        assert_eq!(a.screen, Screen::Slim);
    }
    // `d` would open Flagged and `s` swipe, `?` help: none of that happens.
    for c in ['d', 's', '?', 'u'] {
        let mut a = done();
        let n = a.swipe.history.len();
        key(&mut a, c);
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.screen, Screen::Slim, "{c}");
        assert_eq!(a.swipe.history.len(), n, "{c} must not undo");
        assert!(a.all_reviewed());
    }
}

#[test]
fn a_mouse_click_dismisses() {
    let mut a = done();
    draw(&a, 80, 24);
    assert!(mouse_up(&mut a).is_empty());
    assert_eq!(a.overlay, Overlay::None);
}

#[test]
fn rebound_quit_and_reset_keys() {
    let toml = r#"
[keys]
quit = "x"
reset = "ctrl+r"
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    let keys = cfg.keymap().unwrap();
    let mut a = app_with(3, cfg.clone(), keys.clone());
    for u in a.review.units.iter_mut().take(2) {
        u.decision = Decision::Accepted;
    }
    accept_last(&mut a);
    assert_eq!(a.overlay, Overlay::AllDone);
    let t = text(&a, 80, 24);
    assert!(t.contains("x quit"), "{t}");
    assert!(t.contains("^r reset"), "{t}");
    // The old keys now just dismiss.
    let mut b = a;
    assert!(key(&mut b, 'q').is_empty());
    assert_eq!(b.overlay, Overlay::None);
    let mut c = app_with(3, cfg, keys);
    for u in c.review.units.iter_mut().take(2) {
        u.decision = Decision::Accepted;
    }
    accept_last(&mut c);
    assert_eq!(key(&mut c, 'x'), vec![Effect::Save, Effect::Quit]);
    let mut d = app_with(3, Config::default(), KeyMap::default());
    d.overlay = Overlay::AllDone;
    press(&mut d, KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(d.overlay, Overlay::None);
}

// ---- Rendering -------------------------------------------------------

#[test]
fn popup_renders_inside_the_screen_at_common_sizes() {
    for (w, h) in [(60u16, 20u16), (80, 24), (120, 40)] {
        let a = done();
        let r = rows(&draw(&a, w, h));
        let joined = r.join("\n");
        assert!(joined.contains("Hurray! All changes reviewed."), "{w}x{h}");
        assert!(
            joined.contains("q quit  ·  R reset  ·  any other key to continue"),
            "{w}x{h}\n{joined}"
        );
        assert!(joined.contains('╭') && joined.contains('╯'));
        assert_eq!(r.len(), h as usize);
        assert!(r.iter().all(|l| l.chars().count() == w as usize));
    }
}

#[test]
fn popup_never_panics_at_tiny_sizes_on_list_or_swipe() {
    for swipe in [false, true] {
        let mut a = done();
        if swipe {
            a.enter_swipe();
            a.overlay = Overlay::AllDone;
        }
        for (w, h) in [(0u16, 0u16), (1, 1), (10, 3), (3, 10), (20, 6), (200, 3)] {
            draw(&a, w, h);
        }
    }
}
