//! Sessions of Codex: finding the running ones, and sending them a message
//! with `codex queue --thread <id> --message <text>` ("Queue a message for
//! an existing session").
//!
//! Codex keeps one rollout per thread,
//! `<codex dir>/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`. Its first
//! line is the thread's meta; on Codex 0.167:
//! `{"timestamp","ordinal","type":"session_meta","payload":{"id","session_id","cwd","originator","source","cli_version",…,"base_instructions":{…},"git":{…}}}`
//! (about 19 KB, the instructions included). A thread counts as running
//! when its rollout changed in the last [`RECENT`]. Thread names are in
//! `<codex dir>/session_index.jsonl`, a line `{"id","thread_name","updated_at"}`
//! each, the last for an id winning.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, Read, Seek};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::{Agent, Dirs, Target};

/// A thread whose rollout changed this recently counts as running.
pub const RECENT: Duration = Duration::from_secs(2 * 60 * 60);
/// At most this many Codex threads are offered.
const MAX: usize = 10;
/// The most of a rollout's first line read.
const HEAD_CAP: u64 = 256 * 1024;
/// The most of `session_index.jsonl` read, from its end.
const INDEX_CAP: u64 = 1024 * 1024;
/// How long `codex queue` may take.
const QUEUE_TIMEOUT: Duration = Duration::from_secs(30);

/// What a rollout's first line says about its thread.
#[derive(Debug, Clone, PartialEq)]
struct Meta {
    id: String,
    cwd: PathBuf,
    /// Not a `codex exec` run or another tool's (`mcp`, a subagent): no
    /// one is there to read a message.
    interactive: bool,
}

/// The Codex threads running now in `repo`, the most recently written
/// first.
pub fn list(dirs: &Dirs, repo: &Path) -> Vec<Target> {
    let Ok(repo) = repo.canonicalize() else {
        return Vec::new();
    };
    let since = SystemTime::now().checked_sub(RECENT).unwrap_or(UNIX_EPOCH);
    let mut recent = rollouts(&dirs.codex.join("sessions"), since);
    recent.sort_by_key(|r| std::cmp::Reverse(r.0));
    let mut out: Vec<Target> = Vec::new();
    for (changed, path) in recent {
        if out.len() == MAX {
            break;
        }
        let Some(m) = head(&path).and_then(|l| meta(&l)) else {
            continue;
        };
        if !m.interactive || out.iter().any(|t| t.id == m.id) || !super::inside(&m.cwd, &repo) {
            continue;
        }
        out.push(Target {
            agent: Agent::Codex,
            name: super::short_id(&m.id),
            id: m.id,
            cwd: m.cwd,
            status: String::new(),
            active_at: changed
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
            socket: None,
        });
    }
    if !out.is_empty() {
        let names = names(&dirs.codex);
        for t in &mut out {
            if let Some(n) = names.get(&t.id) {
                t.name = n.clone();
            }
        }
    }
    out
}

/// The `codex` teer runs: `$TEER_CODEX_BIN`, else `codex` on PATH.
pub fn program() -> OsString {
    super::program("TEER_CODEX_BIN", "codex")
}

/// Send `text` to thread `t` with [`program`].
pub fn deliver(dirs: &Dirs, t: &Target, text: &str) -> Result<(), String> {
    queue(&program(), dirs, t, text)
}

/// Run `codex queue` (as `codex`) for thread `t` with `text`, no shell in
/// between, with `CODEX_HOME` the folder the thread was found in.
pub fn queue(codex: &OsStr, dirs: &Dirs, t: &Target, text: &str) -> Result<(), String> {
    super::check_text(text)?;
    if !valid_id(&t.id) {
        return Err("the thread's id isn't one Codex gives".into());
    }
    // An argument can't hold NUL; `=` keeps a text that starts with `-` a value.
    let text = text.replace('\0', "");
    let mut cmd = std::process::Command::new(codex);
    cmd.arg("queue")
        .arg(format!("--thread={}", t.id))
        .arg(format!("--message={text}"))
        .env("CODEX_HOME", &dirs.codex)
        .env("NO_COLOR", "1");
    if t.cwd.is_dir() {
        cmd.current_dir(&t.cwd);
    }
    let finished = crate::util::output_within(&mut cmd, QUEUE_TIMEOUT, 1 << 16).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "Codex isn't installed (no `codex` on PATH)".to_string()
        } else {
            format!(
                "couldn't run codex: {}",
                crate::approve::tidy_output(&e.to_string())
            )
        }
    })?;
    let Some((status, stdout, stderr)) = finished else {
        return Err(format!(
            "codex queue didn't finish within {} s",
            QUEUE_TIMEOUT.as_secs()
        ));
    };
    if status.success() {
        return Ok(());
    }
    let said = [stderr, stdout]
        .iter()
        .map(|b| crate::approve::tidy_output(&String::from_utf8_lossy(b)))
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| match status.code() {
            Some(c) => format!("exit code {c}"),
            None => "stopped by a signal".into(),
        });
    Err(format!("codex queue failed: {said}"))
}

/// An id as Codex gives them (a UUID): letters, digits, `-` and `_`.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Every `rollout-*.jsonl` under `root/YYYY/MM/DD/` changed since `since`,
/// with when. Symlinks aren't followed.
fn rollouts(root: &Path, since: SystemTime) -> Vec<(SystemTime, PathBuf)> {
    fn subdirs(dir: &Path, digits: usize) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.len() == digits && n.bytes().all(|b| b.is_ascii_digit()))
                    && e.file_type().is_ok_and(|t| t.is_dir())
            })
            .map(|e| e.path())
            .collect()
    }
    let mut out = Vec::new();
    for day in subdirs(root, 4)
        .iter()
        .flat_map(|y| subdirs(y, 2))
        .flat_map(|m| subdirs(&m, 2))
    {
        let Ok(entries) = std::fs::read_dir(&day) else {
            continue;
        };
        for e in entries.flatten() {
            let rollout = e
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"));
            if !rollout || !e.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            if let Ok(changed) = e.metadata().and_then(|m| m.modified())
                && changed >= since
            {
                out.push((changed, e.path()));
            }
        }
    }
    out
}

/// The first line of the rollout at `path`, at most [`HEAD_CAP`] of it.
fn head(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut line = Vec::new();
    std::io::BufReader::new(file.take(HEAD_CAP))
        .read_until(b'\n', &mut line)
        .ok()?;
    Some(line)
}

/// The thread a rollout's first line describes: `session_meta` with its
/// fields in `payload` (Codex 0.4x and later), or the fields at the top
/// (older). A line cut short by [`HEAD_CAP`] still gives the `id` and
/// `cwd` it starts with.
fn meta(line: &[u8]) -> Option<Meta> {
    let m = match serde_json::from_slice::<Value>(line) {
        Ok(v) => {
            let p = if v["type"] == "session_meta" {
                &v["payload"]
            } else if v.get("payload").is_none() {
                &v
            } else {
                return None;
            };
            let source = &p["source"];
            Meta {
                id: p["id"].as_str().or(p["session_id"].as_str())?.to_string(),
                cwd: PathBuf::from(p["cwd"].as_str()?),
                interactive: !(source == "exec" || source == "mcp" || source.is_object()),
            }
        }
        Err(_) => {
            let is_meta = find(line, br#""type":"session_meta""#).is_some();
            let source = field(line, "source");
            Meta {
                id: field(line, "id")?,
                cwd: PathBuf::from(field(line, "cwd")?),
                interactive: is_meta && !matches!(source.as_deref(), Some("exec" | "mcp")),
            }
        }
    };
    valid_id(&m.id).then_some(m)
}

/// The string value of the first `"key":` in `line`.
fn field(line: &[u8], key: &str) -> Option<String> {
    let pat = format!("\"{key}\":");
    let at = find(line, pat.as_bytes())? + pat.len();
    serde_json::Deserializer::from_slice(&line[at..])
        .into_iter::<String>()
        .next()?
        .ok()
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Thread names by id, from the end of `session_index.jsonl`.
fn names(codex: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let path = codex.join("session_index.jsonl");
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return out;
    };
    if !meta.file_type().is_file() {
        return out;
    }
    let start = meta.len().saturating_sub(INDEX_CAP);
    let mut buf = Vec::new();
    let read = std::fs::File::open(&path).and_then(|mut f| {
        f.seek(std::io::SeekFrom::Start(start))?;
        f.take(INDEX_CAP).read_to_end(&mut buf)
    });
    if read.is_err() {
        return out;
    }
    let text = String::from_utf8_lossy(&buf);
    // A read that starts mid-file starts mid-line: skip that line.
    for line in text.lines().skip(usize::from(start > 0)) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let (Some(id), Some(name)) = (v["id"].as_str(), v["thread_name"].as_str()) else {
            continue;
        };
        let name = crate::util::sanitize(name).trim().to_string();
        if !name.is_empty() {
            out.insert(id.to_string(), name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "01a18c3d-0cc9-7b12-8ea7-a9324172b8b5";

    #[test]
    fn the_meta_line_of_codex_0_167() {
        let line = json!({
            "timestamp": "2026-10-30T09:56:13.000Z", "ordinal": 0, "type": "session_meta",
            "payload": {
                "session_id": ID, "id": ID, "cwd": "/r", "originator": "codex-tui",
                "source": "cli", "base_instructions": {"text": "…"},
            },
        })
        .to_string();
        let m = meta(line.as_bytes()).unwrap();
        assert_eq!(m.id, ID);
        assert_eq!(m.cwd, PathBuf::from("/r"));
        assert!(m.interactive);
    }

    #[test]
    fn exec_runs_and_subagents_are_not_offered() {
        for source in [json!("exec"), json!("mcp"), json!({"subagent": "review"})] {
            let line = json!({"type": "session_meta",
                "payload": {"id": ID, "cwd": "/r", "source": source}})
            .to_string();
            assert!(!meta(line.as_bytes()).unwrap().interactive, "{source}");
        }
    }

    #[test]
    fn older_and_other_first_lines() {
        let old = json!({"id": ID, "timestamp": "t", "cwd": "/r"}).to_string();
        assert_eq!(meta(old.as_bytes()).unwrap().id, ID);
        let other = json!({"type": "response_item", "payload": {"id": ID, "cwd": "/r"}});
        assert!(meta(other.to_string().as_bytes()).is_none());
        let bad_id = json!({"type": "session_meta", "payload": {"id": "--help", "cwd": "/r"}});
        assert!(meta(bad_id.to_string().as_bytes()).is_none());
    }

    #[test]
    fn a_line_cut_short_still_gives_its_id_and_cwd() {
        // In Codex's order (`json!` would sort the keys).
        let full = format!(
            r#"{{"timestamp":"t","type":"session_meta","payload":{{"session_id":"{ID}","id":"{ID}","cwd":"/r \"q\"","source":"cli","base_instructions":{{"text":"{}"}}}}}}"#,
            "x".repeat(100)
        );
        assert!(serde_json::from_str::<Value>(&full).is_ok());
        let cut = &full.as_bytes()[..full.len() - 40];
        let m = meta(cut).unwrap();
        assert_eq!((m.id.as_str(), m.cwd.to_str().unwrap()), (ID, "/r \"q\""));
        assert!(m.interactive);
    }
}
