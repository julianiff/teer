//! Claude Code adapter: headless `claude -p`, a fork of the user's session
//! for questions when one was given, a fresh run otherwise.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use super::live::{self, Live, LiveTx};
use super::{AgentAdapter, PrepareModel, SessionInfo, commands, sessions};
use crate::review::model::SessionRef;

const STRUCTURED_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const DIVE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub struct ClaudeAdapter {
    pub bin: String,
    /// The code under review is untrusted (a pull request): don't load the
    /// repo's own Claude settings, hooks or MCP servers.
    pub untrusted: bool,
    /// Model and effort for describing units (see [`AgentAdapter::ask_prepare`]).
    pub prepare: PrepareModel,
    /// Model and effort for second opinions (see [`AgentAdapter::ask_second`]).
    pub second: PrepareModel,
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        ClaudeAdapter {
            bin: commands::CLAUDE_BIN.into(),
            untrusted: false,
            prepare: PrepareModel::default(),
            second: PrepareModel::default(),
        }
    }
}

pub fn is_installed(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Pull the structured answer out of `--output-format json` output.
fn parse_structured_result(stdout: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(stdout.trim()).context("claude returned invalid JSON")?;
    if v["is_error"].as_bool().unwrap_or(false) {
        bail!(
            "claude reported an error: {}",
            v["result"].as_str().unwrap_or("unknown")
        );
    }
    if let Some(s) = v.get("structured_output").filter(|s| !s.is_null()) {
        return Ok(s.clone());
    }
    let text = v["result"]
        .as_str()
        .context("claude result has no structured output")?;
    serde_json::from_str(text).context("claude result is not JSON")
}

/// The session a question forks, or none for a fresh run.
fn fork_of(s: &SessionRef) -> Option<&str> {
    (!s.is_fresh()).then_some(s.id.as_str())
}

impl ClaudeAdapter {
    /// Start `claude` with `args` (plus the untrusted flags for a pull
    /// request) in the session's directory, the prompt on stdin.
    async fn start(&self, s: &SessionRef, mut args: Vec<String>, prompt: &str) -> Result<Child> {
        if self.untrusted {
            args.extend(commands::untrusted_flags());
        }
        let mut child = Command::new(&self.bin)
            .args(&args)
            .current_dir(&s.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!("failed to start '{}'; is Claude Code installed?", self.bin)
            })?;
        let mut stdin = child.stdin.take().context("no stdin")?;
        stdin.write_all(prompt.as_bytes()).await?;
        drop(stdin);
        Ok(child)
    }

    /// Run `claude` and parse its answer.
    async fn run_structured(
        &self,
        s: &SessionRef,
        args: Vec<String>,
        prompt: &str,
        timeout: Duration,
    ) -> Result<Value> {
        let child = self.start(s, args, prompt).await?;
        let out = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .context("claude timed out")??;
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() && stdout.trim().is_empty() {
            bail!(
                "claude failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        parse_structured_result(&stdout)
    }

    /// [`Self::run_structured`] with `stream-json` output
    /// ([`commands::claude_live`]), read as it comes: `live` hears each
    /// step and the answer as it is written; the `result` line is the answer.
    async fn run_live(
        &self,
        s: &SessionRef,
        args: Vec<String>,
        prompt: &str,
        timeout: Duration,
        live: &LiveTx,
    ) -> Result<Value> {
        let mut child = self.start(s, args, prompt).await?;
        let stdout = child.stdout.take().context("no stdout")?;
        let mut stderr = child.stderr.take().context("no stderr")?;
        // Read on its own, so a full pipe never stalls the run.
        let errors = tokio::spawn(async move {
            let mut b = Vec::new();
            let _ = stderr.read_to_end(&mut b).await;
            b
        });
        let read = async {
            let mut lines = BufReader::new(stdout).lines();
            let mut stream = Stream::default();
            let mut result = None;
            while let Some(line) = lines.next_line().await? {
                if let Some(r) = stream.line(&line, &s.cwd, live) {
                    result = Some(r);
                }
            }
            child.wait().await?;
            anyhow::Ok(result)
        };
        let result = tokio::time::timeout(timeout, read)
            .await
            .context("claude timed out")??;
        match result {
            Some(line) => parse_structured_result(&line),
            None => {
                let stderr = errors.await.unwrap_or_default();
                bail!("claude failed: {}", String::from_utf8_lossy(&stderr).trim())
            }
        }
    }
}

/// What is read of `stream-json` output so far.
#[derive(Default)]
struct Stream {
    /// The answer's block in the message being written (Claude Code's
    /// `StructuredOutput` tool call): its index and its JSON so far.
    answer: Option<(u64, String)>,
}

impl Stream {
    /// Read one line and tell `live` what it shows: a tool the agent runs, a
    /// piece of the answer. The line itself when it is the `result`.
    fn line(&mut self, line: &str, root: &Path, live: &LiveTx) -> Option<String> {
        let v: Value = serde_json::from_str(line).ok()?;
        match v["type"].as_str()? {
            "result" => return Some(line.to_string()),
            "assistant" => {
                let content = v["message"]["content"].as_array();
                for b in content.into_iter().flatten() {
                    if b["type"] == "tool_use"
                        && let Some(step) =
                            live::tool_step(b["name"].as_str().unwrap_or(""), &b["input"], root)
                    {
                        let _ = live.send(Live::Step(step));
                    }
                }
            }
            "stream_event" => {
                let e = &v["event"];
                match e["type"].as_str()? {
                    "message_start" => self.answer = None,
                    "content_block_start" if e["content_block"]["name"] == "StructuredOutput" => {
                        self.answer = Some((e["index"].as_u64()?, String::new()));
                    }
                    "content_block_delta" if e["delta"]["type"] == "input_json_delta" => {
                        let (i, json) = self.answer.as_mut()?;
                        if e["index"].as_u64() == Some(*i) {
                            json.push_str(e["delta"]["partial_json"].as_str().unwrap_or(""));
                            let _ = live.send(Live::Partial(json.clone()));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }
}

#[async_trait]
impl AgentAdapter for ClaudeAdapter {
    fn list_sessions(&self, repo: &Path) -> Result<Vec<SessionInfo>> {
        let Some(root) = sessions::projects_root() else {
            return Ok(Vec::new());
        };
        sessions::list_sessions(&root, repo, &sessions::running_agents(), SystemTime::now())
    }

    async fn ask_structured(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        let args = commands::claude_dive(fork_of(s), &schema.to_string(), false);
        self.run_structured(s, args, prompt, STRUCTURED_TIMEOUT)
            .await
    }

    async fn ask_dive(
        &self,
        s: &SessionRef,
        prompt: &str,
        schema: &Value,
        web: bool,
    ) -> Result<Value> {
        let args = commands::claude_dive(fork_of(s), &schema.to_string(), web);
        self.run_structured(s, args, prompt, DIVE_TIMEOUT).await
    }

    async fn ask_live(
        &self,
        s: &SessionRef,
        prompt: &str,
        schema: &Value,
        web: bool,
        live: LiveTx,
    ) -> Result<Value> {
        let args = commands::claude_live(fork_of(s), &schema.to_string(), web);
        self.run_live(s, args, prompt, DIVE_TIMEOUT, &live).await
    }

    async fn ask_probe(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        let args = commands::claude_dive(fork_of(s), &schema.to_string(), false);
        self.run_structured(s, args, prompt, PROBE_TIMEOUT).await
    }

    async fn ask_prepare(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        let args = commands::claude_prepare(&self.prepare, &schema.to_string());
        self.run_structured(s, args, prompt, STRUCTURED_TIMEOUT)
            .await
    }

    async fn ask_second(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        let args = commands::claude_prepare(&self.second, &schema.to_string());
        self.run_structured(s, args, prompt, STRUCTURED_TIMEOUT)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/samples/");

    #[test]
    fn parses_saved_structured_result() {
        let s = std::fs::read_to_string(format!("{DIR}claude_structured_result.json")).unwrap();
        assert_eq!(
            parse_structured_result(&s).unwrap(),
            serde_json::json!({"answer": "Hi!"})
        );
    }

    #[test]
    fn a_stream_tells_the_steps_and_the_answer_as_it_is_written() {
        let s = std::fs::read_to_string(format!("{DIR}claude_stream.jsonl")).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut stream = Stream::default();
        let results: Vec<String> = s
            .lines()
            .filter_map(|l| stream.line(l, Path::new("/r"), &tx))
            .collect();
        drop(tx);
        let mut seen = Vec::new();
        while let Ok(l) = rx.try_recv() {
            seen.push(l);
        }
        // The answer the model first wrote as text isn't the answer; the
        // tool call's own JSON isn't either.
        assert_eq!(
            seen,
            [
                Live::Step("search “retry” in src".into()),
                Live::Partial(String::new()),
                Live::Partial(r#"{"text": "Retries are"#.into()),
                Live::Partial(r#"{"text": "Retries are in http.rs."}"#.into()),
            ]
        );
        assert_eq!(results.len(), 1);
        assert_eq!(
            parse_structured_result(&results[0]).unwrap(),
            serde_json::json!({"text": "Retries are in http.rs."})
        );
    }

    #[test]
    fn error_results_fail() {
        assert!(
            parse_structured_result(r#"{"type":"result","is_error":true,"result":"boom"}"#)
                .is_err()
        );
    }
}
