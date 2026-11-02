//! Ask: a question about the code, asked during the review ("show me the
//! Cart class", "how are retries done?"). The agent reads the repository
//! (read-only, never the web) and answers with text, the code to show and
//! the units of this change the question has to do with; teer reads the
//! code itself and keeps only lines that exist.

use std::path::PathBuf;
use std::sync::Arc;

use super::model::{Answer, Review, SessionRef, Unit};
use super::{prompts, validate};
use crate::agent::AgentAdapter;
use crate::agent::live::{Live, partial_string};
use crate::git::ChangedFile;

/// Everything answering a question needs, taken from the review.
pub struct Job {
    pub prompt: String,
    /// Where the code is read: the repository, or the PR's checkout.
    pub root: PathBuf,
    /// The change's files, whose reviewed version is what an answer shows.
    pub files: Vec<ChangedFile>,
    pub units: Vec<Unit>,
}

impl Job {
    /// Question `index` of `review`, its code read under `root`.
    pub fn new(review: &Review, index: usize, root: PathBuf) -> Job {
        Job {
            prompt: prompts::ask_prompt(review, index),
            root,
            files: review.files.clone(),
            units: review.units.clone(),
        }
    }
}

/// What the agent is doing on a question while it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// A step it took: `read src/cart.rs`.
    Step(String),
    /// The answer's text as far as it has come.
    Text(String),
}

/// Ask the agent and check its answer. `progress` hears what it does
/// meanwhile, all of it before this returns.
pub async fn answer(
    adapter: Arc<dyn AgentAdapter>,
    session: SessionRef,
    job: Job,
    progress: impl Fn(Progress) + Send + 'static,
) -> Result<Answer, String> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let relay = tokio::spawn(async move {
        let mut shown = String::new();
        while let Some(live) = rx.recv().await {
            match live {
                Live::Step(s) => progress(Progress::Step(s)),
                Live::Partial(json) => {
                    let Some(text) = partial_string(&json, "text") else {
                        continue;
                    };
                    let text = crate::util::sanitize_agent_text(&text);
                    if !text.trim().is_empty() && text != shown {
                        shown.clone_from(&text);
                        progress(Progress::Text(text));
                    }
                }
            }
        }
    });
    let raw = adapter
        .ask_live(&session, &job.prompt, &prompts::ask_schema(), false, tx)
        .await;
    // The sender is gone with the run: the relay ends once it has told all.
    let _ = relay.await;
    let raw = raw.map_err(|e| format!("{e:#}"))?;
    let (answer, dropped) = tokio::task::spawn_blocking(move || {
        validate::validate_answer(&raw, &job.root, &job.files, &job.units)
    })
    .await
    .map_err(|e| e.to_string())?;
    validate::log_dropped("from the answer to", &dropped);
    answer
}
