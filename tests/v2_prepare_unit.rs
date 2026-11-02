//! Phase 3, the prepare pipeline's output, on synthetic reviews (no repo,
//! no agent): units with no behavior change accepted for you (`za`, `u`,
//! `auto_accept`), the marks written as reasons, the order, and the JSON.

mod common;

use common::v2::{Driver, review, unit};
use teer::config::{AutoAccept, Config};
use teer::keymap::Keymap;
use teer::review::attention::Level;
use teer::review::model::{
    AcceptedBy, Change, Cost, Decision, LineRef, Marker, Prep, Range, Risk, SecondOpinion, Side,
    Unit, UnitStatus, Unmentioned, Verdict,
};
use teer::tui::app::App;

/// Unit `u{i}` in `file` with no behavior change.
fn same(i: usize, file: &str) -> Unit {
    let mut u = unit(i, file);
    u.status = UnitStatus::NoBehaviorChange;
    u.nbc_reason = Some("formatting or comments".into());
    u
}

fn at(line: u32) -> Vec<LineRef> {
    vec![LineRef {
        side: Side::New,
        line,
    }]
}

/// A second opinion that disputes `statement`.
fn disputing(statement: &str) -> SecondOpinion {
    SecondOpinion {
        risk: Risk::Low,
        verdicts: vec![Verdict {
            statement: statement.into(),
            agree: false,
            why: "the retry wasn't removed, it moved to client.ts".into(),
            evidence: at(2),
        }],
        missed: Vec::new(),
    }
}

/// `units` under `cfg`, default keys, in a temp data dir.
fn driver(units: Vec<Unit>, cfg: Config) -> Driver {
    common::isolate_env();
    Driver {
        app: App::new(review(units), cfg, Keymap::default()),
        effects: Vec::new(),
    }
}

fn listed(d: &Driver) -> Vec<String> {
    d.app
        .order
        .iter()
        .map(|&i| d.app.review.units[i].id.clone())
        .collect()
}

fn unit_of<'a>(d: &'a Driver, id: &str) -> &'a Unit {
    d.app.review.unit(id).unwrap()
}

#[test]
fn no_behavior_change_is_accepted_for_you_hidden_and_counted() {
    let d = driver(
        vec![unit(0, "a.rs"), same(1, "a.rs"), same(2, "b.rs")],
        Config::default(),
    );
    for id in ["u1", "u2"] {
        assert_eq!(unit_of(&d, id).decision, Decision::Accepted, "{id}");
        assert_eq!(unit_of(&d, id).accepted_by, Some(AcceptedBy::Auto), "{id}");
    }
    assert_eq!(unit_of(&d, "u0").accepted_by, None);
    assert_eq!(listed(&d), ["u0"]);
    assert_eq!(d.app.auto_accepted(), 2);
    // They count as accepted for finishing.
    assert_eq!(d.app.decision_counts(), (2, 0, 1));
    let screen = d.render(80, 20);
    assert!(
        screen.contains("2 units accepted for you, za shows them"),
        "{screen}"
    );
    assert!(!screen.contains("No behavior change"), "{screen}");
}

#[test]
fn accepting_the_rest_finishes_the_review() {
    let mut d = driver(vec![unit(0, "a.rs"), same(1, "a.rs")], Config::default());
    assert!(!d.app.all_reviewed());
    d.keys("a");
    assert!(d.app.all_reviewed());
    assert_eq!(
        unit_of(&d, "u0").accepted_by,
        Some(AcceptedBy::Reviewer),
        "the reviewer accepted it"
    );
}

#[test]
fn za_lists_them_and_u_takes_one_back() {
    let mut d = driver(
        vec![unit(0, "a.rs"), same(1, "a.rs"), same(2, "b.rs")],
        Config::default(),
    );
    d.keys("za");
    assert!(d.app.show_auto);
    // Last, in a box of their own when their file has nothing else.
    assert_eq!(listed(&d), ["u0", "u1", "u2"]);
    let screen = d.render(80, 24);
    assert!(screen.contains("Accepted for you · 1 file"), "{screen}");
    assert!(
        screen.contains("2 units accepted for you, za hides them"),
        "{screen}"
    );

    d.select_id("u2");
    d.keys("u");
    assert_eq!(d.decision_of("u2"), Decision::Pending);
    assert_eq!(unit_of(&d, "u2").accepted_by, None);
    assert_eq!(
        d.selected().as_deref(),
        Some("u2"),
        "the cursor stays on it"
    );
    assert_eq!(d.app.auto_accepted(), 1);
    // Waiting for the reviewer, it shows as routine.
    assert_eq!(d.app.shown_level(2), Level::Routine);
    assert_eq!(d.app.decision_counts(), (1, 0, 2));
    // Hidden again, it stays listed: it is pending.
    d.keys("za");
    assert_eq!(listed(&d), ["u0", "u2"]);

    // `u` takes that step back: accepted for you again; `ctrl-r` redoes it.
    d.keys("u");
    assert_eq!(unit_of(&d, "u2").accepted_by, Some(AcceptedBy::Auto));
    assert_eq!(listed(&d), ["u0"]);
    d.keys("<c-r>");
    assert_eq!(d.decision_of("u2"), Decision::Pending);
}

#[test]
fn a_unit_taken_back_stays_pending_after_a_restart() {
    let mut d = driver(vec![unit(0, "a.rs"), same(1, "a.rs")], Config::default());
    d.keys("za");
    d.select_id("u1");
    d.keys("u");
    // What is saved, loaded again (as `store::load` and `merge_previous` do).
    let saved = serde_json::to_string(&d.app.review).unwrap();
    let back: teer::review::model::Review = serde_json::from_str(&saved).unwrap();
    let mut fresh = review(vec![unit(0, "a.rs"), same(1, "a.rs")]);
    teer::review::store::merge_previous(&mut fresh, &back);
    let again = App::new(fresh, Config::default(), Keymap::default());
    let u1 = again.review.unit("u1").unwrap();
    assert_eq!((u1.decision, u1.accepted_by), (Decision::Pending, None));
}

#[test]
fn auto_accept_none_keeps_units_pending_and_listed() {
    let cfg = Config {
        auto_accept: AutoAccept::None,
        ..Config::default()
    };
    let d = driver(vec![unit(0, "a.rs"), same(1, "a.rs")], cfg);
    assert_eq!(d.decision_of("u1"), Decision::Pending);
    assert_eq!(unit_of(&d, "u1").accepted_by, None);
    assert_eq!(listed(&d), ["u0", "u1"]);
    assert_eq!(d.app.shown_level(1), Level::Routine);
    let screen = d.render(80, 20);
    assert!(!screen.contains("accepted for you"), "{screen}");
    assert!(!screen.contains('○'), "{screen}");
}

#[test]
fn a_disputed_unit_with_no_behavior_change_is_routine_and_waits() {
    let mut disputed = same(1, "a.rs");
    disputed.checks.second = Some(disputing("Only reformats."));
    let d = driver(vec![unit(0, "a.rs"), disputed], Config::default());
    assert_eq!(d.decision_of("u1"), Decision::Pending);
    assert_eq!(d.app.assessment("u1").unwrap().level, Level::Routine);
    assert!(listed(&d).contains(&"u1".to_string()));
    assert_eq!(
        unit_of(&d, "u1").reasons,
        [
            "The second model disputes \"Only reformats.\": \"the retry wasn't removed, it moved to client.ts\""
        ]
    );
}

#[test]
fn a_dispute_arriving_later_takes_the_acceptance_back() {
    let mut d = driver(vec![unit(0, "a.rs"), same(1, "a.rs")], Config::default());
    assert!(unit_of(&d, "u1").auto_accepted());
    d.app.review.unit_mut("u1").unwrap().checks.second = Some(disputing("Only reformats."));
    d.app.rebuild_order();
    assert_eq!(d.decision_of("u1"), Decision::Pending);
    assert_eq!(unit_of(&d, "u1").accepted_by, None);
    assert_eq!(listed(&d), ["u0", "u1"]);
}

#[test]
fn marks_become_reasons_and_rows_show_only_the_dispute() {
    let mut u = unit(0, "pay.rs");
    u.prep = Prep::Ready;
    u.changes = vec![Change {
        marker: Marker::Changed,
        statement: "Retries the charge.".into(),
        evidence: at(1),
        risk: Risk::Low,
    }];
    u.checks.cost = Some(Cost {
        why: "Charges the card again on retry.".into(),
        evidence: at(1),
    });
    u.checks.second = Some(disputing("Retries the charge."));
    u.checks.unmentioned = Some(Unmentioned {
        text: "Also stops sending the receipt.".into(),
        evidence: at(1),
    });
    let d = driver(vec![u], Config::default());
    let u = unit_of(&d, "u0");
    assert_eq!(
        u.reasons,
        [
            "Expensive to undo: Charges the card again on retry.",
            "The second model disputes \"Retries the charge.\": \"the retry wasn't removed, it moved to client.ts\"",
            "Not mentioned in the commits or PR description: Also stops sending the receipt.",
        ]
    );
    let rows = teer::tui::slim::build(&d.app, 40);
    let row: String = rows.lines[rows.spans[0].0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        row.contains("◆ s0") && row.trim_end_matches(['│', ' ']).ends_with('⚠'),
        "{row}"
    );
    let screen = d.render(140, 30);
    for glyph in ['$', '≠'] {
        assert!(!screen.contains(glyph), "{glyph}:\n{screen}");
    }
    // The preview says why under its heading.
    assert!(screen.contains("Why this level"), "{screen}");
    assert!(
        screen.contains("· Expensive to undo: Charges the card again on retry."),
        "{screen}"
    );
    assert!(
        screen.contains("· Not mentioned in the commits or PR description"),
        "{screen}"
    );
}

/// Unit `id` in `file` at line `line`, its level set by `level`: ▲ removed
/// while something still calls it, ◆ changed and not described, ●
/// changed and needing no description.
fn at_level(id: &str, file: &str, line: u32, level: Level) -> Unit {
    let mut u = unit(0, file);
    u.id = id.into();
    u.symbol = id.into();
    u.new_range = Some(Range::new(line, line + 1));
    match level {
        Level::Attention => {
            u.status = UnitStatus::Removed;
            u.old_range = u.new_range.take();
            u.callers = vec!["elsewhere::main".into()];
        }
        Level::Look => u.prep = Prep::NotRequested,
        _ => {}
    }
    u
}

#[test]
fn queue_order_is_level_then_file_with_callers_first_and_starts_on_a_pending_attention_unit() {
    let mut caller = at_level("caller", "a.rs", 20, Level::Routine);
    let mut callee = at_level("callee", "a.rs", 5, Level::Routine);
    caller.callees = vec!["callee".into()];
    callee.callers = vec!["caller".into()];
    let mut done = at_level("done", "a.rs", 1, Level::Attention);
    done.decision = Decision::Accepted;
    let units = vec![
        caller,
        callee,
        at_level("look_b", "b.rs", 1, Level::Look),
        done,
        at_level("close_c", "c.rs", 1, Level::Attention),
        at_level("look_a", "a.rs", 30, Level::Look),
        same(9, "a.rs"),
    ];
    let mut d = driver(units, Config::default());
    // Listed by level (`zf`): ▲, ◆, ●, each by path, callers before callees.
    d.keys("zf");
    assert_eq!(
        listed(&d),
        ["done", "close_c", "look_a", "look_b", "caller", "callee"]
    );
    // The review starts on the first pending ▲, whatever the grouping.
    let start = driver(d.app.review.units.clone(), Config::default());
    assert_eq!(start.selected().as_deref(), Some("close_c"));
    // By file, files go by their most attention unit, then by path.
    let d = start;
    assert_eq!(
        listed(&d),
        ["done", "look_a", "caller", "callee", "close_c", "look_b"]
    );
}

#[test]
fn json_has_reasons_and_accepted_by_and_keeps_the_old_fields() {
    let mut cost = unit(0, "a.rs");
    cost.checks.cost = Some(Cost {
        why: "Drops a column.".into(),
        evidence: at(1),
    });
    let mut r = review(vec![cost, same(1, "a.rs")]);
    r.settle(AutoAccept::NoBehaviorChange);
    let json = serde_json::to_value(&r).unwrap();
    let units = json["units"].as_array().unwrap();
    assert_eq!(units[0]["reasons"][0], "Expensive to undo: Drops a column.");
    assert_eq!(units[0]["accepted_by"], serde_json::Value::Null);
    assert_eq!(units[0]["decision"], "Pending");
    assert_eq!(units[1]["accepted_by"], "auto");
    assert_eq!(units[1]["decision"], "Accepted");
    assert_eq!(units[1]["reasons"], serde_json::json!([]));
    for old in [
        "id", "file", "symbol", "status", "checks", "changes", "prep",
    ] {
        assert!(units[0].get(old).is_some(), "{old}");
    }
    // A review saved before these fields loads.
    let mut old = units[0].clone();
    old.as_object_mut().unwrap().remove("reasons");
    old.as_object_mut().unwrap().remove("accepted_by");
    let u: Unit = serde_json::from_value(old).unwrap();
    assert!(u.reasons.is_empty() && u.accepted_by.is_none());
}
