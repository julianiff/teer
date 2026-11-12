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

use std::ffi::{OsStr, OsString};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// Whether pi sessions can be reached: pi isn't installed, or is without
/// teer's extension, or has it (sessions started since register).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PiState {
    NotFound,
    NoExtension,
    Ready,
}

/// Where each agent keeps its sessions: `$TEER_CLAUDE_DIR` (else
/// `$CLAUDE_CONFIG_DIR`, else `~/.claude`), `$TEER_CODEX_DIR` (else
/// `$CODEX_HOME`, else `~/.codex`), `$TEER_PI_DIR` (else
/// `$PI_CODING_AGENT_DIR`, else `~/.pi/agent`): each agent's own variable
/// moves its folder for teer as it does for the agent. Tests point them
/// elsewhere.
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
            claude: var("TEER_CLAUDE_DIR")
                .or_else(|| var("CLAUDE_CONFIG_DIR"))
                .unwrap_or_else(|| home.join(".claude")),
            codex: var("TEER_CODEX_DIR")
                .or_else(|| var("CODEX_HOME"))
                .unwrap_or_else(|| home.join(".codex")),
            pi: var("TEER_PI_DIR")
                .or_else(|| {
                    // pi expands a leading `~` in its own variable.
                    var("PI_CODING_AGENT_DIR").map(|p| match p.strip_prefix("~") {
                        Ok(rest) => home.join(rest),
                        Err(_) => p,
                    })
                })
                .unwrap_or_else(|| home.join(".pi").join("agent")),
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

// ------------------------------------------------------- shared by the agents

/// The longest message teer sends, in bytes: a tile's result is far
/// shorter; a longer one goes to the clipboard instead.
pub const MAX_TEXT: usize = 32 * 1024;

/// How long reaching a session's socket may take.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// Whether `text` may go as a message: not blank, at most [`MAX_TEXT`].
pub(crate) fn check_text(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("the message is empty".into());
    }
    if text.len() > MAX_TEXT {
        return Err(format!(
            "the message is too long ({} KB; at most {} KB)",
            text.len().div_ceil(1024),
            MAX_TEXT / 1024
        ));
    }
    Ok(())
}

/// Whether `cwd` is `repo` (canonical) or inside it, once resolved. A
/// relative or missing `cwd` is not.
pub(crate) fn inside(cwd: &Path, repo: &Path) -> bool {
    cwd.is_absolute() && cwd.canonicalize().is_ok_and(|c| c.starts_with(repo))
}

/// The bytes of the regular file at `path`, when it holds at most `cap`
/// of them. A symlink, a FIFO or a device is never opened or followed.
pub(crate) fn read_small(path: &Path, cap: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() || meta.len() > cap {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    // The opened file must be the one looked at, not a symlink put there since.
    let opened = file.metadata().ok()?;
    if !opened.is_file() || (opened.dev(), opened.ino()) != (meta.dev(), meta.ino()) {
        return None;
    }
    let mut buf = Vec::new();
    file.take(cap + 1).read_to_end(&mut buf).ok()?;
    (buf.len() as u64 <= cap).then_some(buf)
}

/// The JSON object in the regular file at `path` (at most `cap` bytes).
pub(crate) fn read_json(path: &Path, cap: u64) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_slice(&read_small(path, cap)?).ok()?;
    v.is_object().then_some(v)
}

/// A session's socket as its registry names it: an absolute path to a
/// `.sock` file, else `None`.
pub(crate) fn socket_path(s: &str) -> Option<PathBuf> {
    let p = PathBuf::from(s);
    (p.is_absolute() && s.ends_with(".sock") && !s.contains('\0')).then_some(p)
}

/// Connect to the Unix socket at `path`, giving up after `timeout` (a
/// listener whose queue is full keeps a connect waiting).
pub(crate) fn connect(path: &Path, timeout: Duration) -> std::io::Result<UnixStream> {
    let (tx, rx) = std::sync::mpsc::channel();
    let p = path.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(UnixStream::connect(p));
    });
    rx.recv_timeout(timeout).unwrap_or_else(|_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "no answer within a second",
        ))
    })
}

/// A session's id cut to 8 characters: its name when it has none.
pub(crate) fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// The program `$var` names (tests point it at a script), else `default`.
pub(crate) fn program(var: &str, default: &str) -> OsString {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.into())
}

/// Where `program` is: itself when it names a path, else the first
/// executable of that name in an absolute folder on PATH.
pub(crate) fn find_program(program: &OsStr) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let runs = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    let p = Path::new(program);
    if p.components().count() > 1 || p.is_absolute() {
        return runs(p).then(|| p.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(program))
        .find(|c| runs(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_neither_blank_nor_too_long() {
        assert!(check_text("apply the advice").is_ok());
        assert_eq!(check_text(" \n").unwrap_err(), "the message is empty");
        let long = "x".repeat(MAX_TEXT + 1);
        assert_eq!(
            check_text(&long).unwrap_err(),
            "the message is too long (33 KB; at most 32 KB)"
        );
        assert!(check_text(&"x".repeat(MAX_TEXT)).is_ok());
    }

    #[test]
    fn a_socket_is_an_absolute_sock_path() {
        assert_eq!(
            socket_path("/tmp/cc-socks/1.sock"),
            Some(PathBuf::from("/tmp/cc-socks/1.sock"))
        );
        assert_eq!(socket_path("cc-socks/1.sock"), None);
        assert_eq!(socket_path("/tmp/cc-socks/1.json"), None);
        assert_eq!(socket_path(""), None);
    }

    #[test]
    fn a_cwd_counts_when_it_is_the_repo_or_inside_it() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        std::fs::create_dir_all(d.path().join("other")).unwrap();
        let canon = repo.canonicalize().unwrap();
        assert!(inside(&repo, &canon));
        assert!(inside(&repo.join("sub"), &canon));
        assert!(inside(&repo.join("sub/.."), &canon));
        assert!(!inside(&d.path().join("other"), &canon));
        assert!(!inside(&repo.join("gone"), &canon));
        assert!(!inside(Path::new("repo"), &canon));
    }

    #[test]
    fn small_files_only_and_never_through_a_symlink() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f.json");
        std::fs::write(&f, r#"{"a":1}"#).unwrap();
        assert_eq!(read_small(&f, 7).unwrap(), br#"{"a":1}"#);
        assert!(read_small(&f, 6).is_none());
        assert_eq!(read_json(&f, 64).unwrap()["a"], 1);
        let link = d.path().join("link.json");
        std::os::unix::fs::symlink(&f, &link).unwrap();
        assert!(read_small(&link, 64).is_none());
        assert!(read_small(d.path(), 64).is_none());
        std::fs::write(&f, "[1]").unwrap();
        assert!(read_json(&f, 64).is_none(), "an object, not an array");
    }

    #[test]
    fn a_program_is_found_as_a_path_or_on_path() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("tool");
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        assert_eq!(find_program(p.as_os_str()), None, "not executable");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(find_program(p.as_os_str()), Some(p.clone()));
        assert_eq!(find_program(OsStr::new("/no/such/tool")), None);
        assert!(find_program(OsStr::new("sh")).is_some());
        assert_eq!(find_program(OsStr::new("teer-no-such-program")), None);
    }

    #[test]
    fn a_dead_socket_is_refused_and_a_live_one_connects() {
        let d = tempfile::Builder::new()
            .prefix("ts")
            .tempdir_in("/tmp")
            .unwrap();
        let sock = d.path().join("s.sock");
        let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        assert!(connect(&sock, CONNECT_TIMEOUT).is_ok());
        drop(l);
        // A child another test forks meanwhile holds the listener until it
        // execs: give it that moment.
        let refused = (0..200).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            connect(&sock, CONNECT_TIMEOUT).is_err()
        });
        assert!(refused);
        assert!(connect(&d.path().join("none.sock"), CONNECT_TIMEOUT).is_err());
    }
}
