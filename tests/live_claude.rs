//! Live check against a real Claude Code session. Ignored by default;
//! it costs tokens and needs a repo with changes made by that session:
//!
//! ```sh
//! TEER_LIVE_REPO=/path/to/repo TEER_LIVE_SESSION=<id> \
//!   cargo test --test live_claude -- --ignored --nocapture --test-threads=1
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use teer::agent::AgentAdapter;
use teer::agent::claude::ClaudeAdapter;
use teer::review::model::{SessionRef, UnitStatus};
use teer::review::{BuildOptions, build_review};

fn live() -> Option<(PathBuf, SessionRef)> {
    let repo = PathBuf::from(std::env::var("TEER_LIVE_REPO").ok()?);
    let id = std::env::var("TEER_LIVE_SESSION").ok()?;
    let repo = teer::git::repo_root(&repo).unwrap();
    Some((
        repo.clone(),
        SessionRef {
            agent: "claude".into(),
            id,
            cwd: repo,
        },
    ))
}

#[tokio::test]
#[ignore]
async fn live_prepare_contract() {
    let Some((repo, session)) = live() else {
        return;
    };
    let mut review = build_review(&repo, &BuildOptions::default()).unwrap();
    let adapter: Arc<dyn AgentAdapter> = Arc::new(ClaudeAdapter::default());
    teer::cli::prepare_all(&mut review, adapter, session, 4).await;
    let unit = review
        .units
        .iter()
        .find(|u| u.status == UnitStatus::Changed)
        .expect("a changed unit");
    println!("{}: {:#?}", unit.symbol, unit.changes);
    println!("summary: {:?}", unit.summary);
    assert!(!unit.changes.is_empty());
}
