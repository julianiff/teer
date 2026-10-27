//! Slim view, reset, editor, swipe mode.
//! Rendered screens are snapshot tested; behavior runs through the app's
//! event handlers and the runtime with a mocked agent.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::*;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};
use teer::agent::mock::MockAdapter;
use teer::config::{Config, KeyMap};
use teer::review::model::{Decision, Review, SessionRef};
use teer::review::{BuildOptions, build_review};
use teer::tui::app::{App, AppEvent, Effect, Overlay, Screen};
use teer::tui::ui;

const API: &str = "from checkout import charge_order\n\n\ndef handle_checkout(request):\n    return charge_order(request.order)\n\n\ndef main():\n    handle_checkout(parse())\n";
const TEST: &str = "from checkout import charge_order\n\n\ndef test_charge_order():\n    assert charge_order(order())\n";

/// Fixed statements per symbol; evidence on the first line of the right side.
fn responder(prompt: &str, _schema: &Value) -> anyhow::Result<Value> {
    let mut out = Vec::new();
    let mut id = String::new();
    let mut old = None;
    for line in prompt.lines() {
        if let Some(rest) = line.strip_prefix("### Unit ") {
            id = rest.to_string();
        }
        if let Some(r) = line.strip_prefix("old lines: ") {
            old = r.split_once('-').map(|(a, _)| a.parse::<u32>().unwrap());
        }
        if let Some(r) = line.strip_prefix("new lines: ") {
            let new = r.split_once('-').map(|(a, _)| a.parse::<u32>().unwrap());
            let ev = |side: &str| match side {
                "old" => json!([{"side": "old", "line": old.unwrap()}]),
                _ => json!([{"side": "new", "line": new.unwrap() + 1}]),
            };
            let symbol = id.rsplit("::").next().unwrap();
            let changes = match symbol {
                "charge_order" => json!([
                    {"marker": "added", "statement": "Rejects unsupported currencies. 2 callers don't handle it.", "risk": "high", "evidence": ev("new")},
                    {"marker": "changed", "statement": "Saves the payment on the order before returning it, so a failed save now fails the whole charge.", "risk": "medium", "evidence": ev("new")},
                    {"marker": "removed", "statement": "No longer sends the receipt email.", "risk": "low", "evidence": ev("old")}
                ]),
                "new_helper" => {
                    json!([{"marker": "added", "statement": "Charges an order through charge_order.", "risk": "low", "evidence": ev("new")}])
                }
                "add" => {
                    json!([{"marker": "changed", "statement": "Counts items as they are added.", "risk": "low", "evidence": ev("new")}])
                }
                "old_helper" => {
                    json!([{"marker": "removed", "statement": "The doubling helper is gone.", "risk": "medium", "evidence": ev("old")}])
                }
                _ => json!([]),
            };
            let summary = match symbol {
                "new_helper" => "Adds a helper that charges an order.",
                "add" => "Counts items as they are added to the cart.",
                _ => "Changes how orders are charged.",
            };
            out.push(json!({"id": id, "summary": summary, "changes": changes}));
        }
    }
    Ok(json!({ "units": out }))
}

fn session() -> SessionRef {
    SessionRef {
        agent: "mock".into(),
        id: "s1".into(),
        cwd: PathBuf::from("/work/shop"),
    }
}

/// A prepared review of the Python fixture, with paths and commits fixed
/// so snapshots are stable.
async fn fixture_review() -> (Review, FixtureRepo) {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD)
        .write("api.py", API)
        .write("tests/test_checkout.py", TEST)
        .commit("init");
    repo.write("checkout.py", PY_NEW);
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    teer::cli::prepare_all(
        &mut review,
        Arc::new(MockAdapter::new(responder)),
        session(),
        2,
    )
    .await;
    review.repo_root = PathBuf::from("/work/shop");
    review.base = "a1b2c3d4e5f6".into();
    review.agent_session = Some(session());
    (review, repo)
}

fn app_for(review: Review, cfg: Config) -> App {
    let mut app = App::new(review, cfg, KeyMap::default());
    let effects = app.start(true, None, Vec::new(), Some(session()), false);
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Prepare(_))),
        "already prepared: {effects:?}"
    );
    app
}

fn render(app: &App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    t
}

fn key(app: &mut App, c: char) -> Vec<Effect> {
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )))
}

fn code(app: &mut App, c: KeyCode) -> Vec<Effect> {
    app.on_event(Event::Key(KeyEvent::new(c, KeyModifiers::NONE)))
}

fn select(app: &mut App, symbol: &str) {
    let pos = app
        .order
        .iter()
        .position(|&i| app.review.units[i].symbol == symbol)
        .unwrap();
    app.sel = pos;
}

fn ctrl(app: &mut App, c: char) -> Vec<Effect> {
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::CONTROL,
    )))
}

// ---- M3: slim view -----------------------------------------------------

#[tokio::test]
async fn slim_view_snapshot() {
    let (review, _repo) = fixture_review().await;
    let app = app_for(review, Config::default());
    assert_eq!(app.screen, Screen::Slim);
    insta::assert_snapshot!("slim", render(&app, 90, 30).backend());
}

#[tokio::test]
async fn slim_navigation() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    let n = app.order.len();
    key(&mut app, 'j');
    assert_eq!(app.sel, 1);
    key(&mut app, 'G');
    assert_eq!(app.sel, n - 1);
    // The file's name is no stop: the first row is its first unit.
    key(&mut app, 'g');
    assert_eq!(app.sel, 0);
    key(&mut app, 'k');
    assert_eq!(app.sel, 0);

    select(&mut app, "old_helper");
    insta::assert_snapshot!("slim_worth_a_look", render(&app, 90, 30).backend());
    assert_eq!(key(&mut app, 'q'), vec![Effect::Save, Effect::Quit]);
}

#[tokio::test]
async fn clicking_a_unit_selects_it() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    render(&app, 90, 30);
    // Row 6 belongs to a unit further down the stream.
    let target = app
        .targets
        .borrow()
        .iter()
        .filter_map(|(r, t)| match t {
            teer::tui::app::Target::Unit(p) if *p == 2 => Some(*r),
            _ => None,
        })
        .next()
        .unwrap();
    app.on_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 5,
        row: target.y,
        modifiers: KeyModifiers::NONE,
    }));
    assert_eq!(app.sel, 2);
}

#[tokio::test]
async fn rerun_keeps_decisions_for_unchanged_units() {
    let (review, repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    for u in app.review.units.iter_mut() {
        u.decision = Decision::Accepted;
    }
    // The agent edits charge_order again.
    repo.write(
        "checkout.py",
        &PY_NEW.replace("raise ValueError(\"unsupported currency\")", "return None"),
    );
    let mut fresh = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    fresh.repo_root = PathBuf::from("/work/shop");
    let effects = app.on_app_event(AppEvent::Rerun(Ok(Box::new(fresh))));
    let d = |s: &str| {
        app.review
            .units
            .iter()
            .find(|u| u.symbol == s)
            .unwrap()
            .decision
    };
    assert_eq!(d("charge_order"), Decision::Pending);
    assert_eq!(d("Cart::add"), Decision::Accepted);
    // Only the changed unit goes back to the agent.
    assert_eq!(
        effects
            .iter()
            .find_map(|e| if let Effect::Prepare(ids) = e {
                Some(ids.clone())
            } else {
                None
            }),
        Some(vec!["checkout.py::charge_order".to_string()])
    );
}

// ---- M7: swipe mode ----------------------------------------------------

#[tokio::test]
async fn swipe_card_snapshot_and_keys() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    key(&mut app, 's');
    assert_eq!(app.screen, Screen::Swipe);
    let first = app.swipe_current().cloned().unwrap();
    // The close looks come first.
    assert_eq!(app.review.unit(&first).unwrap().symbol, "charge_order");
    insta::assert_snapshot!("swipe_card", render(&app, 90, 26).backend());

    // Accept.
    code(&mut app, KeyCode::Right);
    assert_eq!(
        app.review.unit(&first).unwrap().decision,
        Decision::Accepted
    );
    // j does nothing: there's no down.
    let second = app.swipe_current().cloned().unwrap();
    // Every change carries its code, a card worth a look too.
    assert_eq!(app.review.unit(&second).unwrap().symbol, "old_helper");
    let card = format!("{}", render(&app, 90, 26).backend());
    assert!(card.contains("code change"), "{card}");
    assert!(card.contains("- def old_helper"), "{card}");
    key(&mut app, 'j');
    code(&mut app, KeyCode::Down);
    assert_eq!(app.swipe_current(), Some(&second));
    // k and ↑ are plain "up" and decide nothing.
    key(&mut app, 'k');
    code(&mut app, KeyCode::Up);
    assert_eq!(app.swipe_current(), Some(&second));
    assert_eq!(
        app.review.unit(&second).unwrap().decision,
        Decision::Pending
    );
    // i flags the card for a closer look and moves on; undo brings it back.
    key(&mut app, 'i');
    assert_ne!(app.swipe_current(), Some(&second));
    assert_eq!(
        app.review.unit(&second).unwrap().decision,
        Decision::Investigate
    );
    key(&mut app, 'u');
    assert_eq!(app.swipe_current(), Some(&second));
    assert_eq!(
        app.review.unit(&second).unwrap().decision,
        Decision::Pending
    );
    // ← flags too; h no longer does (see h_goes_back_...).
    code(&mut app, KeyCode::Left);
    assert_eq!(
        app.review.unit(&second).unwrap().decision,
        Decision::Investigate
    );
    assert_ne!(app.swipe_current(), Some(&second));
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.screen, Screen::Swipe);

    while app.swipe_current().is_some() {
        key(&mut app, 'l');
    }
    insta::assert_snapshot!("swipe_summary", render(&app, 90, 20).backend());
}

/// `charge_order` with statements citing what they say: the added check
/// (new 9-10), the saved payment (new 12), the dropped receipt (old 10),
/// and a second opinion that agrees.
fn cited(review: &mut Review) {
    use teer::review::model::{LineRef, Risk, SecondOpinion, Side};
    let u = review
        .units
        .iter_mut()
        .find(|u| u.symbol == "charge_order")
        .unwrap();
    let lines = [
        vec![(Side::New, 9), (Side::New, 10)],
        vec![(Side::New, 12)],
        vec![(Side::Old, 10)],
    ];
    for (c, lines) in u.changes.iter_mut().zip(lines) {
        c.evidence = lines
            .into_iter()
            .map(|(side, line)| LineRef { side, line })
            .collect();
    }
    u.checks.second = Some(SecondOpinion {
        risk: Risk::High,
        verdicts: Vec::new(),
        missed: Vec::new(),
    });
}

#[tokio::test]
async fn statements_sit_in_the_code_under_the_rows_they_cite() {
    let (mut review, _repo) = fixture_review().await;
    cited(&mut review);
    let mut app = app_for(review, Config::default());
    key(&mut app, 's');
    let card = format!("{}", render(&app, 80, 24).backend());
    insta::assert_snapshot!("swipe_card_cites", card);
    // What the agent wrote sits behind a bar, under a label naming who.
    assert!(
        card.contains("│ ┃ agent (haiku) · checked by sonnet "),
        "{card}"
    );
    assert!(
        card.contains("│ ┃ Changes how orders are charged."),
        "{card}"
    );
    // Each statement sits in a card under the last row it cites, and
    // nothing else marks those rows; the removed row has its old line
    // number, the one the statement cites.
    for row in [
        "│   9 +     if order.currency not in SUPPORTED:",
        "│  10 +         raise ValueError(\"unsupported currency\")",
        "│     ╭──────",
        "│     │ Rejects unsupported currencies. 2 callers don't handle it. │",
        "│  11       result = pay(order)",
    ] {
        assert!(card.contains(row), "{row}:\n{card}");
    }
    // Further down, scrolling, the removed row and the other two cards.
    let mut seen = card;
    for _ in 0..10 {
        key(&mut app, 'J');
        seen.push_str(&format!("{}", render(&app, 80, 24).backend()));
    }
    for row in [
        "│  10 -     send_receipt(order)",
        "│     │ No longer sends the receipt email. │",
        "│     │ Saves the payment on the order",
    ] {
        assert!(seen.contains(row), "{row}:\n{seen}");
    }
}

#[tokio::test]
async fn a_long_path_on_the_card_is_cut_at_the_front() {
    let (mut review, _repo) = fixture_review().await;
    let long = PathBuf::from("application/module/MediaDistribution/Contract/Data/checkout.py");
    for f in review.files.iter_mut().filter(|f| f.path == *"checkout.py") {
        f.path = long.clone();
    }
    for u in review.units.iter_mut().filter(|u| u.file == *"checkout.py") {
        u.file = long.clone();
    }
    let mut app = app_for(review, Config::default());
    key(&mut app, 's');
    let card = format!("{}", render(&app, 80, 24).backend());
    let head = card
        .lines()
        .find(|l| l.contains("Needs a close look"))
        .unwrap();
    assert!(
        head.contains("▲ Needs a close look  …") && head.contains("/Contract/Data/checkout.py │"),
        "{head}"
    );
}

#[tokio::test]
async fn dragging_the_card_swipes() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    key(&mut app, 's');
    render(&app, 90, 26);
    let card = app.swipe_card.get();
    let first = app.swipe_current().cloned().unwrap();
    let (x, y) = (card.x + card.width / 2, card.y + card.height / 2);
    let m = |kind, column, row| {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    };

    // A short drag does nothing.
    app.on_event(m(MouseEventKind::Down(MouseButton::Left), x, y));
    app.on_event(m(MouseEventKind::Drag(MouseButton::Left), x + 5, y));
    app.on_event(m(MouseEventKind::Up(MouseButton::Left), x + 5, y));
    assert_eq!(app.swipe_current(), Some(&first));

    // Past a third of the width: accept. The card follows the drag, tinted.
    app.on_event(m(MouseEventKind::Down(MouseButton::Left), x, y));
    app.on_event(m(
        MouseEventKind::Drag(MouseButton::Left),
        x + card.width / 3 + 1,
        y,
    ));
    insta::assert_snapshot!("swipe_dragging", render(&app, 90, 26).backend());
    app.on_event(m(
        MouseEventKind::Up(MouseButton::Left),
        x + card.width / 3 + 1,
        y,
    ));
    assert_eq!(
        app.review.unit(&first).unwrap().decision,
        Decision::Accepted
    );
}

#[tokio::test]
async fn dragging_left_investigates_and_vertical_drags_are_ignored() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    key(&mut app, 's');
    render(&app, 90, 26);
    let card = app.swipe_card.get();
    let first = app.swipe_current().cloned().unwrap();
    let (x, y) = (card.x + card.width / 2, card.y + card.height / 2);
    let m = |kind, column, row| {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    };
    // Dragging up (or down) the whole card does nothing.
    for dy in [-(card.height as i32 / 2 - 1), card.height as i32 / 2 - 1] {
        let ty = (y as i32 + dy) as u16;
        app.on_event(m(MouseEventKind::Down(MouseButton::Left), x, y));
        app.on_event(m(MouseEventKind::Drag(MouseButton::Left), x, ty));
        app.on_event(m(MouseEventKind::Up(MouseButton::Left), x, ty));
        assert_eq!(app.swipe_current(), Some(&first));
        assert_eq!(app.review.unit(&first).unwrap().decision, Decision::Pending);
    }
    // Past a third of the width to the left: investigate.
    let tx = x - (card.width / 3 + 1);
    app.on_event(m(MouseEventKind::Down(MouseButton::Left), x, y));
    app.on_event(m(MouseEventKind::Drag(MouseButton::Left), tx, y));
    app.on_event(m(MouseEventKind::Up(MouseButton::Left), tx, y));
    assert_eq!(
        app.review.unit(&first).unwrap().decision,
        Decision::Investigate
    );
    assert_ne!(app.swipe_current(), Some(&first));
}

// ---- M8: large reviews and startup ----------------------------------

#[tokio::test]
async fn large_reviews_prepare_close_looks_first_and_the_rest_per_file() {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD)
        .write("src/checkout.rs", RS_OLD)
        .commit("init");
    repo.write("checkout.py", PY_NEW)
        .write("src/checkout.rs", RS_NEW);
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    review.repo_root = PathBuf::from("/work/shop");
    review.base = "a1b2c3d4e5f6".into();
    let cfg = Config {
        large_review: 3,
        ..Config::default()
    };
    let mut app = App::new(review, cfg, KeyMap::default());
    let effects = app.start(true, None, Vec::new(), Some(session()), false);
    // Straight into the list: there is no file overview any more.
    assert_eq!(app.screen, Screen::Slim);
    let asked = |effects: &[Effect]| -> Vec<String> {
        effects
            .iter()
            .flat_map(|e| match e {
                Effect::Prepare(ids) => ids.clone(),
                _ => vec![],
            })
            .collect()
    };
    let prepared = asked(&effects);
    let close: Vec<String> = app
        .order
        .iter()
        .filter(|&&i| app.level_of(i).level == teer::review::attention::Level::Attention)
        .map(|&i| app.review.units[i].id.clone())
        .collect();
    let first_file = app.selected().unwrap().file.clone();
    // Every close look, the selected unit's file, nothing else, and no
    // unit twice.
    for id in &close {
        assert!(prepared.contains(id), "{id} in {prepared:?}");
    }
    for id in &prepared {
        let u = app.review.unit(id).unwrap();
        assert!(close.contains(id) || u.file == first_file, "{id}");
    }
    let mut once = prepared.clone();
    once.sort();
    once.dedup();
    assert_eq!(once.len(), prepared.len(), "{prepared:?}");
    // Getting to the other file prepares what is left of it.
    let other = PathBuf::from(if first_file == *"checkout.py" {
        "src/checkout.rs"
    } else {
        "checkout.py"
    });
    let mut effects = Vec::new();
    for _ in 0..10 {
        effects = ctrl(&mut app, 'd');
        if app.selected().unwrap().file == other {
            break;
        }
    }
    let ids = asked(&effects);
    assert!(!ids.is_empty());
    for id in &ids {
        assert_eq!(app.review.unit(id).unwrap().file, other, "{id}");
        assert!(!prepared.contains(id), "{id} asked twice");
    }
}

#[tokio::test]
async fn starts_on_the_review_without_asking_for_a_session() {
    let (review, _repo) = fixture_review().await;
    let mut fresh = review.clone();
    for u in fresh.units.iter_mut() {
        u.prep = teer::review::model::Prep::NotRequested;
        u.changes.clear();
    }
    let mut app = App::new(fresh, Config::default(), KeyMap::default());
    let s = SessionRef::fresh("claude", "/work/shop".into());
    let effects = app.start(true, None, Vec::new(), Some(s.clone()), false);
    assert_eq!(app.screen, Screen::Slim);
    assert_eq!(app.review.agent_session, Some(s));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Prepare(ids) if !ids.is_empty()))
    );
    let screen = format!("{}", render(&app, 90, 8).backend());
    assert!(screen.contains("preparing 0/4"), "{screen}");

    // Without Claude Code, the review still opens and says why.
    let mut app = App::new(review, Config::default(), KeyMap::default());
    let effects = app.start(
        false,
        Some("Claude Code ('claude') isn't on your PATH.".into()),
        Vec::new(),
        None,
        false,
    );
    assert!(effects.is_empty());
    assert_eq!(app.screen, Screen::Slim);
    assert!(app.review.agent_session.is_none());
    let screen = format!("{}", render(&app, 90, 12).backend());
    assert!(screen.contains("isn't on your PATH"), "{screen}");
}

// ---- Attention groups --------------------------------------------------
#[tokio::test]
async fn units_get_their_level_from_code_facts_and_agent_risk() {
    use teer::review::attention::Level;
    let (review, _repo) = fixture_review().await;
    let app = app_for(review, Config::default());
    let level = |s: &str| {
        app.assessment(&format!("checkout.py::{s}"))
            .unwrap()
            .clone()
    };
    assert_eq!(level("compute_total").level, Level::Same);
    assert_eq!(
        level("compute_total").reasons,
        vec!["local names renamed".to_string()]
    );
    assert_eq!(
        level("format_me").reasons,
        vec!["formatting or comments".to_string()]
    );
    assert_eq!(level("vat").reasons, vec!["renamed from tax".to_string()]);
    assert_eq!(level("new_helper").level, Level::Routine);
    assert_eq!(level("Cart::add").level, Level::Routine);
    // Removed with no callers is a fact; the agent's medium risk raises it.
    let old = level("old_helper");
    assert_eq!(old.level, Level::Look);
    assert_eq!(
        old.reasons,
        vec![
            "removed, no callers found".to_string(),
            "agent: medium risk".to_string()
        ]
    );
    let charge = level("charge_order");
    assert_eq!(charge.level, Level::Attention);
    assert_eq!(
        charge.reasons,
        vec!["3 callers".to_string(), "agent: high risk".to_string()]
    );

    // One file, most attention first, then as a story; no-behavior-change
    // units aren't listed as units.
    let order: Vec<&str> = app
        .order
        .iter()
        .map(|&i| app.review.units[i].symbol.as_str())
        .collect();
    assert_eq!(
        order,
        vec!["charge_order", "old_helper", "new_helper", "Cart::add"]
    );
}

#[tokio::test]
async fn facts_raise_the_level_even_when_the_agent_says_low_risk() {
    use teer::review::attention::Level;
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    let caller = "from lib import helper\n\ndef main():\n    helper(1)\n";
    repo.write(
        "lib.py",
        "def helper(x):\n    return x\n\ndef gone():\n    return 1\n",
    )
    .write("app.py", caller)
    .write(
        "other.py",
        "from lib import gone\n\ndef run():\n    gone()\n",
    )
    .commit("init");
    repo.write("lib.py", "def helper(x, y=0):\n    return x + y\n");
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let all_low = |prompt: &str, _: &Value| -> anyhow::Result<Value> {
        let ids: Vec<&str> = prompt
            .lines()
            .filter_map(|l| l.strip_prefix("### Unit "))
            .collect();
        let units: Vec<Value> = ids
            .iter()
            .map(|id| json!({"id": id, "summary": "Small tweak.", "changes": []}))
            .collect();
        Ok(json!({ "units": units }))
    };
    teer::cli::prepare_all(
        &mut review,
        Arc::new(MockAdapter::new(all_low)),
        session(),
        2,
    )
    .await;
    let app = app_for(review, Config::default());
    let helper = app.assessment("lib.py::helper").unwrap();
    assert_eq!(helper.level, Level::Look);
    assert!(
        helper
            .reasons
            .contains(&"signature changed, 1 caller".to_string()),
        "{helper:?}"
    );
    let gone = app.assessment("lib.py::gone").unwrap();
    assert_eq!(gone.level, Level::Attention);
    assert_eq!(gone.reasons[0], "removed, 1 caller still calls it");
}

#[tokio::test]
async fn the_list_has_one_line_per_unit_and_code_in_every_preview() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    for symbol in ["new_helper", "old_helper", "charge_order"] {
        select(&mut app, symbol);
        let list = format!("{}", render(&app, 100, 30).backend());
        // Every change shows its code in the preview, never in the list rows.
        assert!(list.contains("╭ code change "), "{symbol}:\n{list}");
        let code_rows: Vec<&str> = list
            .lines()
            .filter(|l| l.contains("if order.currency not in SUPPORTED"))
            .collect();
        if symbol == "charge_order" {
            assert_eq!(code_rows.len(), 1, "{list}");
            assert!(code_rows[0].contains("  9 + "), "{list}");
            assert!(code_rows[0].starts_with("\" │"), "{list}");
        } else {
            assert!(code_rows.is_empty(), "{symbol}:\n{list}");
        }
        // Each listed unit has exactly one row (inside its box, its level's
        // icon under the file's name), just its name; the preview pane below
        // says where it is.
        let rows: Vec<&str> = list
            .lines()
            .filter(|l| l.starts_with("\"│"))
            .filter(|l| l.chars().nth(5).is_some_and(|c| "▲◆●".contains(c)))
            .filter(|l| {
                ["charge_order", "old_helper", "new_helper", "Cart::add"]
                    .iter()
                    .any(|s| l.contains(s))
            })
            .collect();
        assert_eq!(rows.len(), 4, "4 unit rows for {symbol}:\n{list}");
        assert!(rows.iter().all(|l| !l.contains("checkout.py")), "{list}");
        // The selected unit's row is the only one with the selection marker.
        let marked = list.lines().filter(|l| l.contains('▌')).count();
        assert_eq!(marked, 1, "{list}");
        let row = list.lines().find(|l| l.contains('▌')).unwrap();
        assert!(row.contains(symbol), "{symbol}: {row}");
    }
}

#[tokio::test]
async fn flagged_only_keys_do_nothing_in_the_list() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    select(&mut app, "charge_order");
    for c in ['c', 'v', 'o', 'n', 't', 'x', 'z'] {
        assert!(key(&mut app, c).is_empty(), "{c}");
        assert_eq!(app.screen, Screen::Slim, "{c}");
        assert_eq!(app.overlay, Overlay::None, "{c}");
    }
}

#[tokio::test]
async fn e_opens_the_unit_in_the_editor_at_its_line() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    select(&mut app, "charge_order");
    let line = app.selected().unwrap().display_line();
    let want = Effect::Edit {
        file: "checkout.py".into(),
        line,
    };
    assert_eq!(key(&mut app, 'e'), vec![want]);
    // From a swipe card it's the card on top.
    key(&mut app, 's');
    let top = app.swipe_current().cloned().unwrap();
    let (file, line) = {
        let u = app.review.unit(&top).unwrap();
        (u.file.clone(), u.display_line())
    };
    assert_eq!(key(&mut app, 'e'), vec![Effect::Edit { file, line }]);
}

#[tokio::test]
async fn reset_clears_every_decision_after_confirming() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    key(&mut app, 's');
    code(&mut app, KeyCode::Right);
    key(&mut app, 'i');
    key(&mut app, 'h');
    assert!(
        app.review
            .units
            .iter()
            .any(|u| u.decision != Decision::Pending)
    );
    let total = app
        .review
        .units
        .iter()
        .filter(|u| u.status != teer::review::model::UnitStatus::NoBehaviorChange)
        .count();

    // esc cancels.
    assert!(key(&mut app, 'R').is_empty());
    assert_eq!(app.overlay, Overlay::ConfirmReset);
    insta::assert_snapshot!("confirm_reset", render(&app, 90, 20).backend());
    code(&mut app, KeyCode::Esc);
    assert_eq!(app.overlay, Overlay::None);
    assert!(
        app.review
            .units
            .iter()
            .any(|u| u.decision != Decision::Pending)
    );

    // enter resets: nothing is looked at, the swipe stack is full again.
    key(&mut app, 'R');
    assert_eq!(code(&mut app, KeyCode::Enter), vec![Effect::Save]);
    assert!(
        app.review
            .units
            .iter()
            .all(|u| u.decision == Decision::Pending)
    );
    assert_eq!(app.swipe.cards.len(), total);
    assert_eq!(app.swipe.cursor, 0);
    assert!(app.swipe.history.is_empty());
}

#[tokio::test]
async fn the_accept_button_sits_bottom_right_and_is_clickable() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    key(&mut app, 's');
    let screen = format!("{}", render(&app, 90, 26).backend());
    assert!(screen.contains("accept [l]"), "{screen}");
    let first = app.swipe_current().cloned().unwrap();
    let card = app.swipe_card.get();
    let (rect, _) = *app
        .targets
        .borrow()
        .iter()
        .find(|(_, t)| *t == teer::tui::app::Target::Action(teer::config::Action::Accept))
        .expect("accept button");
    // Under the card, at its right edge.
    assert_eq!(rect.y, card.y + card.height);
    assert_eq!(rect.x + rect.width, card.x + card.width);
    let m = |kind| {
        Event::Mouse(MouseEvent {
            kind,
            column: rect.x + 2,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        })
    };
    app.on_event(m(MouseEventKind::Down(MouseButton::Left)));
    app.on_event(m(MouseEventKind::Up(MouseButton::Left)));
    assert_eq!(
        app.review.unit(&first).unwrap().decision,
        Decision::Accepted
    );
}

fn symbols(app: &App) -> Vec<String> {
    app.order
        .iter()
        .map(|&i| app.review.units[i].symbol.clone())
        .collect()
}

#[tokio::test]
async fn the_file_sits_in_its_most_critical_box_with_its_units_most_critical_first() {
    let (review, _repo) = fixture_review().await;
    let app = app_for(review, Config::default());
    let backend = render(&app, 90, 30);
    let text = format!("{}", backend.backend());
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle}\n{text}"))
    };
    // One file, in the box of its close look; no other level's box.
    assert!(at("╭─ Needs a close look · 1 file") < at("│ checkout.py"));
    assert!(!text.contains("Worth a look ·") && !text.contains("Routine ·"));
    assert!(at("│ checkout.py") < at("▲ charge_order"));
    assert!(at("▲ charge_order") < at("◆ old_helper"));
    assert!(at("◆ old_helper") < at("● new_helper"));
    assert!(at("● new_helper") < at("● Cart::add"));
    assert!(at("● Cart::add") < at("○ compute_total  local names renamed"));
    assert_eq!(
        symbols(&app),
        vec!["charge_order", "old_helper", "new_helper", "Cart::add"]
    );
    // Start on the file's first unit: its close look.
    assert_eq!(app.selected().unwrap().symbol, "charge_order");
}

#[tokio::test]
async fn deciding_does_not_move_units_and_pending_jumps_skip_decided() {
    use teer::tui::app::Target;
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    let before = symbols(&app);
    // Accept the first (charge_order); selection moves to the next pending one.
    key(&mut app, 'l');
    assert_eq!(symbols(&app), before);
    assert_eq!(app.selected().unwrap().symbol, "old_helper");
    assert_eq!(app.review.units[app.order[0]].decision, Decision::Accepted);
    app.rebuild_order();
    assert_eq!(symbols(&app), before, "rebuilding keeps the order too");
    assert_eq!(app.selected().unwrap().symbol, "old_helper");

    // Flag old_helper; decided units are skipped by ] and [.
    key(&mut app, 'i');
    assert_eq!(app.selected().unwrap().symbol, "new_helper");
    key(&mut app, 'g');
    assert_eq!(app.sel, 0);
    key(&mut app, ']');
    assert_eq!(app.selected().unwrap().symbol, "new_helper");
    key(&mut app, '[');
    assert_eq!(app.sel, 2, "no pending unit before new_helper");
    app.sel = 3;
    key(&mut app, '[');
    assert_eq!(app.selected().unwrap().symbol, "new_helper");

    // Rows are unchanged in count and mouse targets still map to positions.
    render(&app, 90, 30);
    let units: Vec<usize> = app
        .targets
        .borrow()
        .iter()
        .filter_map(|(_, t)| match t {
            Target::Unit(p) => Some(*p),
            _ => None,
        })
        .collect();
    for p in 0..4 {
        assert!(units.contains(&p), "{p} in {units:?}");
    }
}

#[tokio::test]
async fn swipe_cards_go_most_attention_first_then_in_list_order() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    key(&mut app, 's');
    let cards: Vec<String> = app.swipe.cards.clone();
    let mut list: Vec<usize> = app
        .order
        .iter()
        .copied()
        .filter(|&i| {
            app.review.units[i].status != teer::review::model::UnitStatus::NoBehaviorChange
        })
        .collect();
    list.sort_by_key(|&i| std::cmp::Reverse(app.level_of(i).level));
    let list: Vec<String> = list
        .into_iter()
        .map(|i| app.review.units[i].id.clone())
        .collect();
    assert_eq!(cards, list);
    let names: Vec<String> = cards
        .iter()
        .map(|id| app.review.unit(id).unwrap().symbol.clone())
        .collect();
    assert_eq!(
        names,
        vec!["charge_order", "old_helper", "new_helper", "Cart::add"]
    );
}

#[tokio::test]
async fn entering_swipe_keeps_only_flagged_and_pending_cards_starting_on_the_first() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    let all: Vec<String> = {
        key(&mut app, 's');
        app.swipe.cards.clone()
    };
    assert_eq!(all.len(), 4);
    code(&mut app, KeyCode::Esc);
    assert_eq!(app.screen, Screen::Slim);
    app.review.unit_mut(&all[0]).unwrap().decision = Decision::Investigate;
    app.review.unit_mut(&all[1]).unwrap().decision = Decision::Accepted;
    app.review.unit_mut(&all[3]).unwrap().decision = Decision::Accepted;
    key(&mut app, 's');
    // List order, accepted ones dropped; the flagged first card is shown again.
    assert_eq!(app.swipe.cards, vec![all[0].clone(), all[2].clone()]);
    assert_eq!(app.swipe.cursor, 0);
    assert_eq!(app.swipe_current(), Some(&all[0]));
    let text = screen_text(&app);
    assert!(text.contains("1 of 2"), "{text}");
    assert!(text.contains("⚑ flagged"), "{text}");
}

#[tokio::test]
async fn accepted_cards_stay_for_the_session_and_are_dropped_on_re_entry() {
    let mut app = swipe_app().await;
    let first = app.swipe_current().cloned().unwrap();
    key(&mut app, 'l');
    assert_eq!(app.swipe.cards.len(), 4);
    key(&mut app, 'h');
    assert_eq!(app.swipe_current(), Some(&first));
    assert!(screen_text(&app).contains("✓ accepted"));
    code(&mut app, KeyCode::Esc);
    key(&mut app, 's');
    assert_eq!(app.swipe.cards.len(), 3);
    assert!(!app.swipe.cards.contains(&first));
    assert!(screen_text(&app).contains("1 of 3"));
}

#[tokio::test]
async fn all_accepted_opens_on_the_summary_and_says_nothing_needs_a_look() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    for u in &mut app.review.units {
        u.decision = Decision::Accepted;
    }
    key(&mut app, 's');
    assert!(app.swipe.cards.is_empty());
    assert!(app.swipe_current().is_none());
    let text = screen_text(&app);
    assert!(text.contains("All changes triaged"), "{text}");
    assert!(text.contains("Nothing needs a look"), "{text}");
    assert!(text.contains("4 accepted"), "{text}");
    // With a flagged unit left over the deck is not empty; the summary
    // after it still points at the Flagged screen.
    let id = app.review.units[0].id.clone();
    app.review.unit_mut(&id).unwrap().decision = Decision::Investigate;
    code(&mut app, KeyCode::Esc);
    key(&mut app, 's');
    assert_eq!(app.swipe.cards.len(), 1);
    key(&mut app, 'i');
    let text = screen_text(&app);
    assert!(
        text.contains("press d to see them") || text.contains("press d"),
        "{text}"
    );
}

#[tokio::test]
async fn reset_in_swipe_brings_every_card_back() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    for u in &mut app.review.units {
        u.decision = Decision::Accepted;
    }
    app.review.units[0].decision = Decision::Pending;
    key(&mut app, 's');
    assert_eq!(app.swipe.cards.len(), 1);
    key(&mut app, 'R');
    key(&mut app, 'y');
    assert_eq!(app.swipe.cards.len(), 4);
    assert_eq!(app.swipe.cursor, 0);
}

// ---- Swipe: previous card, state badge, re-deciding --------------------

async fn swipe_app() -> App {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    key(&mut app, 's');
    app
}

fn decision_of(app: &App, id: &str) -> Decision {
    app.review.unit(id).unwrap().decision
}

fn screen_text(app: &App) -> String {
    format!("{}", render(app, 90, 26).backend())
}

#[tokio::test]
async fn l_accepts_and_advances() {
    let mut app = swipe_app().await;
    let first = app.swipe_current().cloned().unwrap();
    key(&mut app, 'l');
    assert_eq!(decision_of(&app, &first), Decision::Accepted);
    assert_eq!(app.swipe.cursor, 1);
    assert_ne!(app.swipe_current(), Some(&first));
    // The counter follows: second card of four.
    assert!(screen_text(&app).contains("2 of 4"));
}

#[tokio::test]
async fn h_goes_back_and_the_card_shows_its_state() {
    let mut app = swipe_app().await;
    let first = app.swipe_current().cloned().unwrap();
    assert!(screen_text(&app).contains("not reviewed yet"));
    key(&mut app, 'l');
    key(&mut app, 'h');
    assert_eq!(app.swipe_current(), Some(&first));
    // Going back changes no decision.
    assert_eq!(decision_of(&app, &first), Decision::Accepted);
    let text = screen_text(&app);
    assert!(text.contains("✓ accepted"), "{text}");
    assert!(text.contains("1 of 4"), "{text}");
    insta::assert_snapshot!("swipe_card_accepted", render(&app, 90, 26).backend());
    // A flagged card says so too.
    key(&mut app, 'i');
    key(&mut app, 'h');
    assert!(screen_text(&app).contains("⚑ flagged"));
    // → still accepts a card that is shown again, and advances.
    code(&mut app, KeyCode::Right);
    assert_eq!(decision_of(&app, &first), Decision::Accepted);
    assert_eq!(app.swipe.cursor, 1);
}

#[tokio::test]
async fn h_on_the_first_card_does_nothing() {
    let mut app = swipe_app().await;
    let first = app.swipe_current().cloned().unwrap();
    key(&mut app, 'h');
    assert_eq!(app.swipe_current(), Some(&first));
    assert_eq!(decision_of(&app, &first), Decision::Pending);
    assert_eq!(app.screen, Screen::Swipe);
}

#[tokio::test]
async fn h_from_the_summary_returns_to_the_last_card() {
    let mut app = swipe_app().await;
    let last = app.swipe.cards.last().cloned().unwrap();
    while app.swipe_current().is_some() {
        key(&mut app, 'l');
    }
    // Accepting the last card opens the all-done popup; a key dismisses it.
    assert_eq!(app.overlay, Overlay::AllDone);
    assert!(screen_text(&app).contains("Hurray!"));
    code(&mut app, KeyCode::Esc);
    assert!(screen_text(&app).contains("All changes triaged"));
    key(&mut app, 'h');
    assert_eq!(app.swipe_current(), Some(&last));
    assert!(screen_text(&app).contains("4 of 4"));
    assert_eq!(decision_of(&app, &last), Decision::Accepted);
}

#[tokio::test]
async fn re_deciding_a_card_overrides_it() {
    let mut app = swipe_app().await;
    let first = app.swipe_current().cloned().unwrap();
    key(&mut app, 'l');
    key(&mut app, 'h');
    key(&mut app, 'i');
    assert_eq!(decision_of(&app, &first), Decision::Investigate);
    assert_eq!(app.decision_counts(), (0, 1, 3));
    key(&mut app, 'h');
    code(&mut app, KeyCode::Right);
    assert_eq!(decision_of(&app, &first), Decision::Accepted);
    assert_eq!(app.decision_counts(), (1, 0, 3));
}

#[tokio::test]
async fn undo_restores_the_previous_decision_and_the_cursor() {
    let mut app = swipe_app().await;
    let first = app.swipe_current().cloned().unwrap();
    key(&mut app, 'l'); // first accepted
    key(&mut app, 'l'); // second accepted
    key(&mut app, 'h');
    key(&mut app, 'h'); // back on first
    key(&mut app, 'i'); // first accepted -> flagged, cursor on second
    assert_eq!(app.swipe.cursor, 1);
    key(&mut app, 'u');
    // The earlier decision comes back (not "pending") and so does the cursor.
    assert_eq!(decision_of(&app, &first), Decision::Accepted);
    assert_eq!(app.swipe_current(), Some(&first));
    key(&mut app, 'u');
    assert_eq!(app.swipe.cursor, 1);
    assert_eq!(
        decision_of(&app, &app.swipe.cards[1].clone()),
        Decision::Pending
    );
}

#[tokio::test]
async fn i_and_left_investigate_but_h_does_not() {
    let mut app = swipe_app().await;
    let ids = app.swipe.cards.clone();
    key(&mut app, 'i');
    code(&mut app, KeyCode::Left);
    assert_eq!(decision_of(&app, &ids[0]), Decision::Investigate);
    assert_eq!(decision_of(&app, &ids[1]), Decision::Investigate);
    // h moves back without deciding anything.
    key(&mut app, 'h');
    assert_eq!(app.swipe.cursor, 1);
    assert_eq!(decision_of(&app, &ids[2]), Decision::Pending);
    assert_eq!(app.decision_counts(), (0, 2, 2));

    // In the list, h still investigates.
    let (review, _repo) = fixture_review().await;
    let mut list = app_for(review, Config::default());
    let id = list.selected_id().unwrap();
    key(&mut list, 'h');
    assert_eq!(decision_of(&list, &id), Decision::Investigate);
}

#[tokio::test]
async fn the_swipe_footer_shows_next_previous_and_investigate() {
    let mut app = swipe_app().await;
    let text = screen_text(&app);
    for hint in ["l accept · next", "h previous", "i investigate", "u undo"] {
        assert!(text.contains(hint), "{hint} missing:\n{text}");
    }
    // Wide enough for everything: the buttons are clickable.
    let ids = app.swipe.cards.clone();
    let footer_y = 25;
    let find_footer = |app: &App, a| {
        app.targets
            .borrow()
            .iter()
            .rev()
            .find(|(r, t)| *t == teer::tui::app::Target::Action(a) && r.y == footer_y)
            .map(|(r, _)| *r)
            .unwrap()
    };
    let press = |app: &mut App, r: ratatui::layout::Rect| {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.on_event(Event::Mouse(MouseEvent {
                kind,
                column: r.x + 1,
                row: r.y,
                modifiers: KeyModifiers::NONE,
            }));
        }
    };
    let r = find_footer(&app, teer::config::Action::Accept);
    press(&mut app, r);
    assert_eq!(decision_of(&app, &ids[0]), Decision::Accepted);
    assert_eq!(app.swipe.cursor, 1);
    render(&app, 90, 26);
    let r = find_footer(&app, teer::config::Action::PrevCard);
    press(&mut app, r);
    assert_eq!(app.swipe.cursor, 0);
    render(&app, 90, 26);
    let r = find_footer(&app, teer::config::Action::Investigate);
    press(&mut app, r);
    assert_eq!(decision_of(&app, &ids[0]), Decision::Investigate);
}

#[tokio::test]
async fn long_code_lines_wrap_instead_of_being_cut() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    select(&mut app, "charge_order");
    // 48 columns: the code's box takes 4 of them; tall enough for all of it.
    let list = format!("{}", render(&app, 48, 60).backend());
    let lines: Vec<&str> = list.lines().collect();
    let line_of = |text: &str| lines.iter().position(|l| l.contains(text)).expect(&list);
    // The rest goes on the next line, under the statement, with no number
    // or sign, and words stay whole.
    let at = line_of("│   9 +     if order.currency not in");
    assert!(
        lines[at + 1].starts_with("\" │           SUPPORTED:"),
        "{list}"
    );
    let at = line_of("│  10 +         raise ValueError(");
    assert!(
        lines[at + 1].starts_with("\" │               \"unsupported currency\")"),
        "{list}"
    );
    // No code is cut: the only ellipsis is the "more lines" hint.
    let code = &lines[line_of("code change")..];
    assert!(
        code.iter()
            .filter(|l| l.contains('…'))
            .all(|l| l.contains("more line")),
        "{list}"
    );
}
