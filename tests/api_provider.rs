//! The model-API provider against a fake server on localhost: the agent
//! loop with teer's repository tools, in Anthropic's and OpenAI's format,
//! the fallback when the API refuses the answer's schema, and retries.

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use teer::agent::api::ApiAdapter;
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
/// body) and keeps what it was sent.
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
            let text = body.to_string();
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{text}",
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
