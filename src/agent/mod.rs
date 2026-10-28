//! Agent adapters. teer talks to what prepares its reviews only through
//! [`AgentAdapter`]: Claude Code, Codex, or a model API (see
//! [`crate::config::Provider`]).

pub mod api;
pub mod claude;
pub mod codex;
pub mod commands;
pub mod mock;
pub mod repo_tools;
pub mod schema;
pub mod sessions;

use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::review::model::SessionRef;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub agent: String,
    pub id: String,
    pub cwd: PathBuf,
    pub title: String,
    pub last_activity: chrono::DateTime<chrono::Local>,
    /// Modified recently, or a running agent process is attached to it.
    pub open: bool,
}

impl SessionInfo {
    pub fn to_ref(&self) -> SessionRef {
        SessionRef {
            agent: self.agent.clone(),
            id: self.id.clone(),
            cwd: self.cwd.clone(),
        }
    }
}

/// A model and effort (`claude --model` / `--effort`, `codex -m` /
/// `model_reasoning_effort`, or the API's). `None` leaves the provider's
/// own default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrepareModel {
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[async_trait]
pub trait AgentAdapter: Send + Sync {
    fn list_sessions(&self, repo: &Path) -> Result<Vec<SessionInfo>>;
    /// Ask a question in a throwaway run (a fork of the session, or a fresh
    /// headless run); the answer must match `schema`.
    async fn ask_structured(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value>;
    /// A question about a flagged unit (`DiveKind`). `web` allows web search and fetch on top of
    /// the read-only repo tools; adapters without that split ignore it.
    async fn ask_dive(
        &self,
        s: &SessionRef,
        prompt: &str,
        schema: &Value,
        _web: bool,
    ) -> Result<Value> {
        self.ask_structured(s, prompt, schema).await
    }
    /// Describe units for the review: a fresh run (never a fork of the
    /// session), without tools (the code is in the prompt), on the
    /// preparation model. Adapters without that split ask as usual.
    async fn ask_prepare(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.ask_structured(s, prompt, schema).await
    }
    /// Writing a Behavior probe: a Flagged-screen question without the web,
    /// given longer, since the agent may have to read a lot of a big repository.
    async fn ask_probe(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.ask_dive(s, prompt, schema, false).await
    }
    /// A second opinion on units already described: like [`Self::ask_prepare`]
    /// (fresh, no tools, the code in the prompt), on the second model, so the
    /// check doesn't come from the model that wrote the description.
    async fn ask_second(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.ask_prepare(s, prompt, schema).await
    }
}
