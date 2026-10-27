//! The Tests tile end to end with the mock agent: `c` searches the test files
//! for tests that reach a flagged unit and lists them at once, without an
//! agent; `a` then has the agent read them: which cover it and what dangerous
//! cases none does. Tests whose lines don't exist or never name the unit are
//! dropped, gaps must cite the unit; the tile lists both, and a later
//! Behavior probe is told the gaps.

mod common;

use std::sync::Arc;

use common::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};
use teer::agent::mock::MockAdapter;
use teer::config::{Config, KeyMap};
use teer::review::dive::{DiveEvent, TestsJob, interpret_tests, search_tests};
use teer::review::model::{Decision, DiveKind, Risk, SessionRef};
use teer::review::prompts::{ProbeSetting, behavior_prompt};
use teer::review::{BuildOptions, build_review, test_search, visual};
use teer::tui::app::{App, AppEvent, Effect, Pane, Screen};
use teer::tui::ui;

const TEST_FILE: &str = "from checkout import charge_order\n\n\ndef test_charges_once():\n    assert charge_order(order()) == 'paid'\n\n\ndef test_tax():\n    assert tax(2) == 0.4\n";

/// The first new line of `charge_order` as the prompt shows it.
fn unit_line(prompt: &str) -> u32 {
    prompt
        .lines()
        .find_map(|l| l.strip_prefix("new lines: "))
        .and_then(|r| r.split('-').next())
        .and_then(|n| n.parse().ok())
        .unwrap()
}

fn responder(prompt: &str, schema: &Value) -> anyhow::Result<Value> {
    assert!(prompt.starts_with("DIVE:Tests"), "{prompt}");
    assert!(schema["properties"]["gaps"].is_object());
    // teer searched first: the test that calls the unit is listed with its
    // code; the one that doesn't is not.
    assert!(
        prompt.contains(
            "1. test_charges_once (tests/test_checkout.py:4-5): calls this unit at line 5\n"
        ),
        "{prompt}"
    );
    assert!(
        prompt.contains("      5      assert charge_order(order()) == 'paid'\n"),
        "{prompt}"
    );
    assert!(!prompt.contains("2. test_tax"), "{prompt}");
    assert!(prompt.contains("Don't search for more tests"), "{prompt}");
    let line = unit_line(prompt);
    let at = |s: u32| json!([{"file": "checkout.py", "start": s, "end": s}]);
    Ok(json!({
        "text": "Searched tests/ for charge_order.",
        "covered": [
            // Only part of a found test: kept under teer's name and lines.
            {"name": "charges once", "file": "tests/test_checkout.py", "start": 5, "end": 5,
             "checks": "A paid order returns 'paid'."},
            // Never names the unit, its file or a caller.
            {"name": "test_tax", "file": "tests/test_checkout.py", "start": 8, "end": 9, "checks": "x"},
            // Lines that don't exist.
            {"name": "test_ghost", "file": "tests/test_checkout.py", "start": 40, "end": 44, "checks": "x"},
            {"name": "test_outside", "file": "../secrets.py", "start": 1, "end": 1, "checks": "x"}
        ],
        "gaps": [
            {"case": "an unsupported currency", "danger": "Raises before paying; callers don't catch it.",
             "risk": "medium", "evidence": at(line)},
            {"case": "the payment times out", "danger": "No receipt and no retry: the order looks unpaid.",
             "risk": "high", "evidence": at(line + 2)},
            {"case": "made up", "danger": "x", "risk": "high", "evidence": at(999)}
        ]
    }))
}

fn key(a: &mut App, c: char) -> Vec<Effect> {
    a.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )))
}

fn text(app: &App) -> String {
    let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..40)
        .map(|y| (0..120).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
        .collect()
}

#[tokio::test]
async fn c_lists_the_found_tests_and_a_has_the_agent_read_them() {
    isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD)
        .write("tests/test_checkout.py", TEST_FILE)
        .commit("init");
    repo.write("checkout.py", PY_NEW);
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let session = SessionRef::fresh("mock", repo.path());
    review.agent_session = Some(session.clone());
    for u in review
        .units
        .iter_mut()
        .filter(|u| u.symbol == "charge_order")
    {
        u.decision = Decision::Investigate;
    }
    let mut app = App::new(review, Config::default(), KeyMap::default());
    key(&mut app, 'd');
    assert_eq!(app.screen, Screen::Flagged);
    // Before anything was found, `a` has nothing to read.
    assert!(key(&mut app, 'a').is_empty());
    assert!(
        app.status.contains("searches the tests first"),
        "{}",
        app.status
    );
    let effects = key(&mut app, 'c');
    assert_eq!(app.flagged.pane, Pane::Answer(DiveKind::Tests));
    let [Effect::Dive { unit_id, kind, .. }] = &effects[..] else {
        panic!("{effects:?}")
    };
    assert_eq!(*kind, DiveKind::Tests);
    assert!(text(&app).contains("searching the test files"));

    // teer searches by itself, the way the runtime does: no agent.
    let unit = app.review.unit(unit_id).unwrap().clone();
    let callers = test_search::callers(&app.review, &unit);
    app.on_app_event(AppEvent::Dive(search_tests(&repo.path(), &unit, &callers)));
    key(&mut app, 'z');
    let screen = text(&app);
    assert!(screen.contains("1 test reaches it"), "{screen}");
    assert!(screen.contains("· test_charges_once"), "{screen}");
    assert!(
        screen.contains("calls it · line 5 · tests/test_checkout.py:4-5"),
        "{screen}"
    );
    assert!(!screen.contains("test_tax"), "{screen}");
    assert!(
        screen.contains("a  the agent reads them: which really cover it"),
        "{screen}"
    );
    assert!(screen.contains("found by teer "), "{screen}");
    key(&mut app, 'z');

    // `a` has the agent read them.
    let effects = key(&mut app, 'a');
    let [Effect::InterpretTests { unit_id }] = &effects[..] else {
        panic!("{effects:?}")
    };
    assert!(text(&app).contains("asking the agent"));
    let unit = app.review.unit(unit_id).unwrap().clone();
    let job = TestsJob {
        file: app.review.file_of(&unit).unwrap().clone(),
        target: visual::target(&app.review, &unit),
        search: unit.dive.results[&DiveKind::Tests]
            .coverage
            .as_ref()
            .and_then(|c| c.search.clone())
            .unwrap(),
        root: repo.path(),
        unit,
    };
    let m = MockAdapter::new(responder);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    interpret_tests(Arc::new(m.clone()), session, job, tx).await;
    let mut said = Vec::new();
    let ev = loop {
        match rx.recv().await.unwrap() {
            DiveEvent::Progress { text, .. } => said.push(text),
            ev => break ev,
        }
    };
    assert_eq!(said, ["the agent reads the test teer found…"]);
    assert!(matches!(ev, DiveEvent::Ready { .. }), "{ev:?}");
    app.on_app_event(AppEvent::Dive(ev));
    assert_eq!(*m.web_calls.lock().unwrap(), [false], "never the web");

    let u = app.flagged_unit().unwrap();
    let c = u.dive.results[&DiveKind::Tests].coverage.as_ref().unwrap();
    let names: Vec<&str> = c.covered.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["test_charges_once"]);
    let cases: Vec<&str> = c.gaps.iter().map(|g| g.case.as_str()).collect();
    assert_eq!(
        cases,
        ["the payment times out", "an unsupported currency"],
        "most dangerous first, the made-up one dropped"
    );
    assert_eq!(c.gaps[0].risk, Risk::High);

    assert!(text(&app).contains("╭ c Tests ✓"), "{}", text(&app));
    // Zoomed, the tile is as wide as the screen.
    key(&mut app, 'z');
    let screen = text(&app);
    assert!(screen.contains("1 test covers it"), "{screen}");
    assert!(
        screen.contains("The agent read the 1 test teer found."),
        "{screen}"
    );
    assert!(
        screen.contains("found by teer, read by the agent"),
        "{screen}"
    );
    assert!(screen.contains("✓ test_charges_once"), "{screen}");
    assert!(
        screen.contains("A paid order returns 'paid'. · tests/test_checkout.py:4-5"),
        "{screen}"
    );
    assert!(
        screen.contains("Dangerous cases no test covers"),
        "{screen}"
    );
    assert!(
        screen.contains("! the payment times out (high risk)"),
        "{screen}"
    );
    assert!(
        screen.contains("Searched tests/ for charge_order."),
        "{screen}"
    );

    // A Behavior probe asked next is told to run those cases.
    let u = app.flagged_unit().unwrap();
    let file = app.review.file_of(u).unwrap();
    let p = behavior_prompt(u, file, &ProbeSetting::default(), None);
    assert!(p.contains("Dangerous cases no test covers"), "{p}");
    assert!(p.contains("- the payment times out (High risk)"), "{p}");
    assert!(
        p.contains("- test_charges_once (tests/test_checkout.py:4-5)"),
        "{p}"
    );
}
