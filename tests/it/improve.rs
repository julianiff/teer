//! Improve mode (see FEATURES.md, improve): the inserts the agent
//! proposes, asked for in this mode only, listed in place of the units by
//! what each does; `a` and `x` as marks, `:w` putting the inserts to add
//! in at once and then asking whether to check again; the preview; the
//! header and the bar; another mode hides the list and keeps the inserts.

use crate::common;

use std::path::PathBuf;

use common::v2::{Driver, review, unit};
use teer::fix::{HunkState, InsertKind, Proposal, ProposedHunk, Reason};
use teer::git::{ChangedFile, FileStatus};
use teer::keymap::Action;
use teer::mode::Mode;
use teer::review::dive::DiveEvent;
use teer::review::model::{Decision, DiveKind, DiveResult, PrInfo, Range, SessionRef, Unit};
use teer::security::SecondCheck;
use teer::session::app::{AppEvent, Effect, Overlay, Screen};
use teer::session::finish::Step;
use teer::tui::app::Target;

/// Twenty lines `l1`..`l20`, changed at lines 2 and 17.
fn file(path: &str) -> ChangedFile {
    let old: Vec<String> = (1..=20).map(|i| format!("l{i}")).collect();
    let mut new = old.clone();
    new[1] = "L2".into();
    new[16] = "L17".into();
    ChangedFile {
        path: PathBuf::from(path),
        old_path: None,
        status: FileStatus::Modified,
        old_content: Some(old.join("\n")),
        new_content: Some(new.join("\n")),
    }
}

/// Unit `u{i}` over all twenty lines of `path`.
fn changed(i: usize, path: &str) -> Unit {
    let mut u = unit(i, path);
    u.old_range = Some(Range::new(1, 20));
    u.new_range = Some(Range::new(1, 20));
    u
}

/// Three units with a code change each (`u0` in `a.rs`, `u1` in `b.rs`,
/// `u2` in `c.rs`), an agent, in review mode.
fn driver() -> Driver {
    let mut r = review(vec![
        changed(0, "a.rs"),
        changed(1, "b.rs"),
        changed(2, "c.rs"),
    ]);
    r.files = vec![file("a.rs"), file("b.rs"), file("c.rs")];
    let mut d = Driver::with(r, teer::keymap::Keymap::default());
    d.app.has_agent = true;
    d.app.review.agent_session = Some(SessionRef::default());
    d.app.set_mode(Mode::Review);
    d
}

/// An insert replacing line `line` of `file` (`l{line}`) with `new`,
/// between a line of context on each side.
fn insert(file: &str, line: u32, new: &str, why: &str, kind: InsertKind) -> ProposedHunk {
    ProposedHunk {
        file: PathBuf::from(file),
        lines: Range::new(line - 1, line + 1),
        old: format!("l{}\nl{line}\nl{}", line - 1, line + 1),
        new: format!("l{}\n{new}\nl{}", line - 1, line + 1),
        why: why.into(),
        kind: Some(kind),
        parses: true,
        state: HunkState::Proposed,
    }
}

/// The inserts `hunks`, the second model agreeing.
fn inserts(hunks: Vec<ProposedHunk>) -> Proposal {
    Proposal {
        reason: Reason::Improve,
        summary: "Adds what the change lacks.".into(),
        hunks,
        second: Some(SecondCheck {
            agree: true,
            why: "each adds what it says".into(),
            evidence: None,
        }),
        dropped: 0,
    }
}

/// Unit `id` answered with `p`.
fn answered(d: &mut Driver, id: &str, p: Proposal) {
    let u = d.app.review.unit_mut(id).unwrap();
    u.dive.results.insert(
        DiveKind::Improve,
        DiveResult {
            text: p.summary.clone(),
            proposal: Some(p),
            ..Default::default()
        },
    );
}

/// A driver with inserts on `u0` (a gap, a polish) and `u1` (stronger),
/// `u2` answered with nothing, in improve mode.
fn listed() -> Driver {
    let mut d = driver();
    answered(
        &mut d,
        "u0",
        inserts(vec![
            insert(
                "a.rs",
                5,
                "L5 = int(l5)",
                "cast the id before it is used",
                InsertKind::Gap,
            ),
            insert(
                "a.rs",
                12,
                "L12 documented",
                "say what the rule is",
                InsertKind::Polish,
            ),
        ]),
    );
    answered(
        &mut d,
        "u1",
        inserts(vec![insert(
            "b.rs",
            9,
            "L9 tested",
            "a test for the empty cart",
            InsertKind::Stronger,
        )]),
    );
    answered(&mut d, "u2", inserts(Vec::new()));
    d.app.set_mode(Mode::Improve);
    d
}

fn states(d: &Driver, id: &str) -> Vec<HunkState> {
    let u = d.app.review.unit(id).unwrap();
    u.dive.results[&DiveKind::Improve]
        .proposal
        .as_ref()
        .unwrap()
        .hunks
        .iter()
        .map(|h| h.state)
        .collect()
}

fn dives(effects: &[Effect]) -> Vec<(String, DiveKind)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Dive { unit_id, kind, .. } => Some((unit_id.clone(), *kind)),
            _ => None,
        })
        .collect()
}

// ---- Asked in the mode only ---------------------------------------------

#[test]
fn turning_the_mode_on_asks_every_unit_once_and_never_in_another_mode() {
    let mut d = driver();
    // Review mode asks for nothing.
    assert!(dives(&d.app.choose_mode(Mode::Review)).is_empty());
    // Improve mode asks every listed unit, in the queue's order, once.
    let e = d.app.choose_mode(Mode::Improve);
    assert_eq!(e[0], Effect::Save);
    let asked = dives(&e);
    assert_eq!(asked.len(), 3, "{asked:?}");
    assert!(asked.iter().all(|(_, k)| *k == DiveKind::Improve));
    let ids: Vec<&str> = asked.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["u0", "u1", "u2"]);
    assert!(dives(&d.app.choose_mode(Mode::Improve)).is_empty());
    assert_eq!(d.app.improve_progress(), (0, 3));
    // The header counts the units the agent has looked at.
    let s = d.render(100, 20);
    assert!(s.starts_with(" ✨ "), "{s}");
    assert!(s.contains("looking for what to add 0/3"), "{s}");
    assert!(
        s.contains("the agent looks for what to add · 0 of 3 units"),
        "{s}"
    );
    // An answer lands under its kind, and that unit isn't asked again.
    let p = inserts(vec![insert(
        "a.rs",
        5,
        "L5 = int(l5)",
        "cast it",
        InsertKind::Gap,
    )]);
    let e = d.app.on_app_event(AppEvent::Dive(DiveEvent::Proposed {
        id: "u0".into(),
        fingerprint: "fp0".into(),
        kind: DiveKind::Improve,
        text: p.summary.clone(),
        proposal: Box::new(p),
    }));
    assert_eq!(e, vec![Effect::Save]);
    assert_eq!(states(&d, "u0"), [HunkState::Proposed]);
    assert_eq!(d.app.inserts().len(), 1);
    // A failure isn't asked again this session.
    d.app.on_app_event(AppEvent::Dive(DiveEvent::Failed {
        id: "u1".into(),
        fingerprint: "fp1".into(),
        kind: DiveKind::Improve,
        error: "no".into(),
    }));
    assert!(d.app.improve_failed.contains("u1"));
    assert!(dives(&d.app.choose_mode(Mode::Improve)).is_empty());
    assert_eq!(d.app.improve_progress(), (2, 3));
}

// ---- The list -------------------------------------------------------------

#[test]
fn the_list_has_a_box_per_kind_with_the_inserts_and_their_state() {
    let d = listed();
    let s = d.render(100, 30);
    assert!(s.contains("─ Fills a gap · 1 insert ─"), "{s}");
    assert!(s.contains("─ Makes it stronger · 1 insert ─"), "{s}");
    assert!(s.contains("─ Polish · 1 insert ─"), "{s}");
    let at = |text: &str| s.find(text).unwrap_or_else(|| panic!("{text}:\n{s}"));
    assert!(at("Fills a gap") < at("Makes it stronger"));
    assert!(at("Makes it stronger") < at("Polish ·"));
    // A row: the bar on the first, `+`, the file, the unit, why, the state.
    let first = s
        .lines()
        .find(|l| l.contains("cast the id"))
        .unwrap_or_else(|| panic!("{s}"));
    assert!(first.contains("▌ + a.rs  s0"), "{first}");
    assert!(first.trim_end().ends_with("proposed │"), "{first}");
    let second = s.lines().find(|l| l.contains("empty cart")).unwrap();
    assert!(second.contains("  + b.rs  s1"), "{second}");
    // Under the boxes, the unit with nothing to add.
    assert!(s.contains("1 unit has nothing to add"), "{s}");
    // The bar counts the inserts.
    assert!(s.lines().last().unwrap().contains("3 proposed"), "{s}");
    // The order the cursor goes through.
    assert_eq!(
        d.app
            .inserts()
            .iter()
            .map(|i| (i.unit, i.hunk, i.kind))
            .collect::<Vec<_>>(),
        [
            (0, 0, InsertKind::Gap),
            (1, 0, InsertKind::Stronger),
            (0, 1, InsertKind::Polish)
        ]
    );
}

#[test]
fn the_cursor_moves_over_the_inserts_and_the_queues_cursor_follows() {
    let mut d = listed();
    assert_eq!(d.app.isel, 0);
    assert_eq!(d.selected().as_deref(), Some("u0"));
    d.keys("j");
    assert_eq!(d.app.isel, 1);
    assert_eq!(d.selected().as_deref(), Some("u1"));
    d.keys("j");
    assert_eq!(d.app.isel, 2);
    assert_eq!(d.selected().as_deref(), Some("u0"));
    d.keys("jjj");
    assert_eq!(d.app.isel, 2);
    d.keys("gg");
    assert_eq!(d.app.isel, 0);
    d.keys("G");
    assert_eq!(d.app.isel, 2);
    // `n` goes to the next insert still proposed.
    d.keys("gg");
    d.app
        .review
        .proposal_mut("u1", DiveKind::Improve)
        .unwrap()
        .hunks[0]
        .state = HunkState::Applied;
    d.keys("n");
    assert_eq!(d.app.isel, 2);
    d.keys("N");
    assert_eq!(d.app.isel, 0);
    // A click on a row.
    let s = d.render(100, 30);
    assert!(s.contains("▌"), "{s}");
    let targets = d.app.targets.borrow().clone();
    assert!(
        targets.iter().any(|(_, t)| *t == Target::Insert(1)),
        "{targets:?}"
    );
    d.app.select_insert(1);
    assert_eq!(d.app.isel, 1);
    let s = d.render(100, 30);
    let row = s.lines().find(|l| l.contains("empty cart")).unwrap();
    assert!(row.contains("▌"), "{row}");
}

#[test]
fn a_marks_an_insert_to_add_x_leaves_it_out_and_either_key_takes_it_back() {
    let mut d = listed();
    // `a`: to add, and on to the next insert still proposed.
    assert_eq!(d.keys("a"), vec![Effect::Save]);
    assert_eq!(states(&d, "u0"), [HunkState::Picked, HunkState::Proposed]);
    assert!(
        d.status().starts_with("To add: :w puts 1 insert in"),
        "{}",
        d.status()
    );
    assert_eq!(d.app.isel, 1);
    let s = d.render(100, 30);
    let row = s.lines().find(|l| l.contains("cast the id")).unwrap();
    assert!(row.trim_end().ends_with("to add │"), "{row}");
    let bar = s.lines().last().unwrap();
    assert!(bar.contains("1 to add · 2 proposed"), "{bar}");
    assert!(bar.contains(":w puts them in"), "{bar}");
    // `a` on one to add takes it back; the cursor stays.
    d.keys("k");
    d.keys("a");
    assert_eq!(states(&d, "u0"), [HunkState::Proposed, HunkState::Proposed]);
    assert_eq!(d.app.isel, 0);
    assert_eq!(d.status(), "Taken back: proposed again.");
    // `x` leaves out, and on to the next; `x` again takes it back.
    d.keys("j");
    assert_eq!(d.keys("x"), vec![Effect::Save]);
    assert_eq!(states(&d, "u1"), [HunkState::LeftOut]);
    assert_eq!(d.app.isel, 2);
    d.keys("k");
    let s = d.render(100, 30);
    let row = s.lines().find(|l| l.contains("empty cart")).unwrap();
    assert!(row.trim_end().ends_with("left out │"), "{row}");
    assert!(
        s.lines()
            .last()
            .unwrap()
            .contains("2 proposed · 1 left out"),
        "{s}"
    );
    d.keys("x");
    assert_eq!(states(&d, "u1"), [HunkState::Proposed]);
    // `a` on one left out marks it to add; `x` on one to add leaves it out.
    d.keys("x");
    d.keys("k");
    d.keys("a");
    assert_eq!(states(&d, "u1"), [HunkState::Picked]);
    d.keys("k");
    d.keys("x");
    assert_eq!(states(&d, "u1"), [HunkState::LeftOut]);
    // `P`: every open insert of the unit, left out ones too.
    d.keys("G");
    d.keys("P");
    assert_eq!(states(&d, "u0"), [HunkState::Picked, HunkState::Picked]);
    assert!(
        d.status().starts_with("2 inserts of this unit to add"),
        "{}",
        d.status()
    );
    // Nothing was written, and nothing asked.
    assert!(
        !d.effects.iter().any(|e| matches!(
            e,
            Effect::ApplyHunks { .. } | Effect::ApplyInserts { .. } | Effect::Rerun
        )),
        "{:?}",
        d.effects
    );
    assert_eq!(d.overlay(), &Overlay::None);
}

#[test]
fn w_puts_the_inserts_to_add_in_at_once_then_asks_whether_to_check_again() {
    let mut d = listed();
    // Nothing to add yet: the sheet has no such step.
    d.app.press(Action::FinishOpen, None);
    assert!(matches!(d.overlay(), Overlay::Finish { .. }));
    assert!(!d.app.finish_steps().contains(&Step::PutInserts));
    d.keys("<esc>");
    d.keys("a");
    d.keys("a");
    assert_eq!(
        d.app.picks(),
        vec![("u0".to_string(), vec![0]), ("u1".to_string(), vec![0])]
    );
    d.app.press(Action::FinishOpen, None);
    assert_eq!(d.app.finish_steps()[0], Step::PutInserts);
    assert_eq!(d.app.step_label(Step::PutInserts), "put 2 inserts in");
    let s = d.render(100, 40);
    assert!(s.contains("put 2 inserts in"), "{s}");
    assert!(
        s.contains("Writes the 2 inserts marked to add into 2 files"),
        "{s}"
    );
    // ⏎ writes them at once, without a word about the pending units.
    let e = d.keys("<cr>");
    assert_eq!(
        e,
        vec![Effect::ApplyInserts {
            picks: vec![("u0".into(), vec![0]), ("u1".into(), vec![0])],
        }]
    );
    assert_eq!(d.overlay(), &Overlay::None);
    assert_eq!(d.status(), "Putting 2 inserts in…");
    // Written: put in, and teer asks whether to check again; no re-run on
    // its own.
    let e = d.app.on_app_event(AppEvent::InsertsApplied {
        results: vec![
            ("u0".into(), vec![0], Ok(vec![PathBuf::from("a.rs")])),
            ("u1".into(), vec![0], Ok(vec![PathBuf::from("b.rs")])),
        ],
    });
    assert_eq!(e, vec![Effect::Save]);
    assert_eq!(states(&d, "u0"), [HunkState::Applied, HunkState::Proposed]);
    assert_eq!(states(&d, "u1"), [HunkState::Applied]);
    assert!(
        matches!(d.overlay(), Overlay::ConfirmCheck { .. }),
        "{:?}",
        d.overlay()
    );
    let s = d.render(100, 30);
    assert!(
        s.contains("Put 2 inserts into 2 files. Check the change again?"),
        "{s}"
    );
    assert!(s.lines().last().unwrap().contains("2 put in"), "{s}");
    // No: nothing more happens; `r` re-runs later.
    assert!(d.keys("n").is_empty());
    assert_eq!(d.overlay(), &Overlay::None);
    assert!(
        d.status().starts_with("Not checked again: ") && d.status().contains("re-runs"),
        "{}",
        d.status()
    );
    // Yes: the review runs again.
    d.app.overlay = Overlay::ConfirmCheck {
        question: "again?".into(),
    };
    assert_eq!(d.keys("y"), vec![Effect::Rerun]);
    // A unit whose inserts weren't written keeps them to add, and the
    // status says why; nothing is asked then.
    d.keys("G");
    d.keys("a");
    let e = d.app.on_app_event(AppEvent::InsertsApplied {
        results: vec![(
            "u0".into(),
            vec![1],
            Err("a.rs changed since the change was proposed".into()),
        )],
    });
    assert!(e.is_empty());
    assert_eq!(states(&d, "u0")[1], HunkState::Picked);
    assert_eq!(d.overlay(), &Overlay::None);
    assert_eq!(
        d.status(),
        "Nothing put in. Not put in: s0: a.rs changed since the change was proposed."
    );
}

#[test]
fn on_a_pull_request_the_inserts_to_add_go_as_suggestions_with_the_review() {
    let mut d = listed();
    d.app.review.pr = Some(PrInfo::default());
    d.keys("a");
    d.app.press(Action::FinishOpen, None);
    assert!(!d.app.finish_steps().contains(&Step::PutInserts));
    d.keys("<esc>");
    let entries = teer::approve::collect_suggestions(&d.app.review, &d.app.order);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].id, "u0");
    assert_eq!(entries[0].why, "cast the id before it is used");
}

#[test]
fn f_flags_the_unit_and_l_looks_closer_where_p_and_x_work_too() {
    let mut d = listed();
    d.keys("G");
    d.keys("a");
    assert_eq!(states(&d, "u0"), [HunkState::Proposed, HunkState::Picked]);
    assert_eq!(d.app.isel, 2);
    // `f` flags the insert's unit: its inserts to add go with the notes.
    assert_eq!(d.keys("f"), vec![Effect::Save]);
    assert_eq!(
        d.app.review.unit("u0").unwrap().decision,
        Decision::Investigate
    );
    assert!(d.status().starts_with("Flagged s0"), "{}", d.status());
    let notes = teer::notes::proposed(d.app.review.unit("u0").unwrap());
    assert!(notes.contains("**Inserts**"), "{notes}");
    assert!(notes.contains("to add: say what the rule is"), "{notes}");
    assert!(!notes.contains("cast the id"), "not to add:\n{notes}");
    // `l` looks closer: the unit view at the insert, under "Inserts".
    d.keys("l");
    assert_eq!(d.screen(), Screen::Unit);
    assert_eq!(d.app.unit.id, "u0");
    assert_eq!(d.app.unit.fix, Some(1));
    let s = d.render(100, 40);
    assert!(s.contains(" Inserts"), "{s}");
    assert!(s.contains("polish · a.rs:11-13"), "{s}");
    assert!(
        s.contains("p take back · P all to add · x leave out"),
        "{s}"
    );
    // `p` there takes it back, `x` leaves it out.
    d.keys("p");
    assert_eq!(states(&d, "u0"), [HunkState::Proposed, HunkState::Proposed]);
    d.keys("x");
    assert_eq!(states(&d, "u0"), [HunkState::Proposed, HunkState::LeftOut]);
    let s = d.render(100, 40);
    assert!(s.contains("p to add · P all to add · x take back"), "{s}");
    d.keys("h");
    assert_eq!(d.screen(), Screen::Queue);
    assert_eq!(d.app.isel, 2);
}

#[test]
fn the_preview_shows_the_summary_the_check_and_the_hunk_with_its_keys() {
    let d = listed();
    let s = d.render(100, 40);
    assert!(s.contains("╭ inserts ─"), "{s}");
    assert!(s.contains("Adds what the change lacks."), "{s}");
    assert!(s.contains("cast the id before it is used"), "{s}");
    assert!(s.contains("✓ check agrees"), "{s}");
    assert!(s.contains("+ L5 = int(l5)"), "{s}");
    assert!(s.contains("fills a gap · a.rs:4-6"), "{s}");
    assert!(s.contains("p to add · P all to add · x leave out"), "{s}");
    // Beside the list on a wide terminal.
    let s = d.render(160, 40);
    let row = s.lines().find(|l| l.contains("cast the id")).unwrap();
    assert!(
        row.contains("Fills a gap") || row.contains("why") || row.contains("│"),
        "{row}"
    );
}

#[test]
fn another_mode_hides_the_list_and_keeps_the_inserts() {
    let mut d = listed();
    d.app.set_mode(Mode::Review);
    let s = d.render(100, 30);
    assert!(!s.contains("Fills a gap"), "{s}");
    assert!(s.contains("· 3 units"), "{s}");
    assert_eq!(states(&d, "u0").len(), 2);
    // The preview says the inserts are there, in improve mode only.
    assert!(!s.contains("insert"), "{s}");
    d.app.set_mode(Mode::Improve);
    let s = d.render(100, 30);
    assert!(s.contains("Fills a gap"), "{s}");
    assert_eq!(d.app.isel, 0);
}

#[test]
fn with_no_agent_or_nothing_to_add_the_list_says_so() {
    let mut d = Driver::new(3);
    d.app.set_mode(Mode::Improve);
    let s = d.render(100, 20);
    assert!(s.contains("No agent: improve mode needs one"), "{s}");
    let mut d = driver();
    for id in ["u0", "u1", "u2"] {
        answered(&mut d, id, inserts(Vec::new()));
    }
    d.app.set_mode(Mode::Improve);
    let s = d.render(100, 20);
    assert!(
        s.contains("Nothing to add: the agent found no insert for 3 units"),
        "{s}"
    );
    assert!(s.lines().last().unwrap().contains("No insert yet"), "{s}");
}
