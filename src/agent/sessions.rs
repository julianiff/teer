//! Claude Code session discovery.
//!
//! Observed on Claude Code 2.1.285: transcripts live in
//! `~/.claude/projects/<encoded cwd>/<session id>.jsonl`, where the encoded
//! cwd replaces every non-alphanumeric character with `-`
//! (`/Users/a.b/my_repo` -> `-Users-a-b-my-repo`). Each line is a JSON
//! record; user prompts have `"type":"user"` with `message.content` as a
//! string or a list of `{type:"text", text}` blocks, and carry `cwd`.
//! `CLAUDE_CONFIG_DIR` relocates `~/.claude`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use serde_json::Value;

use super::SessionInfo;
use super::commands;

/// A session counts as open if its transcript changed this recently.
pub const RECENT: Duration = Duration::from_secs(10 * 60);
const HEAD_BYTES: u64 = 512 * 1024;

fn encode_project_dir(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn projects_root() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("projects"));
    }
    directories::BaseDirs::new().map(|b| b.home_dir().join(".claude").join("projects"))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionMeta {
    pub cwd: Option<PathBuf>,
    pub title: Option<String>,
    pub ai_title: Option<String>,
}

fn user_text(record: &Value) -> Option<String> {
    let content = record.get("message")?.get("content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let text = text.trim();
    // Slash commands, command output and system caveats aren't prompts.
    if text.is_empty() || text.starts_with('<') || text.starts_with("Caveat:") {
        return None;
    }
    Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Read the title and cwd from the start of a transcript.
fn parse_session(reader: impl Read) -> SessionMeta {
    let mut meta = SessionMeta::default();
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if meta.cwd.is_none()
            && let Some(cwd) = v.get("cwd").and_then(|c| c.as_str())
        {
            meta.cwd = Some(PathBuf::from(cwd));
        }
        match ty {
            "ai-title" => {
                meta.ai_title = v.get("aiTitle").and_then(|t| t.as_str()).map(String::from)
            }
            "user" if meta.title.is_none() => {
                let meta_msg = v.get("isMeta").and_then(|m| m.as_bool()).unwrap_or(false);
                let sidechain = v
                    .get("isSidechain")
                    .and_then(|m| m.as_bool())
                    .unwrap_or(false);
                if !meta_msg && !sidechain {
                    meta.title = user_text(&v);
                }
            }
            _ => {}
        }
    }
    meta
}

#[derive(Debug, Clone, Default)]
pub struct RunningAgents {
    /// Session ids named on a running agent's command line (`--resume <id>`).
    pub session_ids: Vec<String>,
    /// Working directories of running agent processes.
    pub cwds: Vec<PathBuf>,
}

/// Parse `ps -axo pid=,ppid=,args=` output into (pid, ppid, args).
fn parse_ps(out: &str) -> Vec<(u32, u32, String)> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            Some((pid, ppid, it.collect::<Vec<_>>().join(" ")))
        })
        .collect()
}

fn is_claude_process(args: &str) -> bool {
    let argv0 = args.split_whitespace().next().unwrap_or("");
    Path::new(argv0)
        .file_name()
        .is_some_and(|n| n == commands::CLAUDE_BIN)
}

fn session_id_in_args(args: &str) -> Option<String> {
    let parts: Vec<&str> = args.split_whitespace().collect();
    parts
        .windows(2)
        .find(|w| matches!(w[0], "--resume" | "-r" | "--session-id"))
        .map(|w| w[1].to_string())
}

fn process_cwds(pids: &[u32]) -> HashMap<u32, PathBuf> {
    let mut out = HashMap::new();
    if pids.is_empty() {
        return out;
    }
    if Path::new("/proc").exists() {
        for &pid in pids {
            if let Ok(p) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
                out.insert(pid, p);
            }
        }
        return out;
    }
    let list = pids
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let (bin, args) = commands::lsof_cwd(&list);
    if let Ok(o) = Command::new(bin).args(args).output() {
        let mut pid = 0;
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            if let Some(p) = line.strip_prefix('p') {
                pid = p.parse().unwrap_or(0);
            } else if let Some(n) = line.strip_prefix('n') {
                out.insert(pid, PathBuf::from(n));
            }
        }
    }
    out
}

pub fn running_agents() -> RunningAgents {
    let (bin, args) = commands::ps_all();
    let Ok(o) = Command::new(bin).args(args).output() else {
        return RunningAgents::default();
    };
    let procs: Vec<_> = parse_ps(&String::from_utf8_lossy(&o.stdout))
        .into_iter()
        .filter(|(_, _, a)| is_claude_process(a) && !a.contains(" -p") && !a.contains(" --print"))
        .collect();
    let pids: Vec<u32> = procs.iter().map(|p| p.0).collect();
    let cwds = process_cwds(&pids);
    RunningAgents {
        session_ids: procs
            .iter()
            .filter_map(|p| session_id_in_args(&p.2))
            .collect(),
        cwds: cwds.into_values().collect(),
    }
}

/// List sessions for `repo`, newest first, open ones first.
pub fn list_sessions(
    root: &Path,
    repo: &Path,
    running: &RunningAgents,
    now: SystemTime,
) -> Result<Vec<SessionInfo>> {
    let prefix = encode_project_dir(repo);
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(root) else {
        return Ok(out);
    };
    for dir in dirs.flatten() {
        let name = dir.file_name().to_string_lossy().to_string();
        if name != prefix && !name.starts_with(&format!("{prefix}-")) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            let meta = parse_session(file.take(HEAD_BYTES));
            let Some(cwd) = meta.cwd.clone() else {
                continue;
            };
            // Folder names are ambiguous ("a-b" vs "a/b"), so check the real cwd.
            if !cwd.starts_with(repo) {
                continue;
            }
            let Some(title) = meta.title.clone().or(meta.ai_title.clone()) else {
                continue;
            };
            let modified = f
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let id = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let recent = now
                .duration_since(modified)
                .map(|d| d < RECENT)
                .unwrap_or(true);
            out.push(SessionInfo {
                agent: "claude".into(),
                open: recent || running.session_ids.contains(&id),
                id,
                cwd,
                title,
                last_activity: chrono::DateTime::<chrono::Local>::from(modified),
            });
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.last_activity));
    // A running agent in a directory most likely holds that directory's newest session.
    for cwd in &running.cwds {
        if let Some(s) = out.iter_mut().find(|s| &s.cwd == cwd) {
            s.open = true;
        }
    }
    out.sort_by(|a, b| {
        b.open
            .cmp(&a.open)
            .then(b.last_activity.cmp(&a.last_activity))
    });
    Ok(out)
}

/// How long ago a session may have edited the changes and still count as
/// the one that wrote them.
pub const WROTE_WITHIN: Duration = Duration::from_secs(24 * 60 * 60);
/// How much of each transcript's end is read for its edits: the newest
/// edits are at the end, and transcripts grow to hundreds of megabytes.
const TAIL_BYTES: u64 = 4 * 1024 * 1024;
/// The most transcripts read, newest first.
const MAX_TRANSCRIPTS: usize = 20;
/// The Claude Code tools that write a file, and the input naming it.
const WRITE_TOOLS: [(&str, &str); 4] = [
    ("Edit", "file_path"),
    ("MultiEdit", "file_path"),
    ("Write", "file_path"),
    ("NotebookEdit", "notebook_path"),
];

/// The Claude Code session that most likely wrote the changes: the newest
/// one started in `repo` (or below it) whose Edit, MultiEdit, Write or
/// NotebookEdit calls touched one of `changed` (paths relative to `repo`)
/// within [`WROTE_WITHIN`] before `now`. Only the last [`TAIL_BYTES`] of
/// each of the [`MAX_TRANSCRIPTS`] newest transcripts are read.
pub fn wrote_changes(
    root: &Path,
    repo: &Path,
    changed: &[PathBuf],
    now: SystemTime,
) -> Option<SessionInfo> {
    if changed.is_empty() {
        return None;
    }
    let since = now
        .checked_sub(WROTE_WITHIN)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let prefix = encode_project_dir(repo);
    let mut transcripts: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|d| {
            let name = d.file_name().to_string_lossy().to_string();
            name == prefix || name.starts_with(&format!("{prefix}-"))
        })
        .filter_map(|d| std::fs::read_dir(d.path()).ok())
        .flat_map(|files| files.flatten())
        .filter(|f| f.path().extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .filter_map(|f| {
            let modified = f.metadata().and_then(|m| m.modified()).ok()?;
            (modified >= since).then(|| (modified, f.path()))
        })
        .collect();
    transcripts.sort_by_key(|(m, _)| std::cmp::Reverse(*m));
    let since = chrono::DateTime::<chrono::Utc>::from(since);
    transcripts
        .into_iter()
        .take(MAX_TRANSCRIPTS)
        .find_map(|(modified, path)| {
            let mut file = std::fs::File::open(&path).ok()?;
            let tail = read_tail(&mut file, TAIL_BYTES).ok()?;
            if !touched(&tail, repo, changed, since) {
                return None;
            }
            // Where it was started, which a resume must run from, is at its start.
            let file = std::fs::File::open(&path).ok()?;
            let meta = parse_session(file.take(HEAD_BYTES));
            let cwd = meta.cwd.filter(|c| c.starts_with(repo))?;
            Some(SessionInfo {
                agent: "claude".into(),
                id: path.file_stem()?.to_string_lossy().to_string(),
                cwd,
                title: meta.title.or(meta.ai_title).unwrap_or_default(),
                open: now
                    .duration_since(modified)
                    .map(|d| d < RECENT)
                    .unwrap_or(true),
                last_activity: chrono::DateTime::<chrono::Local>::from(modified),
            })
        })
}

/// The last `max` bytes of `file`, from the first whole line in them.
fn read_tail(file: &mut std::fs::File, max: u64) -> std::io::Result<String> {
    use std::io::{Seek, SeekFrom};
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.take(max).read_to_end(&mut buf)?;
    if start > 0 {
        // The first line is cut: drop it.
        let cut = buf
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buf.len(), |i| i + 1);
        buf.drain(..cut);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// A transcript's lines `jsonl` hold a call of one of [`WRITE_TOOLS`] made
/// in `repo` after `since` on one of `changed` (relative to `repo`).
fn touched(
    jsonl: &str,
    repo: &Path,
    changed: &[PathBuf],
    since: chrono::DateTime<chrono::Utc>,
) -> bool {
    jsonl
        .lines()
        // Cheap before parsing: most lines are no edit.
        .filter(|l| l.contains("\"tool_use\""))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("assistant"))
        .filter(|v| {
            // A line without a time is judged by its file's.
            v.get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .is_none_or(|t| t >= since)
        })
        .any(|v| {
            let cwd = v.get("cwd").and_then(|c| c.as_str()).map(PathBuf::from);
            let Some(blocks) = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            else {
                return false;
            };
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                .filter_map(|b| {
                    let name = b.get("name")?.as_str()?;
                    let (_, key) = WRITE_TOOLS.iter().find(|(tool, _)| *tool == name)?;
                    b.get("input")?.get(*key)?.as_str()
                })
                .any(|p| {
                    let p = Path::new(p);
                    let full = match &cwd {
                        Some(cwd) if p.is_relative() => cwd.join(p),
                        _ => p.to_path_buf(),
                    };
                    full.strip_prefix(repo)
                        .is_ok_and(|rel| changed.iter().any(|c| c == rel))
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_like_claude_code() {
        assert_eq!(
            encode_project_dir(Path::new("/Users/julian.iff/self_me/teer")),
            "-Users-julian-iff-self-me-teer"
        );
    }

    #[test]
    fn parses_saved_sample() {
        let f = std::fs::File::open(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/samples/claude_session.jsonl"
        ))
        .unwrap();
        let meta = parse_session(f);
        assert_eq!(meta.cwd, Some(PathBuf::from("/work/shop")));
        assert_eq!(
            meta.title.as_deref(),
            Some("Add retries to the checkout payment flow and reject unknown currencies")
        );
        assert_eq!(meta.ai_title.as_deref(), Some("Checkout payment retries"));
    }

    #[test]
    fn detects_claude_processes() {
        let ps = "  123     1 claude --resume 11111111-2222\n  124   123 /opt/homebrew/bin/claude\n  125     1 vim claude.md\n";
        let procs = parse_ps(ps);
        assert_eq!(procs.len(), 3);
        assert!(is_claude_process(&procs[0].2));
        assert!(is_claude_process(&procs[1].2));
        assert!(!is_claude_process(&procs[2].2));
        assert_eq!(
            session_id_in_args(&procs[0].2).as_deref(),
            Some("11111111-2222")
        );
    }

    #[test]
    fn lists_sessions_from_projects_dir() {
        let root = tempfile::tempdir().unwrap();
        let repo = Path::new("/work/shop");
        let dir = root.path().join(encode_project_dir(repo));
        std::fs::create_dir_all(&dir).unwrap();
        let sample = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/samples/claude_session.jsonl"
        ))
        .unwrap();
        std::fs::write(
            dir.join("11111111-2222-3333-4444-555555555555.jsonl"),
            sample,
        )
        .unwrap();
        // A session from another repo whose folder name shares the prefix.
        let other = root
            .path()
            .join(format!("{}-api", encode_project_dir(repo)));
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("x.jsonl"),
            r#"{"type":"user","cwd":"/work/shop-api","message":{"content":"hi"}}"#,
        )
        .unwrap();

        let far_future = SystemTime::now() + Duration::from_secs(3600 * 24);
        let sessions =
            list_sessions(root.path(), repo, &RunningAgents::default(), far_future).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(!sessions[0].open);
        let running = RunningAgents {
            session_ids: vec![],
            cwds: vec![PathBuf::from("/work/shop")],
        };
        let sessions = list_sessions(root.path(), repo, &running, far_future).unwrap();
        assert!(sessions[0].open);
    }

    /// A transcript line: the agent calling `tool` on `path`, from `cwd`, at `at`.
    fn edit(tool: &str, path: &str, cwd: &str, at: chrono::DateTime<chrono::Utc>) -> String {
        let key = if tool == "NotebookEdit" {
            "notebook_path"
        } else {
            "file_path"
        };
        serde_json::json!({
            "type": "assistant", "cwd": cwd, "timestamp": at.to_rfc3339(),
            "message": {"role": "assistant", "content": [
                {"type": "text", "text": "editing"},
                {"type": "tool_use", "id": "t", "name": tool, "input": {key: path}}
            ]}
        })
        .to_string()
    }

    /// Write session `id` of `repo` under `root`: its first prompt, then `lines`,
    /// last changed at `modified`.
    fn transcript(root: &Path, repo: &Path, id: &str, lines: &[String], modified: SystemTime) {
        let dir = root.join(encode_project_dir(repo));
        std::fs::create_dir_all(&dir).unwrap();
        let start = serde_json::json!({
            "type": "user", "cwd": repo, "message": {"content": format!("work on {id}")}
        });
        let body = std::iter::once(start.to_string())
            .chain(lines.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n");
        let path = dir.join(format!("{id}.jsonl"));
        std::fs::write(&path, body + "\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    #[test]
    fn the_newest_session_that_edited_a_changed_file_wrote_the_changes() {
        let root = tempfile::tempdir().unwrap();
        let repo = Path::new("/work/shop");
        let now = SystemTime::now();
        let ago = |mins: u64| now - Duration::from_secs(mins * 60);
        let utc = |t: SystemTime| chrono::DateTime::<chrono::Utc>::from(t);
        let changed = [PathBuf::from("src/pay.rs"), PathBuf::from("nb.ipynb")];
        let at = utc(ago(30));
        transcript(
            root.path(),
            repo,
            "older",
            &[edit("Edit", "/work/shop/src/pay.rs", "/work/shop", at)],
            ago(30),
        );
        // Newer, but it only read files and edited one that didn't change.
        transcript(
            root.path(),
            repo,
            "reader",
            &[
                edit("Read", "/work/shop/src/pay.rs", "/work/shop", utc(ago(5))),
                edit("Write", "/work/shop/README.md", "/work/shop", utc(ago(5))),
            ],
            ago(5),
        );
        let found = wrote_changes(root.path(), repo, &changed, now).unwrap();
        assert_eq!(found.id, "older");
        assert_eq!(found.cwd, PathBuf::from("/work/shop"));
        assert_eq!(found.title, "work on older");

        // A newer one wins; a relative path counts from the line's cwd.
        transcript(
            root.path(),
            repo,
            "newer",
            &[edit("MultiEdit", "pay.rs", "/work/shop/src", utc(ago(10)))],
            ago(10),
        );
        assert_eq!(
            wrote_changes(root.path(), repo, &changed, now).unwrap().id,
            "newer"
        );
        transcript(
            root.path(),
            repo,
            "notebook",
            &[edit(
                "NotebookEdit",
                "/work/shop/nb.ipynb",
                "/work/shop",
                utc(ago(1)),
            )],
            ago(1),
        );
        assert_eq!(
            wrote_changes(root.path(), repo, &changed, now).unwrap().id,
            "notebook"
        );

        // Nothing it touched changed: no session.
        assert!(wrote_changes(root.path(), repo, &[PathBuf::from("other.rs")], now).is_none());
        assert!(wrote_changes(root.path(), repo, &[], now).is_none());
    }

    #[test]
    fn sessions_older_than_a_day_or_of_another_repo_dont_count() {
        let root = tempfile::tempdir().unwrap();
        let repo = Path::new("/work/shop");
        let changed = [PathBuf::from("a.py")];
        let now = SystemTime::now();
        let day = Duration::from_secs(24 * 3600);
        let utc = chrono::DateTime::<chrono::Utc>::from;
        // Changed within the day, but the edit itself is two days old.
        transcript(
            root.path(),
            repo,
            "stale-edit",
            &[edit(
                "Edit",
                "/work/shop/a.py",
                "/work/shop",
                utc(now - 2 * day),
            )],
            now - Duration::from_secs(60),
        );
        // Not changed within the day at all.
        transcript(
            root.path(),
            repo,
            "stale-file",
            &[edit(
                "Edit",
                "/work/shop/a.py",
                "/work/shop",
                utc(now - 2 * day),
            )],
            now - 2 * day,
        );
        // Another repository whose folder name shares the prefix.
        let other = Path::new("/work/shop-api");
        transcript(
            root.path(),
            other,
            "other",
            &[edit(
                "Edit",
                "/work/shop-api/a.py",
                "/work/shop-api",
                utc(now),
            )],
            now,
        );
        assert!(wrote_changes(root.path(), repo, &changed, now).is_none());
    }

    #[test]
    fn only_the_end_of_a_transcript_is_read() {
        let root = tempfile::tempdir().unwrap();
        let repo = Path::new("/work/shop");
        let now = SystemTime::now();
        let at = chrono::DateTime::<chrono::Utc>::from(now);
        let filler = serde_json::json!({"type": "user", "message": {"content": "x".repeat(1000)}})
            .to_string();
        let mut lines = vec![edit("Edit", "/work/shop/a.py", "/work/shop", at)];
        lines.extend(std::iter::repeat_n(
            filler,
            (TAIL_BYTES / 1000 + 10) as usize,
        ));
        transcript(root.path(), repo, "long", &lines, now);
        // The edit sits before the last TAIL_BYTES: it isn't seen.
        assert!(wrote_changes(root.path(), repo, &[PathBuf::from("a.py")], now).is_none());
        lines.push(edit("Write", "/work/shop/b.py", "/work/shop", at));
        transcript(root.path(), repo, "long", &lines, now);
        assert_eq!(
            wrote_changes(root.path(), repo, &[PathBuf::from("b.py")], now)
                .unwrap()
                .id,
            "long"
        );
    }
}
