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

/// Ask the agent and check its answer.
pub async fn answer(
    adapter: Arc<dyn AgentAdapter>,
    session: SessionRef,
    job: Job,
) -> Result<Answer, String> {
    let raw = adapter
        .ask_dive(&session, &job.prompt, &prompts::ask_schema(), false)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let (answer, dropped) = tokio::task::spawn_blocking(move || {
        validate::validate_answer(&raw, &job.root, &job.files, &job.units)
    })
    .await
    .map_err(|e| e.to_string())?;
    validate::log_dropped("from the answer to", &dropped);
    answer
}
