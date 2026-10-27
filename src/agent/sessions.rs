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
}
