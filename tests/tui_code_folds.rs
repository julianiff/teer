//! Every code box that shows the agent's statements, in the list preview,
//! on the swipe card and on the Flagged screen, shows only the code they
//! cite: each cited line with a little around it, the rest left out with
//! a quiet `...` under the line numbers.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};
use teer::agent::mock::MockAdapter;
use teer::config::Config;
use teer::keymap::Keymap;
use teer::review::dive::DiveEvent;
use teer::review::model::{Decision, SessionRef};
use teer::review::{BuildOptions, build_review};
use teer::tui::app::{App, AppEvent, Screen};
use teer::tui::ui;

/// A 30-line function; changed, at line 4 and at line 26.
fn settle(changed: bool) -> String {
    let mut s = String::from("def settle(order):\n");
    for i in 2..=30 {
        let step = match (changed, i) {
            (true, 4) => "    audit(order)".to_string(),
            (true, 26) => "    charge_twice(order)".to_string(),
            _ => format!("    step_{i}(order)"),
        };
        s.push_str(&step);
        s.push('\n');
    }
    s
}

/// One statement, citing only the change at line 26.
fn responder(prompt: &str, _schema: &Value) -> anyhow::Result<Value> {
    let id = prompt
        .lines()
        .find_map(|l| l.strip_prefix("### Unit "))
        .unwrap_or_default();
    Ok(json!({"units": [{
        "id": id,
        "summary": "Charges the order twice.",
        "changes": [{
            "marker": "changed",
            "statement": "Charges the order a second time.",
            "risk": "high",
            "evidence": [{"side": "new", "line": 26}]
        }]
    }]}))
}

async fn app() -> (App, FixtureRepo) {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("settle.py", &settle(false)).commit("init");
    repo.write("settle.py", &settle(true));
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let session = SessionRef {
        agent: "mock".into(),
        id: "s1".into(),
        cwd: PathBuf::from("/work/shop"),
    };
    teer::cli::prepare_all(
        &mut review,
        Arc::new(MockAdapter::new(responder)),
        session.clone(),
        2,
    )
    .await;
    let mut app = App::new(review, Config::default(), Keymap::default());
    app.start(true, None, Vec::new(), Some(session), false);
    app.open_v1_list();
    (app, repo)
}

fn screen(app: &App) -> String {
    let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    format!("{}", t.backend())
}

fn press(app: &mut App, c: KeyCode) {
    app.on_event(Event::Key(KeyEvent::new(c, KeyModifiers::NONE)));
}

/// The cited line 26 with two lines on either side; lines 1-24, the change
/// at line 4 among them, left out as `...`; nothing else.
fn folded(out: &str, place: &str) {
    for shown in [
        "  25   step_25(order)",
        "  26 - step_26(order)",
        "  26 + charge_twice(order)",
        "  27   step_27(order)",
        "  28   step_28(order)",
    ] {
        assert!(out.contains(shown), "{place}: {shown}\n{out}");
    }
    let dots = out.lines().filter(|l| l.contains("│ ... ")).count();
    assert_eq!(dots, 1, "{place}\n{out}");
    for hidden in ["step_24(order)", "audit(order)", "step_29(order)"] {
        assert!(!out.contains(hidden), "{place}: {hidden}\n{out}");
    }
}

#[tokio::test]
async fn every_code_box_with_statements_shows_only_the_code_they_cite() {
    let (mut app, _repo) = app().await;
    assert_eq!(app.review.units.len(), 1);
    folded(&screen(&app), "list preview");
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(app.screen, Screen::Swipe);
    folded(&screen(&app), "swipe card");
    press(&mut app, KeyCode::Esc);
    app.review.units[0].decision = Decision::Investigate;
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(app.screen, Screen::Flagged);
    let out = screen(&app);
    folded(&out, "Flagged screen");
    // The same box: the statement is in the code, not listed above it.
    assert!(
        out.contains("│ Charges the order a second time. │"),
        "{out}"
    );
    assert!(!out.contains("~ Charges the order a second time."), "{out}");
    // An answer gets a tile of its own; the code keeps its tile. (The
    // recommendation was asked for as the review opened: it arrives.)
    let ids: Vec<String> = app.review.units.iter().map(|u| u.id.clone()).collect();
    for id in ids {
        app.on_app_event(AppEvent::Dive(DiveEvent::Ready {
            id,
            kind: teer::review::model::DiveKind::Recommend,
            text: "Looks fine.".into(),
            sources: Vec::new(),
            visual: None,
            behavior: None,
            coverage: None,
        }));
    }
    let out = screen(&app);
    folded(&out, "Flagged screen with an answer");
    assert!(
        out.contains("│ Charges the order a second time. │"),
        "{out}"
    );
    assert!(
        out.contains("╭ ␣w Recommend ✓") && out.contains("Looks fine."),
        "{out}"
    );
}
