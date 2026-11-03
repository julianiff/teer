//! The queue's behavior (v2 Phase 4), typed in Neovim notation through
//! `App::on_key` on a synthetic review: where it opens, auto-advance, the
//! note line of `f`, `F` and a selection, `A`/`F` on the file's pending
//! units, counts, undo and redo of file actions with their notes, and
//! resuming where the review was left. Drawing is `v2_queue`'s.

use crate::common;

use std::path::PathBuf;

use common::v2::{Driver, review, unit, units};
use teer::keymap::Keymap;
use teer::review::model::{Change, Decision, Marker, Prep, Review, Risk, Unit, UnitStatus};
use teer::review::prepare::PrepareEvent;
use teer::review::store;
use teer::tui::app::{AppEvent, Effect, Grouping, Overlay, Screen};

use Decision::{Accepted as A, Investigate as F, Pending as P};

/// Set the decisions of the units at list positions 0, 1, ...
fn decide(d: &mut Driver, each: &[Decision]) {
    for (p, x) in each.iter().enumerate() {
        let i = d.app.order[p];
        d.app.review.units[i].decision = *x;
    }
}

/// Put `note` on unit `id`, as written on its code.
fn note_on(d: &mut Driver, id: &str, note: &str) {
    let u = d.app.review.unit_mut(id).unwrap();
    u.dive.note = note.into();
    u.dive.note_on = Some(u.fingerprint.clone());
}

/// The note line on show: its unit and its text.
fn note_line(d: &Driver) -> Option<(String, String)> {
    match d.overlay() {
        Overlay::Note { id, text, .. } => Some((id.clone(), text.clone())),
        _ => None,
    }
}

/// A review of `n` units (see `common::v2::units`) saved under its own
/// repo path, so tests don't share a file.
fn saved(n: usize, repo: &str) -> Review {
    let mut r = review(units(n));
    r.repo_root = PathBuf::from(repo);
    r
}

/// What `teer` loads for `r` on the next start.
fn load(r: &Review) -> Review {
    store::load(&r.repo_root, None, &r.base, &r.head, &r.head_commit).expect("saved")
}

// ---- Where it opens -----------------------------------------------------------

#[test]
fn teer_opens_on_the_queue_by_level() {
    let mut d = Driver::new(6);
    assert_eq!(d.screen(), Screen::Queue);
    assert_eq!(d.app.grouping, Grouping::Unit);
    d.keys("zf");
    assert_eq!(
        d.app.grouping,
        Grouping::File,
        "zf switches to the file boxes"
    );
    d.keys("zf");
    assert_eq!(d.app.grouping, Grouping::Unit);
}

/// Unit `u{i}` in `file`, removed while something still calls it: ▲.
fn close_look(i: usize, file: &str) -> Unit {
    let mut u = unit(i, file);
    u.status = UnitStatus::Removed;
    u.callers = vec!["caller".into()];
    u
}

#[test]
fn it_starts_on_the_first_pending_close_look() {
    let mut r = review(vec![
        unit(0, "a.rs"),
        close_look(1, "b.rs"),
        close_look(2, "c.rs"),
    ]);
    r.units[1].decision = A;
    let d = Driver::with(r, Keymap::default());
    assert_eq!(d.selected().as_deref(), Some("u2"));
    assert_eq!(d.sel(), 1, "▲ first: u1, u2, then u0");
}

#[test]
fn l_opens_the_unit_view_on_any_unit() {
    let mut d = Driver::new(6);
    decide(&mut d, &[A, F]);
    for (pos, key) in [(0, "l"), (1, "<cr>"), (2, "l")] {
        d.app.screen = Screen::Queue;
        d.select(pos);
        d.keys(key);
        assert_eq!(d.screen(), Screen::Unit, "{key} on row {pos}");
    }
    // `s` was the v1 list's way into swipe; the queue has none.
    d.app.screen = Screen::Queue;
    d.keys("s");
    assert_eq!(d.screen(), Screen::Queue);
}

// ---- Auto-advance ---------------------------------------------------------------

#[test]
fn auto_advance_through_mixed_states() {
    let mut d = Driver::new(9);
    decide(&mut d, &[P, A, F, P, A, F, P, A, A]);
    d.select(0);
    d.keys("a");
    assert_eq!(d.sel(), 3, "past the accepted and the flagged one");
    d.keys("f<esc>");
    assert_eq!(d.sel(), 6);
    assert_eq!(d.decision(3), F);
    // The last pending unit: nothing left, the cursor stays.
    d.keys("a");
    assert_eq!(d.decisions(), vec![A, A, F, F, A, F, A, A, A]);
    assert_eq!(d.sel(), 6);
    assert_eq!(
        *d.overlay(),
        Overlay::None,
        "no popup: the bottom bar says it"
    );
    assert_eq!(d.app.decision_counts(), (6, 3, 0));
}

#[test]
fn past_the_last_pending_unit_it_goes_on_from_the_top() {
    let mut d = Driver::new(6);
    decide(&mut d, &[A, P, A, A, P, A]);
    d.select(4);
    d.keys("a");
    assert_eq!(d.sel(), 1, "the one pending unit left is above");
    d.keys("a");
    assert_eq!(d.sel(), 1, "none left: it stays");
    assert_eq!(d.positions(P), Vec::<usize>::new());
}

#[test]
fn counts_decide_this_unit_and_the_next_pending_ones() {
    let mut d = Driver::new(9);
    decide(&mut d, &[P, A, F, P, P, P]);
    d.keys("3a");
    assert_eq!(d.decisions()[..6], [A, A, F, A, A, P]);
    assert_eq!(d.sel(), 5, "on to the next pending unit");
    assert_eq!(d.undo_steps(), 1, "one step");
    assert_eq!(d.status(), "Accepted 3 units.");
    // A count on `f` opens no note line (as in Phase 1).
    d.keys("2f");
    assert_eq!(d.decisions()[5..7], [F, F]);
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.sel(), 7);
    assert_eq!(d.app.decision_counts(), (4, 3, 2));
}

// ---- The note line --------------------------------------------------------------

#[test]
fn f_flags_without_asking_for_a_note() {
    let mut d = Driver::new(6);
    let effects = d.keys("f");
    assert!(effects.contains(&Effect::Save), "{effects:?}");
    assert_eq!(d.decision_of("u0"), F);
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.note("u0"), "");
    assert_eq!(d.sel(), 1, "on to the next pending unit");
}

#[test]
fn i_saves_a_note_on_the_code_as_it_is_and_keeps_the_decision() {
    let mut d = Driver::new(6);
    d.keys("i");
    assert_eq!(note_line(&d), Some(("u0".into(), String::new())));
    let effects = d.keys("needs a test<cr>");
    assert!(effects.contains(&Effect::Save), "{effects:?}");
    assert_eq!(d.note("u0"), "needs a test");
    let u0 = d.app.review.unit("u0").unwrap();
    assert_eq!(
        u0.dive.note_on.as_deref(),
        Some("fp0"),
        "written on this code"
    );
    assert_eq!(d.decision_of("u0"), P);
    assert_eq!(*d.overlay(), Overlay::None);
}

#[test]
fn i_on_a_noted_unit_prefills_the_line_and_empty_clears_it() {
    let mut d = Driver::new(3);
    note_on(&mut d, "u0", "old");
    d.keys("i");
    assert_eq!(note_line(&d), Some(("u0".into(), "old".into())));
    d.keys(" and new<cr>");
    assert_eq!(d.note("u0"), "old and new");
    d.keys("i<c-u><cr>");
    assert_eq!(d.note("u0"), "", "an empty note is no note");
    assert_eq!(d.app.review.unit("u0").unwrap().dive.note_on, None);
}

#[test]
fn esc_in_the_note_line_drops_the_edit() {
    let mut d = Driver::new(3);
    note_on(&mut d, "u0", "old");
    d.keys("i<bs><bs>ther");
    assert_eq!(note_line(&d).map(|l| l.1).as_deref(), Some("other"));
    let effects = d.keys("<esc>");
    assert!(!effects.contains(&Effect::Save), "{effects:?}");
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.note("u0"), "old");
}

// ---- A and F --------------------------------------------------------------------

#[test]
fn upper_a_accepts_the_files_pending_units_at_once() {
    // f0: u0 u1 u2, f1: u3 u4 u5.
    let mut d = Driver::new(6);
    decide(&mut d, &[P, F, P, P]);
    note_on(&mut d, "u1", "keep me");
    d.select(0);
    let effects = d.keys("A");
    assert!(effects.contains(&Effect::Save), "{effects:?}");
    assert_eq!(*d.overlay(), Overlay::None, "no confirm");
    assert_eq!(d.decisions(), vec![A, F, A, P, P, P], "the flag stays");
    assert_eq!(d.note("u1"), "keep me");
    assert_eq!(d.status(), "Accepted 2 units.");
    assert_eq!(d.undo_steps(), 1);
    assert_eq!(d.sel(), 3, "on to the next file's pending unit");
    // Nothing pending in the file: nothing happens.
    d.select(1);
    d.keys("A");
    assert_eq!(d.status(), "Nothing pending in this file.");
    assert_eq!(d.undo_steps(), 1);
    assert_eq!(d.sel(), 1);
}

#[test]
fn upper_f_flags_the_files_pending_units_without_asking_for_a_note() {
    let mut d = Driver::new(6);
    decide(&mut d, &[P, P, P, P, A, P]);
    d.select(3);
    d.keys("F");
    assert_eq!(d.decisions(), vec![P, P, P, F, A, F], "u4 stays accepted");
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.sel(), 0, "on to the next pending unit, from the top");
    assert_eq!(d.undo_steps(), 1);
}

// ---- Visual mode ----------------------------------------------------------------

#[test]
fn visual_f_flags_every_selected_unit_without_asking_for_a_note() {
    let mut d = Driver::new(6);
    decide(&mut d, &[P, A, P]);
    d.keys("Vjjf");
    assert_eq!(d.decisions()[..3], [F, F, F], "the accepted one too");
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.sel(), 3);
    assert_eq!(d.app.visual, None);
}

/// The list moves under a selection (a description comes in and raises a
/// unit to ▲, listed first): the selection keeps its units, and `a`
/// decides those, not the rows they were on.
#[test]
fn a_selection_keeps_its_units_when_a_description_moves_the_list() {
    let mut all = units(6);
    for u in &mut all {
        u.prep = Prep::Pending;
    }
    let mut d = Driver::of(all);
    d.select(0);
    d.keys("Vj");
    assert_eq!((d.id_at(0), d.id_at(1)), ("u0".into(), "u1".into()));
    d.app.on_app_event(AppEvent::Prep(PrepareEvent::Ready {
        id: "u5".into(),
        fingerprint: "fp5".into(),
        changes: vec![Change {
            marker: Marker::Changed,
            statement: "It charges twice.".into(),
            evidence: Vec::new(),
            risk: Risk::High,
        }],
        summary: None,
        cost: None,
        unmentioned: None,
        dropped: Vec::new(),
        checking: false,
    }));
    assert_eq!(d.id_at(0), "u5", "▲ first");
    assert_eq!(d.selected().as_deref(), Some("u1"));
    assert_eq!(d.app.visual_rows(), Some(1..=2));
    let bar = d.bar(140, 30);
    assert!(bar.contains("2 units"), "{bar}");
    d.keys("a");
    assert_eq!(d.decision_of("u0"), A);
    assert_eq!(d.decision_of("u1"), A);
    assert_eq!(d.decision_of("u5"), P, "never selected");
    assert_eq!(d.app.review.history.undo.len(), 1);
}

/// A unit the selection started on that is no longer listed (accepted for
/// you, hidden) ends the selection.
#[test]
fn a_selection_whose_first_unit_leaves_the_list_ends() {
    let mut d = Driver::new(6);
    d.select(1);
    d.keys("Vj");
    d.app.review.unit_mut("u1").unwrap().status = UnitStatus::NoBehaviorChange;
    d.app.rebuild_order();
    let listed = |d: &Driver, id: &str| d.app.order.iter().any(|&i| d.app.review.units[i].id == id);
    assert!(!listed(&d, "u1"), "accepted for you, hidden");
    assert_eq!(d.app.visual, None);
    d.keys("a");
    assert_eq!(
        d.decision_of("u2"),
        A,
        "a decides the unit under the cursor"
    );
    assert_eq!(d.decision_of("u3"), P);
}

// ---- Undo and redo --------------------------------------------------------------

#[test]
fn undo_and_redo_take_file_actions_back_and_forth() {
    let mut d = Driver::new(6);
    decide(&mut d, &[P, A]);
    d.keys("F");
    assert_eq!(d.decisions()[..3], [F, A, F]);
    d.keys("A");
    assert_eq!(d.decisions(), vec![F, A, F, A, A, A]);
    assert_eq!(d.undo_steps(), 2);
    // The accept of f1, then the flags of f0.
    d.keys("u");
    assert_eq!(d.decisions(), vec![F, A, F, P, P, P]);
    let effects = d.keys("u");
    assert!(effects.contains(&Effect::Save), "{effects:?}");
    assert_eq!(d.decisions(), vec![P, A, P, P, P, P]);
    assert_eq!(d.sel(), 0, "on the step's first unit");
    d.keys("<c-r>");
    assert_eq!(d.decisions()[..3], [F, A, F]);
    d.keys("<c-r>");
    assert_eq!(d.decisions(), vec![F, A, F, A, A, A]);
}

#[test]
fn undo_takes_back_a_selections_flag_and_leaves_a_note_typed_with_i() {
    let mut d = Driver::new(6);
    d.keys("Vjf");
    d.select(4);
    d.keys("iown<cr>");
    assert_eq!(d.undo_steps(), 1, "a note alone is no step");
    d.keys("u");
    assert_eq!(d.decisions()[..2], [P, P]);
    assert_eq!(d.note("u4"), "own");
}

// ---- The old Flagged screen's resolution ----------------------------------------

#[test]
fn the_queue_has_no_needs_fix() {
    let mut d = Driver::new(3);
    d.keys("ix<cr>f");
    d.select(0);
    d.keys("xo");
    let saved = serde_json::to_string(&d.app.review).unwrap();
    assert!(!saved.contains("\"resolution\""), "{saved}");
    let u0 = d.app.review.unit("u0").unwrap();
    assert_eq!(u0.decision, F);
    assert_eq!(u0.dive.note, "x", "a flag with a note is what goes out");
}

// ---- Persistence ----------------------------------------------------------------

#[test]
fn every_move_saves_the_cursor() {
    let mut d = Driver::new(6);
    // A move saves only the cursor (the review can be tens of megabytes).
    let effects = d.keys("j");
    assert_eq!(effects, vec![Effect::SaveCursor]);
    assert_eq!(d.app.review.cursor.as_deref(), Some("u1"));
    let effects = d.keys("G");
    assert!(effects.contains(&Effect::SaveCursor), "{effects:?}");
    assert_eq!(d.app.review.cursor.as_deref(), Some("u5"));
    // Nowhere to go: nothing to save.
    assert!(d.keys("j").is_empty());
    let effects = d.keys("a");
    // A decision saves the review, which holds the cursor too.
    assert_eq!(
        effects.iter().filter(|e| **e == Effect::Save).count(),
        1,
        "one save per key: {effects:?}"
    );
    assert!(!effects.contains(&Effect::SaveCursor), "{effects:?}");
}

#[test]
fn resume_after_quit() {
    let r = saved(9, "/v2/queue-act/resume-after-quit");
    let mut d = Driver::with(r, Keymap::default());
    d.keys("ainote<cr>f");
    d.keys("Vja");
    d.keys("k");
    let at = d.selected();
    assert_eq!(at.as_deref(), Some("u3"));
    assert_eq!(d.keys("q"), vec![Effect::Save, Effect::Quit]);
    store::save(&d.app.review).unwrap();

    let mut back = Driver::with(load(&d.app.review), Keymap::default());
    assert_eq!(back.selected(), at, "on the same unit");
    assert_eq!(back.decisions(), d.decisions());
    assert_eq!(back.note("u1"), "note");
    back.keys("uu");
    assert_eq!(back.decisions()[..4], [A, P, P, P], "undo goes on");
    assert_eq!(back.note("u1"), "note", "a note is no step: it stays");
}

#[test]
fn resume_after_a_crash() {
    // No `q`: what the last key saved is all there is.
    let r = saved(9, "/v2/queue-act/resume-after-a-crash");
    let mut d = Driver::with(r, Keymap::default());
    // The runtime saves when a key asks for it.
    for keys in ["a", "4j"] {
        let effects = d.keys(keys);
        if effects.contains(&Effect::Save) {
            store::save(&d.app.review).unwrap();
        } else if effects.contains(&Effect::SaveCursor) {
            store::save_cursor(&d.app.review).unwrap();
        }
    }
    let back = Driver::with(load(&d.app.review), Keymap::default());
    assert_eq!(back.selected().as_deref(), Some("u5"));
    assert_eq!(back.decision_of("u0"), A);
}

#[test]
fn a_cursor_on_a_unit_that_is_gone_resumes_on_the_first_pending_close_look() {
    let mut r = review(vec![
        unit(0, "a.rs"),
        close_look(1, "b.rs"),
        close_look(2, "b.rs"),
    ]);
    r.units[1].decision = A;
    r.cursor = Some("gone".into());
    let d = Driver::with(r, Keymap::default());
    assert_eq!(d.selected().as_deref(), Some("u2"));
    assert_eq!(d.app.review.cursor.as_deref(), Some("u2"));
}
