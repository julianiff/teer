//! Sessions of Claude Code: finding the running ones, and sending them a
//! message through their messaging socket.
//!
//! Read out of Claude Code 2.1.322; not a documented interface, so every
//! step checks what it finds and says why when it stops.
//! - Registry: `<claude dir>/sessions/<pid>.json` with `pid`, `sessionId`,
//!   `cwd`, `messagingSocketPath`, `peerProtocol`, `name`, `status`,
//!   `kind` and times in milliseconds (`startedAt`, `updatedAt`,
//!   `statusUpdatedAt`).
//! - Key: `<claude dir>/sessions/<pid>.<sha256 hex of the socket path>.key`,
//!   JSON with `peerToken`. The path hashed is the absolute one, `.` and
//!   `..` resolved, symlinks not (Node's `path.resolve`).
//! - Protocol 1: newline-delimited JSON over the socket. An optional
//!   `{"type":"auth","token":…}` line, then
//!   `{"type":"user","message":{"role":"user","content":…},"session_id":…,"from":"teer","uuid":…}`.
//!   The session drops the message when `session_id` isn't its own (the pid
//!   now runs another session). Nothing comes back: a refused key closes
//!   the connection, as a delivered message does.
//!
//! The key is never logged or shown.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use super::{Agent, Dirs, Target};

/// The messaging protocol teer speaks.
const PROTOCOL: u64 = 1;
/// Largest registry file read.
const REGISTRY_CAP: u64 = 64 * 1024;
/// Largest key file read (Claude Code writes far less).
const KEY_CAP: u64 = 4096;
/// How long to wait for the session to close its end once the message is
/// written: it sends nothing back.
const CLOSE_WAIT: Duration = Duration::from_secs(1);

/// A session as its registry file describes it.
#[derive(Debug, Clone, PartialEq)]
struct Registered {
    pid: u32,
    session_id: String,
    cwd: PathBuf,
    socket: Option<PathBuf>,
    protocol: Option<u64>,
    name: Option<String>,
    status: String,
    active_at: u64,
}

impl Registered {
    fn target(&self) -> Target {
        Target {
            agent: Agent::ClaudeCode,
            id: self.session_id.clone(),
            name: self
                .name
                .clone()
                .unwrap_or_else(|| super::short_id(&self.session_id)),
            cwd: self.cwd.clone(),
            status: self.status.clone(),
            active_at: self.active_at,
            socket: self.socket.clone(),
        }
    }
}

/// The registry entry in `v`, read from `<pid>.json`: `None` when a field
/// it needs is missing, the pid isn't the file's, or it is Claude Code's
/// background daemon (no conversation).
fn parse(v: &Value, file_pid: u32) -> Option<Registered> {
    let pid = u32::try_from(v["pid"].as_u64()?).ok()?;
    let session_id = v["sessionId"].as_str()?.trim();
    let cwd = PathBuf::from(v["cwd"].as_str()?);
    if pid != file_pid || session_id.is_empty() || v["kind"].as_str() == Some("daemon") {
        return None;
    }
    let text = |k: &str| {
        v[k].as_str()
            .map(|s| crate::util::sanitize(s).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let active_at = ["statusUpdatedAt", "updatedAt", "startedAt"]
        .iter()
        .filter_map(|k| v[*k].as_u64())
        .max()
        .unwrap_or(0);
    Some(Registered {
        pid,
        session_id: session_id.to_string(),
        cwd,
        socket: v["messagingSocketPath"]
            .as_str()
            .and_then(super::socket_path),
        protocol: v["peerProtocol"].as_u64(),
        name: text("name"),
        status: text("status").unwrap_or_default(),
        active_at,
    })
}

/// Every session in the registry, whether it still runs or not.
fn registered(dirs: &Dirs) -> Vec<Registered> {
    let Ok(entries) = std::fs::read_dir(dirs.claude.join("sessions")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let pid: u32 = name.to_str()?.strip_suffix(".json")?.parse().ok()?;
            let v = super::read_json(&e.path(), REGISTRY_CAP)?;
            parse(&v, pid)
        })
        .collect()
}

/// The Claude Code sessions running now in `repo`: their socket takes a
/// connection.
pub fn list(dirs: &Dirs, repo: &Path) -> Vec<Target> {
    let Ok(repo) = repo.canonicalize() else {
        return Vec::new();
    };
    registered(dirs)
        .into_iter()
        .filter(|r| super::inside(&r.cwd, &repo))
        .filter(|r| {
            r.socket
                .as_deref()
                .is_some_and(|s| super::connect(s, super::CONNECT_TIMEOUT).is_ok())
        })
        .map(|r| r.target())
        .collect()
}

/// Send `text` to session `t`: its registry entry is read again, so a
/// session that ended or changed its socket since the list is noticed.
pub fn deliver(dirs: &Dirs, t: &Target, text: &str) -> Result<(), String> {
    super::check_text(text)?;
    // A session resumed in another process can have two entries for a
    // while: the listed one first.
    let mut all: Vec<Registered> = registered(dirs)
        .into_iter()
        .filter(|r| r.session_id == t.id)
        .collect();
    all.sort_by_key(|r| r.socket != t.socket);
    let r = all
        .into_iter()
        .next()
        .ok_or("the session isn't registered any more; has it ended?")?;
    if r.protocol != Some(PROTOCOL) {
        let n = r
            .protocol
            .map_or("none given".to_string(), |n| n.to_string());
        return Err(format!(
            "this Claude Code's messaging (protocol {n}) is not known to teer"
        ));
    }
    let socket = r
        .socket
        .clone()
        .ok_or("the session names no messaging socket")?;
    let token = key(dirs, r.pid, &socket)?;
    if token.is_none() {
        crate::log::write(&format!(
            "send: no key for Claude Code session {} (pid {}); sending without one",
            r.session_id, r.pid
        ));
    }
    let lines = message_lines(token.as_deref(), &r.session_id, text);
    let mut s = super::connect(&socket, super::CONNECT_TIMEOUT)
        .map_err(|e| format!("couldn't reach the session ({e})"))?;
    let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
    let _ = s.set_read_timeout(Some(CLOSE_WAIT));
    let refused = || "the session closed the connection before taking the message".to_string();
    s.write_all(lines.as_bytes()).map_err(|e| match e.kind() {
        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset => refused(),
        _ => format!("couldn't write to the session ({e})"),
    })?;
    let _ = s.shutdown(std::net::Shutdown::Write);
    // It answers nothing; wait briefly for it to close its end.
    let mut buf = [0u8; 512];
    let mut got = 0usize;
    loop {
        match s.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                got += n;
                if got > 64 * 1024 {
                    return Ok(());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return Err(refused()),
            // Still open after the wait: the message was written.
            Err(_) => return Ok(()),
        }
    }
}

/// The lines teer writes: the key's (when there is one), then the message.
fn message_lines(token: Option<&str>, session_id: &str, text: &str) -> String {
    let mut out = String::new();
    if let Some(token) = token {
        out.push_str(&json!({"type": "auth", "token": token}).to_string());
        out.push('\n');
    }
    let msg = json!({
        "type": "user",
        "message": {"role": "user", "content": text},
        "session_id": session_id,
        "from": "teer",
        "uuid": uuid_v4(),
    });
    out.push_str(&msg.to_string());
    out.push('\n');
    out
}

/// Where Claude Code keeps the key of session `pid` listening on `socket`.
pub fn key_path(dirs: &Dirs, pid: u32, socket: &Path) -> PathBuf {
    let hash = hex(&sha256(resolve(socket).as_os_str().as_encoded_bytes()));
    dirs.claude
        .join("sessions")
        .join(format!("{pid}.{hash}.key"))
}

/// The key of session `pid`: from the file named by its socket, else the
/// only `<pid>.*.key` there is. `None` when there is none; an error when
/// the file found isn't a key.
fn key(dirs: &Dirs, pid: u32, socket: &Path) -> Result<Option<String>, String> {
    let exact = key_path(dirs, pid, socket);
    let path = if std::fs::symlink_metadata(&exact).is_ok() {
        exact
    } else {
        let prefix = format!("{pid}.");
        let found: Vec<PathBuf> = std::fs::read_dir(dirs.claude.join("sessions"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".key"))
            })
            .map(|e| e.path())
            .collect();
        match <[PathBuf; 1]>::try_from(found) {
            Ok([one]) => one,
            Err(found) => {
                if found.len() > 1 {
                    crate::log::write(&format!(
                        "send: {} keys for Claude Code pid {pid}, none named by its socket",
                        found.len()
                    ));
                }
                return Ok(None);
            }
        }
    };
    let token = super::read_json(&path, KEY_CAP).and_then(|v| {
        v["peerToken"]
            .as_str()
            .filter(|t| !t.is_empty() && t.len() <= 1024)
            .map(str::to_string)
    });
    token
        .map(Some)
        .ok_or_else(|| "the session's key file can't be read".to_string())
}

/// `p` made absolute the way Node's `path.resolve` does for an absolute
/// path: `.`, `..` and repeated or trailing slashes resolved, symlinks not.
fn resolve(p: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for c in p.components() {
        match c {
            Component::Normal(x) => out.push(x),
            Component::ParentDir => {
                out.pop();
            }
            _ => {}
        }
    }
    out
}

/// A random (version 4) UUID.
fn uuid_v4() -> String {
    let mut b = random16();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex(&b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// 16 random bytes: from the system, else from std's randomly seeded
/// hasher keys.
fn random16() -> [u8; 16] {
    let mut b = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok()
    {
        return b;
    }
    use std::hash::{BuildHasher, Hasher};
    for (i, half) in b.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        half.copy_from_slice(&h.finish().to_le_bytes());
    }
    b
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of `data` (FIPS 180-4).
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&(data.len() as u64).wrapping_mul(8).to_be_bytes());
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (wi, word) in w.iter_mut().zip(block.as_chunks::<4>().0) {
            *wi = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for (k, wi) in K.iter().zip(w) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(*k)
                .wrapping_add(wi);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (o, x) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *o = x.to_be_bytes();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_standard_vectors() {
        let hash = |s: &[u8]| hex(&sha256(s));
        assert_eq!(
            hash(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hash(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hash(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hash(&[b'a'; 1000]),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    #[test]
    fn the_key_is_named_by_the_socket_path_as_given() {
        // `/tmp` is a symlink on macOS; Claude Code hashes the path unresolved.
        let dirs = Dirs {
            claude: PathBuf::from("/c"),
            codex: PathBuf::new(),
            pi: PathBuf::new(),
        };
        let want = PathBuf::from(
            "/c/sessions/12307.c47f4518f2c4d17f640b4614f0bf0010a7b0608447ec40b3f96b1b931f230973.key",
        );
        assert_eq!(
            key_path(&dirs, 12307, Path::new("/tmp/cc-socks/12307.sock")),
            want
        );
        assert_eq!(
            key_path(&dirs, 12307, Path::new("/tmp//cc-socks/./x/../12307.sock")),
            want
        );
    }

    #[test]
    fn resolve_is_lexical() {
        assert_eq!(resolve(Path::new("/a/./b/../c/")), PathBuf::from("/a/c"));
        assert_eq!(resolve(Path::new("/../a")), PathBuf::from("/a"));
    }

    #[test]
    fn uuids_are_version_4_and_differ() {
        let a = uuid_v4();
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert!(parts[2].starts_with('4'));
        assert!(matches!(
            parts[3].chars().next(),
            Some('8' | '9' | 'a' | 'b')
        ));
        assert_ne!(a, uuid_v4());
    }

    #[test]
    fn the_lines_are_the_key_then_the_message() {
        let lines = message_lines(Some("k"), "s1", "fix \"it\"\nnow");
        let v: Vec<Value> = lines
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(v[0], json!({"type": "auth", "token": "k"}));
        assert_eq!(v[1]["message"]["content"], "fix \"it\"\nnow");
        assert_eq!(v[1]["session_id"], "s1");
        assert_eq!(v.len(), 2);
        assert!(lines.ends_with('\n'));
        let bare = message_lines(None, "s1", "x");
        assert_eq!(bare.lines().count(), 1);
    }

    #[test]
    fn a_registry_entry_needs_its_pid_id_and_cwd() {
        let v = json!({
            "pid": 7, "sessionId": "2cbe5981-aaaa", "cwd": "/r", "kind": "interactive",
            "messagingSocketPath": "/tmp/cc-socks/7.sock", "peerProtocol": 1,
            "status": "idle", "startedAt": 1, "updatedAt": 3, "statusUpdatedAt": 2,
        });
        let r = parse(&v, 7).unwrap();
        assert_eq!(r.name, None);
        assert_eq!(r.target().name, "2cbe5981");
        assert_eq!(r.active_at, 3);
        assert_eq!(r.socket, Some(PathBuf::from("/tmp/cc-socks/7.sock")));
        assert!(parse(&v, 8).is_none(), "another pid's file");
        let mut daemon = v.clone();
        daemon["kind"] = json!("daemon");
        assert!(parse(&daemon, 7).is_none());
        let mut rel = v.clone();
        rel["messagingSocketPath"] = json!("7.sock");
        assert_eq!(parse(&rel, 7).unwrap().socket, None);
        let mut named = v;
        named["name"] = json!("teer-12\u{1b}[2J");
        assert_eq!(parse(&named, 7).unwrap().target().name, "teer-12 [2J");
    }
}
