//! Persistence: reviews as JSON in the XDG data dir, keyed by repo, base
//! and head, so reopening teer restores decisions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::model::{PrInfo, Prep, Review};
use crate::util::hash_hex;

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
    write_atomic(
        &scope_dir(&review.repo_root, pr).join("latest"),
        path.to_string_lossy().as_bytes(),
    )?;
    Ok(path)
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
    let bytes = std::fs::read(review_path(repo, pr, base, head, head_commit)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The most recently saved review of the repo, or of the PR when `pr` is given.
pub fn load_latest(repo: &Path, pr: Option<&PrInfo>) -> Option<Review> {
    let path = std::fs::read_to_string(scope_dir(repo, pr).join("latest")).ok()?;
    serde_json::from_slice(&std::fs::read(path.trim()).ok()?).ok()
}

/// Carry decisions and agent answers over from a previous review for units
/// whose code didn't change.
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
            u.dive = prev.dive.clone();
            if matches!(prev.prep, Prep::Ready | Prep::Unavailable) {
                u.changes = prev.changes.clone();
                u.summary = prev.summary.clone();
                u.prep = prev.prep;
                u.checks = prev.checks.clone();
            }
        } else {
            // The reviewer's note stays: it may be what changed the code.
            u.dive.note = prev.dive.note.clone();
            u.dive.note_on = prev.dive.note_on.clone();
        }
    }
    if new.agent_session.is_none() {
        new.agent_session = old.agent_session.clone();
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn dive_is_dropped_when_the_code_changed_but_the_note_stays() {
        let mut flagged = flagged_with_dive("before");
        flagged.dive.note_on = Some("before".into());
        let old = review(flagged);
        let mut new = review(unit("after"));
        merge_previous(&mut new, &old);
        let dive = &new.units[0].dive;
        assert!(dive.results.is_empty() && dive.resolution.is_none());
        assert_eq!(dive.note, "check");
        // Written on the old code: it says so.
        assert!(new.units[0].note_is_stale());
        // Same as before this feature: a changed unit starts undecided.
        assert_eq!(new.units[0].decision, Decision::Pending);
    }
}
