//! Live checks of the providers other than Claude Code. Ignored by default;
//! they cost tokens.
//!
//! ```sh
//! # Codex, logged in (models from `codex debug models`):
//! TEER_LIVE_CODEX_FAST=gpt-6.1-terra TEER_LIVE_CODEX_SECOND=gpt-6.1-sol \
//!   cargo test --test live_providers codex -- --ignored --nocapture
//! # A model API:
//! TEER_LIVE_API_FORMAT=anthropic TEER_LIVE_API_KEY_ENV=ANTHROPIC_API_KEY \
//!   TEER_LIVE_API_FAST=claude-haiku-5-5 TEER_LIVE_API_SECOND=claude-sonnet-5-5 \
//!   cargo test --test live_providers api -- --ignored --nocapture
//! ```
//! `TEER_LIVE_API_URL` points at another server (OpenRouter, Ollama, ...).

use std::sync::Arc;

use teer::agent::api::ApiAdapter;
use teer::agent::codex::CodexAdapter;
use teer::agent::{AgentAdapter, PrepareModel};
use teer::config::{ApiConfig, ApiFormat};
use teer::review::dive::{DiveEvent, run_dive};
use teer::review::model::{DiveKind, Review, SessionRef, UnitStatus};
use teer::review::prepare::{PrepareEvent, second_opinions};
use teer::review::{BuildOptions, build_review};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

fn model(name: Option<String>, effort: &str) -> PrepareModel {
    PrepareModel {
        model: name,
        effort: Some(effort.into()).filter(|e: &String| !e.is_empty()),
    }
}

/// A small repo with an uncommitted change to two functions.
fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .current_dir(d.path())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(
        d.path().join("cart.py"),
        "def total(items):\n    return sum(i[\"price\"] for i in items)\n\n\ndef checkout(items, coupon=None):\n    t = total(items)\n    if coupon:\n        t = t * 0.9\n    return round(t, 2)\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    std::fs::write(
        d.path().join("cart.py"),
        "def total(items):\n    return sum(i[\"price\"] * i.get(\"qty\", 1) for i in items)\n\n\ndef checkout(items, coupon=None):\n    t = total(items)\n    if coupon == \"HALF\":\n        t = t * 0.5\n    elif coupon:\n        t = t * 0.9\n    return round(t, 2)\n",
    )
    .unwrap();
    d
}

/// Describe, check and ask one Flagged question, printing what came back.
async fn round_trip(adapter: Arc<dyn AgentAdapter>, name: &str) {
    let d = repo();
    let mut review: Review = build_review(d.path(), &BuildOptions::default()).unwrap();
    let session = SessionRef::fresh(name, review.repo_root.clone());

    let t = std::time::Instant::now();
    teer::cli::prepare_all(&mut review, adapter.clone(), session.clone(), 4).await;
    println!("described in {:.1}s", t.elapsed().as_secs_f64());
    let changed: Vec<_> = review
        .units
        .iter()
        .filter(|u| u.status == UnitStatus::Changed)
        .cloned()
        .collect();
    for u in &changed {
        println!("{}: {:?}\n  {:#?}", u.symbol, u.summary, u.changes);
        assert!(!u.changes.is_empty(), "{} has no statements", u.symbol);
    }
    let file = review.files[0].clone();

    let t = std::time::Instant::now();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let units = changed.iter().map(|u| (u.clone(), file.clone())).collect();
    second_opinions(adapter.clone(), session.clone(), units, tx).await;
    while let Ok(ev) = rx.try_recv() {
        if let PrepareEvent::Second { id, opinion } = ev {
            println!("second opinion on {id}: {opinion:?}");
            assert!(opinion.is_ok(), "{opinion:?}");
        }
    }
    println!("checked in {:.1}s", t.elapsed().as_secs_f64());

    let t = std::time::Instant::now();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let unit = changed
        .iter()
        .find(|u| u.symbol == "total")
        .unwrap()
        .clone();
    run_dive(
        adapter,
        session,
        unit,
        file,
        DiveKind::Recommend,
        false,
        None,
        tx,
    )
    .await;
    match rx.recv().await.unwrap() {
        DiveEvent::Ready { text, sources, .. } => {
            println!("recommend: {text}\nsources: {sources:?}")
        }
        other => panic!("{other:?}"),
    }
    println!("answered in {:.1}s", t.elapsed().as_secs_f64());
}

#[tokio::test]
#[ignore]
async fn codex_describes_checks_and_answers() {
    let adapter = CodexAdapter {
        prepare: model(env("TEER_LIVE_CODEX_FAST"), "low"),
        second: model(env("TEER_LIVE_CODEX_SECOND"), "low"),
        ..CodexAdapter::default()
    };
    round_trip(Arc::new(adapter), "codex").await;
}

#[tokio::test]
#[ignore]
async fn api_describes_checks_and_answers() {
    let Some(format) = env("TEER_LIVE_API_FORMAT") else {
        return;
    };
    let api = ApiConfig {
        format: if format == "openai" {
            ApiFormat::Openai
        } else {
            ApiFormat::Anthropic
        },
        base_url: env("TEER_LIVE_API_URL").unwrap_or_default(),
        key_env: env("TEER_LIVE_API_KEY_ENV").unwrap_or_default(),
    };
    let key = teer::cli::api_key(&api).unwrap();
    let second = env("TEER_LIVE_API_SECOND");
    let adapter = ApiAdapter::new(
        api.format,
        &api.url(),
        key,
        model(env("TEER_LIVE_API_FAST"), ""),
        model(second.clone(), ""),
        model(second, ""),
    )
    .unwrap();
    round_trip(Arc::new(adapter), "api").await;
}

/// Codex's strict mode takes every schema teer asks with, made strict.
#[tokio::test]
#[ignore]
async fn codex_takes_every_schema() {
    use teer::review::prompts as p;
    let schemas = [
        ("units", p::units_schema(true)),
        ("summary", p::summary_schema()),
        ("second", p::second_schema()),
        ("ask", p::ask_schema()),
        ("dive", p::dive_schema()),
        ("visual", p::visual_schema()),
        ("behavior", p::behavior_schema()),
        ("tests", p::tests_schema()),
        ("commit", p::commit_message_schema()),
    ];
    let d = tempfile::tempdir().unwrap();
    let adapter = Arc::new(CodexAdapter {
        prepare: model(env("TEER_LIVE_CODEX_FAST"), "low"),
        ..CodexAdapter::default()
    });
    let session = SessionRef::fresh("codex", d.path().to_path_buf());
    let asks = schemas.iter().map(|(name, schema)| {
        let (adapter, session) = (adapter.clone(), session.clone());
        async move {
            let prompt = "Schema test: answer with the smallest valid JSON object: empty arrays, \
one-word strings, the first enum value. Do not use tools.";
            (*name, adapter.ask_prepare(&session, prompt, schema).await)
        }
    });
    let mut failed = Vec::new();
    for (name, r) in futures::future::join_all(asks).await {
        match r {
            Ok(v) => println!("{name}: ok {v}"),
            Err(e) => {
                println!("{name}: FAILED {e:#}");
                failed.push(name);
            }
        }
    }
    assert!(failed.is_empty(), "{failed:?}");
}
