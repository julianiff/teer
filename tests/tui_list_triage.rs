//! Triage in place in the queue (the v1 list shares it): decisions, jumps,
//! undo.
//! Driven through key events on a synthetic review (no repo, no agent).

mod common;

use common::v2::Driver;
use teer::review::model::{Decision, UnitStatus};
use teer::tui::app::{Overlay, Screen};

/// `n` pending units, three per file (f0.rs, f1.rs, ...), listed in order.
fn app(n: usize) -> Driver {
    let d = Driver::new(n);
    assert_eq!(d.app.order.len(), n, "every unit is listed");
    d
}

fn count(d: &Driver, want: Decision) -> usize {
    d.app
        .review
        .units
        .iter()
        .filter(|u| u.decision == want)
        .count()
}

#[test]
fn unbound_keys_do_nothing_in_the_list() {
    let mut a = app(5);
    a.select(1);
    for keys in ["v", "e", "w", "b", "}"] {
        assert!(a.keys(keys).is_empty(), "{keys}");
        assert_eq!(a.sel(), 1, "{keys:?} doesn't move");
        assert_eq!(*a.overlay(), Overlay::None);
        assert!(a.decisions().iter().all(|d| *d == Decision::Pending));
        assert_eq!(a.undo_steps(), 0);
    }
}

#[test]
fn a_and_f_decide_the_selected_unit_and_advance_to_next_pending() {
    let mut a = app(6);
    a.keys("a");
    assert_eq!(a.decision(0), Decision::Accepted);
    assert_eq!(a.sel(), 1);
    // `f` opens the note line on the flagged unit; esc keeps the flag.
    a.keys("f<esc>");
    assert_eq!(a.decision(1), Decision::Investigate);
    assert_eq!(a.sel(), 2);
    a.keys("f<esc>");
    assert_eq!(a.decision(2), Decision::Investigate);
    assert_eq!(a.sel(), 3);

    // Advancing skips units already decided.
    a.app.review.units[a.app.order[4]].decision = Decision::Accepted;
    a.keys("a");
    assert_eq!(a.decision(3), Decision::Accepted);
    assert_eq!(a.sel(), 5);

    // No pending unit after: selection stays.
    a.keys("a");
    assert_eq!(a.decision(5), Decision::Accepted);
    assert_eq!(a.sel(), 5);
}

#[test]
fn deciding_never_opens_a_confirmation() {
    let mut a = app(9);
    for keys in ["a", "f<esc>", "f<esc>", "a", "a", "a", "a", "a"] {
        a.keys(keys);
        assert_eq!(*a.overlay(), Overlay::None, "{keys}");
    }
    assert_eq!(a.undo_steps(), 8);
}

#[test]
fn u_undoes_one_decision_at_a_time() {
    let mut a = app(6);
    a.keys("a"); // u0
    a.keys("f<esc>"); // u1
    a.keys("f<esc>"); // u2
    assert_eq!(a.decision(2), Decision::Investigate);

    a.keys("u");
    assert_eq!(
        a.decisions()[..3],
        [Decision::Accepted, Decision::Investigate, Decision::Pending],
        "only the latest decision is undone"
    );
    a.keys("uu");
    assert!(a.decisions().iter().all(|d| *d == Decision::Pending));
    // Nothing left: a further undo is harmless.
    a.keys("u");
    assert!(a.decisions().iter().all(|d| *d == Decision::Pending));
}

#[test]
fn undo_restores_the_previous_decision_not_pending() {
    let mut a = app(3);
    a.select(1);
    a.keys("a");
    a.select(1);
    a.keys("f<esc>");
    assert_eq!(a.decision(1), Decision::Investigate);
    a.keys("u");
    assert_eq!(a.decision(1), Decision::Accepted);
}

#[test]
fn n_and_shift_n_skip_decided_units() {
    let mut a = app(7);
    for i in [1, 2, 4] {
        a.app.review.units[a.app.order[i]].decision = Decision::Accepted;
    }
    a.select(0);
    a.keys("n");
    assert_eq!(a.sel(), 3);
    a.keys("n");
    assert_eq!(a.sel(), 5);
    a.keys("N");
    assert_eq!(a.sel(), 3);
    a.keys("N");
    assert_eq!(a.sel(), 0);
    // No pending unit before: stays put.
    a.keys("N");
    assert_eq!(a.sel(), 0);
    // None after the last pending one.
    a.select(6);
    a.keys("n");
    assert_eq!(a.sel(), 6);
}

#[test]
fn bracket_f_jumps_between_files() {
    let mut a = app(8); // f0: 0-2, f1: 3-5, f2: 6-7
    a.select(1);
    a.keys("]f");
    assert_eq!(a.sel(), 3);
    a.keys("j");
    a.keys("]f");
    assert_eq!(a.sel(), 6);
    a.keys("]f");
    assert_eq!(a.sel(), 6, "no next file");
    a.keys("[f");
    assert_eq!(a.sel(), 3, "first unit of the previous file");
    a.select(5);
    a.keys("[f");
    assert_eq!(a.sel(), 0, "from mid-file, the previous file's first unit");
    a.keys("[f");
    assert_eq!(a.sel(), 0);
    // ctrl+d and ctrl+u scroll now; they don't move the cursor.
    a.keys("<c-d>");
    assert_eq!(a.sel(), 0);
    a.select(4);
    a.keys("<c-u>");
    assert_eq!(a.sel(), 4);
}

#[test]
fn esc_is_back_from_the_list() {
    // Small review: Back from the list has nowhere to go and doesn't quit.
    let mut a = app(5);
    assert!(a.keys("<esc>").is_empty());
    assert_eq!(a.screen(), Screen::Queue);

    // Large review: there is no file overview to go back to either.
    let mut a = app(5);
    a.app.large = true;
    a.keys("<esc>");
    assert_eq!(a.screen(), Screen::Queue);
}

#[test]
fn k_still_moves_up_and_j_down_in_the_list() {
    let mut a = app(4);
    a.keys("jj");
    assert_eq!(a.sel(), 2);
    a.keys("k");
    assert_eq!(a.sel(), 1);
    a.keys("<up>");
    assert_eq!(a.sel(), 0);
    assert!(
        a.decisions().iter().all(|d| *d == Decision::Pending),
        "k only moves; it decides nothing on the list"
    );
}

#[test]
fn units_that_leave_the_list_are_not_decided_after_a_rebuild() {
    let mut a = app(5);
    a.select(1);
    // u1 stops being a behavior change (e.g. after a rerun) and leaves `order`.
    a.app.review.units[1].status = UnitStatus::NoBehaviorChange;
    a.app.rebuild_order();
    assert_eq!(a.app.order.len(), 4);
    assert_eq!(
        a.selected().as_deref(),
        Some("u2"),
        "selection stays in range"
    );
    a.keys("a");
    // Accepted for you, not by `a`.
    assert!(a.app.review.units[1].auto_accepted());
    assert_eq!(a.app.review.units[2].decision, Decision::Accepted);
    assert!(!a.app.review.units[2].auto_accepted());
}

#[test]
fn undo_history_is_shared_between_the_list_and_swipe() {
    let mut a = app(6);
    // Swipe is on the v1 screens (`--swipe`).
    a.app.open_v1_list();
    a.keys("a"); // list: u0 accepted
    assert_eq!(a.undo_steps(), 1);

    a.keys("s");
    assert_eq!(a.screen(), Screen::Swipe);
    // Swipe starts on the first card not accepted.
    assert_ne!(a.app.swipe_current().map(String::as_str), Some("u0"));
    a.keys("a");
    assert_eq!(count(&a, Decision::Accepted), 2);
    assert_eq!(a.undo_steps(), 2, "the review's history holds both");
    a.keys("u");
    assert_eq!(count(&a, Decision::Accepted), 1);

    // Back on the list, decisions made there are undoable and swipe sees them.
    a.keys("<esc>");
    assert_eq!(a.screen(), Screen::Slim);
    a.keys("f<esc>");
    assert_eq!(count(&a, Decision::Investigate), 1);
    a.keys("u");
    assert_eq!(count(&a, Decision::Investigate), 0);
    // The list's first accept is still in the history.
    a.keys("u");
    assert_eq!(count(&a, Decision::Accepted), 0);
}

#[test]
fn list_decisions_are_undone_one_step_at_a_time_from_swipe_too() {
    let mut a = app(6);
    a.keys("aaa");
    assert_eq!(a.undo_steps(), 3);
    a.app.screen = Screen::Swipe;
    a.keys("u");
    assert_eq!(a.undo_steps(), 2);
    assert_eq!(count(&a, Decision::Accepted), 2);
}
