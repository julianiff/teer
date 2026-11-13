//! A scriptable adapter for tests. Records everything sent to it.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use super::live::{Live, LiveTx};
use super::{AgentAdapter, SessionInfo};
use crate::review::model::SessionRef;

type Responder = dyn Fn(&str, &Value) -> Result<Value> + Send + Sync;

#[derive(Clone)]
pub struct MockAdapter {
    pub sessions: Vec<SessionInfo>,
    responder: Arc<Responder>,
    /// Prompts received by `ask_structured`.
    pub prompts: Arc<Mutex<Vec<String>>>,
    /// The `web` flag of every `ask_dive` call.
    pub web_calls: Arc<Mutex<Vec<bool>>>,
    /// How many questions came through `ask_prepare`.
    pub prepare_calls: Arc<Mutex<usize>>,
    /// How many came through `ask_second`.
    pub second_calls: Arc<Mutex<usize>>,
    /// What `ask_live` tells before it answers.
    pub live: Vec<Live>,
}

impl MockAdapter {
    pub fn new(responder: impl Fn(&str, &Value) -> Result<Value> + Send + Sync + 'static) -> Self {
        MockAdapter {
            sessions: Vec::new(),
            responder: Arc::new(responder),
            prompts: Arc::default(),
            web_calls: Arc::default(),
            prepare_calls: Arc::default(),
            second_calls: Arc::default(),
            live: Vec::new(),
        }
    }
}

#[async_trait]
impl AgentAdapter for MockAdapter {
    fn list_sessions(&self, _repo: &Path) -> Result<Vec<SessionInfo>> {
        Ok(self.sessions.clone())
    }

    async fn ask_structured(&self, _s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        (self.responder)(prompt, schema)
    }

    async fn ask_dive(
        &self,
        s: &SessionRef,
        prompt: &str,
        schema: &Value,
        web: bool,
    ) -> Result<Value> {
        self.web_calls.lock().unwrap().push(web);
        self.ask_structured(s, prompt, schema).await
    }

    async fn ask_live(
        &self,
        s: &SessionRef,
        prompt: &str,
        schema: &Value,
        web: bool,
        live: LiveTx,
    ) -> Result<Value> {
        for l in &self.live {
            let _ = live.send(l.clone());
        }
        self.ask_dive(s, prompt, schema, web).await
    }

    async fn ask_prepare(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        *self.prepare_calls.lock().unwrap() += 1;
        self.ask_structured(s, prompt, schema).await
    }

    async fn ask_second(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        *self.second_calls.lock().unwrap() += 1;
        self.ask_structured(s, prompt, schema).await
    }
}

/// A responder for dive prompts: keys on the `DIVE:<kind>` marker and
/// answers Recommend with `{text, sources}`. Other prompts get an error.
pub fn dive_responder(prompt: &str, _schema: &Value) -> Result<Value> {
    if prompt.contains("DIVE:Recommend") {
        return Ok(serde_json::json!({
            "text": "mock Recommend answer",
            "sources": ["https://example.com/doc", "a.rs:1"],
        }));
    }
    anyhow::bail!("not a dive prompt")
}
