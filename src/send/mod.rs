//! Sending a tile's result to an agent session that is running now:
//! Claude Code, Codex or pi, in this repository. The text goes in as a
//! message the session reads like one typed to it; the agent there
//! applies it. Only textual results go this way (the advice, findings,
//! test gaps, an answer); proposed hunks are put in with `p`/`P`.
//!
//! How each agent is reached:
//! - Claude Code: its session registry (`~/.claude/sessions/<pid>.json`)
//!   names a messaging socket; a line with the session's key, then the
//!   message as a JSON line (see [`claude`]). Not a documented interface:
//!   when it fails, the text goes to the clipboard.
//! - Codex: `codex queue --thread <id> --message <text>` (see [`codex`]).
//! - pi: teer's own pi extension, `teer-inbox.ts`, opens a socket per
//!   session and registers it (see [`pi`]).

pub mod claude;
pub mod codex;
pub mod pi;

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Which agent a session belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Agent {
    ClaudeCode,
    Codex,
    Pi,
}

impl Agent {
    pub fn title(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Pi => "pi",
        }
    }
}

/// A session a result can be sent to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Target {
    pub agent: Agent,
    /// The session's id: Claude Code's session id, Codex's thread id,
    /// pi's session id.
    pub id: String,
    /// Its name as the agent shows it (`macos-41`), else the id cut short.
    pub name: String,
    /// Where it runs.
    pub cwd: PathBuf,
    /// `idle`, `busy`, or empty when the agent doesn't say.
    pub status: String,
    /// When it was last active, in milliseconds since the epoch; 0 when
    /// unknown.
    pub active_at: u64,
    /// How it is reached: its socket (Claude Code, pi); none for Codex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket: Option<PathBuf>,
}

/// Where each agent keeps its sessions: `$TEER_CLAUDE_DIR` (else
/// `~/.claude`), `$TEER_CODEX_DIR` (else `$CODEX_HOME`, else `~/.codex`),
/// `$TEER_PI_DIR` (else `~/.pi/agent`). Tests point them elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub pi: PathBuf,
}

impl Dirs {
    pub fn from_env() -> Dirs {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Dirs {
            claude: var("TEER_CLAUDE_DIR").unwrap_or_else(|| home.join(".claude")),
            codex: var("TEER_CODEX_DIR")
                .or_else(|| var("CODEX_HOME"))
                .unwrap_or_else(|| home.join(".codex")),
            pi: var("TEER_PI_DIR").unwrap_or_else(|| home.join(".pi").join("agent")),
        }
    }
}

/// The sessions running now in `repo` (their working directory is it or
/// inside it), of every agent, the most recently active first.
pub fn list(dirs: &Dirs, repo: &Path) -> Vec<Target> {
    let mut all = claude::list(dirs, repo);
    all.extend(codex::list(dirs, repo));
    all.extend(pi::list(dirs, repo));
    all.sort_by_key(|t| std::cmp::Reverse(t.active_at));
    all
}

/// Send `text` to session `t`, or say why it didn't go.
pub fn deliver(dirs: &Dirs, t: &Target, text: &str) -> Result<(), String> {
    match t.agent {
        Agent::ClaudeCode => claude::deliver(dirs, t, text),
        Agent::Codex => codex::deliver(dirs, t, text),
        Agent::Pi => pi::deliver(dirs, t, text),
    }
}
