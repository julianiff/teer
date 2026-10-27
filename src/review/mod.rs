//! The review: building it from git and the semantic units, preparing it
//! with an agent, validating claims, and persisting it.

pub mod ask;
pub mod attention;
pub mod behavior;
pub mod dive;
pub mod intent;
pub mod model;
pub mod prepare;
pub mod prompts;
pub mod store;
pub mod story;
pub mod test_search;
pub mod validate;
pub mod visual;

use std::path::Path;

use anyhow::Result;

use crate::git::{self, ChangedFile, SkippedFile};
use crate::graph;
use crate::units;
use crate::util::hash_hex;
use model::{Decision, Prep, Review, SessionRef, Unit};
use prepare::PrepareEvent;

#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    pub base: Option<String>,
    pub languages: Option<Vec<String>>,
}

/// Build a review from the repository: diff, units, call graph. No agent.
pub fn build_review(cwd: &Path, opts: &BuildOptions) -> Result<Review> {
    let repo = git::repo_root(cwd)?;
    let spec = git::resolve(&repo, opts.base.as_deref())?;
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
    Ok(Review {
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
    })
}

impl Review {
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

    /// Units flagged for a closer look, in display order (file, then line).
    pub fn flagged(&self) -> Vec<&Unit> {
        let mut v: Vec<&Unit> = self
            .units
            .iter()
            .filter(|u| u.decision == Decision::Investigate)
            .collect();
        v.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then(a.display_line().cmp(&b.display_line()))
        });
        v
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

    pub fn apply(&mut self, ev: PrepareEvent) {
        match ev {
            PrepareEvent::Ready {
                id,
                changes,
                summary,
                cost,
                unmentioned,
                checking,
                ..
            } => {
                if let Some(u) = self.unit_mut(&id) {
                    u.summary = summary;
                    u.prep = if changes.is_empty() {
                        Prep::Unavailable
                    } else {
                        Prep::Ready
                    };
                    u.changes = changes;
                    u.checks.cost = cost;
                    u.checks.unmentioned = unmentioned;
                    // New statements: an old check was about other ones.
                    u.checks.second = None;
                    u.checks.second_failed = None;
                    u.checks.second_pending = checking;
                }
            }
            PrepareEvent::Second { id, opinion } => {
                if let Some(u) = self.unit_mut(&id) {
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
            PrepareEvent::Failed { id, error } => {
                crate::log::write(&format!("preparing {id} failed: {error}"));
                if let Some(u) = self.unit_mut(&id) {
                    u.prep = Prep::Unavailable;
                }
            }
        }
    }

    pub fn set_session(&mut self, s: Option<SessionRef>) {
        self.agent_session = s;
    }
}
