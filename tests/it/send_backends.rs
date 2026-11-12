//! Sending a tile's result to an agent session: send_backends (see FEATURES.md).
//!
//! The delivery backends against stand-ins: a Claude Code session and a pi
//! session are Unix sockets in temp dirs with their registry files, Codex
//! is a script that records its arguments ([`send_env`]) with rollout
//! files to find. teer's pi extension itself runs under Node with a
//! stand-in for pi's API, where `node` can run TypeScript (skipped
//! otherwise). Nothing here reaches a real session.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use teer::send::{self, Agent, Dirs, PiState, Target, claude, codex, pi};

use crate::common::{isolate_env, send_env};

/// A repo and each agent's folder in one short temp dir under `/tmp`: a
/// Unix socket path may be at most about 104 bytes.
struct World {
    root: tempfile::TempDir,
    dirs: Dirs,
    repo: PathBuf,
}

fn world() -> World {
    isolate_env();
    let root = tempfile::Builder::new()
        .prefix("tb")
        .tempdir_in("/tmp")
        .unwrap();
    let dirs = Dirs {
        claude: root.path().join("c"),
        codex: root.path().join("x"),
        pi: root.path().join("p"),
    };
    let repo = root.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::create_dir_all(root.path().join("elsewhere")).unwrap();
    let repo = repo.canonicalize().unwrap();
    World { root, dirs, repo }
}

impl World {
    fn path(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }
}

fn write_json(path: &Path, v: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, v.to_string()).unwrap();
}

/// The lines of the first connection to `l` that says something, once it
/// closes its end. (Listing connects and says nothing: that's liveness.)
fn receive(l: UnixListener) -> mpsc::Receiver<Vec<String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let mut s = String::new();
            let _ = conn.unwrap().read_to_string(&mut s);
            if !s.is_empty() {
                let _ = tx.send(s.lines().map(String::from).collect());
                return;
            }
        }
    });
    rx
}

/// Close `l`, leaving its socket file: a session that died. A child that
/// another test forks meanwhile holds the listener until it execs, so wait
/// until the socket refuses.
fn kill(l: UnixListener) {
    let path = l.local_addr().unwrap().as_pathname().unwrap().to_path_buf();
    drop(l);
    for _ in 0..200 {
        if UnixStream::connect(&path).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("{} still takes connections", path.display());
}

fn got(rx: &mpsc::Receiver<Vec<String>>) -> Vec<String> {
    rx.recv_timeout(Duration::from_secs(5)).expect("a message")
}

fn parse(line: &str) -> Value {
    serde_json::from_str(line).unwrap()
}

// ------------------------------------------------------------- Claude Code

const CC_ID: &str = "2cbe5981-1f0e-4c1e-9d55-3b1d5f0e2a10";
const CC_KEY: &str = "0123456789abcdef0123456789abcdef";

/// A Claude Code session `pid` (`<pid>.json` as 2.1.322 writes it)
/// listening at `<root>/<pid>.sock`.
fn claude_session(w: &World, pid: u32, id: &str, cwd: &Path, protocol: u64) -> UnixListener {
    let sock = w.path(&format!("{pid}.sock"));
    let l = UnixListener::bind(&sock).unwrap();
    write_json(
        &w.dirs.claude.join(format!("sessions/{pid}.json")),
        &json!({
            "pid": pid, "sessionId": id, "cwd": cwd, "startedAt": 1000,
            "procStart": "Thu Nov 12 11:06:31 2026", "version": "2.1.322",
            "peerProtocol": protocol, "peerFeatures": ["notify_idle", "artifact_yield"],
            "kind": "interactive", "entrypoint": "cli", "messagingSocketPath": sock,
            "name": format!("teer-{pid}"), "status": "idle",
            "statusUpdatedAt": 3000, "updatedAt": 2000,
        }),
    );
    l
}

/// A key file as Claude Code writes it, at `file`.
fn claude_key(file: &Path) {
    write_json(
        file,
        &json!({"peerToken": CC_KEY, "procStart": "Thu Nov 12 11:06:31 2026", "pidDomain": "d"}),
    );
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn claude_target(w: &World, pid: u32) -> Target {
    Target {
        agent: Agent::ClaudeCode,
        id: CC_ID.into(),
        name: format!("teer-{pid}"),
        cwd: w.repo.clone(),
        status: "idle".into(),
        active_at: 3000,
        socket: Some(w.path(&format!("{pid}.sock"))),
    }
}

#[test]
fn claude_code_sessions_are_the_live_ones_in_the_repo() {
    let w = world();
    let _live = claude_session(&w, 101, CC_ID, &w.repo.join("src"), 1);
    let _other = claude_session(&w, 102, "other-repo", &w.path("elsewhere"), 1);
    kill(claude_session(&w, 103, "ended", &w.repo, 1));
    std::fs::write(w.dirs.claude.join("sessions/104.json"), "{not json").unwrap();

    let found = claude::list(&w.dirs, &w.repo);
    assert_eq!(
        found,
        [Target {
            agent: Agent::ClaudeCode,
            id: CC_ID.into(),
            name: "teer-101".into(),
            cwd: w.repo.join("src"),
            status: "idle".into(),
            active_at: 3000,
            socket: Some(w.path("101.sock")),
        }]
    );
    assert_eq!(send::list(&w.dirs, &w.repo), found);
}

#[test]
fn claude_code_gets_the_key_then_the_message() {
    let w = world();
    let l = claude_session(&w, 201, CC_ID, &w.repo, 1);
    let sock = w.path("201.sock");
    claude_key(&claude::key_path(&w.dirs, 201, &sock));
    let rx = receive(l);

    let text = "Apply the advice:\n- check \"currency\" first";
    send::deliver(&w.dirs, &claude_target(&w, 201), text).unwrap();
    let lines = got(&rx);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(parse(&lines[0]), json!({"type": "auth", "token": CC_KEY}));
    let msg = parse(&lines[1]);
    let uuid = msg["uuid"].as_str().unwrap().to_string();
    assert_eq!(
        msg,
        json!({
            "type": "user",
            "message": {"role": "user", "content": text},
            "session_id": CC_ID,
            "from": "teer",
            "uuid": uuid,
        })
    );
    let groups: Vec<usize> = uuid.split('-').map(str::len).collect();
    assert_eq!(groups, [8, 4, 4, 4, 12]);
    assert_eq!(&uuid[14..15], "4", "a version 4 uuid: {uuid}");
}

#[test]
fn claude_code_without_a_key_gets_no_auth_line() {
    let w = world();
    let rx = receive(claude_session(&w, 301, CC_ID, &w.repo, 1));
    send::deliver(&w.dirs, &claude_target(&w, 301), "hello").unwrap();
    let lines = got(&rx);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(parse(&lines[0])["message"]["content"], "hello");
}

#[test]
fn claude_code_key_named_otherwise_is_used_when_it_is_the_only_one() {
    let w = world();
    let rx = receive(claude_session(&w, 401, CC_ID, &w.repo, 1));
    // Named by another socket path than the one registered.
    claude_key(
        &w.dirs
            .claude
            .join(format!("sessions/401.{}.key", "f".repeat(64))),
    );
    send::deliver(&w.dirs, &claude_target(&w, 401), "hello").unwrap();
    let lines = got(&rx);
    assert_eq!(parse(&lines[0]), json!({"type": "auth", "token": CC_KEY}));
}

#[test]
fn claude_code_with_another_protocol_is_refused() {
    let w = world();
    let l = claude_session(&w, 501, CC_ID, &w.repo, 2);
    let err = send::deliver(&w.dirs, &claude_target(&w, 501), "hello").unwrap_err();
    assert_eq!(
        err,
        "this Claude Code's messaging (protocol 2) is not known to teer"
    );
    l.set_nonblocking(true).unwrap();
    assert_eq!(
        l.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "nothing was sent"
    );
}

#[test]
fn claude_code_session_that_ended_is_said_so() {
    let w = world();
    let _l = claude_session(&w, 601, CC_ID, &w.repo, 1);
    std::fs::remove_file(w.dirs.claude.join("sessions/601.json")).unwrap();
    let err = send::deliver(&w.dirs, &claude_target(&w, 601), "hello").unwrap_err();
    assert!(err.contains("isn't registered any more"), "{err}");
    let err = send::deliver(&w.dirs, &claude_target(&w, 601), "  ").unwrap_err();
    assert_eq!(err, "the message is empty");
}

// ----------------------------------------------------------------------- pi

const PI_ID: &str = "0199f0c2-7a51-7c3e-b1a2-5e8d1c3f9a77";
const PI_KEY: &str = "fedcba9876543210fedcba9876543210";

/// A pi session `pid` registered as teer's extension does it, listening at
/// `<root>/pi-<pid>.sock`.
fn pi_session(w: &World, pid: u32, id: &str, cwd: &Path) -> UnixListener {
    let sock = w.path(&format!("pi-{pid}.sock"));
    let l = UnixListener::bind(&sock).unwrap();
    write_json(
        &w.dirs.pi.join(format!("teer-inbox/{pid}.json")),
        &json!({
            "protocol": 1, "pid": pid, "cwd": cwd, "sessionId": id,
            "sessionFile": format!("/s/2026-11-12T10-00-00-000Z_{id}.jsonl"),
            "socket": sock, "token": PI_KEY, "name": null,
            "startedAt": 1000, "updatedAt": 4000,
        }),
    );
    l
}

/// Answers the first connection that says something as the extension
/// does, with `key` and `id` its own; the lines it got.
fn pi_answers(l: UnixListener, key: &'static str, id: &'static str) -> mpsc::Receiver<Vec<String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let mut conn = conn.unwrap();
            let mut r = BufReader::new(conn.try_clone().unwrap());
            let mut auth = String::new();
            if r.read_line(&mut auth).unwrap_or(0) == 0 {
                continue;
            }
            let a = parse(&auth);
            if a["type"] != "auth" || a["token"] != key {
                conn.write_all(b"{\"ok\":false,\"error\":\"auth\"}\n")
                    .unwrap();
                let _ = tx.send(vec![auth]);
                return;
            }
            let mut user = String::new();
            r.read_line(&mut user).unwrap();
            let u = parse(&user);
            let answer = if u["session_id"] == id {
                json!({"ok": true})
            } else {
                json!({"ok": false, "error": "that session no longer runs in this pi"})
            };
            conn.write_all(format!("{answer}\n").as_bytes()).unwrap();
            let _ = tx.send(vec![auth, user]);
            return;
        }
    });
    rx
}

fn pi_target(w: &World, pid: u32, id: &str) -> Target {
    Target {
        agent: Agent::Pi,
        id: id.into(),
        name: id[..8].into(),
        cwd: w.repo.clone(),
        status: String::new(),
        active_at: 4000,
        socket: Some(w.path(&format!("pi-{pid}.sock"))),
    }
}

#[test]
fn pi_sessions_are_listed_and_take_a_message() {
    let w = world();
    let l = pi_session(&w, 701, PI_ID, &w.repo);
    let _other = pi_session(&w, 702, "other", &w.path("elsewhere"));
    kill(pi_session(&w, 703, "ended-0000", &w.repo));

    let found = pi::list(&w.dirs, &w.repo);
    assert_eq!(found, [pi_target(&w, 701, PI_ID)]);

    let rx = pi_answers(l, PI_KEY, PI_ID);
    send::deliver(&w.dirs, &found[0], "apply the advice").unwrap();
    let lines = got(&rx);
    assert_eq!(parse(&lines[0]), json!({"type": "auth", "token": PI_KEY}));
    assert_eq!(
        parse(&lines[1]),
        json!({"type": "user", "content": "apply the advice", "session_id": PI_ID})
    );
}

#[test]
fn pi_refusals_come_back_as_reasons() {
    let w = world();
    let rx = pi_answers(pi_session(&w, 801, PI_ID, &w.repo), "another-key", PI_ID);
    let err = send::deliver(&w.dirs, &pi_target(&w, 801, PI_ID), "x").unwrap_err();
    assert_eq!(err, "the pi session refused teer's key");
    got(&rx);

    // The session there now is another one. (Both entries name PI_ID: the
    // one with the target's socket is used.)
    let rx = pi_answers(pi_session(&w, 802, PI_ID, &w.repo), PI_KEY, "newer");
    let err = send::deliver(&w.dirs, &pi_target(&w, 802, PI_ID), "x").unwrap_err();
    assert_eq!(err, "pi said: that session no longer runs in this pi");
    got(&rx);
}

#[test]
fn a_pi_session_that_doesnt_answer_times_out() {
    let w = world();
    let l = pi_session(&w, 901, PI_ID, &w.repo);
    let held = std::thread::spawn(move || {
        let (c, _) = l.accept().unwrap();
        std::thread::sleep(Duration::from_secs(4));
        drop(c);
    });
    let err = send::deliver(&w.dirs, &pi_target(&w, 901, PI_ID), "x").unwrap_err();
    assert_eq!(err, "pi didn't answer within 3 s");
    held.join().unwrap();
}

#[test]
fn pi_state_says_whether_pi_and_the_extension_are_there() {
    let w = world();
    send_env();
    assert_eq!(
        pi::state_of(w.path("no-pi").as_os_str(), &w.dirs),
        PiState::NotFound
    );
    // `$TEER_PI_BIN` is the stand-in pi.
    assert_eq!(pi::state(&w.dirs), PiState::NoExtension);
    pi::install(&w.dirs).unwrap();
    assert_eq!(pi::state(&w.dirs), PiState::Ready);
    // An older version of teer's extension is there: not ready.
    std::fs::write(
        pi::extension_path(&w.dirs),
        "// teer-inbox v0\nexport default function () {}\n",
    )
    .unwrap();
    assert_eq!(pi::state(&w.dirs), PiState::NoExtension);
}

#[test]
fn installing_the_pi_extension_writes_it_and_leaves_a_foreign_file() {
    let w = world();
    let path = pi::install(&w.dirs).unwrap();
    assert_eq!(path, w.dirs.pi.join("extensions/teer-inbox.ts"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), pi::EXTENSION);
    assert!(pi::EXTENSION.lines().any(|l| l == "// teer-inbox v1"));
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o644);
    assert_eq!(mode(&w.dirs.pi.join("extensions")), 0o755);

    // teer's own, older: replaced.
    std::fs::write(&path, "// teer-inbox v0\n").unwrap();
    pi::install(&w.dirs).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), pi::EXTENSION);
    let names: Vec<_> = std::fs::read_dir(w.dirs.pi.join("extensions"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["teer-inbox.ts"], "no temp file left");

    // Someone else's file of that name: kept.
    std::fs::write(&path, "export default function () {}\n").unwrap();
    let err = pi::install(&w.dirs).unwrap_err();
    assert!(
        err.contains("isn't teer's; teer won't overwrite it"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "export default function () {}\n"
    );
    // A symlink there: not followed, not replaced.
    std::fs::remove_file(&path).unwrap();
    let target = w.path("theirs.ts");
    std::fs::write(&target, pi::EXTENSION).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(pi::install(&w.dirs).is_err());
    assert!(
        std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

// -------------------------------------------------------------------- Codex

const CX_ID: &str = "019a1b2c-3d4e-7f50-8a9b-0c1d2e3f4a5b";

/// A rollout of thread `id` started on `day` (`YYYY/MM/DD`) in `cwd`, last
/// written `ago`, its first line as Codex 0.167 writes it.
fn rollout(w: &World, day: &str, id: &str, cwd: &Path, source: &str, ago: Duration) {
    let path = w
        .dirs
        .codex
        .join("sessions")
        .join(day)
        .join(format!("rollout-2026-11-12T10-00-00-{id}.jsonl"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let meta = format!(
        r#"{{"timestamp":"2026-11-12T09:00:00.000Z","ordinal":0,"type":"session_meta","payload":{{"session_id":"{id}","id":"{id}","timestamp":"2026-11-12T09:00:00.000Z","cwd":{cwd},"originator":"codex-tui","cli_version":"0.167.2","source":"{source}","base_instructions":{{"text":"{text}"}}}}}}"#,
        cwd = json!(cwd),
        text = "x".repeat(18_000),
    );
    let body = format!("{meta}\n{{\"type\":\"response_item\",\"payload\":{{}}}}\n");
    std::fs::write(&path, body).unwrap();
    let f = std::fs::File::options().write(true).open(&path).unwrap();
    f.set_modified(SystemTime::now() - ago).unwrap();
}

fn codex_id(n: u32) -> String {
    format!("019a1b2c-3d4e-7f50-8a9b-{n:012}")
}

#[test]
fn codex_threads_are_the_recent_ones_in_the_repo() {
    let w = world();
    let min = |m: u64| Duration::from_secs(m * 60);
    rollout(&w, "2026/11/12", CX_ID, &w.repo, "cli", min(1));
    // Started days ago, written to since: still running.
    rollout(
        &w,
        "2026/11/01",
        &codex_id(2),
        &w.repo.join("src"),
        "vscode",
        min(10),
    );
    rollout(
        &w,
        "2026/11/12",
        &codex_id(3),
        &w.path("elsewhere"),
        "cli",
        min(2),
    );
    rollout(&w, "2026/11/12", &codex_id(4), &w.repo, "cli", min(3 * 60));
    rollout(&w, "2026/11/12", &codex_id(5), &w.repo, "exec", min(3));
    std::fs::write(
        w.dirs.codex.join("session_index.jsonl"),
        format!(
            "{}\n{}\n",
            json!({"id": CX_ID, "thread_name": "old name", "updated_at": "t"}),
            json!({"id": CX_ID, "thread_name": "fix currency", "updated_at": "t"}),
        ),
    )
    .unwrap();

    let found = codex::list(&w.dirs, &w.repo);
    let got: Vec<(&str, &str, &Path)> = found
        .iter()
        .map(|t| (t.id.as_str(), t.name.as_str(), t.cwd.as_path()))
        .collect();
    let second = codex_id(2);
    assert_eq!(
        got,
        [
            (CX_ID, "fix currency", w.repo.as_path()),
            (second.as_str(), "019a1b2c", w.repo.join("src").as_path()),
        ]
    );
    assert!(found.iter().all(|t| t.agent == Agent::Codex
        && t.socket.is_none()
        && t.status.is_empty()
        && t.active_at > 0));
    assert!(found[0].active_at > found[1].active_at);
}

#[test]
fn at_most_ten_codex_threads() {
    let w = world();
    for n in 0..12 {
        rollout(
            &w,
            "2026/11/12",
            &codex_id(100 + n),
            &w.repo,
            "cli",
            Duration::from_secs(60 + u64::from(n)),
        );
    }
    let found = codex::list(&w.dirs, &w.repo);
    assert_eq!(found.len(), 10);
    assert_eq!(found[0].id, codex_id(100), "newest first");
}

#[test]
fn codex_gets_the_message_through_codex_queue() {
    let w = world();
    let env = send_env();
    let t = Target {
        agent: Agent::Codex,
        id: CX_ID.into(),
        name: "fix currency".into(),
        cwd: w.repo.clone(),
        status: String::new(),
        active_at: 1,
        socket: None,
    };
    // A text that starts with `-` stays the message, not an option.
    let text = "-- apply the advice:\n'check' \"currency\" $HOME `id`";
    send::deliver(&w.dirs, &t, text).unwrap();
    let read = |ext: &str| {
        std::fs::read_to_string(env.codex_calls.join(format!("{CX_ID}.{ext}"))).unwrap()
    };
    let args: Vec<String> = read("args")
        .split('\0')
        .filter(|a| !a.is_empty())
        .map(String::from)
        .collect();
    assert_eq!(
        args,
        [
            "queue".to_string(),
            format!("--thread={CX_ID}"),
            format!("--message={text}")
        ]
    );
    assert_eq!(read("cwd").trim(), w.repo.to_str().unwrap());
    assert_eq!(read("home"), w.dirs.codex.to_str().unwrap());
}

#[test]
fn a_codex_that_fails_says_why() {
    let w = world();
    send_env();
    let t = Target {
        agent: Agent::Codex,
        id: "fail-1".into(),
        name: "x".into(),
        cwd: w.repo.clone(),
        status: String::new(),
        active_at: 1,
        socket: None,
    };
    let err = codex::deliver(&w.dirs, &t, "hello").unwrap_err();
    assert_eq!(err, "codex queue failed: error: no session named fail-1");
    let err = codex::queue(w.path("no-codex").as_os_str(), &w.dirs, &t, "hello").unwrap_err();
    assert_eq!(err, "Codex isn't installed (no `codex` on PATH)");
    let t = Target {
        id: "--help".into(),
        ..t
    };
    assert!(codex::deliver(&w.dirs, &t, "hello").is_err());
}

// --------------------------------------------- teer's pi extension, in Node

/// Runs the extension with a stand-in for pi's API, as pi would: fires
/// `session_start`, prints `ready`, and on `shutdown` fires
/// `session_shutdown` and prints what reached pi.
const HARNESS: &str = r#"
import { pathToFileURL } from "node:url";
import * as readline from "node:readline";
const [ext, cwd, sessionId] = process.argv.slice(2);
const handlers = {};
const sent = [];
const notes = [];
const pi = {
  on(ev, h) { (handlers[ev] ??= []).push(h); },
  sendUserMessage(content, options) { sent.push({ content, options }); },
  getSessionName() { return "review-fix"; },
};
const ctx = {
  cwd,
  hasUI: true,
  ui: { notify(m, t) { notes.push([m, t]); } },
  sessionManager: {
    getSessionId: () => sessionId,
    getSessionFile: () => `/s/2026-11-12T10-00-00-000Z_${sessionId}.jsonl`,
  },
};
const fire = async (ev, e) => { for (const h of handlers[ev] ?? []) await h(e, ctx); };
(await import(pathToFileURL(ext).href)).default(pi);
await fire("session_start", { reason: "startup" });
console.log("ready");
for await (const line of readline.createInterface({ input: process.stdin })) {
  if (line === "shutdown") {
    await fire("session_shutdown", { reason: "quit" });
    console.log(JSON.stringify({ sent, notes }));
    process.exit(0);
  }
}
"#;

/// `node`, when it runs TypeScript as pi's extensions are written.
fn node_with_typescript() -> Option<()> {
    let out = Command::new("node")
        .args(["-p", "Boolean(process.features.typescript)"])
        .output()
        .ok()?;
    (String::from_utf8_lossy(&out.stdout).trim() == "true").then_some(())
}

/// One request on the extension's socket: its answer.
fn ask(sock: &Path, lines: &[Value]) -> Value {
    let mut s = UnixStream::connect(sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    for l in lines {
        s.write_all(format!("{l}\n").as_bytes()).unwrap();
    }
    let mut answer = String::new();
    BufReader::new(s).read_line(&mut answer).unwrap();
    parse(&answer)
}

#[test]
fn the_pi_extension_registers_answers_and_cleans_up() {
    if node_with_typescript().is_none() {
        eprintln!("skipped: no node that runs TypeScript");
        return;
    }
    let w = world();
    let ext = pi::install(&w.dirs).unwrap();
    let inbox = w.dirs.pi.join("teer-inbox");
    let uid = std::fs::metadata(w.root.path()).unwrap().uid();
    let sockets = w.path(&format!("teer-pi-{uid}"));
    // An entry left by a pi that is gone: swept away.
    let mut gone = Command::new("true").spawn().unwrap();
    let gone_pid = gone.id();
    gone.wait().unwrap();
    std::fs::create_dir_all(&sockets).unwrap();
    std::fs::set_permissions(&sockets, std::fs::Permissions::from_mode(0o700)).unwrap();
    let stale_sock = sockets.join(format!("{gone_pid}.sock"));
    std::fs::write(&stale_sock, "").unwrap();
    write_json(
        &inbox.join(format!("{gone_pid}.json")),
        &json!({"protocol": 1, "pid": gone_pid, "socket": stale_sock}),
    );

    std::fs::write(w.path("harness.mjs"), HARNESS).unwrap();
    let mut node = Running(
        Command::new("node")
            .arg(w.path("harness.mjs"))
            .arg(&ext)
            .arg(&w.repo)
            .arg(PI_ID)
            .env("PI_CODING_AGENT_DIR", &w.dirs.pi)
            // Its socket goes in this test's folder, not the shared one.
            .env("TMPDIR", w.root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let pid = node.0.id();
    let out = BufReader::new(node.0.stdout.take().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in out.lines() {
            let _ = tx.send(line.unwrap());
        }
    });
    let next = || {
        rx.recv_timeout(Duration::from_secs(20))
            .expect("node said nothing")
    };
    assert_eq!(next(), "ready");

    // Registered, privately.
    let entry = inbox.join(format!("{pid}.json"));
    let reg: Value = parse(&std::fs::read_to_string(&entry).unwrap());
    let sock = PathBuf::from(reg["socket"].as_str().unwrap());
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(sock, sockets.join(format!("{pid}.sock")));
    assert_eq!(
        (mode(&entry), mode(&sock), mode(&sockets), mode(&inbox)),
        (0o600, 0o600, 0o700, 0o700)
    );
    assert_eq!(reg["protocol"], 1);
    assert_eq!(reg["sessionId"], PI_ID);
    assert_eq!(reg["cwd"], json!(w.repo));
    assert_eq!(reg["token"].as_str().unwrap().len(), 32);
    assert!(!inbox.join(format!("{gone_pid}.json")).exists(), "swept");
    assert!(!stale_sock.exists(), "swept");

    // Found and reached as teer does it.
    let found = pi::list(&w.dirs, &w.repo);
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].name.as_str(), found[0].id.as_str()),
        ("review-fix", PI_ID)
    );
    send::deliver(&w.dirs, &found[0], "apply the advice").unwrap();

    // Without the key, or for another session: refused, nothing reaches pi.
    let token = reg["token"].clone();
    let user = |id: &str| json!({"type": "user", "content": "nope", "session_id": id});
    assert_eq!(
        ask(
            &sock,
            &[
                json!({"type": "auth", "token": "0".repeat(32)}),
                user(PI_ID)
            ]
        ),
        json!({"ok": false, "error": "auth"})
    );
    assert_eq!(
        ask(&sock, &[user(PI_ID)]),
        json!({"ok": false, "error": "auth"})
    );
    let other = ask(
        &sock,
        &[json!({"type": "auth", "token": token}), user("another")],
    );
    assert_eq!(other["ok"], false);

    node.0
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"shutdown\n")
        .unwrap();
    let report = parse(&next());
    assert_eq!(
        report,
        json!({
            "sent": [{"content": "apply the advice", "options": {"deliverAs": "followUp"}}],
            "notes": [["teer sent a message", "info"]],
        })
    );
    assert!(node.0.wait().unwrap().success());
    assert!(!entry.exists(), "unregistered");
    assert!(!sock.exists(), "socket removed");
    assert!(pi::list(&w.dirs, &w.repo).is_empty());
}

/// A child stopped when the test ends, passed or not.
struct Running(std::process::Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
