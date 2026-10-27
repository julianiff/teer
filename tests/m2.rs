//! M2: preparing a review through an agent adapter, with evidence validation.

mod common;

use std::sync::Arc;

use common::*;
use serde_json::{Value, json};
use teer::agent::mock::MockAdapter;
use teer::review::model::{Prep, SessionRef, UnitStatus};
use teer::review::{BuildOptions, build_review};

/// Answers every unit in the prompt with one valid claim and one claim
/// citing a line far outside the unit.
fn responder(prompt: &str, _schema: &Value) -> anyhow::Result<Value> {
    let mut units = Vec::new();
    let mut id = None;
    for line in prompt.lines() {
        if let Some(rest) = line.strip_prefix("### Unit ") {
            id = Some(rest.to_string());
        }
        if let (Some(uid), Some(r)) = (&id, line.strip_prefix("new lines: ")) {
            let side_line = match r.split_once('-') {
                Some((a, _)) => json!({"side": "new", "line": a.parse::<u32>().unwrap()}),
                None => json!({"side": "old", "line": 0}),
            };
            units.push(json!({
                "id": uid,
                "changes": [
                    {"marker": "changed", "statement": "Does something new.", "risk": "medium", "evidence": [side_line]},
                    {"marker": "added", "statement": "Invented claim.", "risk": "high", "evidence": [{"side": "new", "line": 9999}]}
                ]
            }));
            id = None;
        }
    }
    Ok(json!({ "units": units }))
}

#[tokio::test]
async fn prepares_review_and_drops_claims_without_evidence() {
    let env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD)
        .write("api.py", "def handle(req):\n    return charge_order(req)\n")
        .commit("init");
    repo.write("checkout.py", PY_NEW);
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();

    let mock = MockAdapter::new(responder);
    let prompts = mock.prompts.clone();
    let prepare_calls = mock.prepare_calls.clone();
    let session = SessionRef {
        agent: "mock".into(),
        id: "s1".into(),
        cwd: repo.path(),
    };
    teer::cli::prepare_all(&mut review, Arc::new(mock), session, 2).await;
    // Every request went through the preparation path (fast model, no tools).
    assert_eq!(
        *prepare_calls.lock().unwrap(),
        prompts.lock().unwrap().len()
    );
    assert!(*prepare_calls.lock().unwrap() > 0);

    for u in &review.units {
        match u.status {
            UnitStatus::NoBehaviorChange => {
                assert_eq!(u.prep, Prep::Skipped);
                assert!(u.changes.is_empty());
            }
            UnitStatus::Removed => {
                // Its only valid evidence would be on the old side; the mock cites none.
                assert_eq!(u.prep, Prep::Unavailable, "{}", u.symbol);
                assert!(u.changes.is_empty());
            }
            _ => {
                assert_eq!(u.prep, Prep::Ready, "{}", u.symbol);
                assert_eq!(u.changes.len(), 1, "{}: {:?}", u.symbol, u.changes);
                assert_eq!(u.changes[0].statement, "Does something new.");
            }
        }
    }

    let all = prompts.lock().unwrap().join("\n");
    // No-behavior-change units never reach the agent.
    assert!(!all.contains("### Unit checkout.py::compute_total"));
    assert!(all.contains("### Unit checkout.py::charge_order"));
    // Callers and numbered code are included.
    assert!(all.contains("callers: api.py::handle"));
    assert!(all.contains("    9 |     if order.currency not in SUPPORTED:"));

    let log = std::fs::read_to_string(env.path().join("data").join("teer.log")).unwrap();
    assert!(log.contains("dropped claim for checkout.py::charge_order: \"Invented claim.\""));
}

#[tokio::test]
async fn agent_failure_marks_units_unavailable() {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("a.py", "def a():\n    return 1\n")
        .commit("init");
    repo.write("a.py", "def a():\n    return 2\n");
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let mock = MockAdapter::new(|_, _| anyhow::bail!("session not found"));
    let session = SessionRef {
        agent: "mock".into(),
        id: "s1".into(),
        cwd: repo.path(),
    };
    teer::cli::prepare_all(&mut review, Arc::new(mock), session, 4).await;
    assert_eq!(review.units[0].prep, Prep::Unavailable);
}

#[test]
fn batches_small_units_together() {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD).commit("init");
    repo.write("checkout.py", PY_NEW);
    let review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let todo = review.to_prepare(None);
    let batches = teer::review::prepare::batches(&todo);
    assert_eq!(batches.iter().map(|b| b.len()).sum::<usize>(), todo.len());
    assert_eq!(batches.len(), 1, "small units share one request");
}

#[test]
fn reopening_restores_decisions() {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("a.py", "def a():\n    return 1\n\ndef b():\n    return 1\n")
        .commit("init");
    repo.write("a.py", "def a():\n    return 2\n\ndef b():\n    return 2\n");
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    for u in review.units.iter_mut() {
        u.decision = teer::review::model::Decision::Accepted;
    }
    teer::review::store::save(&review).unwrap();

    // Same changes: decisions come back.
    let mut again = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    teer::cli::restore(&mut again);
    assert!(
        again
            .units
            .iter()
            .all(|u| u.decision == teer::review::model::Decision::Accepted)
    );

    // b changes again: only a keeps its decision.
    repo.write("a.py", "def a():\n    return 2\n\ndef b():\n    return 3\n");
    let mut third = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    teer::cli::restore(&mut third);
    let d = |s: &str| third.units.iter().find(|u| u.symbol == s).unwrap().decision;
    assert_eq!(d("a"), teer::review::model::Decision::Accepted);
    assert_eq!(d("b"), teer::review::model::Decision::Pending);
}
