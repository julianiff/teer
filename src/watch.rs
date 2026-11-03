//! Watching the working tree while the review's notes are with an agent
//! teer doesn't run: copied with `y` on the `:w` sheet, or for the model
//! API. teer looks at the tree every so often and re-runs the review once
//! it changed and then stayed the same for `fix_loop.quiet_seconds`.
//!
//! Polling, not file-system events: one `git status` a second costs little,
//! needs no new dependency, works the same on every platform, and sees what
//! git sees (ignored files such as build output don't count).

use std::path::Path;
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::util::hash_hex;

/// What the tree looked like, hashed: HEAD, what `git status` lists, and
/// the size and modification time of each file it lists. Equal snapshots
/// mean nothing changed that git would show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot(pub String);

/// Look at the tree of `repo` now (runs git; blocking).
pub fn snapshot(repo: &Path) -> Result<Snapshot> {
    let head = crate::git::head_commit(repo).unwrap_or_default();
    let out = crate::git::command(repo)
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ])
        .output()
        .context("failed to run git; is it installed?")?;
    if !out.status.success() {
        bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // Each entry is `XY path`; a file edited again keeps its entry, so its
    // size and time tell.
    let mut stats = Vec::new();
    for entry in out.stdout.split(|b| *b == 0).filter(|e| e.len() > 3) {
        let path = String::from_utf8_lossy(&entry[3..]);
        let meta = std::fs::symlink_metadata(repo.join(path.as_ref()));
        let (len, time) = meta.map_or((0, 0), |m| {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok());
            (m.len(), t.map_or(0, |t| t.as_nanos()))
        });
        stats.extend_from_slice(&len.to_le_bytes());
        stats.extend_from_slice(&time.to_le_bytes());
    }
    Ok(Snapshot(hash_hex(&[head.as_bytes(), &out.stdout, &stats])))
}

/// How often to look while waiting for `quiet`: a quarter of it, between
/// a tenth of a second and a second.
pub fn every(quiet: Duration) -> Duration {
    (quiet / 4).clamp(Duration::from_millis(100), Duration::from_secs(1))
}

/// Tells when the tree has settled: it changed since the first look, then
/// stayed the same for the quiet period.
#[derive(Debug, Clone)]
pub struct Quiet {
    period: Duration,
    /// The tree as last seen.
    last: Option<Snapshot>,
    /// When it was last seen to change; `None` until it has.
    changed: Option<Instant>,
}

impl Quiet {
    pub fn new(period: Duration) -> Quiet {
        Quiet {
            period,
            last: None,
            changed: None,
        }
    }

    /// The quiet period.
    pub fn period(&self) -> Duration {
        self.period
    }

    /// Take in the tree as seen at `at`. The first look is what changes are
    /// measured from. True once the tree changed and then stayed the same
    /// for the quiet period.
    pub fn see(&mut self, seen: Snapshot, at: Instant) -> bool {
        match &self.last {
            None => {
                self.last = Some(seen);
                return false;
            }
            Some(last) if *last != seen => {
                self.last = Some(seen);
                self.changed = Some(at);
            }
            Some(_) => {}
        }
        self.changed
            .is_some_and(|c| at.saturating_duration_since(c) >= self.period)
    }

    /// The tree changed since the first look.
    pub fn changing(&self) -> bool {
        self.changed.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn snap(s: &str) -> Snapshot {
        Snapshot(s.into())
    }

    #[test]
    fn quiet_waits_for_a_change_then_for_the_period() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut q = Quiet::new(s(5));
        // Nothing changed: never quiet, however long.
        assert!(!q.see(snap("a"), t0));
        assert!(!q.see(snap("a"), t0 + s(60)));
        assert!(!q.changing());
        // A change, more changes: the period runs from the last one.
        assert!(!q.see(snap("b"), t0 + s(61)));
        assert!(q.changing());
        assert!(!q.see(snap("c"), t0 + s(64)));
        assert!(!q.see(snap("c"), t0 + s(68)));
        assert!(q.see(snap("c"), t0 + s(69)));
    }

    #[test]
    fn a_zero_period_settles_on_the_look_that_sees_the_change() {
        let t0 = Instant::now();
        let mut q = Quiet::new(Duration::ZERO);
        assert!(!q.see(snap("a"), t0));
        assert!(q.see(snap("b"), t0));
    }

    #[test]
    fn looks_come_a_quarter_period_apart_within_limits() {
        assert_eq!(every(Duration::from_secs(5)), Duration::from_secs(1));
        assert_eq!(every(Duration::from_secs(2)), Duration::from_millis(500));
        assert_eq!(every(Duration::ZERO), Duration::from_millis(100));
    }

    #[test]
    fn a_snapshot_changes_with_edits_new_files_and_commits_only() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        git(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "init"]);
        let clean = snapshot(dir).unwrap();
        assert_eq!(snapshot(dir).unwrap(), clean);

        std::fs::write(dir.join("a.rs"), "fn a() { 1 }\n").unwrap();
        let edited = snapshot(dir).unwrap();
        assert_ne!(edited, clean);
        // Edited again, still modified: the size tells.
        std::fs::write(dir.join("a.rs"), "fn a() { 12 }\n").unwrap();
        let again = snapshot(dir).unwrap();
        assert_ne!(again, edited);

        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/b.rs"), "fn b() {}\n").unwrap();
        let added = snapshot(dir).unwrap();
        assert_ne!(added, again);

        // Ignored files don't count.
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target/out"), "x").unwrap();
        assert_eq!(snapshot(dir).unwrap(), added);

        // A commit of it all leaves a clean tree on another HEAD.
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "fix"]);
        let committed = snapshot(dir).unwrap();
        assert_ne!(committed, clean);
        assert_ne!(committed, added);
    }
}
