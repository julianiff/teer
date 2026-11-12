//! v2's default keys on today's screens (Phase 1): one test per binding of
//! the default table, typed in Neovim notation through `App::on_key` on a
//! synthetic review, and the keymap's modes around them: prefixes and their
//! timeout, counts, the command line, `.`, visual mode, undo and redo saved
//! with the review, and rebinding. The keymap module's own tests cover the
//! notation and the four rules; `no_key_literals` the "no `KeyCode::`" rule.

use crate::common;

use std::collections::HashMap;
use std::path::PathBuf;

use common::v2::{Driver, review, unit, units};
use teer::keymap::{Action, KeySpec, Keymap, Trigger};
use teer::review::model::{Change, Decision, DiveKind, DiveResult, Marker, Range, Risk, Visual};
use teer::session::app::{Effect, Grouping, JumpKind, Overlay, Screen};
use teer::session::unit::Panel;

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
fn f_flags_and_advances_asking_for_no_note() {
    let mut d = Driver::new(6);
    d.keys("f");
    assert_eq!(d.decision(0), F);
    assert_eq!(d.sel(), 1);
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.note("u0"), "");
    d.keys("f");
    assert_eq!(d.decision(1), F);
    assert_eq!(d.sel(), 2);
}

#[test]
fn a_count_on_f_flags_several() {
    let mut d = Driver::new(6);
    d.keys("2f");
    assert_eq!(d.positions(F), vec![0, 1]);
    assert_eq!(*d.overlay(), Overlay::None);
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
    // The queue acts at once (Phase 4).
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
    assert_eq!(*d.overlay(), Overlay::None);
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
    assert_eq!(d.app.visual_rows(), Some(1..=1));
    d.keys("jj");
    assert_eq!(d.sel(), 3);
    assert_eq!(
        d.app.visual_rows(),
        Some(1..=3),
        "moves extend the selection"
    );
    d.keys("a");
    assert_eq!(d.positions(A), vec![1, 2, 3]);
    assert_eq!(d.app.visual, None, "deciding leaves visual mode");
    assert_eq!(d.undo_steps(), 1);
}

#[test]
fn visual_f_flags_the_selection() {
    let mut d = Driver::new(6);
    d.keys("Vjf");
    assert_eq!(d.positions(F), vec![0, 1]);
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.app.visual, None);
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
    // In the unit view: the ask tile, typing; esc stops typing, a second
    // esc closes the tile.
    d.keys("<esc>");
    assert_eq!(d.app.unit.panel, Some(Panel::Ask));
    assert!(!d.app.unit.typing);
    d.keys("<esc>");
    assert_eq!(d.app.unit.panel, None);
    d.keys("K");
    assert_eq!(d.app.unit.panel, Some(Panel::Ask));
    assert!(d.app.unit.typing);
    assert_eq!(*d.overlay(), Overlay::None);
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
fn leader_tools_on_the_queue_open_the_unit_view_with_that_panel() {
    for (keys, kind) in [
        ("<space>t", DiveKind::Tests),
        ("<space>r", DiveKind::Behavior),
        ("<space>v", DiveKind::Visualize),
        ("<space>x", DiveKind::Security),
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
fn the_v1_tile_keys_are_gone_from_the_queue_and_the_unit_view() {
    // v1's tiles: what to do (`␣w`) and the second model (`␣m`). (`␣a`,
    // once the agent reading the tests, asks for advice now.)
    for keys in ["<space>w", "<space>m"] {
        let mut d = Driver::of(units(3).into_iter().map(described).collect());
        d.app.has_agent = true;
        d.app.review.agent_session = Some(teer::review::model::SessionRef::default());
        assert!(d.keys(keys).is_empty(), "{keys}");
        assert_eq!(d.screen(), Screen::Queue, "{keys}");
        assert_eq!(d.decisions(), vec![P, P, P], "{keys}");
        d.keys("l");
        assert_eq!(d.screen(), Screen::Unit, "{keys}");
        assert!(d.keys(keys).is_empty(), "{keys}");
        assert_eq!(d.app.unit.panel, None, "{keys}");
    }
}

#[test]
fn zs_switches_the_queue_to_the_security_order() {
    use teer::security::{Class, Fact, Weight};
    // u4 has a fact that leaves its level as it is: by level it stays in
    // its file, by exposure it leads the queue.
    let mut units = units(6);
    units[4].security = vec![Fact {
        class: Class::EntryPoint,
        weight: Weight::Note,
        text: "a new route".into(),
        side: teer::review::model::Side::New,
        lines: None,
        subject: "POST /refunds".into(),
        origin: Default::default(),
    }];
    let mut d = Driver::of(units);
    let by_level = d.app.order.clone();
    assert_eq!(d.app.review.units[by_level[4]].id, "u4");
    d.app.sel = 2;
    d.keys("zs");
    assert!(d.app.security.by_exposure);
    assert_eq!(d.app.security.exposed, 1);
    assert_eq!(d.app.review.units[d.app.order[0]].id, "u4");
    // The rest keep their order, and the cursor its unit.
    let rest: Vec<usize> = by_level.iter().copied().filter(|&i| i != 4).collect();
    assert_eq!(d.app.order[1..], rest[..]);
    assert_eq!(d.app.selected_id().as_deref(), Some("u2"));
    assert!(
        d.app.status.starts_with("By exposure: 1 unit"),
        "{}",
        d.app.status
    );
    d.keys("zs");
    assert!(!d.app.security.by_exposure);
    assert_eq!(d.app.order, by_level);
    assert_eq!(d.app.security.exposed, 0);
    assert_eq!(d.app.status, "Listed by level.");
}

#[test]
fn leader_p_proves_the_findings_of_the_unit() {
    use teer::review::model::Cite;
    use teer::security::{Audit, Class, Finding, Proof};

    let cite = |line: u32| Cite {
        file: PathBuf::from("f0.rs"),
        start: line,
        end: line,
    };
    let finding = |title: &str| Finding {
        class: Class::Authorization,
        severity: Risk::High,
        confidence: Risk::Medium,
        title: title.into(),
        source: cite(1),
        path: Vec::new(),
        sink: cite(2),
        missing: "the owner check".into(),
        exploit: "tenant B reads tenant A's asset".into(),
        path_verified: true,
        second: None,
        proof: None,
    };
    let audited = |d: &mut Driver, findings: Vec<Finding>| {
        d.app.review.unit_mut("u0").unwrap().dive.results.insert(
            DiveKind::Security,
            DiveResult {
                audit: Some(Audit {
                    findings,
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
    };
    let proving = |d: &Driver| {
        d.app.review.units[0].dive.results[&DiveKind::Security]
            .audit
            .as_ref()
            .unwrap()
            .proving
    };

    // No agent: nothing to prove with.
    let mut d = Driver::new(3);
    assert!(d.keys("<space>p").is_empty());
    assert_eq!(d.status(), "No agent: proving needs one.");

    // No findings: the Security tile asks for them first.
    d.app.has_agent = true;
    d.app.review.agent_session = Some(teer::review::model::SessionRef::default());
    assert!(d.keys("<space>p").is_empty());
    assert_eq!(d.status(), "No findings to prove: ␣x asks for them first.");
    audited(&mut d, Vec::new());
    assert!(d.keys("<space>p").is_empty());
    assert!(
        d.status().starts_with("No findings to prove"),
        "{}",
        d.status()
    );

    // From the queue: the unit under the cursor, the findings not run yet.
    let mut done = finding("already run");
    done.proof = Some(Proof::Unproven {
        why: "needs a database".into(),
    });
    audited(
        &mut d,
        vec![finding("asset loads for another tenant"), done],
    );
    assert_eq!(d.selected().as_deref(), Some("u0"));
    assert_eq!(
        d.keys("<space>p"),
        vec![Effect::ProveFindings {
            unit_id: "u0".into()
        }]
    );
    assert_eq!(d.screen(), Screen::Queue);
    assert!(proving(&d));
    assert_eq!(d.status(), "Proving 1 finding old against new…");
    // One proof at a time.
    assert!(d.keys("<space>p").is_empty());
    assert!(d.status().starts_with("Already proving"), "{}", d.status());

    // What came back clears it; every finding run, nothing is left.
    d.app.on_app_event(teer::session::app::AppEvent::Dive(
        teer::review::dive::DiveEvent::Proved {
            id: "u0".into(),
            fingerprint: "fp0".into(),
            proofs: vec![(
                0,
                Proof::NotShown {
                    case: "c".into(),
                    before: "1".into(),
                    after: "1".into(),
                },
            )],
        },
    ));
    assert!(!proving(&d));
    assert!(d.keys("<space>p").is_empty());
    assert!(
        d.status().starts_with("Nothing left to prove"),
        "{}",
        d.status()
    );

    // In the unit view, with the Security tile open: it gets the focus.
    audited(&mut d, vec![finding("asset loads for another tenant")]);
    d.keys("<space>x");
    assert_eq!(d.screen(), Screen::Unit);
    assert_eq!(d.app.unit.panel, Some(Panel::Tool(DiveKind::Security)));
    d.keys("<tab>");
    assert_ne!(d.app.unit.panel, Some(Panel::Tool(DiveKind::Security)));
    assert_eq!(
        d.keys("<space>p"),
        vec![Effect::ProveFindings {
            unit_id: "u0".into()
        }]
    );
    assert_eq!(d.app.unit.panel, Some(Panel::Tool(DiveKind::Security)));
    assert!(proving(&d));
}

#[test]
fn leader_e_asks_for_a_proposed_change() {
    // The UI of proposed changes asserts what it does.
    let mut d = Driver::new(3);
    d.keys("<space>e");
}

#[test]
fn p_and_shift_p_put_proposed_hunks_into_the_code() {
    // The UI of proposed changes asserts what they do.
    let mut d = Driver::new(3);
    d.keys("p");
    d.keys("P");
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
fn colon_app_saves_and_hands_the_review_to_the_app() {
    let mut d = Driver::new(3);
    assert_eq!(d.keys(":app<cr>"), vec![Effect::Save, Effect::OpenApp]);
}

#[test]
fn colon_w_opens_the_finish_sheet_on_the_step_that_fits() {
    use teer::session::finish::Step;
    let mut d = Driver::new(3);
    d.keys(":w<cr>");
    assert_eq!(*d.overlay(), Overlay::Finish { sel: 0 });
    assert_eq!(d.app.finish_step(), Some(Step::KeepReviewing));
    d.keys("<esc>");

    d.app.review.unit_mut("u1").unwrap().dive.note = "check this".into();
    d.keys("<space>n");
    assert!(matches!(d.overlay(), Overlay::Notes { .. }));
    d.keys(":w<cr>");
    assert!(
        matches!(d.overlay(), Overlay::Finish { .. }),
        "{:?}",
        d.overlay()
    );

    let mut d = Driver::new(3);
    accept(&mut d, &[0, 1, 2]);
    d.keys(":w<cr>");
    assert_eq!(d.app.finish_step(), Some(Step::Commit));
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

// ---- The unit view's tiles ------------------------------------------------------

#[test]
fn tile_keys_move_the_focus_and_zoom_in_the_unit_view_only() {
    let mut d = Driver::new(3);
    d.app.has_agent = true;
    d.app.review.agent_session = Some(teer::review::model::SessionRef::default());
    d.app.cfg.dive_web = false;
    // In the queue they do nothing.
    for keys in ["<tab>", "gt", "<s-tab>", "gT", "zz"] {
        assert!(d.keys(keys).is_empty(), "{keys}");
        assert_eq!((d.screen(), d.sel()), (Screen::Queue, 0), "{keys}");
    }
    d.keys("<space>t<space>r");
    assert_eq!(d.screen(), Screen::Unit);
    let rec = Panel::Recommend;
    let tests = Panel::Tool(DiveKind::Tests);
    let run = Panel::Tool(DiveKind::Behavior);
    // The advice tile is open from the start.
    assert_eq!(d.app.unit.tiles, [rec, tests, run]);
    for (keys, focus) in [
        ("<tab>", None),
        ("gt", Some(rec)),
        ("gt", Some(tests)),
        ("<s-tab>", Some(rec)),
        ("gT", None),
        ("gT", Some(run)),
    ] {
        d.keys(keys);
        assert_eq!(d.app.unit.panel, focus, "{keys}");
    }
    d.keys("zz");
    assert!(d.app.unit.zoom);
    d.keys("zz");
    assert!(!d.app.unit.zoom);
}

// ---- Advice -----------------------------------------------------------------------

#[test]
fn space_a_asks_for_advice_from_the_queue_and_the_unit_view_only_when_pressed() {
    let mut d = Driver::of(units(3).into_iter().map(described).collect());
    d.app.has_agent = true;
    d.app.review.agent_session = Some(teer::review::model::SessionRef::default());
    let asks = |effects: &[Effect]| {
        effects
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Effect::Dive {
                        kind: DiveKind::Recommend,
                        ..
                    }
                )
            })
            .count()
    };
    assert_eq!(asks(&d.keys("l")), 0, "opening asks nothing");
    assert_eq!(asks(&d.keys("<space>a")), 1);
    assert_eq!(d.app.unit.panel, Some(Panel::Recommend));
    d.keys("hh");
    assert_eq!(d.screen(), Screen::Queue);
    // From the queue: the unit view on the next unit, asking.
    d.keys("j");
    let next = d.app.selected_id().unwrap();
    assert_eq!(asks(&d.keys("<space>a")), 1);
    assert_eq!(d.screen(), Screen::Unit);
    assert_eq!(d.app.unit.id, next);
}

// ---- The flagged list -----------------------------------------------------------

#[test]
fn space_f_opens_the_flagged_list_and_again_goes_back() {
    let mut d = Driver::new(3);
    // Nothing flagged: it says so and stays.
    d.keys("<space>f");
    assert_eq!(d.screen(), Screen::Queue);
    assert!(
        d.status().starts_with("Nothing is flagged yet"),
        "{}",
        d.status()
    );
    d.select(1);
    d.keys("f");
    d.keys("<space>f");
    assert_eq!(d.screen(), Screen::Flagged);
    assert_eq!(d.app.flagged.at, d.id_at(1));
    d.keys("<space>f");
    assert_eq!(d.screen(), Screen::Queue);
    // From the unit view, back there.
    d.keys("l<space>f");
    assert_eq!(d.screen(), Screen::Flagged);
    d.keys("<space>f");
    assert_eq!(d.screen(), Screen::Unit);
}

// ---- The old screens' keys ------------------------------------------------------

#[test]
fn the_v1_screens_keys_do_nothing_in_the_queue_or_the_unit_view() {
    // Swipe, the Flagged screen's arrow keys, resolutions, ask again. Its
    // tile keys (`tab` `gt` `zz`) are the unit view's again.
    for keys in ["s", "<right>", "<left>", "o", "x", "r"] {
        let mut d = Driver::new(3);
        d.select(1);
        assert!(d.keys(keys).is_empty(), "{keys}");
        assert_eq!(d.screen(), Screen::Queue, "{keys}");
        assert_eq!(d.sel(), 1, "{keys}");
        assert_eq!(*d.overlay(), Overlay::None, "{keys}");
        d.keys("l");
        assert!(d.keys(keys).is_empty(), "{keys}");
        assert_eq!(d.screen(), Screen::Unit, "{keys}");
        assert_eq!(d.app.unit.panel, None, "{keys}");
        assert_eq!(*d.overlay(), Overlay::None, "{keys}");
        assert_eq!(d.decisions(), vec![P, P, P], "{keys}");
    }
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
    flag(&mut d, &[0, 1]);
    d.keys("l");
    assert_eq!(d.screen(), Screen::Unit);
    let on = d.app.unit.id.clone();
    d.keys("a");
    assert_eq!(d.decision_of(&on), F, "unit view: a is unbound");
    d.keys("e");
    assert_eq!(d.decision_of(&on), A, "unit view: e accepts");
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

    // The queue's help.
    d.keys("?");
    assert_eq!(*d.overlay(), Overlay::Help);
    let help = d.render(120, 50);
    let accept_rows: Vec<&str> = help.lines().filter(|l| l.contains("accept /")).collect();
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
fn f_on_a_unit_with_a_note_keeps_it_as_it_was() {
    let mut d = Driver::of(vec![unit(0, "a.rs"), unit(1, "a.rs")]);
    d.app.review.unit_mut("u0").unwrap().dive.note = "old".into();
    d.keys("f");
    assert_eq!(*d.overlay(), Overlay::None);
    assert_eq!(d.note("u0"), "old");
    assert_eq!(d.decision_of("u0"), F);
}
