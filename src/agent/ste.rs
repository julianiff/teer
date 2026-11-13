//! Every answer in Simplified Technical English (ASD-STE100): an adapter
//! that adds [`RULE`] to each prompt, whatever model answers it (Claude
//! Code, Codex, a model API; the fast, the deep and the second one).
//!
//! The rule goes at the end of the prompt, so a marker line stays first
//! and the task's own rules are read before it. It changes the wording
//! only: never the shape of the answer. A commit message keeps git's own
//! conventions, so its prompt goes out as it is.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use super::live::LiveTx;
use super::{AgentAdapter, SessionInfo};
use crate::review::model::SessionRef;
use crate::review::prompts::COMMIT_MARKER;

/// Added to every prompt but the commit message's.
pub const RULE: &str = "\
LANGUAGE: write every sentence that a person reads in Simplified Technical English (ASD-STE100).
- Use short sentences: at most 20 words in a step, at most 25 words in a description. One instruction per sentence.
- Use the active voice. Use the imperative for instructions (\"Run the tests\", not \"The tests should be run\").
- Use simple verb forms: present, past, future, imperative. Do not use an -ing form as a verb. Do not use the passive voice.
- Use one word for one meaning, and the same word for the same thing each time. Do not use synonyms for variety.
- Use simple, common words. Do not use slang, idioms, or vague words such as \"basically\", \"quite\", \"kind of\".
- Start with the main point. Use a list for steps and for parallel items, where the answer's format permits one.
- Technical names (files, functions, flags, commands, error text), code, and words you quote are exempt. Keep them exact.
This rule changes the wording only. Keep the shape of the answer, its fields and every limit given above.";

/// Wraps an adapter so every answer is asked for in Simplified Technical
/// English.
pub struct Ste(pub Arc<dyn AgentAdapter>);

/// `prompt` with [`RULE`] at its end; a commit message's prompt as it is.
pub fn with_rule(prompt: &str) -> String {
    if prompt.starts_with(COMMIT_MARKER) {
        return prompt.to_string();
    }
    format!("{}\n\n{RULE}\n", prompt.trim_end())
}

#[async_trait]
impl AgentAdapter for Ste {
    fn list_sessions(&self, repo: &Path) -> Result<Vec<SessionInfo>> {
        self.0.list_sessions(repo)
    }

    async fn ask_structured(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.0.ask_structured(s, &with_rule(prompt), schema).await
    }

    async fn ask_dive(
        &self,
        s: &SessionRef,
        prompt: &str,
        schema: &Value,
        web: bool,
    ) -> Result<Value> {
        self.0.ask_dive(s, &with_rule(prompt), schema, web).await
    }

    async fn ask_live(
        &self,
        s: &SessionRef,
        prompt: &str,
        schema: &Value,
        web: bool,
        live: LiveTx,
    ) -> Result<Value> {
        self.0
            .ask_live(s, &with_rule(prompt), schema, web, live)
            .await
    }

    async fn ask_prepare(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.0.ask_prepare(s, &with_rule(prompt), schema).await
    }

    async fn ask_probe(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.0.ask_probe(s, &with_rule(prompt), schema).await
    }

    async fn ask_second(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.0.ask_second(s, &with_rule(prompt), schema).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_goes_last_and_a_marker_line_stays_first() {
        let p = with_rule("DIVE:Recommend\n\nYou are helping\n");
        assert!(p.starts_with("DIVE:Recommend\n\nYou are helping\n\nLANGUAGE:"));
        assert!(p.contains("ASD-STE100"));
    }

    #[test]
    fn a_commit_message_keeps_gits_conventions() {
        let p = format!("{COMMIT_MARKER}\n\nWrite a git commit message");
        assert_eq!(with_rule(&p), p);
    }
}
