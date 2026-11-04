//! The second opinion and "expensive to undo": a scripted agent describes
//! the units, a second scripted model disputes one statement, and the
//! review, the queue's preview and the unit view show both.

use crate::common;

use std::sync::Arc;

use common::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};
use teer::agent::AgentAdapter;
use teer::agent::mock::MockAdapter;
use teer::config::Config;
use teer::keymap::Keymap;
use teer::review::attention::Level;
use teer::review::model::{Review, SessionRef};
use teer::review::prepare::{PrepareEvent, prepare};
use teer::review::prompts::SECOND_RULES;
use teer::review::{BuildOptions, build_review};
use teer::session::app::{Effect, Screen};
use teer::tui::app::App;

/// The first line of each unit in the prompt, by id.
fn units_in(prompt: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut id = None;
    for line in prompt.lines() {
        if let Some(rest) = line.strip_prefix("### Unit ") {
            id = Some(rest.to_string());
        }
        if let (Some(uid), Some(r)) = (&id, line.strip_prefix("new lines: "))
            && let Some((a, _)) = r.split_once('-')
        {
            out.push((uid.clone(), a.parse().unwrap()));
            id = None;
        }
    }
    out
}

/// Describes every unit with one statement; `charge_order` is expensive to
/// undo. As the second model: disputes `charge_order`'s statement, agrees
/// with the rest.
fn responder(prompt: &str, _schema: &Value) -> anyhow::Result<Value> {
    let second = prompt.starts_with(&SECOND_RULES[..40]);
    let units: Vec<Value> = units_in(prompt)
        .into_iter()
        .map(|(id, line)| {
            let ev = json!([{"side": "new", "line": line}]);
            let charge = id.ends_with("charge_order");
            if second {
                json!({
                    "id": id,
                    "risk": if charge { "high" } else { "low" },
                    "verdicts": [{
                        "n": 1,
                        "verdict": if charge { "dispute" } else { "agree" },
                        "why": if charge { "It also stops sending the receipt." } else { "Right." },
                        "evidence": ev
                    }],
                    "missed": []
                })
            } else {
                json!({
                    "id": id,
                    "summary": "Changes it.",
                    "changes": [{"marker": "changed", "statement": "Does something new.", "risk": "low", "evidence": ev}],
                    "undo": {
                        "expensive": charge,
                        "why": if charge { "Charges the card again on retry." } else { "" },
                        "evidence": if charge { ev.clone() } else { json!([]) }
                    }
                })
            }
        })
        .collect();
    Ok(json!({ "units": units }))
}

fn session(repo: &FixtureRepo) -> SessionRef {
    SessionRef::fresh("mock", repo.path())
}

async fn prepared(second: bool) -> (FixtureRepo, Review, MockAdapter) {
    isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD)
        .write("api.py", "def handle(req):\n    return charge_order(req)\n")
        .commit("init");
    repo.write("checkout.py", PY_NEW);
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let mock = MockAdapter::new(responder);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let units = review.to_prepare(None);
    prepare(
        Arc::new(mock.clone()),
        session(&repo),
        units,
        2,
        second,
        Arc::default(),
        tx,
    )
    .await;
    while let Ok(ev) = rx.try_recv() {
        if let PrepareEvent::Ready { checking, .. } = &ev {
            assert_eq!(*checking, second);
        }
        review.apply(ev);
    }
    review.agent_session = Some(session(&repo));
    (repo, review, mock)
}

fn screen(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| teer::tui::ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
        .collect()
}

fn press(app: &mut App, c: char) -> Vec<Effect> {
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )))
}

/// Type `keys` in Neovim notation (`gg`, `<space>t`).
fn keys(app: &mut App, keys: &str) -> Vec<Effect> {
    common::v2::press(app, keys)
}

#[tokio::test]
async fn a_disputed_statement_and_an_expensive_change_are_warnings() {
    let (_repo, review, mock) = prepared(true).await;
    assert!(*mock.second_calls.lock().unwrap() >= 1);
    let charge = review
        .units
        .iter()
        .find(|u| u.symbol == "charge_order")
        .unwrap();
    assert!(!charge.checks.second_pending);
    let cost = charge.checks.cost.as_ref().expect("expensive to undo");
    assert_eq!(cost.why, "Charges the card again on retry.");
    let second = charge.checks.second.as_ref().unwrap();
    assert!(!second.verdicts[0].agree);
    let doubts = second.disagreements(&charge.changes);
    assert_eq!(
        doubts,
        [
            "disputes \"Does something new.\": It also stops sending the receipt.",
            "rates it high risk, not low"
        ]
    );
    // An agreeing second opinion leaves a unit alone.
    let vat = review
        .units
        .iter()
        .find(|u| u.symbol == "new_helper")
        .unwrap();
    assert!(
        vat.checks
            .second
            .as_ref()
            .unwrap()
            .disagreements(&vat.changes)
            .is_empty()
    );

    let a = teer::review::attention::assess(&review, charge);
    assert!(a.level >= Level::Look);
    assert_eq!(a.cost.as_deref(), Some("Charges the card again on retry."));
    assert_eq!(a.doubts.len(), 2);

    // The queue's preview says why, a reason per line under "Why this
    // level"; the row keeps only the dispute's ⚠.
    let mut app = App::new(review, Config::default(), Keymap::default());
    assert_eq!(app.screen, Screen::Queue);
    while app.selected().is_some_and(|u| u.symbol != "charge_order") {
        press(&mut app, 'j');
    }
    let text = screen(&app, 120, 40);
    assert!(text.contains("Why this level"), "{text}");
    assert!(
        text.contains("· Expensive to undo: Charges the card"),
        "{text}"
    );
    assert!(text.contains("· The second model disputes"), "{text}");
    assert!(
        text.contains("· The second model rates it high risk, not low."),
        "{text}"
    );
    assert!(text.contains('⚠') && !text.contains('$'), "{text}");
}

#[tokio::test]
async fn the_unit_view_shows_the_second_model_where_it_disputes() {
    let (_repo, review, _mock) = prepared(true).await;
    let mut app = App::new(review, Config::default(), Keymap::default());
    app.has_agent = true;
    // The unit view on `symbol`, drawn, then back to the queue.
    let open = |app: &mut App, symbol: &str| {
        keys(app, "gg");
        while app.selected().is_some_and(|u| u.symbol != symbol) {
            press(app, 'j');
        }
        keys(app, "l");
        assert_eq!(app.screen, Screen::Unit);
        let text = screen(app, 120, 80);
        keys(app, "h");
        text
    };
    // Already checked while preparing: the unit view shows it, the model
    // that wrote it named next to the heading.
    let text = open(&mut app, "charge_order");
    assert!(
        text.contains("Second model  second opinion (sonnet)"),
        "{text}"
    );
    assert!(
        text.contains("A second model disagrees on 2 points"),
        "{text}"
    );
    assert!(text.contains("✗ \"Does something new.\""), "{text}");
    assert!(
        text.contains("It also stops sending the receipt. (L8)"),
        "{text}"
    );
    // Where it agrees, there is nothing to say.
    let text = open(&mut app, "new_helper");
    assert!(!text.contains("Second model"), "{text}");
    // Nothing asks it again: it ran while preparing.
    assert!(app.review.units.iter().all(|u| !u.checks.second_pending));
}

#[tokio::test]
async fn without_a_second_opinion_nothing_is_checked() {
    let (_repo, review, mock) = prepared(false).await;
    assert_eq!(*mock.second_calls.lock().unwrap(), 0);
    assert!(review.units.iter().all(|u| u.checks.second.is_none()));
    // The cost still comes with the description.
    let charge = review
        .units
        .iter()
        .find(|u| u.symbol == "charge_order")
        .unwrap();
    assert!(charge.checks.cost.is_some());
}

#[tokio::test]
async fn a_pull_request_check_carries_the_untrusted_notice() {
    let mock = MockAdapter::new(|_, _| Ok(json!({"units": []})));
    let wrapped = teer::pr::UntrustedCode(Arc::new(mock.clone()));
    wrapped
        .ask_second(
            &SessionRef::fresh("mock", ".".into()),
            "check this",
            &json!({}),
        )
        .await
        .unwrap();
    assert_eq!(*mock.second_calls.lock().unwrap(), 1);
    assert!(mock.prompts.lock().unwrap()[0].contains("SECURITY NOTE"));
}
