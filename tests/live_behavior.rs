//! Live Behavior check against real Claude Code: a fixture change in Python
//! and one in JavaScript, each probed by the real agent and run in the
//! sandbox. Ignored by default; it costs tokens and takes a minute or two:
//!
//! ```sh
//! cargo test --test live_behavior -- --ignored --nocapture --test-threads=1
//! ```
#![cfg(target_os = "macos")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{FixtureRepo, isolate_env};
use teer::agent::AgentAdapter;
use teer::agent::claude::ClaudeAdapter;
use teer::review::behavior::{Job, run};
use teer::review::dive::DiveEvent;
use teer::review::model::SessionRef;
use teer::review::{BuildOptions, build_review};

async fn probe(r: &FixtureRepo, symbol: &str) {
    let review = build_review(&r.path(), &BuildOptions::default()).unwrap();
    let unit = review
        .units
        .iter()
        .find(|u| u.symbol == symbol)
        .unwrap_or_else(|| panic!("no unit {symbol}"))
        .clone();
    let job = Job {
        repo: review.repo_root.clone(),
        base: review.base.clone(),
        files: review.files.clone(),
        review_id: review.id.clone(),
        file: review.file_of(&unit).unwrap().clone(),
        unit,
        timeout: Duration::from_secs(60),
    };
    let adapter: Arc<dyn AgentAdapter> = Arc::new(ClaudeAdapter::default());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let started = std::time::Instant::now();
    run(
        adapter,
        SessionRef::fresh("claude", review.repo_root.clone()),
        job,
        tx,
    )
    .await;
    let ev = rx.recv().await.unwrap();
    println!("--- {symbol} ({:.0?})", started.elapsed());
    match &ev {
        DiveEvent::Ready {
            text,
            behavior: Some(b),
            ..
        } => {
            println!(
                "{text}\ncommand: {:?}\nreceipt: {}",
                b.command,
                b.dir.display()
            );
            for c in &b.cases {
                println!(
                    "{} {:<14} {:<30} before={:?} after={:?} expect={:?}",
                    if c.surprise() {
                        "!"
                    } else if c.changed() {
                        "≠"
                    } else {
                        "="
                    },
                    c.name,
                    c.input,
                    c.before,
                    c.after,
                    c.expect
                );
            }
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn live_python_probe() {
    isolate_env();
    let r = FixtureRepo::new();
    r.write(
        "shop/pricing.py",
        "def shipping_cost(weight_kg, country):\n    if country == \"CH\":\n        return 7.0\n    if weight_kg > 2:\n        return 15.0 + (weight_kg - 2) * 2.5\n    return 9.5\n",
    )
    .write(
        "shop/cart.py",
        "from shop.pricing import shipping_cost\n\n\ndef total(items, country):\n    weight = sum(i[\"kg\"] for i in items)\n    return sum(i[\"price\"] for i in items) + shipping_cost(weight, country)\n",
    )
    .write("shop/__init__.py", "")
    .commit("base");
    r.write(
        "shop/pricing.py",
        "def shipping_cost(weight_kg, country):\n    if country in (\"CH\", \"LI\"):\n        return 7.0\n    if weight_kg >= 2:\n        return round(15.0 + (weight_kg - 2) * 2.5)\n    return 9.5\n",
    );
    probe(&r, "shipping_cost").await;
}

#[tokio::test]
#[ignore]
async fn live_javascript_probe() {
    isolate_env();
    let r = FixtureRepo::new();
    r.write(
        "src/slug.js",
        "export function slugify(title) {\n  return title.toLowerCase().trim().replace(/\\s+/g, '-');\n}\n",
    )
    .write("package.json", "{\"type\": \"module\"}\n")
    .commit("base");
    r.write(
        "src/slug.js",
        "export function slugify(title, max = 40) {\n  return title\n    .normalize('NFKD')\n    .replace(/[\\u0300-\\u036f]/g, '')\n    .toLowerCase()\n    .trim()\n    .replace(/[^a-z0-9]+/g, '-')\n    .slice(0, max);\n}\n",
    );
    probe(&r, "slugify").await;
}

/// Behavior on one unit of a review teer saved, as if `t` were pressed on
/// it. Uses the real data dir (and so the repository's probe checkout):
///
/// ```sh
/// TEER_LIVE_REVIEW=~/.local/share/teer/reviews/<repo>/<file>.json TEER_LIVE_UNIT='::activate' \
///   cargo test --test live_behavior live_saved_review -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore]
async fn live_saved_review() {
    let (Ok(path), Ok(unit_id)) = (
        std::env::var("TEER_LIVE_REVIEW"),
        std::env::var("TEER_LIVE_UNIT"),
    ) else {
        return;
    };
    let review: teer::review::model::Review =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let unit = review
        .units
        .iter()
        .find(|u| u.id.ends_with(&unit_id))
        .unwrap_or_else(|| panic!("no unit ending in {unit_id}"))
        .clone();
    let repo = review
        .pr
        .as_ref()
        .and_then(|p| p.repo_dir.clone())
        .unwrap_or_else(|| review.repo_root.clone());
    let job = Job {
        repo,
        base: review.base.clone(),
        files: review.files.clone(),
        review_id: review.id.clone(),
        file: review.file_of(&unit).unwrap().clone(),
        unit,
        timeout: Duration::from_secs(120),
    };
    let adapter: Arc<dyn AgentAdapter> = Arc::new(ClaudeAdapter::default());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let started = std::time::Instant::now();
    let session = SessionRef::fresh("claude", review.repo_root.clone());
    let task = tokio::spawn(run(adapter, session, job, tx));
    while let Some(ev) = rx.recv().await {
        match ev {
            DiveEvent::Progress { text, .. } => {
                println!("[{:>4.0?}] {text}", started.elapsed())
            }
            DiveEvent::Ready {
                text,
                behavior: Some(b),
                ..
            } => {
                println!("[{:>4.0?}] {text}", started.elapsed());
                for (claim, check) in b.claim_checks() {
                    println!("  claim {check:?}: {claim}");
                }
                for c in &b.cases {
                    println!(
                        "  {} {:<24} claim {} after={:?}",
                        if c.surprise() { "!" } else { " " },
                        c.name,
                        c.claim,
                        c.after
                    );
                }
                println!("  tests: {:?} {:?}", b.tests, b.test_command);
                println!("  receipt: {}", b.dir.display());
            }
            other => println!("[{:>4.0?}] {other:?}", started.elapsed()),
        }
    }
    task.await.unwrap();
}

/// The Tests tile on one unit of a saved review, as if `c` then `a` were
/// pressed on it (read-only, no sandbox):
///
/// ```sh
/// TEER_LIVE_REVIEW=... TEER_LIVE_UNIT='::activate' \
///   cargo test --test live_behavior live_saved_review_tests -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore]
async fn live_saved_review_tests() {
    let (Ok(path), Ok(unit_id)) = (
        std::env::var("TEER_LIVE_REVIEW"),
        std::env::var("TEER_LIVE_UNIT"),
    ) else {
        return;
    };
    let review: teer::review::model::Review =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let unit = review
        .units
        .iter()
        .find(|u| u.id.ends_with(&unit_id))
        .unwrap()
        .clone();
    let started = std::time::Instant::now();
    let callers = teer::review::test_search::callers(&review, &unit);
    let search = match teer::review::dive::search_tests(&review.repo_root, &unit, &callers) {
        DiveEvent::Ready {
            coverage: Some(c), ..
        } => c.search.unwrap(),
        other => panic!("{other:?}"),
    };
    println!("[{:.0?}] found {}", started.elapsed(), search.total);
    let job = teer::review::dive::TestsJob {
        file: review.file_of(&unit).unwrap().clone(),
        target: teer::review::visual::target(&review, &unit),
        root: review.repo_root.clone(),
        unit,
        search,
    };
    let adapter: Arc<dyn AgentAdapter> = Arc::new(ClaudeAdapter::default());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let session = SessionRef::fresh("claude", review.repo_root.clone());
    teer::review::dive::interpret_tests(adapter, session, job, tx).await;
    let ev = loop {
        match rx.recv().await.unwrap() {
            DiveEvent::Progress { text, .. } => println!("[{:.0?}] {text}", started.elapsed()),
            ev => break ev,
        }
    };
    println!("[{:.0?}]", started.elapsed());
    match ev {
        DiveEvent::Ready {
            text,
            coverage: Some(c),
            ..
        } => {
            println!("{text}");
            for t in &c.covered {
                println!(
                    "  covered: {} ({}:{}-{}): {}",
                    t.name,
                    t.cite.file.display(),
                    t.cite.start,
                    t.cite.end,
                    t.checks
                );
            }
            for g in &c.gaps {
                println!(
                    "  gap [{:?}] {}: {} {:?}",
                    g.risk, g.case, g.danger, g.evidence
                );
            }
        }
        other => panic!("{other:?}"),
    }
}
