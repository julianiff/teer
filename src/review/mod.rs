//! The review: building it from git and the semantic units, preparing it
//! with an agent, validating claims, and persisting it.

pub mod ask;
pub mod attention;
pub mod behavior;
pub mod comments;
pub mod diff;
pub mod dive;
pub mod history;
pub mod intent;
pub mod model;
pub mod prepare;
pub mod prompts;
pub mod scope;
pub mod store;
pub mod story;
pub mod test_search;
pub mod validate;

use std::path::Path;

use anyhow::Result;

use crate::git::{self, ChangedFile, SkippedFile};
use crate::graph;
use crate::units;
use crate::util::{hash_hex, sanitize};
use model::{Prep, Review, SessionRef, Unit};
use prepare::PrepareEvent;

#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    pub base: Option<String>,
    pub languages: Option<Vec<String>>,
    /// SARIF files of other tools' findings (`--sarif`), absolute: read on
    /// every build, so a re-run pins them again (`security::sarif`).
    pub sarif: Vec<std::path::PathBuf>,
}

/// Build a review from the repository: diff, units, call graph. No agent.
/// With a base, the branch since it, what isn't committed yet included
/// (untracked files too): a re-run sees the fixes an agent made without
/// committing them.
pub fn build_review(cwd: &Path, opts: &BuildOptions) -> Result<Review> {
    build(cwd, opts, true)
}

/// [`build_review`] of the commits since the base alone: a pull request's,
/// in teer's own checkout of it.
pub fn build_commits_review(cwd: &Path, opts: &BuildOptions) -> Result<Review> {
    build(cwd, opts, false)
}

/// With a base, `tree` takes in the working tree too (see [`git::resolve`]).
fn build(cwd: &Path, opts: &BuildOptions, tree: bool) -> Result<Review> {
    let repo = git::repo_root(cwd)?;
    let spec = git::resolve(&repo, opts.base.as_deref(), tree)?;
    let (files, skipped) = git::changes(&repo, &spec)?;
    let analysis = units::analyze(&files, opts.languages.as_deref());
    let mut units = analysis.units.clone();
    let graph = graph::build(&repo, &analysis, &mut units);
    // Uncommitted changes have no commits to tell their story.
    let intent = if spec.head == git::WORKTREE {
        Default::default()
    } else {
        intent::read(&repo, &spec.base, &spec.head)
    };
    let mut review = Review {
        id: hash_hex(&[
            repo.to_string_lossy().as_bytes(),
            spec.base.as_bytes(),
            spec.head.as_bytes(),
            spec.head_commit.as_bytes(),
        ]),
        repo_root: repo,
        base: spec.base,
        head: spec.head,
        head_commit: spec.head_commit,
        agent_session: None,
        created_at: chrono::Utc::now(),
        files,
        units,
        skipped,
        graph,
        pr: None,
        intent,
        questions: Vec::new(),
        version: 0,
        history: Default::default(),
        branch: None,
        cursor: None,
        mode: None,
    };
    crate::security::facts::annotate(&mut review, &crate::security::SecurityConfig::load());
    // Other tools' findings, after teer's own facts (which start afresh).
    if !opts.sarif.is_empty() {
        crate::security::sarif::import_files(&mut review, &opts.sarif);
    }
    Ok(review)
}

impl Review {
    /// What the review is of, as its header says it: "your changes", "feature
    /// since main, 6 commits", "PR #482 by octocat".
    pub fn subject(&self) -> String {
        if let Some(pr) = &self.pr {
            let author = self.intent.pr.as_ref().map(|m| m.author.as_str());
            return match author.filter(|a| !a.is_empty()) {
                Some(a) => format!("PR #{} by {}", pr.number, sanitize(a)),
                None => format!("PR #{}", pr.number),
            };
        }
        if self.head == git::WORKTREE {
            return "your changes".into();
        }
        // No commits read (git failed, or a review made up in a test): no count.
        let commits = match self.intent.commits.len() {
            0 => String::new(),
            1 => ", 1 commit".into(),
            n => format!(", {n} commits"),
        };
        match &self.branch {
            Some(b) => format!("{} since {}{commits}", sanitize(&b.head), sanitize(&b.base)),
            None => {
                let short: String = self.base.chars().take(7).collect();
                format!("since {short}{commits}")
            }
        }
    }

    pub fn file_of(&self, unit: &Unit) -> Option<&ChangedFile> {
        self.files.iter().find(|f| f.path == unit.file)
    }

    pub fn unit(&self, id: &str) -> Option<&Unit> {
        self.units.iter().find(|u| u.id == id)
    }

    /// One line naming the changed files left out of the review, each as
    /// `name` puts it: "2 files were not shown: logo.png, data.csv".
    /// `None` when there are none.
    pub fn not_shown(&self, name: impl Fn(&SkippedFile) -> String) -> Option<String> {
        const MAX_NAMED: usize = 5;
        let n = self.skipped.len();
        if n == 0 {
            return None;
        }
        let mut names: Vec<String> = self.skipped.iter().take(MAX_NAMED).map(name).collect();
        if n > MAX_NAMED {
            names.push(format!("and {} more", n - MAX_NAMED));
        }
        let (files, were) = if n == 1 {
            ("file", "was")
        } else {
            ("files", "were")
        };
        Some(format!(
            "{n} {files} {were} not shown: {}",
            names.join(", ")
        ))
    }

    pub fn unit_mut(&mut self, id: &str) -> Option<&mut Unit> {
        self.units.iter_mut().find(|u| u.id == id)
    }

    /// Units (with their files) the agent still needs to describe, limited to `ids` if given.
    pub fn to_prepare(&self, ids: Option<&[String]>) -> Vec<(Unit, ChangedFile)> {
        self.units
            .iter()
            .filter(|u| u.needs_agent() && matches!(u.prep, Prep::NotRequested | Prep::Pending))
            .filter(|u| ids.is_none_or(|ids| ids.contains(&u.id)))
            .filter_map(|u| self.file_of(u).map(|f| (u.clone(), f.clone())))
            .collect()
    }

    /// Unit `id` while its code is still `fingerprint`, the code an answer
    /// was asked about; `None` once a re-run changed it (or removed the
    /// unit) since.
    pub fn unit_as_asked(&mut self, id: &str, fingerprint: &str) -> Option<&mut Unit> {
        self.unit_mut(id).filter(|u| u.fingerprint == fingerprint)
    }

    /// Keep what the agent said about a unit. An answer about code a re-run
    /// changed since it was asked is dropped: the unit keeps no result of
    /// it and is asked again about the code it has now.
    pub fn apply(&mut self, ev: PrepareEvent) {
        match ev {
            PrepareEvent::Ready {
                id,
                fingerprint,
                changes,
                summary,
                cost,
                unmentioned,
                suspects,
                checking,
                ..
            } => {
                if let Some(u) = self.unit_as_asked(&id, &fingerprint) {
                    u.summary = summary;
                    u.prep = if changes.is_empty() {
                        Prep::Unavailable
                    } else {
                        Prep::Ready
                    };
                    u.changes = changes;
                    u.checks.cost = cost;
                    u.checks.unmentioned = unmentioned;
                    u.checks.suspects = suspects;
                    // New statements: an old check was about other ones.
                    u.checks.second = None;
                    u.checks.second_failed = None;
                    u.checks.second_pending = checking;
                }
            }
            PrepareEvent::Second {
                id,
                fingerprint,
                opinion,
            } => {
                if let Some(u) = self.unit_as_asked(&id, &fingerprint) {
                    u.checks.second_pending = false;
                    match opinion {
                        Ok(o) => {
                            u.checks.second = Some(o);
                            u.checks.second_failed = None;
                        }
                        Err(e) => {
                            crate::log::write(&format!("second opinion on {id} failed: {e}"));
                            u.checks.second_failed = Some(e);
                        }
                    }
                }
            }
            PrepareEvent::Failed {
                id,
                fingerprint,
                error,
            } => {
                crate::log::write(&format!("preparing {id} failed: {error}"));
                if let Some(u) = self.unit_as_asked(&id, &fingerprint) {
                    u.prep = Prep::Unavailable;
                }
            }
        }
    }

    pub fn set_session(&mut self, s: Option<SessionRef>) {
        self.agent_session = s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(head: &str, base: &str) -> Review {
        let mut r: Review = serde_json::from_value(serde_json::json!({
            "id": "r", "repo_root": "/work/subject-test", "base": base, "head": head,
            "head_commit": head, "agent_session": null, "created_at": "2026-11-02T00:00:00Z",
            "files": [], "units": []
        }))
        .unwrap();
        r.intent.commits = vec![Default::default(); 3];
        r
    }

    #[test]
    fn the_subject_says_what_is_reviewed() {
        assert_eq!(review(git::WORKTREE, "b1").subject(), "your changes");

        let r = review("h2", "b2");
        assert_eq!(r.subject(), "since b2, 3 commits");
        let mut r = r;
        r.branch = Some(crate::review::model::BranchNames {
            head: "feature".into(),
            base: "origin/main".into(),
        });
        assert_eq!(r.subject(), "feature since origin/main, 3 commits");
        let mut one = r.clone();
        one.intent.commits.truncate(1);
        assert_eq!(one.subject(), "feature since origin/main, 1 commit");

        let mut pr = review("h3", "b3");
        pr.pr = Some(crate::review::model::PrInfo {
            number: 482,
            ..Default::default()
        });
        assert_eq!(pr.subject(), "PR #482");
        pr.intent.pr = Some(crate::review::model::PrMeta {
            author: "octocat".into(),
            ..Default::default()
        });
        assert_eq!(pr.subject(), "PR #482 by octocat");
    }
}
