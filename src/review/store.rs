//! Persistence: reviews as JSON in the XDG data dir, keyed by repo, base
//! and head, so reopening teer restores decisions.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::model::{PrInfo, Prep, Review};
use crate::util::hash_hex;

/// The version of the saved format. 1 added the version itself and the
/// undo history (`Review.history`). A file without one is 0 and loads as it
/// is; a file from a newer teer doesn't load. Fields that may be missing
/// (`Review.cursor`, a step's `notes`) need no new version.
pub const CURRENT: u32 = 1;

/// Writes `Review.version`: always [`CURRENT`], the format this teer writes.
pub(crate) fn current_version<S: serde::Serializer>(
    _: &u32,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_u32(CURRENT)
}

fn home() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `$TEER_DATA_DIR`, else `$XDG_DATA_HOME/teer`, else `~/.local/share/teer`.
/// Unit tests get a temp dir of their own instead.
pub fn data_dir() -> PathBuf {
    #[cfg(test)]
    {
        static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        if std::env::var_os("TEER_DATA_DIR").is_none() {
            return DIR
                .get_or_init(|| tempfile::tempdir().unwrap())
                .path()
                .into();
        }
    }
    if let Some(d) = std::env::var_os("TEER_DATA_DIR") {
        return PathBuf::from(d);
    }
    if let Some(d) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("teer");
    }
    if cfg!(windows) {
        return directories::BaseDirs::new()
            .map(|b| b.data_dir().join("teer"))
            .unwrap_or_else(|| PathBuf::from("teer"));
    }
    home().join(".local").join("share").join("teer")
}

/// `$TEER_CONFIG`, else `$XDG_CONFIG_HOME/teer/config.toml`, else `~/.config/teer/config.toml`.
pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("TEER_CONFIG") {
        return PathBuf::from(p);
    }
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("teer").join("config.toml");
    }
    home().join(".config").join("teer").join("config.toml")
}

fn repo_dir(repo: &Path) -> PathBuf {
    data_dir()
        .join("reviews")
        .join(hash_hex(&[repo.to_string_lossy().as_bytes()]))
}

/// Where reviews live: under the repository, or, for a pull request, under
/// the PR itself. The two never share a directory, so a PR review can't
/// fall back to the repo's latest review or the other way round.
fn scope_dir(repo: &Path, pr: Option<&PrInfo>) -> PathBuf {
    match pr {
        Some(pr) => data_dir()
            .join("reviews")
            .join(format!("pr-{}", pr.store_key())),
        None => repo_dir(repo),
    }
}

/// Where the review of `base..head` at `head_commit` is saved, in the PR's
/// namespace when `pr` is given.
pub fn review_path(
    repo: &Path,
    pr: Option<&PrInfo>,
    base: &str,
    head: &str,
    head_commit: &str,
) -> PathBuf {
    let key = hash_hex(&[base.as_bytes(), head.as_bytes()]);
    let short: String = head_commit.chars().take(12).collect();
    scope_dir(repo, pr).join(format!("{short}-{key}.json"))
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("no parent dir")?;
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::io::Write::write_all(&mut tmp, bytes)?;
    tmp.persist(path)?;
    Ok(())
}

pub fn save(review: &Review) -> Result<PathBuf> {
    let pr = review.pr.as_ref();
    let path = review_path(
        &review.repo_root,
        pr,
        &review.base,
        &review.head,
        &review.head_commit,
    );
    write_atomic(&path, &serde_json::to_vec_pretty(review)?)?;
    write_atomic(&cursor_path(&path), cursor_bytes(review))?;
    write_atomic(
        &scope_dir(&review.repo_root, pr).join("latest"),
        path.to_string_lossy().as_bytes(),
    )?;
    Ok(path)
}

/// Save only where the queue's cursor is (`Review.cursor`), next to the
/// review: a review can be tens of megabytes, and the cursor moves on every
/// key.
pub fn save_cursor(review: &Review) -> Result<()> {
    let path = review_path(
        &review.repo_root,
        review.pr.as_ref(),
        &review.base,
        &review.head,
        &review.head_commit,
    );
    write_atomic(&cursor_path(&path), cursor_bytes(review))
}

/// Where the cursor of the review saved at `path` is kept.
fn cursor_path(path: &Path) -> PathBuf {
    path.with_extension("cursor")
}

fn cursor_bytes(review: &Review) -> &[u8] {
    review.cursor.as_deref().unwrap_or_default().as_bytes()
}

/// The saved review of `base..head` at `head_commit`, in the PR's namespace
/// when `pr` is given.
pub fn load(
    repo: &Path,
    pr: Option<&PrInfo>,
    base: &str,
    head: &str,
    head_commit: &str,
) -> Option<Review> {
    read(&review_path(repo, pr, base, head, head_commit))
}

/// The most recently saved review of the repo, or of the PR when `pr` is given.
pub fn load_latest(repo: &Path, pr: Option<&PrInfo>) -> Option<Review> {
    let path = std::fs::read_to_string(scope_dir(repo, pr).join("latest")).ok()?;
    read(Path::new(path.trim()))
}

/// The review saved at `path`, unless it doesn't parse or a newer teer
/// saved it in a format this one doesn't know.
fn read(path: &Path) -> Option<Review> {
    let mut json: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if json.get("version").and_then(Value::as_u64).unwrap_or(0) == 0 {
        migrate_v1(&mut json);
    }
    let mut review: Review = serde_json::from_value(json).ok()?;
    // The cursor moves more often than the review is saved.
    if let Ok(id) = std::fs::read_to_string(cursor_path(path)) {
        review.cursor = (!id.is_empty()).then_some(id);
    }
    (review.version <= CURRENT).then_some(review)
}

/// A review teer v1 saved (format 0), as JSON, in v2's states. v1 resolved
/// a flagged unit as "needs fix" or "ok" on its Flagged screen
/// (`dive.resolution`); v2 has no such state, so a unit with either is
/// flagged, its note kept, unless it was accepted since. Flagged units stay
/// flagged, with or without a note. The resolution itself is dropped as the
/// review loads. v1 kept no swipe position or cursor, so the queue opens on
/// its first pending ▲ (`App::new`).
fn migrate_v1(review: &mut Value) {
    let units = review.get_mut("units").and_then(Value::as_array_mut);
    for u in units.into_iter().flatten() {
        let resolved = u.pointer("/dive/resolution").is_some_and(|r| !r.is_null());
        let accepted = u.get("decision").and_then(Value::as_str) == Some("Accepted");
        if resolved && !accepted {
            u["decision"] = Value::from("Investigate");
        }
    }
}

/// Carry decisions and agent answers over from a previous review for units
/// whose code didn't change. A unit whose code changed starts pending and
/// keeps only the reviewer's note, marked as written on other code
/// ([`super::model::Unit::note_is_stale`]), and the notes put away before.
pub fn merge_previous(new: &mut Review, old: &Review) {
    let by_id: HashMap<&str, &super::model::Unit> =
        old.units.iter().map(|u| (u.id.as_str(), u)).collect();
    for u in new.units.iter_mut() {
        let Some(prev) = by_id.get(u.id.as_str()) else {
            continue;
        };
        // Changed code starts over: no decision, no dive.
        if prev.fingerprint == u.fingerprint {
            u.decision = prev.decision;
            u.accepted_by = prev.accepted_by;
            u.dive = prev.dive.clone();
            if matches!(prev.prep, Prep::Ready | Prep::Unavailable) {
                u.changes = prev.changes.clone();
                u.summary = prev.summary.clone();
                u.prep = prev.prep;
                u.checks = prev.checks.clone();
                u.reasons = prev.reasons.clone();
            }
        } else {
            // The reviewer's note stays until the reviewer acts on it: it
            // may be what changed the code.
            u.dive.note = prev.dive.note.clone();
            u.dive.note_on = prev.dive.note_on.clone();
            u.dive.archived = prev.dive.archived.clone();
        }
    }
    // Undo and redo go on for units whose decision came along: their code
    // didn't change.
    let same: HashSet<&str> = new
        .units
        .iter()
        .filter(|u| {
            by_id
                .get(u.id.as_str())
                .is_some_and(|p| p.fingerprint == u.fingerprint)
        })
        .map(|u| u.id.as_str())
        .collect();
    let mut history = old.history.clone();
    history.retain_units(|id| same.contains(id));
    new.history = history;
    // The queue resumes on the unit it was left on, if it is still there
    // (else on the first pending ▲: see `App::new`).
    if new.cursor.is_none() {
        new.cursor = old.cursor.clone();
    }
    if new.agent_session.is_none() {
        new.agent_session = old.agent_session.clone();
    }
    // A re-run knows the commits, not the names the header shows.
    if new.branch.is_none() {
        new.branch = old.branch.clone();
    }
    // gh didn't answer this time (or a re-run doesn't ask): keep what it said before.
    if new.pr.is_some() && new.intent.pr.is_none() {
        new.intent.pr = old.intent.pr.clone();
    }
    // The same commits keep their summary.
    if new.intent.summary.is_none()
        && new.head != crate::git::WORKTREE
        && (new.base.as_str(), new.head.as_str()) == (old.base.as_str(), old.head.as_str())
    {
        new.intent.summary = old.intent.summary.clone();
    }
    // Questions are about the code, not one version of it: they stay, with
    // the code their answers showed then.
    if new.questions.is_empty() {
        new.questions = old.questions.clone();
    }
    // An approval belongs to one head: a new head starts unapproved.
    if let (Some(n), Some(o)) = (new.pr.as_mut(), old.pr.as_ref())
        && n.approved_head.is_none()
        && n.head_sha.is_some()
        && o.approved_head == n.head_sha
    {
        n.approved_head = o.approved_head.clone();
    }
}

/// The units of `new` whose code is not what it was in `old`: changed,
/// or not there before. In `new`'s order.
pub fn changed_units(new: &Review, old: &Review) -> Vec<String> {
    let before: HashMap<&str, &str> = old
        .units
        .iter()
        .map(|u| (u.id.as_str(), u.fingerprint.as_str()))
        .collect();
    new.units
        .iter()
        .filter(|u| before.get(u.id.as_str()) != Some(&u.fingerprint.as_str()))
        .map(|u| u.id.clone())
        .collect()
}

/// Load a previous review for the same changes and carry its state over.
pub fn restore(review: &mut Review) {
    // A PR review only ever sees reviews of the same PR, never the repo's own.
    let pr = review.pr.as_ref();
    let prev = load(
        &review.repo_root,
        pr,
        &review.base,
        &review.head,
        &review.head_commit,
    )
    .or_else(|| load_latest(&review.repo_root, pr));
    if let Some(prev) = prev {
        merge_previous(review, &prev);
    }
}

// ---- Who has a review open ---------------------------------------------

/// A review this process has open: a lock file beside the saved reviews of
/// its repository (or its pull request), so two front ends (teer in a
/// terminal and Teer.app, or two windows) never save the same review at
/// the same time. Let go when dropped.
#[derive(Debug)]
pub struct Lock {
    /// Empty when there was nowhere to write it (saving fails then too).
    path: PathBuf,
    token: u64,
}

/// Who has a review open already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub pid: u32,
    /// What has it open: `teer in a terminal`, `Teer.app`.
    pub by: String,
}

impl std::fmt::Display for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (process {})", self.by, self.pid)
    }
}

/// The locks this process holds, by token: another of its windows asking
/// for one of them is turned away like another process.
static HELD: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
static NEXT_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Hold `review` open for `by`, unless a live process has it open: then
/// who. A lock whose process is gone is taken over.
pub fn lock(review: &Review, by: &str) -> std::result::Result<Lock, Held> {
    use std::io::Write;
    let dir = scope_dir(&review.repo_root, review.pr.as_ref());
    let path = dir.join("lock");
    let pid = std::process::id();
    let token =
        (u64::from(pid) << 32) | NEXT_TOKEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if std::fs::create_dir_all(&dir).is_err() {
        return Ok(Lock {
            path: PathBuf::new(),
            token,
        });
    }
    for _ in 0..3 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                let _ = write!(f, "{pid}\n{token}\n{by}\n");
                HELD.lock().expect("held locks").push(token);
                return Ok(Lock { path, token });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Some((holder, their, by)) = read_lock(&path)
                    && alive(holder, their)
                {
                    return Err(Held { pid: holder, by });
                }
                // Left behind by a process that is gone.
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => break,
        }
    }
    Ok(Lock {
        path: PathBuf::new(),
        token,
    })
}

/// The pid, token and holder a lock file names.
fn read_lock(path: &Path) -> Option<(u32, u64, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let pid = lines.next()?.trim().parse().ok()?;
    let token = lines.next()?.trim().parse().ok()?;
    Some((pid, token, lines.next().unwrap_or("teer").to_string()))
}

/// Whether the lock `token` of process `pid` is still held.
fn alive(pid: u32, token: u64) -> bool {
    if pid == std::process::id() {
        return HELD.lock().expect("held locks").contains(&token);
    }
    process_alive(pid)
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    true
}

impl Drop for Lock {
    fn drop(&mut self) {
        if let Ok(mut held) = HELD.lock() {
            held.retain(|t| *t != self.token);
        }
        if read_lock(&self.path).is_some_and(|(_, t, _)| t == self.token) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::history::{Change, Step};
    use crate::review::model::{Decision, Dive, DiveKind, DiveResult, Unit};

    fn unit(fingerprint: &str) -> Unit {
        serde_json::from_value(serde_json::json!({
            "id": "a::f", "file": "a.rs", "symbol": "f", "kind": "Function",
            "status": "Changed", "old_range": null, "new_range": null,
            "fingerprint": fingerprint, "changes": [], "callers": [], "callees": [],
            "decision": "Pending"
        }))
        .unwrap()
    }

    fn review(u: Unit) -> Review {
        Review {
            id: "r".into(),
            repo_root: PathBuf::new(),
            base: String::new(),
            head: String::new(),
            head_commit: String::new(),
            agent_session: None,
            pr: None,
            intent: Default::default(),
            created_at: chrono::Utc::now(),
            files: Vec::new(),
            units: vec![u],
            skipped: Vec::new(),
            graph: Default::default(),
            questions: Vec::new(),
            version: 0,
            history: Default::default(),
            branch: None,
            cursor: None,
        }
    }

    fn flagged_with_dive(fp: &str) -> Unit {
        let mut u = unit(fp);
        u.decision = Decision::Investigate;
        let mut dive = Dive::default();
        dive.results.insert(
            DiveKind::Recommend,
            DiveResult {
                text: "why".into(),
                sources: Vec::new(),
                visual: None,
                behavior: None,
                coverage: None,
                audit: None,
            },
        );
        dive.note = "check".into();
        u.dive = dive;
        u
    }

    #[test]
    fn dive_and_decision_carry_over_when_fingerprints_match() {
        let old = review(flagged_with_dive("same"));
        let mut new = review(unit("same"));
        merge_previous(&mut new, &old);
        assert_eq!(new.units[0].decision, Decision::Investigate);
        assert_eq!(new.units[0].dive, old.units[0].dive);
    }

    #[test]
    fn who_accepted_carries_over_with_the_decision() {
        use crate::review::model::AcceptedBy;
        let mut accepted = unit("same");
        accepted.decision = Decision::Accepted;
        accepted.accepted_by = Some(AcceptedBy::Auto);
        let old = review(accepted);
        let mut new = review(unit("same"));
        merge_previous(&mut new, &old);
        assert_eq!(new.units[0].accepted_by, Some(AcceptedBy::Auto));
        // Changed code starts over.
        let mut new = review(unit("other"));
        merge_previous(&mut new, &old);
        assert_eq!(
            (new.units[0].decision, new.units[0].accepted_by),
            (Decision::Pending, None)
        );
    }

    #[test]
    fn dive_is_dropped_when_the_code_changed_but_the_note_stays() {
        let mut flagged = flagged_with_dive("before");
        flagged.dive.note_on = Some("before".into());
        let old = review(flagged);
        let mut new = review(unit("after"));
        merge_previous(&mut new, &old);
        let dive = &new.units[0].dive;
        assert!(dive.results.is_empty());
        assert_eq!(dive.note, "check");
        // Written on the old code: it says so.
        assert!(new.units[0].note_is_stale());
        // Same as before this feature: a changed unit starts undecided.
        assert_eq!(new.units[0].decision, Decision::Pending);
    }

    #[test]
    fn a_changed_unit_keeps_its_archived_notes_and_starts_pending() {
        let mut flagged = flagged_with_dive("before");
        flagged.dive.note_on = Some("before".into());
        flagged.archive_note();
        flagged.dive.note = "second".into();
        flagged.dive.note_on = Some("before".into());
        let old = review(flagged);
        let mut new = review(unit("after"));
        merge_previous(&mut new, &old);
        let u = &new.units[0];
        assert_eq!(u.decision, Decision::Pending);
        assert_eq!(u.dive.note, "second");
        assert!(u.note_is_stale());
        assert_eq!(u.dive.archived.len(), 1);
        assert_eq!(u.dive.archived[0].text, "check");
    }

    #[test]
    fn changed_units_are_the_changed_and_the_new_ones() {
        let old = saved_at(
            "/teer-tests/changed",
            vec![named("a", "1"), named("b", "2")],
        );
        let new = saved_at(
            "/teer-tests/changed",
            vec![named("c", "3"), named("a", "1"), named("b", "2b")],
        );
        assert_eq!(changed_units(&new, &old), ["c", "b"]);
        assert!(changed_units(&old, &old).is_empty());
    }

    fn named(id: &str, fingerprint: &str) -> Unit {
        Unit {
            id: id.into(),
            ..unit(fingerprint)
        }
    }

    fn change(id: &str, before: Decision, after: Decision) -> Change {
        Change {
            id: id.into(),
            before,
            after,
        }
    }

    /// A review saved where no other test saves one.
    fn saved_at(repo: &str, units: Vec<Unit>) -> Review {
        let mut r = review(unit("x"));
        r.repo_root = PathBuf::from(repo);
        r.units = units;
        r
    }

    /// What teer 0.1 wrote: no `version`, no `history`.
    const V1_REVIEW: &str = r#"{
      "id": "v1",
      "repo_root": "/teer-tests/v1",
      "base": "abc123",
      "head": "WORKTREE",
      "head_commit": "def456",
      "agent_session": { "agent": "claude", "id": "", "cwd": "/teer-tests/v1" },
      "created_at": "2026-10-27T12:00:00Z",
      "files": [],
      "units": [{
        "id": "a.rs::f", "file": "a.rs", "symbol": "f", "kind": "Function",
        "status": "Changed", "old_range": null, "new_range": null,
        "fingerprint": "fp", "changes": [], "callers": [], "callees": [],
        "decision": "Accepted"
      }],
      "skipped": [],
      "questions": []
    }"#;

    fn write_v1(text: &str) {
        let path = review_path(
            Path::new("/teer-tests/v1"),
            None,
            "abc123",
            "WORKTREE",
            "def456",
        );
        write_atomic(&path, text.as_bytes()).unwrap();
    }

    #[test]
    fn a_review_saved_before_versions_loads_as_version_0() {
        write_v1(V1_REVIEW);
        let r = load(
            Path::new("/teer-tests/v1"),
            None,
            "abc123",
            "WORKTREE",
            "def456",
        )
        .expect("a v1 file loads");
        assert_eq!(r.version, 0);
        assert!(r.history.is_empty());
        assert_eq!(r.units[0].decision, Decision::Accepted);
        // Who accepted it wasn't kept then; the reasons come with the levels.
        assert_eq!(r.units[0].accepted_by, None);
        assert!(r.units[0].reasons.is_empty());
        // Saved again, it is the current format.
        let path = save(&r).unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(json["version"], CURRENT);
        assert_eq!(
            load_latest(Path::new("/teer-tests/v1"), None)
                .unwrap()
                .version,
            CURRENT
        );
    }

    #[test]
    fn a_review_from_a_newer_teer_is_not_loaded() {
        let mut r = saved_at("/teer-tests/newer", vec![unit("x")]);
        r.version = CURRENT;
        let path = save(&r).unwrap();
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        json["version"] = (CURRENT + 1).into();
        write_atomic(&path, &serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(load(&r.repo_root, None, &r.base, &r.head, &r.head_commit).is_none());
        assert!(load_latest(&r.repo_root, None).is_none());
    }

    #[test]
    fn the_history_is_saved_and_loaded() {
        let mut r = saved_at(
            "/teer-tests/history",
            vec![named("a", "1"), named("b", "2")],
        );
        r.history.record(Step {
            changes: vec![
                change("a", Decision::Pending, Decision::Accepted),
                change("b", Decision::Pending, Decision::Investigate),
            ],
            ..Default::default()
        });
        r.history.record(Step {
            changes: vec![change("a", Decision::Accepted, Decision::Investigate)],
            ..Default::default()
        });
        r.history.undo();
        save(&r).unwrap();
        let back = load(&r.repo_root, None, &r.base, &r.head, &r.head_commit).unwrap();
        assert_eq!(back.history, r.history);
        assert_eq!(back.version, CURRENT);
    }

    #[test]
    fn a_rerun_keeps_the_history_of_units_whose_code_stayed() {
        let mut old = saved_at("/teer-tests/rerun", vec![named("a", "1"), named("b", "2")]);
        old.history.record(Step {
            changes: vec![
                change("a", Decision::Pending, Decision::Accepted),
                change("b", Decision::Pending, Decision::Accepted),
            ],
            ..Default::default()
        });
        old.history.record(Step {
            changes: vec![change("b", Decision::Accepted, Decision::Investigate)],
            ..Default::default()
        });
        // `b` changed; `a` didn't.
        let mut new = saved_at("/teer-tests/rerun", vec![named("a", "1"), named("b", "2b")]);
        merge_previous(&mut new, &old);
        assert_eq!(
            new.history.undo,
            vec![Step {
                changes: vec![change("a", Decision::Pending, Decision::Accepted)],
                ..Default::default()
            }]
        );
        assert!(new.history.redo.is_empty());
    }

    #[test]
    fn the_cursor_is_saved_and_carried_over() {
        let mut r = saved_at("/teer-tests/cursor", vec![named("a", "1"), named("b", "2")]);
        r.cursor = Some("b".into());
        save(&r).unwrap();
        let back = load(&r.repo_root, None, &r.base, &r.head, &r.head_commit).unwrap();
        assert_eq!(back.cursor.as_deref(), Some("b"));
        // A new run of the same changes starts where the last one was left,
        // even when the unit's code changed since.
        let mut new = saved_at(
            "/teer-tests/cursor",
            vec![named("a", "1"), named("b", "2b")],
        );
        merge_previous(&mut new, &back);
        assert_eq!(new.cursor.as_deref(), Some("b"));
    }

    #[test]
    fn a_v1_resolution_becomes_a_flag_with_its_note() {
        use Decision::*;
        // Each unit, its decision, the resolution v1 saved with it, its note.
        let v1 = [
            ("fix", Investigate, "NeedsFix", "the +1"),
            ("ok", Investigate, "Ok", ""),
            ("plain", Investigate, "", ""),
            // Reset after resolving it: the fix still stands.
            ("reset", Pending, "NeedsFix", "why"),
            // Accepted after resolving it: accepted.
            ("later", Accepted, "NeedsFix", "n"),
            ("note", Pending, "", "a question"),
        ];
        let units = v1
            .iter()
            .map(|&(id, decision, _, note)| {
                let mut u = named(id, "1");
                u.decision = decision;
                u.dive.note = note.into();
                u
            })
            .collect();
        let r = saved_at("/teer-tests/v1-resolutions", units);
        let path = save(&r).unwrap();
        // Saved by v1: no version, and a resolution on the resolved units.
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        json.as_object_mut().unwrap().remove("version");
        for (u, (_, _, resolution, _)) in json["units"].as_array_mut().unwrap().iter_mut().zip(v1) {
            u["dive"]["resolution"] = match resolution {
                "" => serde_json::Value::Null,
                r => r.into(),
            };
        }
        write_atomic(&path, &serde_json::to_vec(&json).unwrap()).unwrap();

        let back = load(&r.repo_root, None, &r.base, &r.head, &r.head_commit).unwrap();
        let got: Vec<(&str, Decision, &str)> = back
            .units
            .iter()
            .map(|u| (u.id.as_str(), u.decision, u.dive.note.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("fix", Investigate, "the +1"),
                ("ok", Investigate, ""),
                ("plain", Investigate, ""),
                ("reset", Investigate, "why"),
                ("later", Accepted, "n"),
                ("note", Pending, "a question"),
            ]
        );
        // The resolution is gone once it loads.
        save(&back).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("\"resolution\""), "{saved}");

        // A file of this teer keeps what it says.
        let mut r = back;
        r.units[1].decision = Pending;
        save(&r).unwrap();
        let back = load(&r.repo_root, None, &r.base, &r.head, &r.head_commit).unwrap();
        assert_eq!(back.units[1].decision, Pending);
    }

    #[test]
    fn a_review_is_open_in_one_place_at_a_time() {
        let mut review = test_review_for_lock();
        let first = lock(&review, "Teer.app").expect("free");
        let held = lock(&review, "teer in a terminal").expect_err("held");
        assert_eq!(held.by, "Teer.app");
        assert_eq!(held.pid, std::process::id());
        // Another repository is free.
        review.repo_root = PathBuf::from("/work/other-shop");
        let other = lock(&review, "Teer.app").expect("another repo");
        drop(other);
        review.repo_root = PathBuf::from("/work/lock-shop");
        drop(first);
        assert!(lock(&review, "teer in a terminal").is_ok());
    }

    #[test]
    fn a_lock_left_by_a_process_that_is_gone_is_taken_over() {
        let mut review = test_review_for_lock();
        review.repo_root = PathBuf::from("/work/stale-shop");
        let dir = scope_dir(&review.repo_root, None);
        std::fs::create_dir_all(&dir).unwrap();
        // No process has this pid (above the largest pid macOS and Linux give).
        std::fs::write(dir.join("lock"), "99999999\n7\nteer\n").unwrap();
        assert!(lock(&review, "Teer.app").is_ok());
    }

    fn test_review_for_lock() -> Review {
        serde_json::from_value(serde_json::json!({
            "id": "r", "repo_root": "/work/lock-shop", "base": "a", "head": "WORKTREE",
            "head_commit": "b", "agent_session": null, "created_at": "2026-11-04T00:00:00Z",
            "files": [], "units": []
        }))
        .expect("a review")
    }
}
