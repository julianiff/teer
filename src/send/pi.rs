//! Sessions of pi: finding the running ones, and sending them a message,
//! through teer's own pi extension (`teer-inbox.ts`, embedded here as
//! [`EXTENSION`]), which each session loads at start; installing that
//! extension ([`install`], to `<pi dir>/extensions/`).
//!
//! Each pi session with the extension listens on a Unix socket of its own
//! and registers in `<pi dir>/teer-inbox/<pid>.json` (0600):
//! `{protocol: 1, pid, cwd, sessionId, sessionFile, socket, token, name,
//! startedAt, updatedAt}`. teer connects, writes `{"type":"auth","token":…}`
//! and `{"type":"user","content":…,"session_id":…}` as lines, and reads one
//! answer line: `{"ok":true}`, or `{"ok":false,"error":…}`.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use super::{Agent, Dirs, PiState, Target};

/// The extension teer installs, as pi loads it.
pub const EXTENSION: &str = include_str!("teer-inbox.ts");
/// The line that marks the extension as teer's, and its version.
const MARKER: &str = "// teer-inbox v1";
/// What every version's marker starts with.
const MARKER_ANY: &str = "// teer-inbox v";
/// The extension's file name in `<pi dir>/extensions/`.
const FILE: &str = "teer-inbox.ts";
/// The registry protocol teer speaks.
const PROTOCOL: u64 = 1;
/// Largest registry file read.
const REGISTRY_CAP: u64 = 16 * 1024;
/// Largest extension file looked into.
const EXTENSION_CAP: u64 = 1024 * 1024;
/// How long pi has to answer.
const ANSWER_WAIT: Duration = Duration::from_secs(3);

/// A session as its registry file describes it.
#[derive(Debug, Clone, PartialEq)]
struct Registered {
    session_id: String,
    cwd: PathBuf,
    socket: PathBuf,
    token: String,
    name: Option<String>,
    active_at: u64,
}

/// The entry in `v`, read from `<pid>.json`: `None` unless it is protocol
/// 1, the pid is the file's, and it names a session, a cwd, a socket and a
/// key.
fn parse(v: &Value, file_pid: u32) -> Option<Registered> {
    if v["protocol"].as_u64() != Some(PROTOCOL) || v["pid"].as_u64() != Some(file_pid.into()) {
        return None;
    }
    let session_id = v["sessionId"].as_str()?.trim();
    let token = v["token"].as_str()?;
    if session_id.is_empty() || token.is_empty() || token.len() > 1024 {
        return None;
    }
    Some(Registered {
        session_id: session_id.to_string(),
        cwd: PathBuf::from(v["cwd"].as_str()?),
        socket: super::socket_path(v["socket"].as_str()?)?,
        token: token.to_string(),
        name: v["name"]
            .as_str()
            .map(|s| crate::util::sanitize(s).trim().to_string())
            .filter(|s| !s.is_empty()),
        active_at: v["updatedAt"]
            .as_u64()
            .or(v["startedAt"].as_u64())
            .unwrap_or(0),
    })
}

/// Every session registered, whether it still runs or not.
fn registered(dirs: &Dirs) -> Vec<Registered> {
    let Ok(entries) = std::fs::read_dir(dirs.pi.join("teer-inbox")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let pid: u32 = name.to_str()?.strip_suffix(".json")?.parse().ok()?;
            parse(&super::read_json(&e.path(), REGISTRY_CAP)?, pid)
        })
        .collect()
}

/// The pi sessions running now in `repo` that loaded teer's extension:
/// their socket takes a connection.
pub fn list(dirs: &Dirs, repo: &Path) -> Vec<Target> {
    let Ok(repo) = repo.canonicalize() else {
        return Vec::new();
    };
    registered(dirs)
        .into_iter()
        .filter(|r| super::inside(&r.cwd, &repo))
        .filter(|r| super::connect(&r.socket, super::CONNECT_TIMEOUT).is_ok())
        .map(|r| Target {
            agent: Agent::Pi,
            name: r.name.unwrap_or_else(|| super::short_id(&r.session_id)),
            id: r.session_id,
            cwd: r.cwd,
            status: String::new(),
            active_at: r.active_at,
            socket: Some(r.socket),
        })
        .collect()
}

/// Send `text` to session `t`: its registry entry (and key) is read again.
pub fn deliver(dirs: &Dirs, t: &Target, text: &str) -> Result<(), String> {
    super::check_text(text)?;
    // A session resumed in another pi has two entries: the listed one first.
    let mut all: Vec<Registered> = registered(dirs)
        .into_iter()
        .filter(|r| r.session_id == t.id)
        .collect();
    all.sort_by_key(|r| Some(&r.socket) != t.socket.as_ref());
    let r = all
        .into_iter()
        .next()
        .ok_or("the pi session isn't registered any more; has it ended?")?;
    let mut s = super::connect(&r.socket, super::CONNECT_TIMEOUT)
        .map_err(|e| format!("couldn't reach the pi session ({e})"))?;
    let _ = s.set_write_timeout(Some(ANSWER_WAIT));
    let _ = s.set_read_timeout(Some(ANSWER_WAIT));
    let lines = format!(
        "{}\n{}\n",
        json!({"type": "auth", "token": r.token}),
        json!({"type": "user", "content": text, "session_id": r.session_id}),
    );
    s.write_all(lines.as_bytes())
        .map_err(|e| format!("couldn't write to the pi session ({e})"))?;
    let mut answer = String::new();
    let read = std::io::BufReader::new((&s).take(4096)).read_line(&mut answer);
    match read {
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            return Err(format!(
                "pi didn't answer within {} s",
                ANSWER_WAIT.as_secs()
            ));
        }
        Err(e) => return Err(format!("pi's answer didn't come ({e})")),
        Ok(0) => return Err("pi closed the connection without an answer".into()),
        Ok(_) => {}
    }
    let v: Value =
        serde_json::from_str(answer.trim()).map_err(|_| "pi's answer isn't JSON".to_string())?;
    if v["ok"] == true {
        return Ok(());
    }
    Err(match v["error"].as_str() {
        Some("auth") => "the pi session refused teer's key".into(),
        Some(e) => format!("pi said: {}", crate::approve::tidy_output(e)),
        None => "pi refused the message".into(),
    })
}

/// The `pi` teer looks for: `$TEER_PI_BIN`, else `pi` on PATH.
pub fn program() -> OsString {
    super::program("TEER_PI_BIN", "pi")
}

/// Whether pi is installed, and teer's extension with it.
pub fn state(dirs: &Dirs) -> PiState {
    state_of(&program(), dirs)
}

/// [`state`] with pi as the program `pi`.
pub fn state_of(pi: &OsStr, dirs: &Dirs) -> PiState {
    if super::find_program(pi).is_none() {
        return PiState::NotFound;
    }
    match super::read_small(&extension_path(dirs), EXTENSION_CAP) {
        Some(b) if has_line(&b, |l| l == MARKER) => PiState::Ready,
        _ => PiState::NoExtension,
    }
}

/// Where pi loads teer's extension from.
pub fn extension_path(dirs: &Dirs) -> PathBuf {
    dirs.pi.join("extensions").join(FILE)
}

fn has_line(bytes: &[u8], want: impl Fn(&str) -> bool) -> bool {
    String::from_utf8_lossy(bytes)
        .lines()
        .any(|l| want(l.trim_end()))
}

/// Write teer's extension where pi finds it (`<pi dir>/extensions/`):
/// the file written. An older version of it is replaced; any other file
/// of that name is left alone.
pub fn install(dirs: &Dirs) -> Result<PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let dir = dirs.pi.join("extensions");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(&dir)
        .map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    let path = dir.join(FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            let teers = super::read_small(&path, EXTENSION_CAP)
                .is_some_and(|b| has_line(&b, |l| l.starts_with(MARKER_ANY)));
            if !teers {
                return Err(format!(
                    "{} is there and isn't teer's; teer won't overwrite it",
                    path.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("couldn't look at {}: {e}", path.display())),
    }
    let failed = |e: std::io::Error| format!("couldn't write {}: {e}", path.display());
    // Not `.ts` while it is written: pi loads only those.
    let mut tmp = tempfile::Builder::new()
        .prefix(".teer-inbox-")
        .suffix(".tmp")
        .tempfile_in(&dir)
        .map_err(failed)?;
    tmp.write_all(EXTENSION.as_bytes()).map_err(failed)?;
    tmp.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o644))
        .map_err(failed)?;
    tmp.as_file().sync_all().map_err(failed)?;
    tmp.persist(&path).map_err(|e| failed(e.error))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extension_carries_its_marker_and_needs_only_node() {
        assert!(has_line(EXTENSION.as_bytes(), |l| l == MARKER));
        for line in EXTENSION.lines().filter(|l| l.starts_with("import ")) {
            assert!(line.contains("\"node:"), "not a Node built-in: {line}");
        }
    }

    #[test]
    fn a_registry_entry_needs_protocol_1_and_its_own_pid() {
        let v = json!({
            "protocol": 1, "pid": 42, "cwd": "/r", "sessionId": "0199aa-bb",
            "sessionFile": "/s/2026_0199aa-bb.jsonl", "socket": "/tmp/teer-pi-501/42.sock",
            "token": "ab", "name": null, "startedAt": 1, "updatedAt": 5,
        });
        let r = parse(&v, 42).unwrap();
        assert_eq!((r.active_at, r.name.as_deref()), (5, None));
        assert!(parse(&v, 43).is_none());
        for (k, bad) in [
            ("protocol", json!(2)),
            ("token", json!("")),
            ("socket", json!("42.sock")),
            ("sessionId", json!(" ")),
        ] {
            let mut w = v.clone();
            w[k] = bad;
            assert!(parse(&w, 42).is_none(), "{k}");
        }
    }
}
