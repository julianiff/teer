//! Model APIs with a key: Anthropic's Messages API, and OpenAI's chat
//! completions, which OpenRouter, Ollama, LM Studio, vLLM and others speak
//! too. teer runs the agent loop itself. Questions that need the repository
//! (Flagged tabs, Ask, Behavior probes, commit messages) get the read-only
//! tools of [`super::repo_tools`], run in the checkout under review;
//! describing units and second opinions get none, since the code is in the
//! prompt. The answer is held to teer's schema where the API can do that
//! (`output_config.format`, `response_format`), and read as JSON either way.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::{AgentAdapter, PrepareModel, SessionInfo, repo_tools, schema};
use crate::config::ApiFormat;
use crate::review::model::SessionRef;

const STRUCTURED_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const DIVE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Model turns one question may take; the last ones are told to answer.
const MAX_TURNS: usize = 40;
const WRAP_UP: usize = 3;
/// Tries per request when the API is busy or the connection fails.
const TRIES: u32 = 4;
/// `anthropic-version`: requests send no sampling settings, which this
/// version refuses.
const ANTHROPIC_VERSION: &str = "2026-10-21";
const MAX_TOKENS: u32 = 16_000;

const SYSTEM: &str = "You work for teer, a code review tool: answer the request about the code under review. \
Your final reply is only the JSON object the request asks for, with no other text.";
const SYSTEM_TOOLS: &str = " You can look at the repository under review with list_files, read_file and grep \
(read-only; paths are relative to its root). Look as far as the request needs, no further.";

pub struct ApiAdapter {
    pub format: ApiFormat,
    /// Without a trailing slash.
    pub base_url: String,
    pub key: Option<String>,
    /// Describing units.
    pub prepare: PrepareModel,
    /// Second opinions.
    pub second: PrepareModel,
    /// Questions that read the repository.
    pub deep: PrepareModel,
    client: reqwest::Client,
    /// The API refused a schema for the answer: ask for the JSON in words.
    no_format: AtomicBool,
}

impl ApiAdapter {
    pub fn new(
        format: ApiFormat,
        base_url: &str,
        key: Option<String>,
        prepare: PrepareModel,
        second: PrepareModel,
        deep: PrepareModel,
    ) -> Result<ApiAdapter> {
        Ok(ApiAdapter {
            format,
            base_url: base_url.trim_end_matches('/').to_string(),
            key,
            prepare,
            second,
            deep,
            client: client()?,
            no_format: AtomicBool::new(false),
        })
    }

    async fn ask(
        &self,
        m: &PrepareModel,
        repo: Option<&Path>,
        prompt: &str,
        schema: &Value,
        timeout: Duration,
    ) -> Result<Value> {
        tokio::time::timeout(timeout, self.converse(m, repo, prompt, schema))
            .await
            .with_context(|| {
                format!("the model API gave no answer within {}s", timeout.as_secs())
            })?
    }

    /// One question, turn by turn: the model reads with the tools (when
    /// `repo` is given) until it answers.
    async fn converse(
        &self,
        m: &PrepareModel,
        repo: Option<&Path>,
        prompt: &str,
        schema: &Value,
    ) -> Result<Value> {
        let model = m.model.as_deref().context("no model set for the API")?;
        let mut chat = Chat::new(prompt, repo.is_some());
        for turn in 0..MAX_TURNS {
            let reply = self.send(&chat, model, m.effort.as_deref(), schema).await?;
            match chat.take(self.format, reply)? {
                Step::Answer(text) => return parse_answer(&text).map(schema::drop_nulls),
                Step::Tools(calls) => {
                    let root = repo
                        .context("the model asked for a tool it wasn't given")?
                        .to_path_buf();
                    let results = tokio::task::spawn_blocking(move || {
                        calls
                            .into_iter()
                            .map(|c| {
                                let r = repo_tools::run(&root, &c.name, &c.input);
                                (c.id, r)
                            })
                            .collect::<Vec<_>>()
                    })
                    .await?;
                    let wrap_up = turn + WRAP_UP >= MAX_TURNS;
                    chat.results(self.format, results, wrap_up);
                }
            }
        }
        bail!("the model kept reading without answering ({MAX_TURNS} turns)")
    }

    /// Send the conversation; when the API refuses the answer's schema,
    /// once more without it (and from then on).
    async fn send(
        &self,
        chat: &Chat,
        model: &str,
        effort: Option<&str>,
        schema: &Value,
    ) -> Result<Value> {
        let formatted = !self.no_format.load(Ordering::Relaxed);
        let body = chat.body(self.format, model, effort, schema, formatted);
        match self.post(&body).await {
            Err(e) if formatted && e.downcast_ref::<BadRequest>().is_some() => {
                let plain = chat.body(self.format, model, effort, schema, false);
                let reply = self.post(&plain).await.map_err(|_| e)?;
                self.no_format.store(true, Ordering::Relaxed);
                Ok(reply)
            }
            r => r,
        }
    }

    fn endpoint(&self) -> String {
        match self.format {
            ApiFormat::Anthropic => format!("{}/v1/messages", self.base_url),
            ApiFormat::Openai => format!("{}/chat/completions", self.base_url),
        }
    }

    async fn post(&self, body: &Value) -> Result<Value> {
        let mut wait = Duration::from_secs(2);
        for attempt in 1..=TRIES {
            let req = auth(
                self.client.post(self.endpoint()),
                self.format,
                self.key.as_deref(),
            )
            .json(body);
            let res = match req.send().await {
                Ok(r) => r,
                Err(e) if attempt < TRIES && (e.is_connect() || e.is_timeout()) => {
                    tokio::time::sleep(wait).await;
                    wait *= 2;
                    continue;
                }
                Err(e) => return Err(e).context("couldn't reach the model API"),
            };
            let status = res.status();
            if status.is_success() {
                return res
                    .json()
                    .await
                    .context("the model API answered something that isn't JSON");
            }
            let busy = matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504 | 529);
            if busy && attempt < TRIES {
                let after = res
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(|s| Duration::from_secs(s.min(60)));
                tokio::time::sleep(after.unwrap_or(wait)).await;
                wait *= 2;
                continue;
            }
            let text = res.text().await.unwrap_or_default();
            let msg = error_message(&text);
            if status.as_u16() == 400 {
                return Err(BadRequest(msg).into());
            }
            bail!("the model API answered {status}: {msg}");
        }
        unreachable!("the last try returns")
    }
}

/// A 400: the request itself was refused (a model name, a schema).
#[derive(Debug)]
struct BadRequest(String);

impl std::fmt::Display for BadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the model API refused the request: {}", self.0)
    }
}

impl std::error::Error for BadRequest {}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .user_agent(concat!("teer/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("couldn't set up HTTPS")
}

fn auth(
    req: reqwest::RequestBuilder,
    format: ApiFormat,
    key: Option<&str>,
) -> reqwest::RequestBuilder {
    match (format, key) {
        (ApiFormat::Anthropic, k) => {
            let req = req.header("anthropic-version", ANTHROPIC_VERSION);
            match k {
                Some(k) => req.header("x-api-key", k),
                None => req,
            }
        }
        (ApiFormat::Openai, Some(k)) => req.bearer_auth(k),
        (ApiFormat::Openai, None) => req,
    }
}

/// The API's own words for an error, from `{"error": {"message": ..}}`.
fn error_message(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let msg = v["error"]["message"]
        .as_str()
        .or(v["error"].as_str())
        .or(v["message"].as_str())
        .map(String::from)
        .unwrap_or_else(|| body.trim().chars().take(300).collect());
    crate::util::sanitize(&msg)
}

/// The models the API offers, as `GET /models` lists them.
pub async fn list_models(
    format: ApiFormat,
    base_url: &str,
    key: Option<&str>,
) -> Result<Vec<String>> {
    let base = base_url.trim_end_matches('/');
    let url = match format {
        ApiFormat::Anthropic => format!("{base}/v1/models?limit=1000"),
        ApiFormat::Openai => format!("{base}/models"),
    };
    let res = auth(client()?.get(url), format, key)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("couldn't reach the model API")?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("{status}: {}", error_message(&text));
    }
    let v: Value = serde_json::from_str(&text).context("the model list isn't JSON")?;
    Ok(v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

/// A tool call the model made.
#[derive(Debug, Clone, PartialEq)]
struct Call {
    id: String,
    name: String,
    input: Value,
}

#[derive(Debug, PartialEq)]
enum Step {
    Answer(String),
    Tools(Vec<Call>),
}

/// The conversation so far, in the API's own message format.
struct Chat {
    system: String,
    messages: Vec<Value>,
    tools: bool,
}

impl Chat {
    fn new(prompt: &str, tools: bool) -> Chat {
        let mut system = SYSTEM.to_string();
        if tools {
            system.push_str(SYSTEM_TOOLS);
        }
        Chat {
            system,
            messages: vec![json!({ "role": "user", "content": prompt })],
            tools,
        }
    }

    fn body(
        &self,
        format: ApiFormat,
        model: &str,
        effort: Option<&str>,
        answer: &Value,
        formatted: bool,
    ) -> Value {
        let mut system = self.system.clone();
        if !formatted {
            system.push_str(&format!(
                "\nThe JSON object must match this JSON Schema:\n{}",
                schema::drop_nulls(answer.clone())
            ));
        }
        match format {
            ApiFormat::Anthropic => {
                let mut b = json!({
                    "model": model,
                    "max_tokens": MAX_TOKENS,
                    "system": system,
                    "messages": self.messages,
                    // Each turn re-sends the conversation: cache it.
                    "cache_control": { "type": "ephemeral" },
                });
                if self.tools {
                    b["tools"] = repo_tools::tools()
                        .into_iter()
                        .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.schema }))
                        .collect();
                }
                let mut out = serde_json::Map::new();
                if formatted {
                    out.insert(
                        "format".into(),
                        json!({ "type": "json_schema", "schema": schema::for_anthropic(answer) }),
                    );
                }
                if let Some(e) = effort {
                    out.insert("effort".into(), json!(e));
                }
                if !out.is_empty() {
                    b["output_config"] = Value::Object(out);
                }
                b
            }
            ApiFormat::Openai => {
                let mut messages = vec![json!({ "role": "system", "content": system })];
                messages.extend(self.messages.iter().cloned());
                let mut b = json!({ "model": model, "messages": messages });
                if self.tools {
                    b["tools"] = repo_tools::tools()
                        .into_iter()
                        .map(|t| {
                            json!({ "type": "function", "function": {
                                "name": t.name, "description": t.description, "parameters": t.schema } })
                        })
                        .collect();
                }
                if formatted {
                    b["response_format"] = json!({ "type": "json_schema", "json_schema": {
                        "name": "answer", "strict": false, "schema": schema::for_anthropic(answer) } });
                }
                if let Some(e) = effort {
                    b["reasoning_effort"] = json!(e);
                }
                b
            }
        }
    }

    /// Read the model's reply and keep it in the conversation.
    fn take(&mut self, format: ApiFormat, reply: Value) -> Result<Step> {
        match format {
            ApiFormat::Anthropic => {
                let content = reply["content"].as_array().cloned().unwrap_or_default();
                match reply["stop_reason"].as_str().unwrap_or("") {
                    "refusal" => bail!("the model declined to answer"),
                    "max_tokens" => bail!("the model's answer was cut off (max_tokens)"),
                    _ => {}
                }
                let calls: Vec<Call> = content
                    .iter()
                    .filter(|b| b["type"] == "tool_use")
                    .map(|b| Call {
                        id: b["id"].as_str().unwrap_or("").into(),
                        name: b["name"].as_str().unwrap_or("").into(),
                        input: b["input"].clone(),
                    })
                    .collect();
                let text: String = content
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect();
                // Thinking blocks go back unchanged, as the API wants.
                self.messages
                    .push(json!({ "role": "assistant", "content": content }));
                if calls.is_empty() {
                    Ok(Step::Answer(text))
                } else {
                    Ok(Step::Tools(calls))
                }
            }
            ApiFormat::Openai => {
                let choice = &reply["choices"][0];
                let msg = choice["message"].clone();
                match choice["finish_reason"].as_str().unwrap_or("") {
                    "length" => bail!("the model's answer was cut off (length)"),
                    "content_filter" => bail!("the model declined to answer"),
                    _ => {}
                }
                if msg.is_null() {
                    bail!(
                        "the model API sent no message: {}",
                        error_message(&reply.to_string())
                    );
                }
                let calls: Vec<Call> = msg["tool_calls"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|c| Call {
                                id: c["id"].as_str().unwrap_or("").into(),
                                name: c["function"]["name"].as_str().unwrap_or("").into(),
                                input: match &c["function"]["arguments"] {
                                    Value::String(s) => {
                                        serde_json::from_str(s).unwrap_or(Value::Null)
                                    }
                                    v => v.clone(),
                                },
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let text = match &msg["content"] {
                    Value::String(s) => s.clone(),
                    Value::Array(parts) => {
                        parts.iter().filter_map(|p| p["text"].as_str()).collect()
                    }
                    _ => String::new(),
                };
                self.messages.push(msg);
                if calls.is_empty() {
                    Ok(Step::Answer(text))
                } else {
                    Ok(Step::Tools(calls))
                }
            }
        }
    }

    /// The tools' results, all in one message; near the turn limit, a
    /// reminder to answer now.
    fn results(
        &mut self,
        format: ApiFormat,
        results: Vec<(String, Result<String, String>)>,
        wrap_up: bool,
    ) {
        const ANSWER_NOW: &str =
            "You are out of turns: answer now with the JSON object, no more tool calls.";
        match format {
            ApiFormat::Anthropic => {
                let mut content: Vec<Value> = results
                    .into_iter()
                    .map(|(id, r)| match r {
                        Ok(text) => json!({ "type": "tool_result", "tool_use_id": id, "content": text }),
                        Err(e) => json!({ "type": "tool_result", "tool_use_id": id, "content": e, "is_error": true }),
                    })
                    .collect();
                if wrap_up {
                    content.push(json!({ "type": "text", "text": ANSWER_NOW }));
                }
                self.messages
                    .push(json!({ "role": "user", "content": content }));
            }
            ApiFormat::Openai => {
                for (id, r) in results {
                    let content = r.unwrap_or_else(|e| format!("error: {e}"));
                    self.messages
                        .push(json!({ "role": "tool", "tool_call_id": id, "content": content }));
                }
                if wrap_up {
                    self.messages
                        .push(json!({ "role": "user", "content": ANSWER_NOW }));
                }
            }
        }
    }
}

/// The JSON object in the model's final reply: the whole reply, or the
/// part in a code fence or between the outer braces.
fn parse_answer(text: &str) -> Result<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Ok(v);
    }
    let inner = t
        .find('{')
        .zip(t.rfind('}'))
        .filter(|(a, b)| a < b)
        .map(|(a, b)| &t[a..=b]);
    if let Some(v) = inner.and_then(|s| serde_json::from_str::<Value>(s).ok()) {
        return Ok(v);
    }
    let head: String = t.chars().take(200).collect();
    bail!(
        "the model's answer is not JSON: {}",
        crate::util::sanitize(&head)
    )
}

#[async_trait]
impl AgentAdapter for ApiAdapter {
    fn list_sessions(&self, _repo: &Path) -> Result<Vec<SessionInfo>> {
        Ok(Vec::new())
    }

    async fn ask_structured(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.ask(&self.deep, Some(&s.cwd), prompt, schema, STRUCTURED_TIMEOUT)
            .await
    }

    async fn ask_dive(
        &self,
        s: &SessionRef,
        prompt: &str,
        schema: &Value,
        _web: bool,
    ) -> Result<Value> {
        self.ask(&self.deep, Some(&s.cwd), prompt, schema, DIVE_TIMEOUT)
            .await
    }

    async fn ask_probe(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.ask(&self.deep, Some(&s.cwd), prompt, schema, PROBE_TIMEOUT)
            .await
    }

    async fn ask_prepare(&self, _s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.ask(&self.prepare, None, prompt, schema, STRUCTURED_TIMEOUT)
            .await
    }

    async fn ask_second(&self, _s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.ask(&self.second, None, prompt, schema, STRUCTURED_TIMEOUT)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer_schema() -> Value {
        json!({ "type": "object", "properties": { "ok": { "type": "boolean" } }, "required": ["ok"] })
    }

    #[test]
    fn anthropic_body_has_tools_format_effort_and_no_sampling() {
        let chat = Chat::new("hi", true);
        let b = chat.body(
            ApiFormat::Anthropic,
            "claude-haiku-5-5",
            Some("low"),
            &answer_schema(),
            true,
        );
        assert_eq!(b["model"], "claude-haiku-5-5");
        assert_eq!(b["messages"], json!([{ "role": "user", "content": "hi" }]));
        let names: Vec<&str> = b["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["list_files", "read_file", "grep"]);
        assert_eq!(b["output_config"]["effort"], "low");
        assert_eq!(b["output_config"]["format"]["type"], "json_schema");
        assert_eq!(
            b["output_config"]["format"]["schema"]["additionalProperties"],
            json!(false)
        );
        for absent in ["temperature", "top_p", "top_k", "tool_choice", "thinking"] {
            assert!(b.get(absent).is_none(), "{absent}");
        }
        // Without tools, without a format: the schema goes into the system prompt.
        let plain =
            Chat::new("hi", false).body(ApiFormat::Anthropic, "m", None, &answer_schema(), false);
        assert!(plain.get("tools").is_none() && plain.get("output_config").is_none());
        assert!(plain["system"].as_str().unwrap().contains("\"ok\""));
    }

    #[test]
    fn openai_body_puts_the_system_first_and_tools_as_functions() {
        let chat = Chat::new("hi", true);
        let b = chat.body(
            ApiFormat::Openai,
            "gpt",
            Some("low"),
            &answer_schema(),
            true,
        );
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][1], json!({ "role": "user", "content": "hi" }));
        assert_eq!(b["tools"][1]["function"]["name"], "read_file");
        assert_eq!(b["response_format"]["json_schema"]["strict"], json!(false));
        assert_eq!(b["reasoning_effort"], "low");
        let none = chat.body(ApiFormat::Openai, "gpt", None, &answer_schema(), true);
        assert!(none.get("reasoning_effort").is_none());
    }

    #[test]
    fn anthropic_tool_turns_go_back_as_one_message_of_results() {
        let mut chat = Chat::new("hi", true);
        let reply = json!({
            "stop_reason": "tool_use",
            "content": [
                { "type": "thinking", "thinking": "", "signature": "s" },
                { "type": "tool_use", "id": "t1", "name": "grep", "input": { "pattern": "x" } },
                { "type": "tool_use", "id": "t2", "name": "read_file", "input": { "path": "a" } }
            ]
        });
        let Step::Tools(calls) = chat.take(ApiFormat::Anthropic, reply).unwrap() else {
            panic!()
        };
        assert_eq!(calls.len(), 2);
        assert_eq!(chat.messages[1]["content"][0]["type"], "thinking");
        chat.results(
            ApiFormat::Anthropic,
            vec![
                ("t1".into(), Ok("hit".into())),
                ("t2".into(), Err("nope".into())),
            ],
            true,
        );
        let last = &chat.messages[2];
        assert_eq!(last["role"], "user");
        assert_eq!(
            last["content"][0],
            json!({ "type": "tool_result", "tool_use_id": "t1", "content": "hit" })
        );
        assert_eq!(last["content"][1]["is_error"], json!(true));
        assert_eq!(last["content"][2]["type"], "text");
        let done = json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "{\"ok\":true}" }] });
        assert_eq!(
            chat.take(ApiFormat::Anthropic, done).unwrap(),
            Step::Answer("{\"ok\":true}".into())
        );
        let refused = json!({ "stop_reason": "refusal", "content": [] });
        assert!(chat.take(ApiFormat::Anthropic, refused).is_err());
    }

    #[test]
    fn openai_tool_calls_and_results() {
        let mut chat = Chat::new("hi", true);
        let reply = json!({ "choices": [{ "finish_reason": "tool_calls", "message": {
            "role": "assistant", "content": null,
            "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "list_files", "arguments": "{\"pattern\":\"*.rs\"}" } }]
        } }] });
        let Step::Tools(calls) = chat.take(ApiFormat::Openai, reply).unwrap() else {
            panic!()
        };
        assert_eq!(
            calls,
            [Call {
                id: "c1".into(),
                name: "list_files".into(),
                input: json!({ "pattern": "*.rs" })
            }]
        );
        chat.results(
            ApiFormat::Openai,
            vec![("c1".into(), Err("bad glob".into()))],
            false,
        );
        assert_eq!(
            chat.messages.last().unwrap(),
            &json!({ "role": "tool", "tool_call_id": "c1", "content": "error: bad glob" })
        );
        let cut =
            json!({ "choices": [{ "finish_reason": "length", "message": { "content": "{" } }] });
        assert!(chat.take(ApiFormat::Openai, cut).is_err());
    }

    #[test]
    fn answers_are_read_from_fences_and_prose() {
        assert_eq!(parse_answer(" {\"a\":1} ").unwrap(), json!({ "a": 1 }));
        assert_eq!(
            parse_answer("```json\n{\"a\":1}\n```").unwrap(),
            json!({ "a": 1 })
        );
        assert_eq!(
            parse_answer("Here: {\"a\":{\"b\":2}} done").unwrap(),
            json!({ "a": { "b": 2 } })
        );
        assert!(parse_answer("no json").is_err());
    }

    #[test]
    fn errors_use_the_apis_own_words() {
        assert_eq!(
            error_message(r#"{"type":"error","error":{"type":"x","message":"model not found"}}"#),
            "model not found"
        );
        assert_eq!(error_message(r#"{"error":"bad key"}"#), "bad key");
        assert_eq!(error_message("plain"), "plain");
    }
}
