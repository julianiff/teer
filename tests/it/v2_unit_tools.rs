//! The unit view's tools end to end with the mock agent, on a real repo.
//! The diagram (`␣v`): asked once and never with the web, the picture
//! validated and saved as an HTML page, the panel showing the claim and the
//! edges (or the picture, where the terminal draws images), `gx` opening the
//! page, a failure, rebound keys, a reload. The tests (`␣t`): teer searches
//! the test files by itself and lists what reaches the unit at once, then
//! the agent reads them on its own: which cover it and what dangerous cases
//! none does, which a later old-against-new probe is told.

use crate::common;

use std::path::Path;
use std::sync::Arc;

use common::v2::press;
use common::*;
use serde_json::{Value, json};
use teer::agent::mock::{MockAdapter, dive_responder};
use teer::config::Config;
use teer::keymap::Keymap;
use teer::review::dive::{DiveEvent, TestsJob, interpret_tests, run_dive, search_tests};
use teer::review::model::{DiveKind, Review, Risk, SessionRef};
use teer::review::{BuildOptions, build_review, test_search};
use teer::session::app::{AppEvent, Effect, Screen};
use teer::session::unit::Panel;
use teer::tui::app::App;

struct Fixture {
    app: App,
    adapter: MockAdapter,
    repo: FixtureRepo,
}

fn session(cwd: &Path) -> SessionRef {
    SessionRef {
        agent: "mock".into(),
        id: "s1".into(),
        cwd: cwd.to_path_buf(),
    }
}

/// Open the unit view (`l`) on unit `id`, answering what opening it asks.
async fn open(f: &mut Fixture, id: &str) {
    f.app.sel = f
        .app
        .order
        .iter()
        .position(|&i| f.app.review.units[i].id == id)
        .unwrap();
    let effects = press(&mut f.app, "l");
    assert_eq!((f.app.screen, f.app.unit.id.as_str()), (Screen::Unit, id));
    let asked = run(f, &effects, MockAdapter::new(dive_responder)).await;
    deliver(&mut f.app, asked);
}

/// Run the Dive effects with `adapter` the way the runtime does (Visualize
/// gets its target); the answers, not delivered yet.
async fn run(f: &Fixture, effects: &[Effect], adapter: MockAdapter) -> Vec<DiveEvent> {
    let adapter: Arc<dyn teer::agent::AgentAdapter> = Arc::new(adapter);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    for e in effects {
        let Effect::Dive { unit_id, kind, web } = e else {
            continue;
        };
        let unit = f.app.review.unit(unit_id).unwrap().clone();
        let file = f.app.review.file_of(&unit).unwrap().clone();
        run_dive(
            adapter.clone(),
            session(&f.repo.path()),
            unit,
            file,
            *kind,
            *web,
            tx.clone(),
        )
        .await;
    }
    drop(tx);
    let mut out = Vec::new();
    while let Some(ev) = rx.recv().await {
        out.push(ev);
    }
    out
}

fn deliver(app: &mut App, events: Vec<DiveEvent>) {
    for ev in events {
        app.on_app_event(AppEvent::Dive(ev));
    }
}

/// Type `keys` in Neovim notation (`<space>x`, `gx`).
fn keys(a: &mut App, keys: &str) -> Vec<Effect> {
    press(a, keys)
}

fn text_at(app: &App, w: u16, h: u16) -> String {
    common::v2::screen(app, w, h)
}

/// The tool asked for in `effects`, when there is one: (kind, web).
fn asked(effects: &[Effect]) -> Vec<(DiveKind, bool)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Dive { kind, web, .. } => Some((*kind, *web)),
            _ => None,
        })
        .collect()
}

// ---- The diagram ---------------------------------------------------------------

// ---- The tests -----------------------------------------------------------------

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

/// The agent reading the tests: tests whose lines don't exist or never
/// name the unit are dropped, gaps must cite the unit.
fn tests_responder(prompt: &str, schema: &Value) -> anyhow::Result<Value> {
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

#[tokio::test]
async fn leader_t_lists_the_found_tests_and_the_agent_reads_them_when_asked() {
    isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD)
        .write("tests/test_checkout.py", TEST_FILE)
        .commit("init");
    repo.write("checkout.py", PY_NEW);
    let mut review: Review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let session = SessionRef::fresh("mock", repo.path());
    review.agent_session = Some(session.clone());
    let charge = review
        .units
        .iter()
        .find(|u| u.symbol == "charge_order")
        .unwrap()
        .id
        .clone();
    let mut app = App::new(review, Config::default(), Keymap::default());
    app.has_agent = true;
    let mut f = Fixture {
        app,
        adapter: MockAdapter::new(tests_responder),
        repo,
    };
    open(&mut f, &charge).await;
    let app = &mut f.app;
    let text = |app: &App| text_at(app, 160, 50);

    let effects = keys(app, "<space>t");
    assert_eq!(app.unit.panel, Some(Panel::Tool(DiveKind::Tests)));
    assert_eq!(asked(&effects), [(DiveKind::Tests, false)]);
    // Zoomed: the tile alone, its lines whole.
    keys(app, "zz");
    assert!(text(app).contains("searching the test files"));

    // teer searches by itself, the way the runtime does: no agent. Then the
    // tile asks whether the agent reads what it found.
    let unit = app.review.unit(&charge).unwrap().clone();
    let callers = test_search::callers(&app.review, &unit);
    let effects = app.on_app_event(AppEvent::Dive(search_tests(
        &f.repo.path(),
        &unit,
        &callers,
    )));
    let read = Effect::InterpretTests {
        unit_id: charge.clone(),
    };
    assert!(!effects.contains(&read), "{effects:?}");
    let screen = text(app);
    assert!(screen.contains("the agent reads them"), "{screen}");
    // Yes.
    let effects = keys(app, "<cr>");
    assert!(effects.contains(&read), "{effects:?}");
    let screen = text(app);
    assert!(screen.contains("1 test reaches it"), "{screen}");
    assert!(screen.contains("· test_charges_once"), "{screen}");
    assert!(
        screen.contains("calls it · line 5 · tests/test_checkout.py:4-5"),
        "{screen}"
    );
    assert!(!screen.contains("test_tax"), "{screen}");
    assert!(screen.contains("asking the agent"), "{screen}");

    let unit = app.review.unit(&charge).unwrap().clone();
    let job = TestsJob {
        file: app.review.file_of(&unit).unwrap().clone(),
        regions: teer::review::scope::scope(&app.review, &unit),
        search: unit.dive.results[&DiveKind::Tests]
            .coverage
            .as_ref()
            .and_then(|c| c.search.clone())
            .unwrap(),
        root: f.repo.path(),
        unit,
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    interpret_tests(Arc::new(f.adapter.clone()), session, job, tx).await;
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
    assert_eq!(
        *f.adapter.web_calls.lock().unwrap(),
        [false],
        "never the web"
    );

    let u = app.review.unit(&charge).unwrap();
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

    let screen = text(app);
    assert!(screen.contains("1 test covers it"), "{screen}");
    assert!(
        screen.contains("The agent read the 1 test teer found."),
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
}
