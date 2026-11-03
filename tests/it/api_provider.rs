//! The model-API provider against a fake server on localhost: the agent
//! loop with teer's repository tools, in Anthropic's and OpenAI's format,
//! the fallback when the API refuses the answer's schema, and retries.

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use teer::agent::api::ApiAdapter;
use teer::agent::live::Live;
use teer::agent::{AgentAdapter, PrepareModel};
use teer::config::ApiFormat;
use teer::review::model::SessionRef;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
struct Request {
    head: String,
    body: Value,
}

impl Request {
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    }
}

/// Answers each request with the next of `replies` (status, extra headers,
/// body: JSON, or a string of server-sent events) and keeps what it was sent.
async fn server(replies: Vec<(u16, &'static str, Value)>) -> (String, Arc<Mutex<Vec<Request>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        for (status, headers, body) in replies {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let (head, len) = loop {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..end]).to_string();
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    buf.drain(..end + 4);
                    break (head, len);
                }
            };
            while buf.len() < len {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
            }
            log.lock().unwrap().push(Request {
                head,
                body: serde_json::from_slice(&buf).unwrap_or(Value::Null),
            });
            // A string is a stream of server-sent events.
            let (kind, text) = match body {
                Value::String(events) => ("text/event-stream", events),
                json => ("application/json", json.to_string()),
            };
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: {kind}\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{text}",
                text.len()
            );
            sock.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    (url, seen)
}

fn model(name: &str) -> PrepareModel {
    PrepareModel {
        model: Some(name.into()),
        effort: None,
    }
}

fn adapter(format: ApiFormat, url: &str) -> ApiAdapter {
    ApiAdapter::new(
        format,
        url,
        Some("sk-test".into()),
        model("fast"),
        model("second"),
        model("deep"),
    )
    .unwrap()
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.rs"), "fn needle() {}\n").unwrap();
    d
}

fn session(p: &Path) -> SessionRef {
    SessionRef::fresh("api", p.to_path_buf())
}

fn schema() -> Value {
    json!({ "type": "object", "properties": { "text": { "type": "string", "maxLength": 50 }, "extra": { "type": "string" } }, "required": ["text"] })
}

#[tokio::test]
async fn openai_format_reads_the_repo_with_teers_tools_then_answers() {
    let (url, seen) = server(vec![
        (200, "", json!({ "choices": [{ "finish_reason": "tool_calls", "message": {
            "role": "assistant", "content": null,
            "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "grep", "arguments": "{\"pattern\":\"needle\"}" } }]
        } }] })),
        (200, "", json!({ "choices": [{ "finish_reason": "stop", "message": {
            "role": "assistant", "content": "{\"text\":\"found it\",\"extra\":null}" } }] })),
    ])
    .await;
    let d = repo();
    let a = adapter(ApiFormat::Openai, &format!("{url}/v1"));
    let v = a
        .ask_dive(&session(d.path()), "where is needle?", &schema(), true)
        .await
        .unwrap();
    // The `null` a strict model may fill an optional field with is dropped.
    assert_eq!(v, json!({ "text": "found it" }));
    let reqs = seen.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    assert!(
        reqs[0].head.starts_with("POST /v1/chat/completions "),
        "{}",
        reqs[0].head
    );
    assert_eq!(
        reqs[0].header("authorization").as_deref(),
        Some("Bearer sk-test")
    );
    let b = &reqs[0].body;
    assert_eq!(b["model"], "deep");
    assert_eq!(b["tools"].as_array().unwrap().len(), 3);
    assert_eq!(
        b["response_format"]["json_schema"]["schema"]["additionalProperties"],
        json!(false)
    );
    let last = reqs[1].body["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["role"], "tool");
    assert_eq!(last["tool_call_id"], "c1");
    assert_eq!(last["content"], "a.rs:1: fn needle() {}");
}

#[tokio::test]
async fn anthropic_format_sends_tool_results_and_reads_the_answer() {
    let (url, seen) = server(vec![
        (200, "", json!({ "stop_reason": "tool_use", "content": [
            { "type": "text", "text": "Let me look." },
            { "type": "tool_use", "id": "t1", "name": "read_file", "input": { "path": "a.rs" } },
            { "type": "tool_use", "id": "t2", "name": "read_file", "input": { "path": "../etc/passwd" } }
        ] })),
        (200, "", json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "{\"text\":\"ok\"}" }] })),
    ])
    .await;
    let d = repo();
    let a = adapter(ApiFormat::Anthropic, &url);
    let v = a
        .ask_probe(&session(d.path()), "probe", &schema())
        .await
        .unwrap();
    assert_eq!(v, json!({ "text": "ok" }));
    let reqs = seen.lock().unwrap().clone();
    assert!(
        reqs[0].head.starts_with("POST /v1/messages "),
        "{}",
        reqs[0].head
    );
    assert_eq!(reqs[0].header("x-api-key").as_deref(), Some("sk-test"));
    assert_eq!(
        reqs[0].header("anthropic-version").as_deref(),
        Some("2026-10-21")
    );
    let b = &reqs[0].body;
    assert_eq!(b["model"], "deep");
    let format = &b["output_config"]["format"]["schema"];
    assert_eq!(
        format["properties"]["text"],
        json!({ "type": "string", "description": "At most 50 characters" })
    );
    let msgs = reqs[1].body["messages"].as_array().unwrap().clone();
    assert_eq!(msgs[1]["role"], "assistant");
    let results = msgs[2]["content"].as_array().unwrap();
    assert_eq!(results[0]["tool_use_id"], "t1");
    assert!(
        results[0]["content"]
            .as_str()
            .unwrap()
            .contains("fn needle() {}")
    );
    // Out of the repository: refused, and the model is told so.
    assert_eq!(results[1]["is_error"], json!(true));
}

#[tokio::test]
async fn describing_units_has_no_tools_and_a_refused_schema_is_asked_for_in_words() {
    let (url, seen) = server(vec![
        (400, "", json!({ "error": { "message": "response_format json_schema is not supported" } })),
        (200, "", json!({ "choices": [{ "finish_reason": "stop", "message": { "content": "```json\n{\"text\":\"a\"}\n```" } }] })),
        (200, "", json!({ "choices": [{ "finish_reason": "stop", "message": { "content": "{\"text\":\"b\"}" } }] })),
    ])
    .await;
    let a = adapter(ApiFormat::Openai, &url);
    let s = session(Path::new("."));
    assert_eq!(
        a.ask_prepare(&s, "describe", &schema()).await.unwrap(),
        json!({ "text": "a" })
    );
    assert_eq!(
        a.ask_second(&s, "check", &schema()).await.unwrap(),
        json!({ "text": "b" })
    );
    let reqs = seen.lock().unwrap().clone();
    assert_eq!(reqs[0].body["model"], "fast");
    assert!(reqs[0].body.get("tools").is_none());
    assert!(reqs[0].body.get("response_format").is_some());
    assert!(reqs[1].body.get("response_format").is_none());
    assert!(
        reqs[1].body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("JSON Schema")
    );
    // From then on without it.
    assert_eq!(reqs[2].body["model"], "second");
    assert!(reqs[2].body.get("response_format").is_none());
}

#[tokio::test]
async fn a_busy_api_is_asked_again_and_errors_carry_its_words() {
    let (url, seen) = server(vec![
        (429, "retry-after: 0\r\n", json!({ "error": { "message": "slow down" } })),
        (200, "", json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "{\"text\":\"x\"}" }] })),
        (401, "", json!({ "type": "error", "error": { "type": "authentication_error", "message": "invalid x-api-key" } })),
    ])
    .await;
    let a = adapter(ApiFormat::Anthropic, &url);
    let s = session(Path::new("."));
    assert_eq!(
        a.ask_prepare(&s, "describe", &schema()).await.unwrap(),
        json!({ "text": "x" })
    );
    let err = a.ask_prepare(&s, "describe", &schema()).await.unwrap_err();
    assert!(format!("{err:#}").contains("invalid x-api-key"), "{err:#}");
    assert_eq!(seen.lock().unwrap().len(), 3);
}

/// Server-sent events, one per `(event, data)`.
fn events(list: &[(&str, Value)]) -> Value {
    Value::String(
        list.iter()
            .map(|(e, d)| format!("event: {e}\ndata: {d}\n\n"))
            .collect(),
    )
}

/// Ask with someone watching: the answer, and what they heard.
async fn ask_live(a: &ApiAdapter, d: &Path) -> (anyhow::Result<Value>, Vec<Live>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let v = a
        .ask_live(&session(d), "where is needle?", &schema(), false, tx)
        .await;
    let mut heard = Vec::new();
    while let Ok(l) = rx.try_recv() {
        heard.push(l);
    }
    (v, heard)
}

#[tokio::test]
async fn anthropic_streamed_tells_the_steps_and_the_answer_as_it_is_written() {
    let start = |i: u64, block: Value| {
        (
            "content_block_start",
            json!({ "type": "content_block_start", "index": i, "content_block": block }),
        )
    };
    let delta = |i: u64, d: Value| {
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": i, "delta": d }),
        )
    };
    let stop = |i: u64| {
        (
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": i }),
        )
    };
    let message = |id: &str| {
        (
            "message_start",
            json!({ "type": "message_start", "message": {
            "id": id, "type": "message", "role": "assistant", "content": [], "stop_reason": null } }),
        )
    };
    let end = |why: &str| {
        [
            (
                "message_delta",
                json!({ "type": "message_delta", "delta": { "stop_reason": why }, "usage": { "output_tokens": 9 } }),
            ),
            ("message_stop", json!({ "type": "message_stop" })),
        ]
    };
    let mut turn1 = vec![
        message("m1"),
        ("ping", json!({ "type": "ping" })),
        start(
            0,
            json!({ "type": "thinking", "thinking": "", "signature": "" }),
        ),
        delta(
            0,
            json!({ "type": "thinking_delta", "thinking": "Look for it." }),
        ),
        delta(0, json!({ "type": "signature_delta", "signature": "sig" })),
        stop(0),
        start(
            1,
            json!({ "type": "tool_use", "id": "t1", "name": "grep", "input": {} }),
        ),
        delta(
            1,
            json!({ "type": "input_json_delta", "partial_json": "{\"pattern\": " }),
        ),
        delta(
            1,
            json!({ "type": "input_json_delta", "partial_json": "\"needle\"}" }),
        ),
        stop(1),
    ];
    turn1.extend(end("tool_use"));
    let mut turn2 = vec![
        message("m2"),
        start(0, json!({ "type": "text", "text": "" })),
        delta(
            0,
            json!({ "type": "text_delta", "text": "{\"text\": \"in a" }),
        ),
        delta(0, json!({ "type": "text_delta", "text": ".rs\"}" })),
        stop(0),
    ];
    turn2.extend(end("end_turn"));
    let (url, seen) = server(vec![(200, "", events(&turn1)), (200, "", events(&turn2))]).await;
    let d = repo();
    let a = adapter(ApiFormat::Anthropic, &url);
    let (v, heard) = ask_live(&a, d.path()).await;
    assert_eq!(v.unwrap(), json!({ "text": "in a.rs" }));
    assert_eq!(
        heard,
        [
            Live::Step("search “needle”".into()),
            Live::Partial("{\"text\": \"in a".into()),
            Live::Partial("{\"text\": \"in a.rs\"}".into()),
        ]
    );
    let reqs = seen.lock().unwrap().clone();
    assert_eq!(reqs[0].body["stream"], json!(true));
    // The reply goes back as the API gave it: the thinking with its
    // signature, the tool call with its input.
    let msgs = reqs[1].body["messages"].as_array().unwrap().clone();
    assert_eq!(
        msgs[1],
        json!({ "role": "assistant", "content": [
            { "type": "thinking", "thinking": "Look for it.", "signature": "sig" },
            { "type": "tool_use", "id": "t1", "name": "grep", "input": { "pattern": "needle" } }
        ] })
    );
    assert_eq!(msgs[2]["content"][0]["content"], "a.rs:1: fn needle() {}");
}

#[tokio::test]
async fn openai_streamed_puts_tool_calls_back_together() {
    let chunk = |delta: Value, finish: Value| {
        (
            "",
            json!({ "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] }),
        )
    };
    let turn1 = events(&[
        chunk(json!({ "role": "assistant", "content": null }), Value::Null),
        chunk(
            json!({ "tool_calls": [{ "index": 0, "id": "c1", "type": "function", "function": { "name": "read_file", "arguments": "" } }] }),
            Value::Null,
        ),
        chunk(
            json!({ "tool_calls": [{ "index": 0, "function": { "arguments": "{\"path\":" } }] }),
            Value::Null,
        ),
        chunk(
            json!({ "tool_calls": [{ "index": 0, "function": { "arguments": "\"a.rs\"}" } }] }),
            Value::Null,
        ),
        chunk(json!({}), json!("tool_calls")),
    ]);
    let turn2 = events(&[
        chunk(json!({ "role": "assistant", "content": "" }), Value::Null),
        chunk(json!({ "content": "{\"text\":\"fn ne" }), Value::Null),
        chunk(json!({ "content": "edle\"}" }), json!("stop")),
    ]);
    // `data: [DONE]` ends an OpenAI stream.
    let done = |v: Value| Value::String(v.as_str().unwrap().to_string() + "data: [DONE]\n\n");
    let (url, seen) = server(vec![(200, "", done(turn1)), (200, "", done(turn2))]).await;
    let d = repo();
    let a = adapter(ApiFormat::Openai, &url);
    let (v, heard) = ask_live(&a, d.path()).await;
    assert_eq!(v.unwrap(), json!({ "text": "fn needle" }));
    assert_eq!(
        heard,
        [
            Live::Step("read a.rs".into()),
            Live::Partial("{\"text\":\"fn ne".into()),
            Live::Partial("{\"text\":\"fn needle\"}".into()),
        ]
    );
    let reqs = seen.lock().unwrap().clone();
    let msgs = reqs[1].body["messages"].as_array().unwrap().clone();
    assert_eq!(
        msgs[2],
        json!({ "role": "assistant", "content": null, "tool_calls": [
            { "id": "c1", "type": "function", "function": { "name": "read_file", "arguments": "{\"path\":\"a.rs\"}" } }
        ] })
    );
    assert_eq!(msgs[3]["content"], "     1\tfn needle() {}");
}

#[tokio::test]
async fn a_server_that_doesnt_stream_still_answers_and_a_broken_stream_fails() {
    let (url, _) = server(vec![
        (200, "", json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "{\"text\":\"ok\"}" }] })),
        (200, "", events(&[
            ("message_start", json!({ "type": "message_start", "message": { "role": "assistant", "content": [] } })),
            ("error", json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } })),
        ])),
        (200, "", events(&[
            ("message_start", json!({ "type": "message_start", "message": { "role": "assistant", "content": [] } })),
        ])),
    ])
    .await;
    let d = repo();
    let a = adapter(ApiFormat::Anthropic, &url);
    let (v, heard) = ask_live(&a, d.path()).await;
    assert_eq!(v.unwrap(), json!({ "text": "ok" }));
    assert!(heard.is_empty());
    let e = ask_live(&a, d.path()).await.0.unwrap_err().to_string();
    assert!(e.contains("Overloaded"), "{e}");
    let e = ask_live(&a, d.path()).await.0.unwrap_err().to_string();
    assert!(e.contains("ended before the reply did"), "{e}");
}

#[tokio::test]
async fn the_model_list_comes_from_the_api() {
    let (url, seen) = server(vec![(
        200,
        "",
        json!({ "data": [{ "id": "claude-haiku-5-5" }, { "id": "claude-sonnet-5-5" }] }),
    )])
    .await;
    let models = teer::agent::api::list_models(ApiFormat::Anthropic, &url, Some("k"))
        .await
        .unwrap();
    assert_eq!(models, ["claude-haiku-5-5", "claude-sonnet-5-5"]);
    assert!(
        seen.lock().unwrap()[0]
            .head
            .starts_with("GET /v1/models?limit=1000 ")
    );
}
