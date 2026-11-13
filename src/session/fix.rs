//! Proposed changes in the unit view: asking for one (`<space>e`, or on
//! its own when something very important has to change), and applying
//! its hunks (`p` the one at the top of the text, `P` all) to the
//! working tree, or, on a pull request, keeping them as suggestions.
//!
//! The unit's text shows the proposal under "Proposed change", each hunk
//! boxed like the code change; `]c` `[c` stop at each after the code
//! change's hunks. The hunk a jump went to is the current one; else the
//! one whose box is at or nearest the top of the text. That is the one
//! `p` puts in.

use std::collections::BTreeSet;
use std::path::PathBuf;

use super::app::{App, Effect, Overlay, Screen};
use super::tools;
use super::unit::Panel;
use crate::config::ProposeFixes;
use crate::fix::{HunkState, Proposal, ProposedHunk, Reason};
use crate::keymap::Action;
use crate::review::diff::{Kind, Row, expand_tabs};
use crate::review::model::{Decision, DiveKind, Risk, Unit};
use crate::syntax::Cache;

/// The change the agent proposed for `u`, when it has one.
pub fn proposal(u: &Unit) -> Option<&Proposal> {
    u.dive.results.get(&DiveKind::Fix)?.proposal.as_ref()
}

/// What a proposed change for `u` would answer (`fix::propose::target`):
/// the most severe security finding that stands, else the reviewer's
/// note, else the advice. `None` when there is nothing to fix yet.
pub fn target(u: &Unit) -> Option<Reason> {
    crate::fix::propose::target(u).map(|(reason, _)| reason)
}

/// What a proposal answers, in a few words: `your note`, `the advice`, or
/// the finding's title; with a finding, its severity.
pub fn answers(r: &Reason) -> (String, Option<Risk>) {
    match r {
        Reason::Finding {
            title, severity, ..
        } => (crate::util::sanitize(title), Some(*severity)),
        Reason::Note => ("your note".into(), None),
        Reason::Advice => ("the advice".into(), None),
    }
}

/// Lines hunk `h` adds and removes.
pub fn counts(h: &ProposedHunk) -> (usize, usize) {
    let lines = h.diff_lines();
    let added = lines.iter().filter(|l| l.starts_with('+')).count();
    let removed = lines.iter().filter(|l| l.starts_with('-')).count();
    (added, removed)
}

/// `src/x.php:3-5`: where hunk `h` is.
pub fn place(h: &ProposedHunk) -> String {
    let file = crate::util::sanitize(&h.file.display().to_string());
    if h.lines.end > h.lines.start {
        format!("{file}:{}-{}", h.lines.start, h.lines.end)
    } else {
        format!("{file}:{}", h.lines.start)
    }
}

/// How hunk `h` stands, in a word: `proposed`, `put in`, `suggested`.
pub fn state_word(s: HunkState) -> &'static str {
    match s {
        HunkState::Proposed => "proposed",
        HunkState::Applied => "put in",
        HunkState::Suggested => "suggested",
    }
}

/// Hunk `h` as diff rows, as the code change has its own: the lines `old`
/// and `new` share as context, the rest removed and added. Removed and
/// context rows carry their line in the code reviewed (from
/// `h.lines.start`), added and context rows theirs in the code with the
/// hunk put in. Tabs are expanded and the indentation every line shares is
/// dropped; with `syntax`, each row has its colours (each side parsed on
/// its own).
pub fn hunk_rows(h: &ProposedHunk, syntax: Option<&Cache>) -> Vec<Row> {
    let paint = |text: &str| syntax.map(|c| c.piece(&h.file, text));
    let (old_classes, new_classes) = (paint(&h.old), paint(&h.new));
    let class_of = |lines: &Option<std::sync::Arc<crate::syntax::Lines>>, k: usize| {
        lines
            .as_ref()
            .and_then(|l| l.get(k).cloned())
            .unwrap_or_default()
    };
    let start = h.lines.start;
    let (mut o, mut n) = (0usize, 0usize);
    let mut rows: Vec<Row> = h
        .diff_lines()
        .iter()
        .map(|l| {
            let text = l.get(1..).unwrap_or_default();
            let (kind, line, old, classes) = match l.as_bytes().first() {
                Some(b'-') => {
                    o += 1;
                    let c = class_of(&old_classes, o - 1);
                    (Kind::Removed, None, Some(start + o as u32 - 1), c)
                }
                Some(b'+') => {
                    n += 1;
                    let c = class_of(&new_classes, n - 1);
                    (Kind::Added, Some(start + n as u32 - 1), None, c)
                }
                _ => {
                    o += 1;
                    n += 1;
                    let c = class_of(&old_classes, o - 1);
                    let (line, old) = (start + n as u32 - 1, start + o as u32 - 1);
                    (Kind::Context, Some(line), Some(old), c)
                }
            };
            let (text, classes) = expand_tabs(text, &classes);
            Row {
                kind,
                line,
                old,
                text,
                classes,
            }
        })
        .collect();
    // The indentation every non-blank line shares.
    let lead = |t: &str| t.len() - t.trim_start_matches(' ').len();
    let common = rows
        .iter()
        .filter(|r| !r.text.trim().is_empty())
        .map(|r| lead(&r.text))
        .min()
        .unwrap_or(0);
    for r in &mut rows {
        let cut = common.min(lead(&r.text));
        r.text.drain(..cut);
        r.classes.drain(..cut.min(r.classes.len()));
    }
    rows
}

/// The name of the tile that says why there is no change to propose.
pub const TILE: &str = "Proposed change";

/// "1 hunk", "3 hunks".
fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

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

    /// Why `<space>e` has nothing to do for `u`, when it has neither a
    /// change nor one on its way: no agent, or nothing to fix yet (and the
    /// keys that give it something). The status says it, and the "Proposed
    /// change" tile that `<space>e` opens then.
    pub fn fix_idle(&self, u: &Unit) -> Option<String> {
        if proposal(u).is_some() || tools::pending(u, DiveKind::Fix) {
            return None;
        }
        if !self.can_ask() {
            return Some("No agent: proposing a change needs one.".into());
        }
        if target(u).is_some() {
            return None;
        }
        let k = |a: Action| self.keys.label(a);
        Some(format!(
            "Nothing to fix yet: {} finds security issues, {} asks what to do, {} writes a note.",
            k(Action::ToolSecurity),
            k(Action::AskOpen),
            k(Action::NoteEdit)
        ))
    }

    /// What the "Proposed change" tile says about `u`: why there is no
    /// change to propose ([`App::fix_idle`]); else, the state having
    /// changed while it was open, where the change is or how to ask.
    pub fn fix_tile_text(&self, u: &Unit) -> String {
        if let Some(why) = self.fix_idle(u) {
            return why;
        }
        if proposal(u).is_some() || tools::pending(u, DiveKind::Fix) {
            return "The change is in the text, under the code change.".into();
        }
        format!(
            "There is something to fix now: {} asks the agent for a change.",
            self.keys.label(Action::ToolFix)
        )
    }

    /// `<space>e`: the agent proposes a change to the unit on show (or
    /// under the cursor, whose unit view opens first); with one there, the
    /// text scrolls to its first hunk not put in yet; on its way or asked
    /// for, to the "Proposed change" section. With nothing to do, a tile
    /// opens beside the ones open and says why ([`Panel::Fix`]).
    pub(crate) fn propose_fix(&mut self) -> Vec<Effect> {
        let Some(id) = self.current_unit().map(|u| u.id.clone()) else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        if self.screen != Screen::Unit {
            effects = self.open_unit(id.clone());
        }
        let Some(u) = self.review.unit(&id) else {
            return effects;
        };
        if let Some(p) = proposal(u) {
            if p.hunks.is_empty() {
                self.status = format!(
                    "The proposed change has no hunk left: {} dropped.",
                    plural(p.dropped, "hunk")
                );
                self.show_fix_section();
                return effects;
            }
            let i = p.open().first().copied().unwrap_or(0);
            self.show_fix(i);
            return effects;
        }
        let pending = tools::pending(u, DiveKind::Fix);
        let idle = self.fix_idle(u);
        if let Some(why) = idle {
            self.status = why;
            self.open_tile(Panel::Fix);
            return effects;
        }
        // Something to show in the text: the tile has nothing more to say.
        self.close_tile(Panel::Fix);
        self.show_fix_section();
        if pending {
            self.status =
                "A change is on its way: the agent writes it, then teer checks it.".into();
            return effects;
        }
        effects.extend(self.dive_on(&id, DiveKind::Fix));
        self.status = "The agent proposes a change; teer checks it before it shows.".into();
        effects
    }

    /// `p` (`all`: `P`): put the proposed hunk at the top of the text
    /// (every open one) to the working tree, after asking; on a pull
    /// request, keep it as a suggestion for the review. From the queue or
    /// the flagged list the unit view opens first, at its first open hunk.
    pub(crate) fn apply_hunks(&mut self, all: bool) -> Vec<Effect> {
        let Some(id) = self.current_unit().map(|u| u.id.clone()) else {
            return Vec::new();
        };
        let Some(p) = self.review.unit(&id).and_then(proposal) else {
            self.status = format!(
                "No proposed change: {} asks for one.",
                self.keys.label(Action::ToolFix)
            );
            return Vec::new();
        };
        if p.hunks.is_empty() {
            self.status = format!(
                "No proposed hunk to put in: {} dropped.",
                plural(p.dropped, "hunk")
            );
            return Vec::new();
        }
        let open = p.open();
        let states: Vec<HunkState> = p.hunks.iter().map(|h| h.state).collect();
        let mut effects = Vec::new();
        if self.screen != Screen::Unit {
            effects = self.open_unit(id.clone());
            if let Some(&i) = open.first() {
                self.show_fix(i);
            }
        }
        // What the key takes, and how what it would take stands.
        let (todo, seen) = if all {
            (open, states)
        } else {
            let Some(i) = self.current_fix() else {
                return effects;
            };
            let s = states[i];
            let todo = if s == HunkState::Proposed {
                vec![i]
            } else {
                Vec::new()
            };
            (todo, vec![s])
        };
        if todo.is_empty() {
            let what = if all {
                "Every proposed hunk is"
            } else {
                "This hunk is"
            };
            let finish = self.keys.label(Action::FinishOpen);
            let suggested = seen.contains(&HunkState::Suggested);
            let applied = seen.contains(&HunkState::Applied);
            self.status = match (applied, suggested) {
                (false, true) => {
                    format!("{what} suggested already: it goes into the review on {finish}.")
                }
                (true, false) => format!("{what} put in already."),
                _ => format!("{what} put in or suggested already."),
            };
            return effects;
        }
        if self.review.pr.is_some() {
            effects.extend(self.suggest_hunks(&id, &todo));
            return effects;
        }
        self.overlay = Overlay::ConfirmApply { id, hunks: todo };
        effects
    }

    /// The question before hunks `hunks` of unit `id` are written: "Put
    /// this hunk into src/x.php? (+3 −1)", "Put 4 hunks into 2 files?".
    pub fn apply_question(&self, id: &str, hunks: &[usize]) -> String {
        let Some(p) = self.review.unit(id).and_then(proposal) else {
            return String::new();
        };
        let picked: Vec<&ProposedHunk> = hunks.iter().filter_map(|&i| p.hunks.get(i)).collect();
        let files: BTreeSet<&std::path::Path> = picked.iter().map(|h| h.file.as_path()).collect();
        let (added, removed) = picked
            .iter()
            .map(|h| counts(h))
            .fold((0, 0), |(a, r), (x, y)| (a + x, r + y));
        let file = |f: &std::path::Path| crate::util::sanitize(&f.display().to_string());
        let mut q = match (picked.as_slice(), files.iter().next()) {
            ([h], _) => format!(
                "Put this hunk into {}? (+{added} −{removed})",
                file(&h.file)
            ),
            (_, Some(f)) if files.len() == 1 => format!(
                "Put {} into {}? (+{added} −{removed})",
                plural(picked.len(), "hunk"),
                file(f)
            ),
            _ => format!(
                "Put {} into {}?",
                plural(picked.len(), "hunk"),
                plural(files.len(), "file")
            ),
        };
        let broken = picked.iter().filter(|h| !h.parses).count();
        match (broken, picked.len()) {
            (0, _) => {}
            (1, 1) => q.push_str(" It doesn't parse."),
            (n, _) => q.push_str(&format!(" {} of them don't parse.", n)),
        }
        q
    }

    /// The answer to [`Overlay::ConfirmApply`]: yes writes hunks `hunks`
    /// of unit `id` to the working tree.
    pub(crate) fn apply_answer(&mut self, id: String, hunks: Vec<usize>, yes: bool) -> Vec<Effect> {
        if !yes {
            return Vec::new();
        }
        self.status = match hunks.len() {
            1 => "Putting the hunk in…".to_string(),
            n => format!("Putting {n} hunks in…"),
        };
        vec![Effect::ApplyHunks { unit_id: id, hunks }]
    }

    /// On a pull request nothing is written: hunks `hunks` of unit `id`
    /// become suggestions for the review, and the unit is flagged when it
    /// was pending (a step `u` takes back).
    fn suggest_hunks(&mut self, id: &str, hunks: &[usize]) -> Vec<Effect> {
        if let Some(p) = self.review.proposal_mut(id) {
            for &i in hunks {
                if let Some(h) = p.hunks.get_mut(i) {
                    h.state = HunkState::Suggested;
                }
            }
        }
        let pending = self
            .review
            .unit(id)
            .is_some_and(|u| u.decision == Decision::Pending);
        if pending {
            self.decide_ids(&[id.to_string()], Decision::Investigate);
        }
        self.status = format!(
            "Suggested: it goes into the review on {}.",
            self.keys.label(Action::FinishOpen)
        );
        vec![Effect::Save]
    }

    /// Hunks `hunks` of unit `id` were written (or not): say so, mark them
    /// applied, and review again.
    pub(crate) fn fix_applied(
        &mut self,
        id: &str,
        hunks: &[usize],
        result: Result<Vec<PathBuf>, String>,
    ) -> Vec<Effect> {
        match result {
            Ok(files) => {
                if let Some(p) = self.review.proposal_mut(id) {
                    for &i in hunks {
                        if let Some(h) = p.hunks.get_mut(i) {
                            h.state = HunkState::Applied;
                        }
                    }
                }
                self.status = format!("Put into {}; reviewing again.", plural(files.len(), "file"));
                // The re-run's status says what the change did.
                self.fix_by = Some(super::app::ChangedBy::Proposal);
                vec![Effect::Save, Effect::Rerun]
            }
            Err(e) => {
                self.status = format!("Not put in: {e}");
                Vec::new()
            }
        }
    }

    /// The proposed hunk of the unit on show that `p` puts in, by index:
    /// see [`App::fix_at`], with its boxes where they were last drawn.
    pub fn current_fix(&self) -> Option<usize> {
        let rows = self.unit.fixes.borrow().clone();
        self.fix_at(&rows)
    }

    /// The proposed hunk of the unit on show that is current when its
    /// boxes start at `rows` of the text (by hunk): the one a jump (or a
    /// click) went to; else the one whose box is at or nearest the top of
    /// the text; with no rows to go by, the first one not put in.
    pub fn fix_at(&self, rows: &[usize]) -> Option<usize> {
        let p = self.review.unit(&self.unit.id).and_then(proposal)?;
        let n = p.hunks.len();
        if let Some(i) = self.unit.fix.filter(|&i| i < n) {
            return Some(i);
        }
        if n > 0 && rows.len() == n {
            let at = self.detail_offset();
            return (0..n).min_by_key(|&i| rows[i].abs_diff(at));
        }
        p.open().first().copied().or((n > 0).then_some(0))
    }

    /// A click on proposed hunk `i` of the unit on show (the window's
    /// buttons on it): it becomes the current one.
    pub fn select_fix(&mut self, i: usize) {
        let n = self
            .review
            .unit(&self.unit.id)
            .and_then(proposal)
            .map_or(0, |p| p.hunks.len());
        if i < n {
            self.unit.fix = Some(i);
        }
    }
}
