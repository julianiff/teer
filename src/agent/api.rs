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

use super::live::{self, Live, LiveTx};
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
        live: Option<&LiveTx>,
    ) -> Result<Value> {
        tokio::time::timeout(timeout, self.converse(m, repo, prompt, schema, live))
            .await
            .with_context(|| {
                format!("the model API gave no answer within {}s", timeout.as_secs())
            })?
    }

    /// One question, turn by turn: the model reads with the tools (when
    /// `repo` is given) until it answers. `live` hears each tool call and
    /// the replies as they are written.
    async fn converse(
        &self,
        m: &PrepareModel,
        repo: Option<&Path>,
        prompt: &str,
        schema: &Value,
        live: Option<&LiveTx>,
    ) -> Result<Value> {
        let model = m.model.as_deref().context("no model set for the API")?;
        let mut chat = Chat::new(prompt, repo.is_some());
        for turn in 0..MAX_TURNS {
            let reply = self
                .send(&chat, model, m.effort.as_deref(), schema, live)
                .await?;
            match chat.take(self.format, reply)? {
                Step::Answer(text) => return parse_answer(&text).map(schema::drop_nulls),
                Step::Tools(calls) => {
                    let root = repo
                        .context("the model asked for a tool it wasn't given")?
                        .to_path_buf();
                    for c in &calls {
                        if let (Some(live), Some(step)) =
                            (live, live::tool_step(&c.name, &c.input, &root))
                        {
                            let _ = live.send(Live::Step(step));
                        }
                    }
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
        live: Option<&LiveTx>,
    ) -> Result<Value> {
        let formatted = !self.no_format.load(Ordering::Relaxed);
        let body = chat.body(self.format, model, effort, schema, formatted);
        match self.post(&body, live).await {
            Err(e) if formatted && e.downcast_ref::<BadRequest>().is_some() => {
                let plain = chat.body(self.format, model, effort, schema, false);
                let reply = self.post(&plain, live).await.map_err(|_| e)?;
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

    /// Post `body`, trying again while the API is busy. With `live`, the
    /// reply is streamed and `live` hears its text as it is written; it is
    /// read back into the reply the API gives without streaming.
    async fn post(&self, body: &Value, live: Option<&LiveTx>) -> Result<Value> {
        let mut wait = Duration::from_secs(2);
        let streamed;
        let body = match live {
            Some(_) => {
                let mut b = body.clone();
                b["stream"] = json!(true);
                streamed = b;
                &streamed
            }
            None => body,
        };
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
            let events = res
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with("text/event-stream"));
            if status.is_success() {
                // A server that doesn't stream answers as usual.
                return match live {
                    Some(live) if events => read_events(res, self.format, live).await,
                    _ => res
                        .json()
                        .await
                        .context("the model API answered something that isn't JSON"),
                };
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

/// Read a streamed reply (server-sent events) into the reply the API gives
/// without streaming, telling `live` its text as it is written.
async fn read_events(
    mut res: reqwest::Response,
    format: ApiFormat,
    live: &LiveTx,
) -> Result<Value> {
    let mut reply = Streamed::new(format);
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = res
        .chunk()
        .await
        .context("the model API's stream broke off")?
    {
        buf.extend_from_slice(&chunk);
        while let Some(end) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(data) = line.trim_end().strip_prefix("data:") {
                reply.event(data.trim_start(), live)?;
            }
        }
    }
    reply.finish()
}

/// A streamed reply, put back together event by event.
struct Streamed {
    format: ApiFormat,
    /// Anthropic: the message, its content blocks filled in as they come.
    /// OpenAI: the choice's message, fields filled in as they come.
    message: Value,
    /// Tool calls' arguments as they came: by block (Anthropic) or by call (OpenAI).
    args: Vec<String>,
    /// OpenAI: why the reply stopped.
    finish: Value,
    /// Whether the reply came to its end.
    done: bool,
    /// The reply's text so far.
    text: String,
}

impl Streamed {
    fn new(format: ApiFormat) -> Streamed {
        let message = match format {
            ApiFormat::Anthropic => json!({ "role": "assistant", "content": [] }),
            ApiFormat::Openai => json!({ "role": "assistant", "content": null }),
        };
        Streamed {
            format,
            message,
            args: Vec::new(),
            finish: Value::Null,
            done: false,
            text: String::new(),
        }
    }

    /// One event's data.
    fn event(&mut self, data: &str, live: &LiveTx) -> Result<()> {
        if data == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Ok(());
        };
        if !v["error"].is_null() {
            bail!("the model API stopped: {}", error_message(&v.to_string()));
        }
        let before = self.text.len();
        match self.format {
            ApiFormat::Anthropic => self.anthropic(&v),
            ApiFormat::Openai => self.openai(&v),
        }
        if self.text.len() > before {
            let _ = live.send(Live::Partial(self.text.clone()));
        }
        Ok(())
    }

    /// `message_start`, `content_block_start`, `content_block_delta` (text,
    /// tool input, thinking and its signature, which go back unchanged),
    /// `content_block_stop`, `message_delta`, `message_stop`.
    fn anthropic(&mut self, v: &Value) {
        let i = v["index"].as_u64().unwrap_or(0) as usize;
        match v["type"].as_str().unwrap_or("") {
            "message_start" => self.message = v["message"].clone(),
            "content_block_start" => {
                let Some(blocks) = self.message["content"].as_array_mut() else {
                    return;
                };
                if blocks.len() <= i {
                    blocks.resize(i + 1, Value::Null);
                }
                blocks[i] = v["content_block"].clone();
                if self.args.len() <= i {
                    self.args.resize(i + 1, String::new());
                }
            }
            "content_block_delta" => {
                let d = &v["delta"];
                let piece = |k: &str| d[k].as_str().unwrap_or("").to_string();
                let Some(block) = self.message["content"].get_mut(i) else {
                    return;
                };
                let mut append = |k: &str, s: &str| {
                    let had = block[k].as_str().unwrap_or("").to_string();
                    block[k] = json!(had + s);
                };
                match d["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        append("text", &piece("text"));
                        self.text.push_str(&piece("text"));
                    }
                    "thinking_delta" => append("thinking", &piece("thinking")),
                    "signature_delta" => append("signature", &piece("signature")),
                    "input_json_delta" => {
                        if let Some(a) = self.args.get_mut(i) {
                            a.push_str(&piece("partial_json"));
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let args = self.args.get(i).cloned().unwrap_or_default();
                if let Some(block) = self.message["content"].get_mut(i)
                    && !args.trim().is_empty()
                {
                    block["input"] = serde_json::from_str(&args).unwrap_or(Value::Null);
                }
            }
            "message_delta" => {
                if let (Some(m), Some(d)) = (self.message.as_object_mut(), v["delta"].as_object()) {
                    m.extend(d.clone());
                }
            }
            "message_stop" => self.done = true,
            _ => {}
        }
    }

    /// A chunk: the choice's `delta` (text and other fields appended, tool
    /// calls by their `index`) and its `finish_reason`.
    fn openai(&mut self, v: &Value) {
        let choice = &v["choices"][0];
        if let Some(delta) = choice["delta"].as_object() {
            for (k, d) in delta {
                match (k.as_str(), d) {
                    ("tool_calls", Value::Array(calls)) => {
                        for (n, c) in calls.iter().enumerate() {
                            self.tool_call_delta(n, c);
                        }
                    }
                    ("role", _) | (_, Value::Null) => {}
                    (k, Value::String(s)) => {
                        let had = self.message[k].as_str().unwrap_or("").to_string();
                        self.message[k] = json!(had + s);
                        if k == "content" {
                            self.text.push_str(s);
                        }
                    }
                    (k, d) => self.message[k] = d.clone(),
                }
            }
        }
        if !choice["finish_reason"].is_null() {
            self.finish = choice["finish_reason"].clone();
            self.done = true;
        }
    }

    /// A piece of tool call `n` of a chunk: its `id` and `name` once, its
    /// arguments in pieces (or whole, as an object, from some servers).
    fn tool_call_delta(&mut self, n: usize, c: &Value) {
        let at = c["index"].as_u64().map_or(n, |i| i as usize);
        if !self.message["tool_calls"].is_array() {
            self.message["tool_calls"] = json!([]);
        }
        let Some(calls) = self.message["tool_calls"].as_array_mut() else {
            return;
        };
        while calls.len() <= at {
            calls.push(json!({ "id": "", "type": "function", "function": { "name": "" } }));
        }
        if self.args.len() <= at {
            self.args.resize(at + 1, String::new());
        }
        let call = &mut calls[at];
        if let Some(id) = c["id"].as_str() {
            call["id"] = json!(id);
        }
        if let Some(name) = c["function"]["name"].as_str() {
            call["function"]["name"] = json!(name);
        }
        match &c["function"]["arguments"] {
            Value::String(s) => self.args[at].push_str(s),
            Value::Null => {}
            whole => call["function"]["arguments"] = whole.clone(),
        }
    }

    /// The reply as the API gives it without streaming.
    fn finish(mut self) -> Result<Value> {
        if !self.done {
            bail!("the model API's stream ended before the reply did");
        }
        match self.format {
            ApiFormat::Anthropic => {
                if let Some(blocks) = self.message["content"].as_array_mut() {
                    blocks.retain(|b| !b.is_null());
                }
                Ok(self.message)
            }
            ApiFormat::Openai => {
                if let Some(calls) = self.message["tool_calls"].as_array_mut() {
                    for (call, args) in calls.iter_mut().zip(&self.args) {
                        if call["function"]["arguments"].is_null() {
                            call["function"]["arguments"] = json!(args);
                        }
                    }
                }
                Ok(
                    json!({ "choices": [{ "message": self.message, "finish_reason": self.finish }] }),
                )
            }
        }
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
        self.ask(
            &self.deep,
            Some(&s.cwd),
            prompt,
            schema,
            STRUCTURED_TIMEOUT,
            None,
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
        self.ask(&self.deep, Some(&s.cwd), prompt, schema, DIVE_TIMEOUT, None)
            .await
    }

    async fn ask_live(
        &self,
        s: &SessionRef,
        prompt: &str,
        schema: &Value,
        _web: bool,
        live: LiveTx,
    ) -> Result<Value> {
        let repo = Some(s.cwd.as_path());
        self.ask(&self.deep, repo, prompt, schema, DIVE_TIMEOUT, Some(&live))
            .await
    }

    async fn ask_probe(&self, s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.ask(
            &self.deep,
            Some(&s.cwd),
            prompt,
            schema,
            PROBE_TIMEOUT,
            None,
        )
        .await
    }

    async fn ask_prepare(&self, _s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.ask(
            &self.prepare,
            None,
            prompt,
            schema,
            STRUCTURED_TIMEOUT,
            None,
        )
        .await
    }

    async fn ask_second(&self, _s: &SessionRef, prompt: &str, schema: &Value) -> Result<Value> {
        self.ask(&self.second, None, prompt, schema, STRUCTURED_TIMEOUT, None)
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
