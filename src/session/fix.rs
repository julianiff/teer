//! Proposed changes in the unit view: asking for one (`<space>e`, or on
//! its own when something very important has to change), and applying
//! its hunks (`p` the one at the top of the text, `P` all) to the
//! working tree, or, on a pull request, keeping them as suggestions.

use std::path::PathBuf;

use super::app::{App, Effect};
use crate::config::ProposeFixes;
use crate::review::model::{DiveKind, Risk};

impl App {
    /// After an audit came or was checked: when a high finding stands and
    /// `propose_fixes` allows it, the agent proposes a change to unit
    /// `id` (once). An audit whose findings a second model is still
    /// checking waits for its word.
    pub(crate) fn propose_if_important(&mut self, id: &str) -> Vec<Effect> {
        if self.cfg.propose_fixes == ProposeFixes::Never || !self.can_ask() {
            return Vec::new();
        }
        let Some(u) = self.review.unit(id) else {
            return Vec::new();
        };
        let Some(audit) = u
            .dive
            .results
            .get(&DiveKind::Security)
            .and_then(|r| r.audit.as_ref())
        else {
            return Vec::new();
        };
        let important = !audit.checking
            && audit
                .findings
                .iter()
                .any(|f| f.stands() && f.severity == Risk::High);
        if !important {
            return Vec::new();
        }
        self.dive_on(id, DiveKind::Fix)
    }

    /// `<space>e`: the agent proposes a change to the unit on show (or
    /// under the cursor); with one there, the text scrolls to it.
    pub(crate) fn propose_fix(&mut self) -> Vec<Effect> {
        self.status = "Proposed changes aren't built yet.".into();
        Vec::new()
    }

    /// `p` (`all`: `P`): put the proposed hunk at the top of the text
    /// (every open one) to the working tree, after asking; on a pull
    /// request, keep it as a suggestion for the review.
    pub(crate) fn apply_hunks(&mut self, all: bool) -> Vec<Effect> {
        let _ = all;
        self.status = "Applying proposed changes isn't built yet.".into();
        Vec::new()
    }

    /// Hunks `hunks` of unit `id` were written (or not): say so, mark them
    /// applied, and review again.
    pub(crate) fn fix_applied(
        &mut self,
        id: &str,
        hunks: &[usize],
        result: Result<Vec<PathBuf>, String>,
    ) -> Vec<Effect> {
        let _ = (id, hunks);
        match result {
            Ok(files) => self.status = format!("Applied to {} file(s).", files.len()),
            Err(e) => self.status = format!("Not applied: {e}"),
        }
        Vec::new()
    }
}
