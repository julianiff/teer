//! The queue, the unit view's code, reset, the editor.
//! Rendered screens are snapshot tested; behavior runs through the app's
//! event handlers and the runtime with a mocked agent.

use crate::common;

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
use teer::config::Config;
use teer::keymap::Keymap;
use teer::review::model::{Decision, Review, SessionRef};
use teer::review::{BuildOptions, build_review};
use teer::tui::app::{App, AppEvent, Effect, Grouping, Overlay, Screen};
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
    let mut app = App::new(review, cfg, Keymap::default());
    let effects = app.start(true, None, Vec::new(), Some(session()));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Prepare(_))),
        "already prepared: {effects:?}"
    );
    app
}

/// The queue listed by file (`zf`): each file's name above its units.
fn by_file(app: &mut App) {
    app.grouping = Grouping::File;
    app.rebuild_order();
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

// ---- M3: the queue -----------------------------------------------------

#[tokio::test]
async fn queue_snapshot() {
    let (review, _repo) = fixture_review().await;
    let app = app_for(review, Config::default());
    assert_eq!(app.screen, Screen::Queue);
    insta::assert_snapshot!("queue", render(&app, 90, 30).backend());
}

#[tokio::test]
async fn queue_navigation() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    let n = app.order.len();
    key(&mut app, 'j');
    assert_eq!(app.sel, 1);
    key(&mut app, 'G');
    assert_eq!(app.sel, n - 1);
    // The box's title is no stop: the first row is its first unit.
    v2::press(&mut app, "gg");
    assert_eq!(app.sel, 0);
    key(&mut app, 'k');
    assert_eq!(app.sel, 0);

    select(&mut app, "old_helper");
    insta::assert_snapshot!("queue_worth_a_look", render(&app, 90, 30).backend());
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
    select(&mut app, "charge_order");
    // In the unit view, the whole change with every statement in it.
    key(&mut app, 'l');
    assert_eq!(app.screen, Screen::Unit);
    let card = format!("{}", render(&app, 80, 24).backend());
    insta::assert_snapshot!("unit_view_cites", card);
    // What the agent wrote sits behind a bar, under a label naming who.
    assert!(card.contains("\" ┃ agent (haiku, sonnet) "), "{card}");
    assert!(
        card.contains("\" ┃ Changes how orders are charged."),
        "{card}"
    );
    // Each statement sits in a card under the last row it cites, inside
    // the code's box, and nothing else marks those rows; the removed row
    // has its old line number, the one the statement cites.
    for row in [
        "\" │   9 +     if order.currency not in SUPPORTED:",
        "\" │  10 +         raise ValueError(\"unsupported currency\")",
        "\" │     ╭──────",
        "\" │     │ Rejects unsupported currencies. 2 callers don't handle it. │",
        "\" │  11       result = pay(order)",
        "\" │  10 -     send_receipt(order)",
        "\" │     │ No longer sends the receipt email. │",
    ] {
        assert!(card.contains(row), "{row}:\n{card}");
    }
    // Further down, scrolling, the third card.
    let mut seen = card;
    for _ in 0..10 {
        ctrl(&mut app, 'd');
        seen.push_str(&format!("{}", render(&app, 80, 24).backend()));
    }
    assert!(
        seen.contains("\" │     │ Saves the payment on the order"),
        "{seen}"
    );
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
    let mut app = App::new(review, cfg, Keymap::default());
    let effects = app.start(true, None, Vec::new(), Some(session()));
    // Straight into the queue: there is no file overview any more.
    assert_eq!(app.screen, Screen::Queue);
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
        effects = v2::press(&mut app, "]f");
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
    let mut app = App::new(fresh, Config::default(), Keymap::default());
    let s = SessionRef::fresh("claude", "/work/shop".into());
    let effects = app.start(true, None, Vec::new(), Some(s.clone()));
    assert_eq!(app.screen, Screen::Queue);
    assert_eq!(app.review.agent_session, Some(s));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Prepare(ids) if !ids.is_empty()))
    );
    let screen = format!("{}", render(&app, 90, 8).backend());
    assert!(screen.contains("preparing 0/4"), "{screen}");

    // Without Claude Code, the review still opens and says why.
    let mut app = App::new(review, Config::default(), Keymap::default());
    let effects = app.start(
        false,
        Some("Claude Code ('claude') isn't on your PATH.".into()),
        Vec::new(),
        None,
    );
    assert!(effects.is_empty());
    assert_eq!(app.screen, Screen::Queue);
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
async fn the_queue_has_one_line_per_unit_and_code_in_every_preview() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    by_file(&mut app);
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
        // icon under the file's name), its name and state; listed by file,
        // the preview pane below says where it is.
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
async fn the_flagged_screens_old_keys_do_nothing_in_the_queue() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    select(&mut app, "charge_order");
    for keys in [
        "o", "x", "r", "s", "zz", "gt", "gT", "<tab>", "<s-tab>", "<right>", "<left>", "<space>w",
        "<space>m", "<space>a",
    ] {
        assert!(v2::press(&mut app, keys).is_empty(), "{keys}");
        assert_eq!(app.screen, Screen::Queue, "{keys}");
        assert_eq!(app.overlay, Overlay::None, "{keys}");
        assert_eq!(app.selected().unwrap().symbol, "charge_order", "{keys}");
        assert!(
            app.review
                .units
                .iter()
                .all(|u| u.decision != Decision::Investigate),
            "{keys}"
        );
    }
}

#[tokio::test]
async fn gf_opens_the_unit_in_the_editor_at_its_line() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    select(&mut app, "charge_order");
    let line = app.selected().unwrap().display_line();
    let want = Effect::Edit {
        file: "checkout.py".into(),
        line,
    };
    assert_eq!(v2::press(&mut app, "gf"), vec![want]);
}

#[tokio::test]
async fn reset_clears_every_decision_after_confirming() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    key(&mut app, 'a');
    v2::press(&mut app, "f<esc>");
    assert!(
        app.review
            .units
            .iter()
            .any(|u| u.decision != Decision::Pending)
    );

    // esc cancels.
    assert!(v2::press(&mut app, ":e!<cr>").is_empty());
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

    // enter resets: nothing is looked at; units with no behavior change are
    // accepted for you again. Then the review re-runs (Phase 7).
    v2::press(&mut app, ":e!<cr>");
    assert_eq!(
        code(&mut app, KeyCode::Enter),
        vec![Effect::Save, Effect::Rerun]
    );
    for u in &app.review.units {
        if u.status == teer::review::model::UnitStatus::NoBehaviorChange {
            assert!(u.auto_accepted(), "{}", u.id);
        } else {
            assert_eq!(u.decision, Decision::Pending, "{}", u.id);
        }
    }
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
    let mut app = app_for(review, Config::default());
    by_file(&mut app);
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
    // The units with no behavior change are accepted for you, not listed.
    assert!(!text.contains("compute_total"), "{text}");
    assert!(at("● Cart::add") < at("3 units accepted for you, za shows them"));
    assert_eq!(
        symbols(&app),
        vec!["charge_order", "old_helper", "new_helper", "Cart::add"]
    );
    // Start on the file's first unit: its close look.
    assert_eq!(app.selected().unwrap().symbol, "charge_order");
    // `za` lists them last in the file, behind their icon.
    v2::press(&mut app, "za");
    let text = format!("{}", render(&app, 90, 30).backend());
    assert!(
        text.find("● Cart::add") < text.find("○ compute_total"),
        "{text}"
    );
    assert!(text.contains("za hides them"), "{text}");
    assert_eq!(app.selected().unwrap().symbol, "charge_order");
}

#[tokio::test]
async fn deciding_does_not_move_units_and_pending_jumps_skip_decided() {
    use teer::tui::app::Target;
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    let before = symbols(&app);
    // Accept the first (charge_order); selection moves to the next pending one.
    key(&mut app, 'a');
    assert_eq!(symbols(&app), before);
    assert_eq!(app.selected().unwrap().symbol, "old_helper");
    assert_eq!(app.review.units[app.order[0]].decision, Decision::Accepted);
    app.rebuild_order();
    assert_eq!(symbols(&app), before, "rebuilding keeps the order too");
    assert_eq!(app.selected().unwrap().symbol, "old_helper");

    // Flag old_helper (esc leaves its note line); decided units are skipped
    // by n and N.
    v2::press(&mut app, "f<esc>");
    assert_eq!(app.selected().unwrap().symbol, "new_helper");
    v2::press(&mut app, "gg");
    assert_eq!(app.sel, 0);
    key(&mut app, 'n');
    assert_eq!(app.selected().unwrap().symbol, "new_helper");
    key(&mut app, 'N');
    assert_eq!(app.sel, 2, "no pending unit before new_helper");
    app.sel = 3;
    key(&mut app, 'N');
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

fn decision_of(app: &App, symbol: &str) -> Decision {
    app.review
        .units
        .iter()
        .find(|u| u.symbol == symbol)
        .unwrap()
        .decision
}

fn screen_text(app: &App) -> String {
    format!("{}", render(app, 90, 26).backend())
}

#[tokio::test]
async fn all_accepted_says_nothing_is_pending_and_counts_units_accepted_for_you() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    for u in &mut app.review.units {
        u.decision = Decision::Accepted;
    }
    let text = screen_text(&app);
    assert!(text.contains("Nothing pending. :w to finish"), "{text}");
    // The 3 units with no behavior change count, accepted for you.
    assert!(text.contains("7 accepted"), "{text}");
    // A flagged unit left over is counted too.
    app.review.units[0].decision = Decision::Investigate;
    let text = screen_text(&app);
    assert!(text.contains("Nothing pending. :w to finish"), "{text}");
    assert!(text.contains("1 flagged"), "{text}");
}

#[tokio::test]
async fn re_deciding_a_unit_overrides_it() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    assert_eq!(app.selected().unwrap().symbol, "charge_order");
    key(&mut app, 'a');
    key(&mut app, 'k');
    v2::press(&mut app, "f<esc>");
    assert_eq!(decision_of(&app, "charge_order"), Decision::Investigate);
    assert_eq!(
        app.decision_counts(),
        (3, 1, 3),
        "the 3 accepted for you count"
    );
    v2::press(&mut app, "gga");
    assert_eq!(decision_of(&app, "charge_order"), Decision::Accepted);
    assert_eq!(app.decision_counts(), (4, 0, 3));
}

#[tokio::test]
async fn undo_restores_the_previous_decision_and_the_cursor() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    key(&mut app, 'a'); // charge_order accepted
    key(&mut app, 'a'); // old_helper accepted
    v2::press(&mut app, "gg");
    v2::press(&mut app, "f<esc>"); // charge_order accepted -> flagged
    assert_eq!(decision_of(&app, "charge_order"), Decision::Investigate);
    assert_eq!(app.selected().unwrap().symbol, "new_helper");
    key(&mut app, 'u');
    // The earlier decision comes back (not "pending") and so does the cursor.
    assert_eq!(decision_of(&app, "charge_order"), Decision::Accepted);
    assert_eq!(app.selected().unwrap().symbol, "charge_order");
    key(&mut app, 'u');
    assert_eq!(decision_of(&app, "old_helper"), Decision::Pending);
    assert_eq!(app.selected().unwrap().symbol, "old_helper");
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
