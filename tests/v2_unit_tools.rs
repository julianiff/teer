//! The unit view's tools end to end with the mock agent, on a real repo.
//! The diagram (`␣v`): asked once and never with the web, the picture
//! validated and saved as an HTML page, the panel showing the claim and the
//! edges (or the picture, where the terminal draws images), `gx` opening the
//! page, a failure, rebound keys, a reload. The tests (`␣t`): teer searches
//! the test files by itself and lists what reaches the unit at once, then
//! the agent reads them on its own: which cover it and what dangerous cases
//! none does, which a later old-against-new probe is told.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::v2::press;
use common::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui_image::picker::{Picker, ProtocolType};
use serde_json::{Value, json};
use teer::agent::mock::{MockAdapter, dive_responder};
use teer::approve::Tools;
use teer::config::Config;
use teer::keymap::{KeySpec, Keymap};
use teer::review::dive::{DiveEvent, TestsJob, interpret_tests, run_dive, search_tests};
use teer::review::model::{DiveKind, Review, Risk, SessionRef, Visual};
use teer::review::prompts::{ProbeSetting, behavior_prompt};
use teer::review::{BuildOptions, build_review, store, test_search, visual};
use teer::tui::Runtime;
use teer::tui::app::{App, AppEvent, Effect, Overlay, Screen};
use teer::tui::picture::Pictures;
use teer::tui::ui;
use teer::tui::unit::Panel;

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

/// The Python fixture with an agent, `adapter` to answer the tools.
fn fixture_with(keys: Keymap, adapter: MockAdapter) -> Fixture {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD).commit("init");
    repo.write("checkout.py", PY_NEW);
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    review.agent_session = Some(session(&repo.path()));
    let mut app = App::new(review, Config::default(), keys);
    app.has_agent = true;
    Fixture { app, adapter, repo }
}

/// [`fixture_with`] the default keys and the mock's dive answers, in the
/// unit view on the first unit the agent describes.
async fn fixture() -> Fixture {
    let mut f = fixture_with(Keymap::default(), MockAdapter::new(dive_responder));
    open_first(&mut f).await;
    f
}

/// Open the unit view (`l`) on the first unit the agent describes, its
/// recommendation (and the next unit's) answered by a mock of its own.
async fn open_first(f: &mut Fixture) {
    let first = f
        .app
        .review
        .units
        .iter()
        .find(|u| u.needs_agent())
        .unwrap()
        .id
        .clone();
    open(f, &first).await;
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
        let target = (*kind == DiveKind::Visualize).then(|| visual::target(&f.app.review, &unit));
        run_dive(
            adapter.clone(),
            session(&f.repo.path()),
            unit,
            file,
            *kind,
            *web,
            target,
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

/// Run the effects with the fixture's adapter and deliver the answers,
/// none of them a failure.
async fn answer(f: &mut Fixture, effects: &[Effect]) {
    let events = run(f, effects, f.adapter.clone()).await;
    for ev in &events {
        if let DiveEvent::Failed { error, .. } = ev {
            panic!("{error}");
        }
    }
    deliver(&mut f.app, events);
}

/// Type `keys` in Neovim notation (`<space>v`, `gx`).
fn keys(a: &mut App, keys: &str) -> Vec<Effect> {
    press(a, keys)
}

fn screen(app: &App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    t
}

fn screen_text(app: &App) -> String {
    text_at(app, 120, 40)
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

/// As if the terminal had answered that it speaks the Kitty graphics protocol.
fn kitty() -> Pictures {
    let mut picker = Picker::halfblocks();
    picker.set_protocol_type(ProtocolType::Kitty);
    Pictures::new(picker)
}

/// Kitty draws an image as cells holding this placeholder character.
const PLACEHOLDER: char = '\u{10EEEE}';

fn picture(app: &App) -> Visual {
    app.review.unit(&app.unit.id).unwrap().dive.results[&DiveKind::Visualize]
        .visual
        .clone()
        .unwrap()
}

/// An opener script that appends what it was asked to open to `opened`.
fn tools(dir: &Path) -> Tools {
    let p = dir.join("opener");
    std::fs::write(
        &p,
        "#!/bin/sh\nd=$(dirname \"$0\"); printf '%s\\n' \"$@\" >> \"$d/opened\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    Tools {
        opener: p.into(),
        ..Tools::default()
    }
}

/// The lines the opener got, once it has run `n` times (it runs detached).
async fn opened(dir: &Path, n: usize) -> Vec<String> {
    for _ in 0..100 {
        if let Ok(s) = std::fs::read_to_string(dir.join("opened"))
            && s.lines().count() >= n
        {
            return s.lines().map(String::from).collect();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Vec::new()
}

fn runtime(tools: Tools) -> Runtime {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    Runtime {
        adapter: None,
        tx,
        build: BuildOptions::default(),
        tools,
    }
}

// ---- The diagram ---------------------------------------------------------------

#[tokio::test]
async fn leader_v_asks_once_without_the_web_and_the_panel_shows_the_picture() {
    let mut f = fixture().await;
    let effects = keys(&mut f.app, "<space>v");
    assert_eq!(f.app.unit.panel, Some(Panel::Tool(DiveKind::Visualize)));
    assert_eq!(asked(&effects), [(DiveKind::Visualize, false)]);
    assert_eq!(f.app.overlay, Overlay::None, "no web, so no confirmation");
    let text = screen_text(&f.app);
    assert!(text.contains("asking the agent"), "{text}");
    assert!(!text.contains("mock picture"), "{text}");
    // Closed and opened again while in flight: nothing new.
    assert!(keys(&mut f.app, "<space>v<space>v").is_empty());

    let events = run(&f, &effects, f.adapter.clone()).await;
    assert_eq!(events.len(), 1);
    // Nothing shows until the answer is delivered.
    assert!(!screen_text(&f.app).contains("mock picture"));
    deliver(&mut f.app, events);
    assert_eq!(*f.adapter.web_calls.lock().unwrap(), [false]);
    let prompt = f.adapter.prompts.lock().unwrap()[0].clone();
    assert!(prompt.starts_with("DIVE:Visualize"), "{prompt}");
    assert!(prompt.contains("Code you may cite"), "{prompt}");

    let text = screen_text(&f.app);
    for want in [
        "␣v Visualize",
        "mock picture: the unit now writes to a store.",
        "caller ─calls→ this unit  [unchanged]",
        "this unit ─writes→ store  [added]",
        "saved to",
        "gx opens it in the browser",
    ] {
        assert!(text.contains(want), "{want:?} missing:\n{text}");
    }
    assert!(!text.contains("asking the agent"), "{text}");
    assert!(
        f.app
            .review
            .unit(&f.app.unit.id)
            .unwrap()
            .dive_pending
            .is_empty()
    );

    // Answered: closed and opened again, nothing is asked.
    assert!(asked(&keys(&mut f.app, "<space>v<space>v")).is_empty());
    assert_eq!(f.adapter.prompts.lock().unwrap().len(), 1);

    // The page: under the data dir, one per review and unit, self-contained.
    let v = picture(&f.app);
    assert_eq!(v.path, visual::page_path(&f.app.review.id, &f.app.unit.id));
    assert!(v.path.starts_with(store::data_dir().join("visuals")));
    let html = std::fs::read_to_string(&v.path).unwrap();
    assert!(html.contains("<svg role=\"img\" aria-label=\"mock picture"));
    assert!(html.contains("<figcaption>mock picture"));
    assert!(!html.contains("<script"));
}

#[tokio::test]
async fn visualize_panel_snapshot() {
    let mut f = fixture().await;
    let effects = keys(&mut f.app, "<space>v");
    answer(&mut f, &effects).await;
    // Fixed paths, so the snapshot is the same on every machine.
    f.app.review.repo_root = PathBuf::from("/work/shop");
    f.app.review.base = "a1b2c3d4e5f6".into();
    let id = f.app.unit.id.clone();
    let u = f.app.review.unit_mut(&id).unwrap();
    let res = u.dive.results.get_mut(&DiveKind::Visualize).unwrap();
    res.visual.as_mut().unwrap().path = PathBuf::from("/data/visuals/r1/checkout.py_charge.html");
    insta::assert_snapshot!("unit_visualize", screen(&f.app, 100, 30).backend());
}

#[tokio::test]
async fn a_failed_question_says_so_and_can_be_asked_again() {
    let broken = MockAdapter::new(|_, _| anyhow::bail!("agent exploded"));
    let mut f = fixture_with(Keymap::default(), broken);
    open_first(&mut f).await;
    let effects = keys(&mut f.app, "<space>v");
    assert_eq!(asked(&effects).len(), 1);
    assert!(screen_text(&f.app).contains("asking the agent"));
    let events = run(&f, &effects, f.adapter.clone()).await;
    assert!(matches!(
        events[0],
        DiveEvent::Failed {
            kind: DiveKind::Visualize,
            ..
        }
    ));
    deliver(&mut f.app, events);
    let text = screen_text(&f.app);
    assert!(text.contains("failed: agent exploded"), "{text}");
    assert!(text.contains("No answer. ␣v asks again."), "{text}");
    assert!(!text.contains("asking the agent"), "{text}");
    let u = f.app.review.unit(&f.app.unit.id).unwrap();
    assert!(u.dive_pending.is_empty());
    assert!(!u.dive.results.contains_key(&DiveKind::Visualize));
    // Closed and opened again, it is asked again.
    assert_eq!(asked(&keys(&mut f.app, "<space>v<space>v")).len(), 1);
}

#[tokio::test]
async fn gx_opens_the_page_and_writes_it_again_when_it_is_gone() {
    let mut f = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime(tools(dir.path()));

    // Nothing drawn yet: say how to draw one.
    assert!(keys(&mut f.app, "gx").is_empty());
    assert_eq!(f.app.status, "No picture yet: press ␣v to draw one.");

    let effects = keys(&mut f.app, "<space>v");
    answer(&mut f, &effects).await;
    // `gx` works with the panel closed too.
    keys(&mut f.app, "<esc>");
    assert_eq!(f.app.unit.panel, None);
    let id = f.app.unit.id.clone();
    let effects = keys(&mut f.app, "gx");
    assert_eq!(
        effects,
        vec![Effect::OpenVisual {
            unit_id: id.clone()
        }]
    );
    for e in effects {
        rt.exec(&mut f.app, e);
    }
    let path = picture(&f.app).path;
    assert_eq!(opened(dir.path(), 1).await, [path.display().to_string()]);
    assert!(f.app.status.starts_with("Opened "), "{}", f.app.status);

    // Deleted: written again from the saved picture, then opened.
    std::fs::remove_file(&path).unwrap();
    for e in keys(&mut f.app, "gx") {
        rt.exec(&mut f.app, e);
    }
    assert!(path.is_file());
    assert_eq!(opened(dir.path(), 2).await.len(), 2);

    // A stored path outside teer's visuals folder is never written to.
    let outside = dir.path().join("elsewhere.html");
    let u = f.app.review.unit_mut(&id).unwrap();
    let res = u.dive.results.get_mut(&DiveKind::Visualize).unwrap();
    res.visual.as_mut().unwrap().path = outside.clone();
    for e in keys(&mut f.app, "gx") {
        rt.exec(&mut f.app, e);
    }
    assert!(!outside.exists());
    assert_eq!(picture(&f.app).path, path);
    assert_eq!(opened(dir.path(), 3).await[2], path.display().to_string());
}

#[tokio::test]
async fn the_keys_can_be_rebound() {
    let (km, warnings) = Keymap::new(
        &HashMap::from([
            ("tool.diagram".to_string(), KeySpec::One("<space>d".into())),
            ("goto.browser".to_string(), KeySpec::One("<c-o>".into())),
        ]),
        1000,
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let mut f = fixture_with(km, MockAdapter::new(dive_responder));
    open_first(&mut f).await;
    assert!(keys(&mut f.app, "<space>v").is_empty());
    assert!(keys(&mut f.app, "gx").is_empty());
    assert!(f.app.status.is_empty(), "{}", f.app.status);
    let effects = keys(&mut f.app, "<space>d");
    assert_eq!(asked(&effects), [(DiveKind::Visualize, false)]);
    answer(&mut f, &effects).await;
    assert!(screen_text(&f.app).contains("^o opens it in the browser"));
    let effects = f.app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(
        matches!(&effects[..], [Effect::OpenVisual { .. }]),
        "{effects:?}"
    );
}

#[tokio::test]
async fn the_picture_survives_a_save_and_reload() {
    let mut f = fixture().await;
    let effects = keys(&mut f.app, "<space>v");
    answer(&mut f, &effects).await;
    let before = picture(&f.app);
    let id = f.app.unit.id.clone();
    store::save(&f.app.review).unwrap();
    let back = store::load_latest(&f.app.review.repo_root, None).unwrap();
    let res = &back.unit(&id).unwrap().dive.results[&DiveKind::Visualize];
    assert_eq!(res.visual.as_ref(), Some(&before));
    assert_eq!(res.text, before.claim);

    f.app = App::new(back, Config::default(), Keymap::default());
    f.app.has_agent = true;
    open(&mut f, &id).await;
    // Answered: the panel opens on it and asks nothing.
    assert!(asked(&keys(&mut f.app, "<space>v")).is_empty());
    let text = screen_text(&f.app);
    assert!(text.contains("this unit ─writes→ store  [added]"), "{text}");
}

#[tokio::test]
async fn with_images_the_panel_shows_the_picture_instead_of_the_list() {
    let mut f = fixture().await;
    let effects = keys(&mut f.app, "<space>v");
    answer(&mut f, &effects).await;
    f.app.pictures = Some(kitty());
    let text = screen_text(&f.app);
    assert!(text.contains(PLACEHOLDER), "{text}");
    assert!(
        text.contains("mock picture: the unit now writes to a store."),
        "{text}"
    );
    assert!(
        !text.contains("─calls→"),
        "the picture replaces the list:\n{text}"
    );
    assert!(text.contains("gx opens it in the browser · "), "{text}");
    // Drawn again from the same pixels on the next frame.
    assert!(screen_text(&f.app).contains(PLACEHOLDER));

    // Too little room for a picture: the list again.
    let small = text_at(&f.app, 120, 14);
    assert!(!small.contains(PLACEHOLDER), "{small}");
    assert!(small.contains("─calls→"), "{small}");

    // The panel closed: no picture.
    keys(&mut f.app, "<esc>");
    assert!(!screen_text(&f.app).contains(PLACEHOLDER));
}

#[tokio::test]
async fn without_images_the_panel_keeps_the_list() {
    let mut f = fixture().await;
    let effects = keys(&mut f.app, "<space>v");
    answer(&mut f, &effects).await;
    assert!(f.app.pictures.is_none(), "tests never ask a real terminal");
    let text = screen_text(&f.app);
    assert!(!text.contains(PLACEHOLDER));
    assert!(text.contains("this unit ─writes→ store  [added]"), "{text}");
}

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
async fn leader_t_lists_the_found_tests_and_the_agent_reads_them() {
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
    let text = |app: &App| text_at(app, 120, 50);

    let effects = keys(app, "<space>t");
    assert_eq!(app.unit.panel, Some(Panel::Tool(DiveKind::Tests)));
    assert_eq!(asked(&effects), [(DiveKind::Tests, false)]);
    assert!(text(app).contains("searching the test files"));

    // teer searches by itself, the way the runtime does: no agent. Then the
    // agent reads what it found, on its own.
    let unit = app.review.unit(&charge).unwrap().clone();
    let callers = test_search::callers(&app.review, &unit);
    let effects = app.on_app_event(AppEvent::Dive(search_tests(
        &f.repo.path(),
        &unit,
        &callers,
    )));
    assert!(
        effects.contains(&Effect::InterpretTests {
            unit_id: charge.clone()
        }),
        "{effects:?}"
    );
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
        target: visual::target(&app.review, &unit),
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

    // An old-against-new probe asked next is told to run those cases.
    let u = app.review.unit(&charge).unwrap();
    let file = app.review.file_of(u).unwrap();
    let p = behavior_prompt(u, file, &ProbeSetting::default(), None);
    assert!(p.contains("Dangerous cases no test covers"), "{p}");
    assert!(p.contains("- the payment times out (High risk)"), "{p}");
    assert!(
        p.contains("- test_charges_once (tests/test_checkout.py:4-5)"),
        "{p}"
    );
}
