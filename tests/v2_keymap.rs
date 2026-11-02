//! v2's default keys on today's screens (Phase 1): one test per binding of
//! the default table, typed in Neovim notation through `App::on_key` on a
//! synthetic review, and the keymap's modes around them: prefixes and their
//! timeout, counts, the command line, `.`, visual mode, undo and redo saved
//! with the review, and rebinding. The keymap module's own tests cover the
//! notation and the four rules; `no_key_literals` the "no `KeyCode::`" rule.

mod common;

use std::collections::HashMap;
use std::path::PathBuf;

use common::v2::{Driver, review, unit, units};
use teer::keymap::{Action, KeySpec, Keymap, Trigger};
use teer::review::model::{
    Change, Decision, DiveKind, DiveResult, Marker, Range, Resolution, Risk, Visual,
};
use teer::tui::app::{Effect, Grouping, JumpKind, Overlay, Pane, Screen};
use teer::tui::unit::Panel;

use Decision::{Accepted as A, Investigate as F, Pending as P};

/// `d` with the units at list positions `at` flagged.
fn flag(d: &mut Driver, at: &[usize]) {
    for &p in at {
        let i = d.app.order[p];
        d.app.review.units[i].decision = F;
    }
}

/// `d` with the units at list positions `at` accepted.
fn accept(d: &mut Driver, at: &[usize]) {
    for &p in at {
        let i = d.app.order[p];
        d.app.review.units[i].decision = A;
    }
}

/// `n` pending units on the v1 list (`--swipe`, until Phase 8): swipe and
/// the Flagged screen are entered from there.
fn v1(n: usize) -> Driver {
    let mut d = Driver::new(n);
    d.app.open_v1_list();
    d
}

/// `u` with one change statement, so the second model has something to check.
fn described(mut u: teer::review::model::Unit) -> teer::review::model::Unit {
    u.changes = vec![Change {
        marker: Marker::Changed,
        statement: "Changes how orders are charged.".into(),
        evidence: Vec::new(),
        risk: Risk::Low,
    }];
    u
}

/// Six units, u1 u3 u4 flagged, open on the Flagged screen at u1. Web
/// search is off so questions go out at once.
fn on_flagged() -> Driver {
    let mut d = Driver::of(units(6).into_iter().map(described).collect());
    // The Flagged screen is v1's (`--swipe`), entered from the v1 list.
    d.app.open_v1_list();
    d.app.cfg.dive_web = false;
    flag(&mut d, &[1, 3, 4]);
    d.select(1);
    d.keys("l");
    assert_eq!(d.screen(), Screen::Flagged);
    assert_eq!(d.app.flagged_unit_id().as_deref(), Some("u1"));
    d
}

fn has_dive(effects: &[Effect], id: &str, want: DiveKind) -> bool {
    effects
        .iter()
        .any(|e| matches!(e, Effect::Dive { unit_id, kind, .. } if unit_id == id && *kind == want))
}

// ---- Moves --------------------------------------------------------------------

#[test]
fn j_and_down_move_down_and_a_count_moves_that_many_rows() {
    let mut d = Driver::new(9);
    d.keys("j");
    assert_eq!(d.sel(), 1);
    d.keys("<down>");
    assert_eq!(d.sel(), 2);
    d.keys("3j");
    assert_eq!(d.sel(), 5);
    d.keys("10j");
    assert_eq!(d.sel(), 8, "no further than the last row");
    assert!(
        d.decisions().iter().all(|x| *x == P),
        "moves decide nothing"
    );
}

#[test]
fn k_and_up_move_up_with_a_count() {
    let mut d = Driver::new(9);
    d.select(8);
    d.keys("k");
    assert_eq!(d.sel(), 7);
    d.keys("<up>");
    assert_eq!(d.sel(), 6);
    d.keys("4k");
    assert_eq!(d.sel(), 2);
    d.keys("9k");
    assert_eq!(d.sel(), 0);
}

#[test]
fn gg_and_home_go_to_the_first_row_and_shift_g_and_end_to_the_last() {
    let mut d = Driver::new(9);
    d.keys("G");
    assert_eq!(d.sel(), 8);
    d.keys("gg");
    assert_eq!(d.sel(), 0);
    d.keys("<end>");
    assert_eq!(d.sel(), 8);
    d.keys("<home>");
    assert_eq!(d.sel(), 0);
}

#[test]
fn n_and_shift_n_jump_between_pending_units_with_a_count() {
    let mut d = Driver::new(9);
    accept(&mut d, &[1, 2, 4]);
    d.keys("n");
    assert_eq!(d.sel(), 3);
    d.keys("n");
    assert_eq!(d.sel(), 5);
    d.keys("N");
    assert_eq!(d.sel(), 3);
    d.keys("N");
    assert_eq!(d.sel(), 0);
    d.keys("N");
    assert_eq!(d.sel(), 0, "nothing pending before the first");
    d.keys("2n");
    assert_eq!(d.sel(), 5, "a count jumps that many times");
    d.keys("2N");
    assert_eq!(d.sel(), 0);
    assert_eq!(d.positions(A), vec![1, 2, 4], "jumps decide nothing");
}

#[test]
fn bracket_f_jumps_between_files_with_a_count() {
    // f0: rows 0-2, f1: 3-5, f2: 6-8.
    let mut d = Driver::new(9);
    d.select(1);
    d.keys("]f");
    assert_eq!(d.sel(), 3);
    d.keys("[f");
    assert_eq!(d.sel(), 0);
    d.keys("2]f");
    assert_eq!(d.sel(), 6);
    d.keys("]f");
    assert_eq!(d.sel(), 6, "no file after the last");
    d.select(5);
    d.keys("[f");
    assert_eq!(
        d.sel(),
        0,
        "from inside a file, the previous file's first unit"
    );
}

#[test]
fn bracket_c_jumps_hunks_or_says_when_they_come() {
    for keys in ["]c", "[c"] {
        let mut d = Driver::new(3);
        let effects = d.keys(keys);
        assert!(effects.is_empty(), "{keys}: {effects:?}");
        assert!(d.status().contains("unk"), "{keys}: {:?}", d.status());
        assert_eq!(d.sel(), 0, "{keys} moves no row");
        assert!(d.decisions().iter().all(|x| *x == P));
    }
}

// ---- Scrolling ----------------------------------------------------------------

/// `d`'s detail area as if drawn `rows` high with `max` lines to scroll.
fn detail(d: &mut Driver, rows: usize, max: usize) {
    d.app.detail_offset();
    d.app.detail_rows.set(rows);
    d.app.detail_max_scroll.set(max);
}

#[test]
fn ctrl_d_and_ctrl_u_scroll_the_detail_half_its_rows() {
    let mut d = Driver::new(3);
    detail(&mut d, 20, 100);
    d.keys("<c-d>");
    assert_eq!(d.app.detail_offset(), 10);
    d.keys("<c-d>");
    assert_eq!(d.app.detail_offset(), 20);
    d.keys("<c-u>");
    assert_eq!(d.app.detail_offset(), 10);
    assert_eq!(d.sel(), 0, "scrolling moves no row");
}

#[test]
fn page_down_and_page_up_scroll_a_page() {
    let mut d = Driver::new(3);
    detail(&mut d, 20, 100);
    d.keys("<pagedown>");
    assert_eq!(d.app.detail_offset(), 18);
    d.keys("<pageup>");
    assert_eq!(d.app.detail_offset(), 0);
    assert_eq!(d.sel(), 0);
}

// ---- Find -----------------------------------------------------------------------

#[test]
fn slash_opens_the_find_list() {
    let mut d = Driver::new(3);
    d.keys("/");
    assert_eq!(
        *d.overlay(),
        Overlay::Jump {
            kind: JumpKind::Find,
            query: String::new(),
            sel: 0
        }
    );
}

// ---- Decisions ------------------------------------------------------------------

#[test]
fn a_accepts_and_advances_to_the_next_pending_unit() {
    let mut d = Driver::new(6);
    accept(&mut d, &[2]);
    let effects = d.keys("a");
    assert!(effects.contains(&Effect::Save), "{effects:?}");
    assert_eq!(d.decision(0), A);
    assert_eq!(d.sel(), 1);
    d.keys("a");
    assert_eq!(d.sel(), 3, "decided units are skipped");
    assert_eq!(d.undo_steps(), 2);
    assert_eq!(*d.overlay(), Overlay::None);
}

#[test]
fn a_count_accepts_this_unit_and_the_next_pending_ones_in_one_step() {
    let mut d = Driver::new(9);
    accept(&mut d, &[2]);
    d.keys("3a");
    assert_eq!(d.positions(A), vec![0, 1, 2, 3]);
    assert_eq!(d.sel(), 4, "on the next pending unit");
    assert_eq!(d.undo_steps(), 1, "one step");
    d.keys("u");
    assert_eq!(d.positions(A), vec![2], "undone together");
}

#[test]
fn f_flags_advances_and_opens_the_note_line_on_the_flagged_unit() {
    let mut d = Driver::new(6);
    d.keys("f");
    assert_eq!(d.decision(0), F);
    assert_eq!(d.sel(), 1);
    assert!(
        matches!(d.overlay(), Overlay::Note { id, .. } if id == "u0"),
        "{:?}",
        d.overlay()
    );
    d.keys("why<cr>");
    assert_eq!(d.note("u0"), "why");
    assert_eq!(*d.overlay(), Overlay::None);
    // Esc keeps the flag, with no note.
    d.keys("f<esc>");
    assert_eq!(d.decision(1), F);
    assert_eq!(d.note("u1"), "");
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.sel(), 2);
}

#[test]
fn a_count_on_f_flags_several_without_a_note_line() {
    let mut d = Driver::new(6);
    d.keys("2f");
    assert_eq!(d.positions(F), vec![0, 1]);
    assert_eq!(
        *d.overlay(),
        Overlay::None,
        "no note line for several units"
    );
    assert_eq!(d.undo_steps(), 1);
}

#[test]
fn i_opens_the_note_line_and_decides_nothing() {
    let mut d = Driver::new(3);
    d.keys("i");
    assert!(
        matches!(d.overlay(), Overlay::Note { id, .. } if id == "u0"),
        "{:?}",
        d.overlay()
    );
    d.keys("hello<cr>");
    assert_eq!(d.note("u0"), "hello");
    assert_eq!(d.decision(0), P);
    assert_eq!(d.sel(), 0);
    assert_eq!(d.undo_steps(), 0, "a note is no decision");
}

#[test]
fn shift_a_accepts_the_files_pending_units_in_one_step() {
    // The queue acts at once (Phase 4); the v1 list asks first.
    let mut d = Driver::new(6);
    flag(&mut d, &[1]);
    d.keys("A");
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.decisions(), vec![A, F, A, P, P, P]);
    assert_eq!(d.undo_steps(), 1);
}

#[test]
fn shift_f_flags_the_files_pending_units_in_one_step() {
    let mut d = Driver::new(6);
    accept(&mut d, &[1]);
    d.keys("F");
    assert_eq!(d.decisions(), vec![F, A, F, P, P, P]);
    assert!(
        matches!(d.overlay(), Overlay::Note { id, also, .. } if id == "u0" && also == &["u2"]),
        "{:?}",
        d.overlay()
    );
    d.keys("<cr>");
    assert_eq!(d.undo_steps(), 1);
    d.keys("u");
    assert_eq!(d.decisions(), vec![P, A, P, P, P, P]);
}

// ---- Visual mode ----------------------------------------------------------------

#[test]
fn visual_mode_applies_to_selection() {
    let mut d = Driver::new(9);
    d.select(1);
    d.keys("V");
    assert_eq!(d.app.visual, Some(1));
    d.keys("jj");
    assert_eq!(d.sel(), 3);
    assert_eq!(d.app.visual, Some(1), "moves extend the selection");
    d.keys("a");
    assert_eq!(d.positions(A), vec![1, 2, 3]);
    assert_eq!(d.app.visual, None, "deciding leaves visual mode");
    assert_eq!(d.undo_steps(), 1);
}

#[test]
fn visual_f_flags_the_selection_with_one_note_line() {
    // The queue opens one line for the selection (Phase 4); the v1 list none.
    let mut d = Driver::new(6);
    d.keys("Vjf");
    assert_eq!(d.positions(F), vec![0, 1]);
    assert!(
        matches!(d.overlay(), Overlay::Note { id, also, .. } if id == "u0" && also == &["u1"]),
        "{:?}",
        d.overlay()
    );
    assert_eq!(d.app.visual, None);
    let mut d = Driver::new(6);
    d.app.open_v1_list();
    d.keys("Vjf");
    assert_eq!(d.positions(F), vec![0, 1]);
    assert_eq!(*d.overlay(), Overlay::None);
}

#[test]
fn esc_leaves_visual_mode_and_decides_nothing() {
    let mut d = Driver::new(6);
    d.keys("Vj<esc>");
    assert_eq!(d.app.visual, None);
    assert_eq!(d.screen(), Screen::Queue);
    assert!(d.decisions().iter().all(|x| *x == P));
    d.keys("Vh");
    assert_eq!(d.app.visual, None, "h is view.back too");
}

#[test]
fn another_action_leaves_visual_mode_then_runs() {
    let mut d = Driver::new(6);
    d.keys("Vj?");
    assert_eq!(d.app.visual, None);
    assert_eq!(*d.overlay(), Overlay::Help);
    assert!(d.decisions().iter().all(|x| *x == P));
}

// ---- Undo, redo, repeat ---------------------------------------------------------

#[test]
fn u_undoes_and_ctrl_r_redoes_through_the_reviews_history() {
    let mut d = Driver::new(6);
    d.keys("aa");
    assert_eq!(d.sel(), 2);
    let effects = d.keys("u");
    assert!(effects.contains(&Effect::Save), "{effects:?}");
    assert_eq!(d.decisions()[..2], [A, P]);
    assert_eq!(d.sel(), 1, "the cursor goes to the unit undone");
    assert_eq!(d.app.review.history.redo.len(), 1);
    let effects = d.keys("<c-r>");
    assert!(effects.contains(&Effect::Save), "{effects:?}");
    assert_eq!(d.decisions()[..2], [A, A]);
    d.keys("uu");
    assert!(d.decisions().iter().all(|x| *x == P));
    d.keys("u");
    assert!(
        d.decisions().iter().all(|x| *x == P),
        "nothing left to undo"
    );
    assert!(!d.status().is_empty(), "says there is nothing to undo");
}

#[test]
fn undo_redo_survives_restart() {
    let mut r = review(units(6));
    r.repo_root = PathBuf::from("/v2/undo-redo-survives-restart");
    let mut d = Driver::with(r, Keymap::default());
    d.keys("aau");
    assert_eq!(d.decisions()[..2], [A, P]);
    teer::review::store::save(&d.app.review).unwrap();

    let r = &d.app.review;
    let back = teer::review::store::load(&r.repo_root, None, &r.base, &r.head, &r.head_commit)
        .expect("saved");
    let mut d = Driver::with(back, Keymap::default());
    assert_eq!(d.decisions()[..2], [A, P]);
    d.keys("<c-r>");
    assert_eq!(d.decisions()[..2], [A, A], "redo after a restart");
    d.keys("uu");
    assert!(
        d.decisions().iter().all(|x| *x == P),
        "undo after a restart"
    );
}

#[test]
fn dot_repeats_the_last_decision_with_its_count() {
    let mut d = Driver::new(12);
    d.keys("2a");
    assert_eq!(d.positions(A), vec![0, 1]);
    d.keys(".");
    assert_eq!(d.positions(A), vec![0, 1, 2, 3]);
    // A typed count replaces the recorded one.
    d.keys("3.");
    assert_eq!(d.positions(A), vec![0, 1, 2, 3, 4, 5, 6]);
    assert_eq!(d.undo_steps(), 3);
}

#[test]
fn dot_repeats_decisions_not_notes() {
    let mut d = Driver::new(6);
    d.keys("a");
    d.keys("ix<cr>");
    assert_eq!(d.note("u1"), "x");
    d.keys(".");
    assert_eq!(d.decision(1), A, "the accept is repeated");
    assert_eq!(*d.overlay(), Overlay::None, "the note line is not");
    assert_eq!(d.note("u2"), "");
}

#[test]
fn dot_after_visual_repeats_on_as_many_units() {
    let mut d = Driver::new(9);
    d.keys("Vja");
    assert_eq!(d.positions(A), vec![0, 1]);
    // The count recorded is the units decided: two.
    d.select(4);
    d.keys(".");
    assert_eq!(d.positions(A), vec![0, 1, 4, 5]);
}

#[test]
fn dot_after_a_file_decision_decides_the_file_under_the_cursor() {
    let mut d = Driver::new(9);
    d.keys("A");
    assert_eq!(d.positions(A), vec![0, 1, 2]);
    d.select(4);
    d.keys(".");
    assert_eq!(*d.overlay(), Overlay::None, "at once, as `A`");
    assert_eq!(d.positions(A), vec![0, 1, 2, 3, 4, 5]);
}

// ---- Views ----------------------------------------------------------------------

#[test]
fn l_and_enter_open_any_unit_in_the_unit_view() {
    for (pos, keys) in [(1, "l"), (2, "<cr>")] {
        let mut d = Driver::new(6);
        flag(&mut d, &[2]);
        d.select(pos);
        d.keys(keys);
        assert_eq!(d.screen(), Screen::Unit, "{keys}");
    }
}

#[test]
fn l_and_enter_open_a_flagged_unit_on_the_flagged_screen() {
    // The v1 list (`--swipe`).
    let mut d = Driver::new(6);
    d.app.open_v1_list();
    flag(&mut d, &[2, 4]);
    d.select(4);
    d.keys("l");
    assert_eq!(d.screen(), Screen::Flagged);
    assert_eq!(d.app.flagged_unit_id().as_deref(), Some("u4"));
    d.keys("<esc>");
    assert_eq!(d.screen(), Screen::Slim);
    d.select(2);
    d.keys("<cr>");
    assert_eq!(d.screen(), Screen::Flagged);
    assert_eq!(d.app.flagged_unit_id().as_deref(), Some("u2"));
}

#[test]
fn l_on_a_unit_not_flagged_says_how_to_flag_it() {
    // The v1 list (`--swipe`); the queue opens any unit.
    let mut d = Driver::new(6);
    d.app.open_v1_list();
    flag(&mut d, &[2]);
    d.keys("l");
    assert_eq!(d.screen(), Screen::Slim);
    assert_eq!(
        d.status(),
        "Only flagged units open for now. f flags this one."
    );
}

#[test]
fn h_and_esc_do_nothing_on_the_list_outside_visual_mode() {
    let mut d = Driver::new(6);
    d.select(2);
    for keys in ["h", "<esc>"] {
        assert!(d.keys(keys).is_empty(), "{keys}");
        assert_eq!(d.screen(), Screen::Queue);
        assert_eq!(d.sel(), 2);
        assert_eq!(*d.overlay(), Overlay::None);
    }
}

#[test]
fn q_and_colon_q_save_and_quit_from_the_list() {
    let mut d = Driver::new(3);
    assert_eq!(d.keys("q"), vec![Effect::Save, Effect::Quit]);
    let mut d = Driver::new(3);
    assert_eq!(d.keys(":q<cr>"), vec![Effect::Save, Effect::Quit]);
}

#[test]
fn shift_k_opens_ask() {
    // On the queue: the unit view, with the ask panel open.
    let mut d = Driver::new(3);
    d.keys("K");
    assert_eq!(d.screen(), Screen::Unit);
    assert_eq!(d.app.unit.panel, Some(Panel::Ask));
    // On the v1 list (`--swipe`): the Ask view.
    let mut d = Driver::new(3);
    d.app.screen = Screen::Slim;
    d.keys("K");
    assert_eq!(*d.overlay(), Overlay::Ask);
}

#[test]
fn question_mark_opens_the_help() {
    let mut d = Driver::new(3);
    d.keys("?");
    assert_eq!(*d.overlay(), Overlay::Help);
}

// ---- Go to ----------------------------------------------------------------------

#[test]
fn gd_lists_what_the_unit_calls_and_gr_what_calls_it() {
    let mut us = units(3);
    us[0].callees = vec!["u2".into()];
    us[2].callers = vec!["u0".into()];
    let mut d = Driver::of(us);
    d.select_id("u0");
    d.keys("gd");
    assert!(
        matches!(d.overlay(), Overlay::Jump { kind: JumpKind::Definition { of }, .. } if of == "u0"),
        "{:?}",
        d.overlay()
    );
    d.app.overlay = Overlay::None;
    d.select_id("u2");
    d.keys("gr");
    assert!(
        matches!(d.overlay(), Overlay::Jump { kind: JumpKind::Callers { of }, .. } if of == "u2"),
        "{:?}",
        d.overlay()
    );
}

#[test]
fn gd_and_gr_with_nothing_to_list_say_so_and_open_nothing() {
    let mut d = Driver::new(3);
    d.keys("gd");
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.status(), "s0 calls nothing in this review.");
    d.keys("gr");
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.status(), "Nothing in this review calls s0.");
}

#[test]
fn gf_opens_the_editor_at_the_unit() {
    let mut us = units(3);
    us[0].new_range = Some(Range::new(12, 20));
    let mut d = Driver::of(us);
    d.select_id("u0");
    assert_eq!(
        d.keys("gf"),
        vec![Effect::Edit {
            file: PathBuf::from("f0.rs"),
            line: 12
        }]
    );
}

#[test]
fn gx_opens_the_picture_or_says_there_is_none() {
    let mut d = Driver::new(3);
    assert!(d.keys("gx").is_empty());
    assert!(!d.status().is_empty(), "says why nothing opened");
    let u = d.app.review.unit_mut("u0").unwrap();
    u.dive.results.insert(
        DiveKind::Visualize,
        DiveResult {
            visual: Some(Visual {
                claim: "Orders flow to the payment.".into(),
                nodes: Vec::new(),
                edges: Vec::new(),
                path: PathBuf::from("/v2/u0.html"),
            }),
            ..Default::default()
        },
    );
    assert_eq!(
        d.keys("gx"),
        vec![Effect::OpenVisual {
            unit_id: "u0".into()
        }]
    );
}

#[test]
fn za_says_when_nothing_is_accepted_for_you() {
    let mut d = Driver::new(3);
    assert!(d.keys("za").is_empty());
    assert_eq!(d.status(), "No units are accepted for you.");
}

// ---- Leader: tools --------------------------------------------------------------

#[test]
fn leader_tools_on_a_unit_not_flagged_say_to_flag_it_first() {
    for keys in [
        "<space>t", "<space>r", "<space>v", "<space>w", "<space>m", "<space>a",
    ] {
        // The v1 list's tiles (the unit view replaces them).
        let mut d = Driver::new(3);
        d.app.open_v1_list();
        assert!(d.keys(keys).is_empty(), "{keys}");
        assert_eq!(d.screen(), Screen::Slim, "{keys}");
        assert_eq!(d.status(), "Flag the unit (f) to use this.", "{keys}");
    }
}

#[test]
fn leader_tools_on_a_flagged_unit_open_it_and_ask_that_tile() {
    for (keys, kind) in [
        ("<space>t", DiveKind::Tests),
        ("<space>r", DiveKind::Behavior),
        ("<space>v", DiveKind::Visualize),
        ("<space>w", DiveKind::Recommend),
    ] {
        // The v1 list (`--swipe`) opens the Flagged screen.
        let mut d = Driver::new(6);
        d.app.screen = Screen::Slim;
        d.app.cfg.dive_web = false;
        flag(&mut d, &[1, 2]);
        d.select(2);
        let effects = d.keys(keys);
        assert_eq!(d.screen(), Screen::Flagged, "{keys}");
        assert_eq!(d.app.flagged_unit_id().as_deref(), Some("u2"), "{keys}");
        assert!(has_dive(&effects, "u2", kind), "{keys}: {effects:?}");
        assert_eq!(d.app.flagged.pane, Pane::Answer(kind), "{keys}");
    }
}

#[test]
fn leader_tools_on_the_queue_open_the_unit_view_with_that_panel() {
    for (keys, kind) in [
        ("<space>t", DiveKind::Tests),
        ("<space>r", DiveKind::Behavior),
        ("<space>v", DiveKind::Visualize),
    ] {
        // Any unit, flagged or not; old against new and the diagram need
        // an agent.
        let mut d = Driver::new(6);
        d.app.has_agent = true;
        d.app.review.agent_session = Some(teer::review::model::SessionRef::default());
        d.app.cfg.dive_web = false;
        d.select(2);
        let effects = d.keys(keys);
        assert_eq!(d.screen(), Screen::Unit, "{keys}");
        assert_eq!(d.app.unit.panel, Some(Panel::Tool(kind)), "{keys}");
        assert!(has_dive(&effects, "u2", kind), "{keys}: {effects:?}");
    }
}

#[test]
fn leader_m_on_a_flagged_unit_asks_the_second_model() {
    let mut d = Driver::of(units(3).into_iter().map(described).collect());
    d.app.screen = Screen::Slim;
    flag(&mut d, &[0]);
    let effects = d.keys("<space>m");
    assert_eq!(d.screen(), Screen::Flagged);
    assert_eq!(effects, vec![Effect::Second(vec!["u0".into()])]);
}

#[test]
fn leader_a_on_a_flagged_unit_wants_the_test_search_first() {
    let mut d = Driver::new(3);
    d.app.screen = Screen::Slim;
    flag(&mut d, &[0]);
    assert!(d.keys("<space>a").is_empty());
    assert_eq!(d.screen(), Screen::Flagged);
    assert_eq!(
        d.status(),
        "Nothing found yet: ␣t searches the tests first."
    );
}

#[test]
fn leader_c_opens_the_commits_or_says_there_are_none() {
    let mut d = Driver::new(3);
    d.keys("<space>c");
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(
        d.status(),
        "No commits: these changes are not committed yet."
    );
}

#[test]
fn leader_n_opens_the_notes_or_says_there_are_none() {
    let mut d = Driver::new(3);
    d.keys("<space>n");
    assert_eq!(*d.overlay(), Overlay::None);
    assert!(d.status().starts_with("No notes yet: i "), "{}", d.status());
    d.app.review.unit_mut("u1").unwrap().dive.note = "check this".into();
    d.keys("<space>n");
    assert_eq!(*d.overlay(), Overlay::Notes { sel: 0 });
}

#[test]
fn leader_s_opens_the_settings() {
    let mut d = Driver::new(3);
    assert_eq!(d.keys("<space>s"), vec![Effect::Settings]);
}

// ---- Command line ---------------------------------------------------------------

#[test]
fn colon_w_opens_the_finish_step_that_fits() {
    let mut d = Driver::new(3);
    d.keys(":w<cr>");
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.status(), "Nothing to finish yet: 3 units pending.");

    d.app.review.unit_mut("u1").unwrap().dive.note = "check this".into();
    d.keys(":w<cr>");
    assert!(
        matches!(d.overlay(), Overlay::Notes { .. }),
        "{:?}",
        d.overlay()
    );

    let mut d = Driver::new(3);
    accept(&mut d, &[0, 1, 2]);
    d.keys(":w<cr>");
    assert_eq!(*d.overlay(), Overlay::AllDone);
}

#[test]
fn colon_e_reruns_and_colon_e_bang_asks_before_resetting() {
    let mut d = Driver::new(3);
    assert_eq!(d.keys(":e<cr>"), vec![Effect::Rerun]);
    accept(&mut d, &[0, 1]);
    d.keys(":e!<cr>");
    assert_eq!(*d.overlay(), Overlay::ConfirmReset);
    let effects = d.keys("y");
    assert!(effects.contains(&Effect::Save), "{effects:?}");
    assert!(d.decisions().iter().all(|x| *x == P));
}

#[test]
fn command_line_accepts_four_commands() {
    // `:w`, `:e`, `:e!` and `:q` are above; anything else is an error that
    // changes nothing.
    for cmd in [":x<cr>", ":wq<cr>", ":E<cr>"] {
        let mut d = Driver::new(3);
        accept(&mut d, &[1]);
        d.select(2);
        assert!(d.keys(cmd).is_empty(), "{cmd}");
        let typed = cmd.trim_end_matches("<cr>");
        assert_eq!(d.status(), format!("Not a command: {typed}"));
        assert_eq!(*d.overlay(), Overlay::None);
        assert_eq!(d.decisions(), vec![P, A, P]);
        assert_eq!(d.sel(), 2);
        // The keys after it are normal keys again.
        d.keys("k");
        assert_eq!(d.sel(), 1);
    }
}

#[test]
fn keys_of_the_notes_view_do_nothing_on_the_list() {
    for keys in ["y", "d", "c", "p"] {
        let mut d = Driver::new(3);
        d.select(1);
        assert!(d.keys(keys).is_empty(), "{keys}");
        assert_eq!(*d.overlay(), Overlay::None, "{keys}");
        assert_eq!(d.sel(), 1, "{keys}");
        assert!(d.decisions().iter().all(|x| *x == P), "{keys}");
    }
}

// ---- The old screens' keys on the list ------------------------------------------

#[test]
fn s_enters_swipe_and_leaves_it() {
    let mut d = v1(3);
    d.keys("s");
    assert_eq!(d.screen(), Screen::Swipe);
    d.keys("s");
    assert_eq!(d.screen(), Screen::Slim);
}

#[test]
fn zf_switches_between_by_unit_and_by_file() {
    // The queue starts by level (Grouping::Unit, Q3).
    let mut d = Driver::new(3);
    d.keys("zf");
    assert_eq!(d.app.grouping, Grouping::File);
    d.keys("zf");
    assert_eq!(d.app.grouping, Grouping::Unit);
}

// ---- Swipe ----------------------------------------------------------------------

#[test]
fn in_swipe_a_accepts_f_flags_and_k_goes_back_a_card() {
    let mut d = v1(6);
    d.keys("s");
    assert_eq!(d.app.swipe_current().map(String::as_str), Some("u0"));
    d.keys("a");
    assert_eq!(d.decision_of("u0"), A);
    d.keys("f");
    assert_eq!(d.decision_of("u1"), F);
    assert_eq!(*d.overlay(), Overlay::None, "no note line in swipe");
    assert_eq!(d.app.swipe_current().map(String::as_str), Some("u2"));
    d.keys("k");
    assert_eq!(d.app.swipe_current().map(String::as_str), Some("u1"));
    assert_eq!(d.decision_of("u1"), F, "going back decides nothing");
    d.keys("j");
    assert_eq!(
        d.app.swipe_current().map(String::as_str),
        Some("u1"),
        "j is unbound in swipe"
    );
}

#[test]
fn in_swipe_h_esc_and_q_go_back_to_the_list() {
    for keys in ["h", "<esc>", "q"] {
        let mut d = v1(3);
        d.keys("s");
        let effects = d.keys(keys);
        assert_eq!(d.screen(), Screen::Slim, "{keys}");
        assert!(!effects.contains(&Effect::Quit), "{keys}");
    }
}

#[test]
fn in_swipe_u_undoes_the_last_card() {
    let mut d = v1(3);
    d.keys("sa");
    assert_eq!(d.decision_of("u0"), A);
    d.keys("u");
    assert_eq!(d.decision_of("u0"), P);
    assert_eq!(d.app.swipe_current().map(String::as_str), Some("u0"));
    d.keys("<c-r>");
    assert_eq!(d.decision_of("u0"), A);
}

// ---- Flagged --------------------------------------------------------------------

#[test]
fn on_flagged_j_k_gg_and_shift_g_move_between_flagged_units() {
    let mut d = on_flagged();
    let at = |d: &Driver| d.app.flagged_unit_id().unwrap();
    d.keys("j");
    assert_eq!(at(&d), "u3");
    d.keys("G");
    assert_eq!(at(&d), "u4");
    d.keys("k");
    assert_eq!(at(&d), "u3");
    d.keys("gg");
    assert_eq!(at(&d), "u1");
}

#[test]
fn on_flagged_a_accepts_the_unit_and_it_leaves_the_flagged_list() {
    let mut d = on_flagged();
    let effects = d.keys("a");
    assert!(effects.contains(&Effect::Save), "{effects:?}");
    assert_eq!(d.decision_of("u1"), A);
    assert_eq!(d.app.flagged_units().len(), 2);
    assert_eq!(d.screen(), Screen::Flagged);
    assert_eq!(d.app.flagged_unit_id().as_deref(), Some("u3"));
}

#[test]
fn on_flagged_f_and_i_open_the_note_line() {
    for keys in ["f", "i"] {
        let mut d = on_flagged();
        d.keys(keys);
        assert!(
            matches!(d.overlay(), Overlay::Note { id, .. } if id == "u1"),
            "{keys}: {:?}",
            d.overlay()
        );
        assert_eq!(d.decision_of("u1"), F, "{keys}: still flagged");
    }
}

#[test]
fn on_flagged_h_unzooms_then_goes_back_and_q_goes_back() {
    let mut d = on_flagged();
    d.keys("zz");
    assert!(d.app.flagged.zoom);
    d.keys("h");
    assert!(!d.app.flagged.zoom);
    assert_eq!(d.screen(), Screen::Flagged);
    d.keys("h");
    assert_eq!(d.screen(), Screen::Slim);

    let mut d = on_flagged();
    let effects = d.keys("q");
    assert_eq!(d.screen(), Screen::Slim, "q goes back to the list");
    assert!(!effects.contains(&Effect::Quit), "{effects:?}");
    let mut d = on_flagged();
    d.keys(":q<cr>");
    assert_eq!(d.screen(), Screen::Slim);
}

#[test]
fn on_flagged_gt_gt_tab_and_arrows_move_the_tile_focus() {
    let mut d = on_flagged();
    {
        let u = d.app.review.unit_mut("u1").unwrap();
        u.dive.asked = vec![DiveKind::Recommend, DiveKind::Tests];
    }
    d.keys("gg");
    let panes = |d: &Driver| {
        let u = d.app.flagged_unit().unwrap();
        d.app.flagged_panes(u)
    };
    let focus = |d: &Driver| {
        let u = d.app.flagged_unit().unwrap();
        d.app.flagged_focus(u)
    };
    let all = panes(&d);
    assert!(all.len() >= 3, "{all:?}");
    for (fwd, back) in [("gt", "gT"), ("<tab>", "<s-tab>"), ("<right>", "<left>")] {
        let from = focus(&d);
        let i = all.iter().position(|p| *p == from).unwrap();
        d.keys(fwd);
        assert_eq!(focus(&d), all[(i + 1) % all.len()], "{fwd}");
        d.keys(back);
        assert_eq!(focus(&d), from, "{back}");
    }
}

#[test]
fn on_flagged_shift_k_opens_the_chat_and_enter_types_in_it() {
    let mut d = on_flagged();
    d.keys("K");
    assert_eq!(d.app.flagged.pane, Pane::Chat);
    assert!(d.app.flagged.typing);
    d.keys("<esc>");
    assert!(!d.app.flagged.typing, "esc stops typing");
    assert_eq!(d.screen(), Screen::Flagged);
    d.keys("<cr>");
    assert!(d.app.flagged.typing, "enter on the chat starts typing");
}

#[test]
fn on_flagged_leader_keys_ask_their_tile() {
    for (keys, kind) in [
        ("<space>w", DiveKind::Recommend),
        ("<space>v", DiveKind::Visualize),
        ("<space>r", DiveKind::Behavior),
        ("<space>t", DiveKind::Tests),
    ] {
        let mut d = on_flagged();
        let effects = d.keys(keys);
        assert!(has_dive(&effects, "u1", kind), "{keys}: {effects:?}");
        assert_eq!(d.app.flagged.pane, Pane::Answer(kind), "{keys}");
    }
    let mut d = on_flagged();
    assert_eq!(d.keys("<space>m"), vec![Effect::Second(vec!["u1".into()])]);
    let mut d = on_flagged();
    assert!(d.keys("<space>a").is_empty());
    assert!(d.status().contains("␣t"), "{}", d.status());
}

#[test]
fn on_flagged_o_and_x_set_the_resolution_and_r_asks_the_tile_again() {
    let mut d = on_flagged();
    let res = |d: &Driver| d.app.review.unit("u1").unwrap().dive.resolution;
    assert!(d.keys("o").contains(&Effect::Save));
    assert_eq!(res(&d), Some(Resolution::Ok));
    d.keys("o");
    assert_eq!(res(&d), None, "the same key again clears it");
    d.keys("x");
    assert_eq!(res(&d), Some(Resolution::NeedsFix));

    d.keys("<space>w");
    d.app.review.unit_mut("u1").unwrap().dive_pending.clear();
    d.app
        .review
        .unit_mut("u1")
        .unwrap()
        .dive
        .results
        .insert(DiveKind::Recommend, DiveResult::default());
    let effects = d.keys("r");
    assert!(has_dive(&effects, "u1", DiveKind::Recommend), "{effects:?}");
}

#[test]
fn on_flagged_gx_opens_the_picture_or_says_how_to_draw_one() {
    let mut d = on_flagged();
    assert!(d.keys("gx").is_empty());
    assert_eq!(d.status(), "No picture yet: press ␣v to draw one.");
}

#[test]
fn on_flagged_n_and_shift_n_do_not_move_between_flagged_units() {
    // n / N are the list's pending jumps; on Flagged, j / k move.
    let mut d = on_flagged();
    d.keys("n");
    assert_eq!(d.app.flagged_unit_id().as_deref(), Some("u1"));
    assert_eq!(d.screen(), Screen::Flagged);
}

// ---- The keymap's modes ---------------------------------------------------------

/// One case per row of the default table: every default key is typed, as
/// it is, by a test in this file.
#[test]
fn every_default_binding() {
    let src = include_str!("v2_keymap.rs");
    let missing: Vec<String> = Keymap::default()
        .bindings()
        .map(|(a, t)| {
            let typed = match t {
                Trigger::Keys(_) => t.notation(),
                Trigger::Command(c) => format!(":{c}<cr>"),
            };
            (a, typed)
        })
        .filter(|(_, typed)| !src.contains(&format!("\"{typed}\"")))
        .map(|(a, typed)| format!("{typed} ({})", a.id()))
        .collect();
    assert!(missing.is_empty(), "no test types {missing:?}");
}

#[test]
fn screens_receive_actions_not_keys() {
    // Move accept from `a` to `e`: every screen follows the keymap.
    let over = HashMap::from([("decide.accept".to_string(), KeySpec::One("e".into()))]);
    let keys = || Keymap::new(&over, 1000).unwrap().0;

    let mut d = Driver::with(review(units(6)), keys());
    assert!(d.keys("a").is_empty());
    assert_eq!(d.decision(0), P, "a is unbound now");
    d.keys("e");
    assert_eq!(d.decision(0), A);

    let mut d = Driver::with(review(units(6)), keys());
    d.app.open_v1_list();
    d.keys("s");
    d.keys("a");
    assert_eq!(d.decision_of("u0"), P, "swipe: a is unbound");
    d.keys("e");
    assert_eq!(d.decision_of("u0"), A, "swipe: e accepts");

    let mut d = Driver::with(review(units(6)), keys());
    d.app.open_v1_list();
    flag(&mut d, &[0, 1]);
    d.keys("l");
    assert_eq!(d.screen(), Screen::Flagged);
    d.keys("a");
    assert_eq!(d.decision_of("u0"), F, "Flagged: a is unbound");
    d.keys("e");
    assert_eq!(d.decision_of("u0"), A, "Flagged: e accepts");
}

#[test]
fn prefix_waits_shows_hint_and_times_out() {
    let mut d = Driver::new(6);
    d.select(3);
    assert!(d.keys("g").is_empty());
    assert_eq!(d.app.keys.pending().as_deref(), Some("g"));
    assert!(
        d.app
            .keys
            .hint()
            .contains(&("d".to_string(), Action::GotoDefinition))
    );
    let s = d.render(120, 30);
    assert!(
        s.contains(Action::GotoDefinition.describe()),
        "the hint line lists what can follow:\n{s}"
    );

    assert!(d.timeout(), "the prefix is dropped");
    assert_eq!(d.app.keys.pending(), None);
    let s = d.render(120, 30);
    assert!(!s.contains(Action::GotoDefinition.describe()), "{s}");
    // `g` alone after the timeout: `g` starts again; `gg` still works.
    d.keys("gg");
    assert_eq!(d.sel(), 0);

    // `<space>`, then the timeout, then `n`: n alone (next pending).
    let mut d = Driver::new(6);
    accept(&mut d, &[1]);
    d.keys("<space>");
    d.timeout();
    d.keys("n");
    assert_eq!(*d.overlay(), Overlay::None, "not the notes view");
    assert_eq!(d.sel(), 2);
}

#[test]
fn the_prefix_timeout_is_configurable() {
    let (keys, _) = Keymap::new(&HashMap::new(), 3000).unwrap();
    let mut d = Driver::with(review(units(6)), keys);
    d.select(4);
    d.keys("g");
    assert!(!d.timeout(), "1.1 s is within 3 s");
    d.keys("g");
    assert_eq!(d.sel(), 0);
}

#[test]
fn counts_apply_and_esc_drops_them() {
    let mut d = Driver::new(9);
    d.keys("3j");
    assert_eq!(d.sel(), 3);
    d.keys("5");
    assert_eq!(d.app.keys.pending().as_deref(), Some("5"));
    d.keys("<esc>");
    assert_eq!(d.app.keys.pending(), None);
    assert_eq!(d.sel(), 3, "esc with a count does nothing else");
    d.keys("a");
    assert_eq!(d.positions(A), vec![3], "the count is gone");
    // A count waits for its action, with no timeout.
    d.keys("2");
    d.timeout();
    d.keys("a");
    assert_eq!(d.positions(A), vec![3, 4, 5]);
}

#[test]
fn rebinding_changes_dispatch_and_help() {
    let over = HashMap::from([
        ("decide.accept".to_string(), KeySpec::One("e".into())),
        (
            "move.next_file".to_string(),
            KeySpec::Many(vec!["<c-n>".into(), "]]".into()]),
        ),
    ]);
    let (keys, warnings) = Keymap::new(&over, 1000).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let mut d = Driver::with(review(units(9)), keys);
    d.keys("a");
    assert_eq!(d.decision(0), P);
    d.keys("e");
    assert_eq!(d.decision(0), A);
    d.keys("<c-n>");
    assert_eq!(d.sel(), 3);
    d.keys("]]");
    assert_eq!(d.sel(), 6);
    d.keys("[f");
    assert_eq!(d.sel(), 3, "the keys not rebound keep working");
    d.keys("]f");
    assert_eq!(d.sel(), 3, "]f is no longer bound");

    // The v1 list's help (the queue's lists `queue::ACTIONS`: Q-draw).
    d.app.open_v1_list();
    d.keys("?");
    assert_eq!(*d.overlay(), Overlay::Help);
    let help = d.render(120, 50);
    let accept_rows: Vec<&str> = help
        .lines()
        .filter(|l| l.contains("accept") && !l.contains("all pending"))
        .collect();
    assert!(
        accept_rows
            .iter()
            .any(|l| l.split_whitespace().any(|w| w == "e")),
        "help names e for accept:\n{help}"
    );
    assert!(
        !accept_rows
            .iter()
            .any(|l| l.split_whitespace().any(|w| w == "a")),
        "and no longer a:\n{help}"
    );
}

#[test]
fn f_on_a_unit_with_a_note_opens_the_note_line_prefilled() {
    // Esc keeps the flag and the note as it was.
    let mut d = Driver::of(vec![unit(0, "a.rs"), unit(1, "a.rs")]);
    d.app.review.unit_mut("u0").unwrap().dive.note = "old".into();
    d.keys("f");
    assert!(
        matches!(d.overlay(), Overlay::Note { id, text, .. } if id == "u0" && text == "old"),
        "{:?}",
        d.overlay()
    );
    d.keys("<esc>");
    assert_eq!(d.note("u0"), "old");
    assert_eq!(d.decision_of("u0"), F);
}
