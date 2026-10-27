//! Triage in place on the Slim list: decisions, jumps, undo.
//! Driven through key events on a synthetic review (no repo, no agent).

mod common;

use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use teer::config::{Config, KeyMap};
use teer::review::model::{Decision, Prep, Review, Unit, UnitKind, UnitStatus};
use teer::tui::app::{App, Overlay, Screen};

/// `n` routine units, three per file (f0.rs, f1.rs, ...), listed in order.
fn app(n: usize) -> App {
    common::isolate_env();
    let units = (0..n)
        .map(|i| Unit {
            id: format!("u{i}"),
            file: PathBuf::from(format!("f{}.rs", i / 3)),
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
            decision: Decision::Pending,
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
    let a = App::new(review, Config::default(), KeyMap::default());
    assert_eq!(a.screen, Screen::Slim);
    assert_eq!(a.order.len(), n, "every unit is listed");
    a
}

fn key(a: &mut App, c: char) {
    a.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )));
}

fn code(a: &mut App, c: KeyCode) {
    a.on_event(Event::Key(KeyEvent::new(c, KeyModifiers::NONE)));
}

fn dec(a: &App, i: usize) -> Decision {
    a.review.units[a.order[i]].decision
}

fn decisions(a: &App) -> Vec<Decision> {
    (0..a.order.len()).map(|i| dec(a, i)).collect()
}

#[test]
fn space_and_v_are_not_bound_in_the_list() {
    let mut a = app(5);
    a.sel = 1;
    for c in [' ', 'v'] {
        key(&mut a, c);
        assert_eq!(a.sel, 1, "{c:?} doesn't move");
        assert_eq!(a.overlay, Overlay::None);
        assert!(decisions(&a).iter().all(|d| *d == Decision::Pending));
        assert!(a.swipe.history.is_empty());
    }
}

#[test]
fn l_h_i_decide_the_selected_unit_and_advance_to_next_pending() {
    let mut a = app(6);
    key(&mut a, 'l');
    assert_eq!(dec(&a, 0), Decision::Accepted);
    assert_eq!(a.sel, 1);
    key(&mut a, 'h');
    assert_eq!(dec(&a, 1), Decision::Investigate);
    assert_eq!(a.sel, 2);
    key(&mut a, 'i');
    assert_eq!(dec(&a, 2), Decision::Investigate);
    assert_eq!(a.sel, 3);

    // Advancing skips units already decided.
    a.review.units[a.order[4]].decision = Decision::Accepted;
    key(&mut a, 'l');
    assert_eq!(dec(&a, 3), Decision::Accepted);
    assert_eq!(a.sel, 5);

    // No pending unit after: selection stays.
    key(&mut a, 'l');
    assert_eq!(dec(&a, 5), Decision::Accepted);
    assert_eq!(a.sel, 5);
}

#[test]
fn deciding_never_opens_a_confirmation() {
    let mut a = app(9);
    for c in ['l', 'h', 'i', 'l', 'l', 'l', 'l', 'l'] {
        key(&mut a, c);
        assert_eq!(a.overlay, Overlay::None);
    }
    assert_eq!(a.swipe.history.len(), 8);
}

#[test]
fn u_undoes_one_decision_at_a_time() {
    let mut a = app(6);
    a.sel = 0;
    key(&mut a, 'l'); // u0
    key(&mut a, 'h'); // u1
    key(&mut a, 'h'); // u2
    assert_eq!(dec(&a, 2), Decision::Investigate);

    key(&mut a, 'u');
    assert_eq!(
        decisions(&a)[..3],
        [Decision::Accepted, Decision::Investigate, Decision::Pending],
        "only the latest decision is undone"
    );
    key(&mut a, 'u');
    key(&mut a, 'u');
    assert!(decisions(&a).iter().all(|d| *d == Decision::Pending));
    // Nothing left: a further undo is harmless.
    key(&mut a, 'u');
    assert!(decisions(&a).iter().all(|d| *d == Decision::Pending));
}

#[test]
fn undo_restores_the_previous_decision_not_pending() {
    let mut a = app(3);
    a.sel = 1;
    key(&mut a, 'l');
    a.sel = 1;
    key(&mut a, 'h');
    assert_eq!(dec(&a, 1), Decision::Investigate);
    key(&mut a, 'u');
    assert_eq!(dec(&a, 1), Decision::Accepted);
}

#[test]
fn bracket_keys_skip_decided_units() {
    let mut a = app(7);
    for i in [1, 2, 4] {
        a.review.units[a.order[i]].decision = Decision::Accepted;
    }
    a.sel = 0;
    key(&mut a, ']');
    assert_eq!(a.sel, 3);
    key(&mut a, ']');
    assert_eq!(a.sel, 5);
    key(&mut a, '[');
    assert_eq!(a.sel, 3);
    key(&mut a, '[');
    assert_eq!(a.sel, 0);
    // No pending unit before: stays put.
    key(&mut a, '[');
    assert_eq!(a.sel, 0);
    // None after the last pending one.
    a.sel = 6;
    key(&mut a, ']');
    assert_eq!(a.sel, 6);
}

fn ctrl(a: &mut App, c: char) {
    a.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::CONTROL,
    )));
}

#[test]
fn ctrl_d_and_ctrl_u_jump_between_files() {
    let mut a = app(8); // f0: 0-2, f1: 3-5, f2: 6-7
    a.sel = 1;
    ctrl(&mut a, 'd');
    assert_eq!(a.sel, 3);
    key(&mut a, 'j');
    ctrl(&mut a, 'd');
    assert_eq!(a.sel, 6);
    ctrl(&mut a, 'd');
    assert_eq!(a.sel, 6, "no next file");
    ctrl(&mut a, 'u');
    assert_eq!(a.sel, 3, "first unit of the previous file");
    a.sel = 5;
    ctrl(&mut a, 'u');
    assert_eq!(a.sel, 0, "from mid-file, the previous file's first unit");
    ctrl(&mut a, 'u');
    assert_eq!(a.sel, 0);
    // The old brace keys do nothing.
    key(&mut a, '}');
    assert_eq!(a.sel, 0);
}

#[test]
fn esc_is_back_from_the_list() {
    // Small review: Back from the list has nowhere to go and doesn't quit.
    let mut a = app(5);
    let effects = a.on_event(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(effects.is_empty());
    assert_eq!(a.screen, Screen::Slim);

    // Large review: there is no file overview to go back to either.
    let mut a = app(5);
    a.large = true;
    code(&mut a, KeyCode::Esc);
    assert_eq!(a.screen, Screen::Slim);
}

#[test]
fn k_still_moves_up_and_j_down_in_the_list() {
    let mut a = app(4);
    key(&mut a, 'j');
    key(&mut a, 'j');
    assert_eq!(a.sel, 2);
    key(&mut a, 'k');
    assert_eq!(a.sel, 1);
    code(&mut a, KeyCode::Up);
    assert_eq!(a.sel, 0);
    assert!(
        decisions(&a).iter().all(|d| *d == Decision::Pending),
        "k only moves; it decides nothing on the list"
    );
}

#[test]
fn units_that_leave_the_list_are_not_decided_after_a_rebuild() {
    let mut a = app(5);
    a.sel = 1;
    // u1 stops being a behavior change (e.g. after a rerun) and leaves `order`.
    a.review.units[1].status = UnitStatus::NoBehaviorChange;
    a.rebuild_order();
    assert_eq!(a.order.len(), 4);
    assert_eq!(
        a.review.units[a.order[a.sel]].id, "u2",
        "selection stays in range"
    );
    key(&mut a, 'l');
    assert_eq!(a.review.units[1].decision, Decision::Pending);
    assert_eq!(a.review.units[2].decision, Decision::Accepted);
}

#[test]
fn undo_history_is_shared_between_the_list_and_swipe() {
    let mut a = app(6);
    key(&mut a, 'l'); // list: u0 accepted
    assert_eq!(a.swipe.history.len(), 1);

    key(&mut a, 's');
    assert_eq!(a.screen, Screen::Swipe);
    // Entering swipe starts fresh history, on the first pending card.
    assert_ne!(a.swipe_current().map(String::as_str), Some("u0"));
    code(&mut a, KeyCode::Right);
    let swiped = a
        .review
        .units
        .iter()
        .filter(|u| u.decision == Decision::Accepted)
        .count();
    assert_eq!(swiped, 2);
    key(&mut a, 'u');
    assert_eq!(
        a.review
            .units
            .iter()
            .filter(|u| u.decision == Decision::Accepted)
            .count(),
        1
    );

    // Back on the list, decisions made there are undoable and swipe sees them.
    code(&mut a, KeyCode::Esc);
    assert_eq!(a.screen, Screen::Slim);
    key(&mut a, 'h');
    let decided = a
        .review
        .units
        .iter()
        .filter(|u| u.decision == Decision::Investigate)
        .count();
    assert_eq!(decided, 1);
    key(&mut a, 'u');
    assert_eq!(
        a.review
            .units
            .iter()
            .filter(|u| u.decision == Decision::Investigate)
            .count(),
        0
    );
}

#[test]
fn list_decisions_are_undone_one_step_at_a_time_from_swipe_too() {
    let mut a = app(6);
    for _ in 0..3 {
        key(&mut a, 'l');
    }
    assert_eq!(a.swipe.history.len(), 3);
    a.screen = Screen::Swipe;
    key(&mut a, 'u');
    assert_eq!(a.swipe.history.len(), 2);
    assert_eq!(
        a.review
            .units
            .iter()
            .filter(|u| u.decision == Decision::Accepted)
            .count(),
        2
    );
}
