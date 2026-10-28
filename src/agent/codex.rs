//! Codex adapter: headless `codex exec` in the repository, read-only, the
//! answer held to teer's schema. Codex has no tool-free mode: describing
//! units runs the same way, on the fast model, and the prompt carries the
//! code so it has no reason to look further.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::{AgentAdapter, PrepareModel, SessionInfo, commands, schema};
use crate::review::model::SessionRef;

const STRUCTURED_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const DIVE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub struct CodexAdapter {
    pub bin: String,
    /// The code under review is untrusted (a pull request): leave its
    /// AGENTS.md, skills and exec rules out.
    pub untrusted: bool,
    /// Model and effort for describing units.
    pub prepare: PrepareModel,
    /// Model and effort for second opinions.
    pub second: PrepareModel,
}

impl Default for CodexAdapter {
    fn default() -> Self {
        CodexAdapter {
            bin: commands::CODEX_BIN.into(),
            untrusted: false,
            prepare: PrepareModel::default(),
            second: PrepareModel::default(),
        }
    }
}

/// The models Codex lists for picking, by its own priority.
pub fn list_models(bin: &str) -> Result<Vec<String>> {
    let (_, args) = commands::codex_models();
    let out = std::process::Command::new(bin)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("failed to start '{bin}'"))?;
    if !out.status.success() {
        bail!("'{bin} debug models' failed");
    }
    parse_models(&String::from_utf8_lossy(&out.stdout))
}

fn parse_models(json: &str) -> Result<Vec<String>> {
    let v: Value = serde_json::from_str(json)
        .context("codex listed its models in a form teer doesn't read")?;
    let mut models: Vec<(i64, String)> = v["models"]
        .as_array()
        .context("no models in codex's catalog")?
        .iter()
        .filter(|m| m["visibility"].as_str() != Some("hide"))
        .filter_map(|m| {
            Some((
                m["priority"].as_i64().unwrap_or(i64::MAX),
                m["slug"].as_str()?.to_string(),
            ))
        })
        .collect();
    models.sort();
    Ok(models.into_iter().map(|(_, s)| s).collect())
}

impl CodexAdapter {
    async fn run(
        &self,
        s: &SessionRef,
        model: &PrepareModel,
        prompt: &str,
        schema: &Value,
        timeout: Duration,
    ) -> Result<Value> {
        let dir = tempfile::tempdir().context("no temporary directory for codex")?;
        let schema_file = dir.path().join("schema.json");
        let out_file = dir.path().join("answer.json");
        std::fs::write(&schema_file, schema::for_openai(schema).to_string())?;
        let args = commands::codex_exec(&s.cwd, model, &schema_file, &out_file, self.untrusted);
        let mut child = Command::new(&self.bin)
            .args(&args)
            .current_dir(&s.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start '{}'; is Codex installed?", self.bin))?;
        let mut stdin = child.stdin.take().context("no stdin")?;
        stdin.write_all(prompt.as_bytes()).await?;
        drop(stdin);
        let out = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .context("codex timed out")??;
        let answer = std::fs::read_to_string(&out_file).unwrap_or_default();
        if answer.trim().is_empty() {
            bail!(
                "codex failed: {}",
                last_error(&String::from_utf8_lossy(&out.stderr))
            );
        }
        let v: Value = serde_json::from_str(answer.trim()).context("codex's answer is not JSON")?;
        Ok(schema::drop_nulls(v))
    }
}

/// What codex said went wrong: its `ERROR:` line, or the last lines.
fn last_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if let Some(e) = lines
        .iter()
        .rev()
        .find(|l| l.starts_with("ERROR") || l.contains("\"message\""))
    {
        return e.to_string();
    }
    let n = lines.len();
    lines[n.saturating_sub(3)..].join(" ")
}

#[async_trait]
impl AgentAdapter for CodexAdapter {
    fn list_sessions(&self, _repo: &Path) -> Result<Vec<SessionInfo>> {
        Ok(Vec::new())
    }

    async fn ask_structured(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.run(
            s,
            &PrepareModel::default(),
            prompt,
            schema,
            STRUCTURED_TIMEOUT,
        )
        .await
    }

    async fn ask_dive(
        &self,
        s: &SessionRef,
        prompt: &str,
        schema: &Value,
        _web: bool,
    ) -> Result<Value> {
        self.run(s, &PrepareModel::default(), prompt, schema, DIVE_TIMEOUT)
            .await
    }

    async fn ask_probe(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.run(s, &PrepareModel::default(), prompt, schema, PROBE_TIMEOUT)
            .await
    }

    async fn ask_prepare(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.run(s, &self.prepare, prompt, schema, STRUCTURED_TIMEOUT)
            .await
    }

    async fn ask_second(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.run(s, &self.second, prompt, schema, STRUCTURED_TIMEOUT)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_visible_models_by_priority() {
        let json = r#"{"models":[
            {"slug":"b","priority":2,"visibility":"list"},
            {"slug":"hidden","priority":0,"visibility":"hide"},
            {"slug":"a","priority":1,"visibility":"list"}]}"#;
        assert_eq!(parse_models(json).unwrap(), ["a", "b"]);
    }

    #[test]
    fn finds_the_error_in_codexs_output() {
        let err = "hook: SessionStart\n  \"message\": \"Invalid schema\",\n  \"param\": \"x\"\n}\n";
        assert_eq!(last_error(err), "\"message\": \"Invalid schema\",");
        assert_eq!(last_error("a\nb\nc\nd\n"), "b c d");
    }
}
