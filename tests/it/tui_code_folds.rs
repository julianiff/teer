//! The queue's preview shows only the code the agent's statements cite:
//! each cited line with a little around it, the rest left out with a quiet
//! `...` under the line numbers. The unit view shows the whole change, the
//! statements written into it the same way.

use crate::common;

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
use teer::review::model::SessionRef;
use teer::review::{BuildOptions, build_review};
use teer::session::app::{AppEvent, Screen};
use teer::tui::app::App;
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
    teer::start::prepare_all(
        &mut review,
        Arc::new(MockAdapter::new(responder)),
        session.clone(),
        2,
    )
    .await;
    let mut app = App::new(review, Config::default(), Keymap::default());
    app.start(true, None, Vec::new(), Some(session));
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
async fn the_preview_shows_only_the_code_the_statements_cite() {
    let (app, _repo) = app().await;
    assert_eq!(app.review.units.len(), 1);
    assert_eq!(app.screen, Screen::Queue);
    let out = screen(&app);
    folded(&out, "queue preview");
    // The statement is in the code, not listed above it.
    assert!(
        out.contains("│ Charges the order a second time. │"),
        "{out}"
    );
    assert!(!out.contains("~ Charges the order a second time."), "{out}");
}

#[tokio::test]
async fn the_unit_view_shows_the_whole_change_with_the_statement_in_it() {
    let (mut app, _repo) = app().await;
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(app.screen, Screen::Unit);
    let out = screen(&app);
    // Every change shows, the one no statement cites too; only unchanged
    // lines between hunks fold.
    for shown in [
        "   4 + audit(order)",
        "  26 + charge_twice(order)",
        "  27   step_27(order)",
    ] {
        assert!(out.contains(shown), "{shown}\n{out}");
    }
    assert!(
        out.contains("│ Charges the order a second time. │"),
        "{out}"
    );
    assert!(!out.contains("~ Charges the order a second time."), "{out}");
    // The advice arrives in its tile; the code stays as it was.
    let units: Vec<(String, String)> = app
        .review
        .units
        .iter()
        .map(|u| (u.id.clone(), u.fingerprint.clone()))
        .collect();
    for (id, fingerprint) in units {
        app.on_app_event(AppEvent::Dive(DiveEvent::Ready {
            id,
            fingerprint,
            kind: teer::review::model::DiveKind::Recommend,
            text: "Looks fine.".into(),
            sources: Vec::new(),
            visual: None,
            behavior: None,
            coverage: None,
        }));
    }
    // The answer sits under the code: at the end of the text.
    press(&mut app, KeyCode::Char('G'));
    let out = screen(&app);
    assert!(
        out.contains("Advice") && out.contains("Looks fine."),
        "{out}"
    );
}
