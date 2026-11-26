//! Improve mode's list (see [`crate::mode::Mode::Improve`]): the inserts
//! the agent proposes for the units, listed in place of the units. Each
//! insert is a hunk of a unit's `Improve` proposal (see [`crate::fix`]),
//! in a box by what it does for the change ([`InsertKind`]: fills a gap,
//! makes it stronger, polish); in a box, by the queue's order of the
//! units, then by line.
//!
//! Nothing is written while the reviewer goes through the list: the
//! cursor moves over inserts, `a` (or `p`) marks the one under it to add
//! ([`HunkState::Picked`]) and goes on to the next one still proposed, `P`
//! marks every open insert of its unit, `x` leaves it out; either mark is
//! taken back with the same key. `:w` then puts every insert to add into
//! the working tree at once ([`App::put_inserts`]), and asks whether to
//! check the change again: it never re-runs on its own. On a pull request
//! the inserts to add go as suggestions with the review posted. `f` flags
//! the insert's unit, so its inserts to add go to your agent with the
//! notes; `l` looks closer at it in the unit view; `i` writes a note on
//! its unit.
//!
//! The inserts are asked for in this mode only: when it is on (at start,
//! or with `<space>m`), every listed unit that needs an agent and is
//! described is asked once, in the queue's order; a unit described later
//! is asked then. A unit with nothing to add is counted under the boxes.
//! Another mode hides the list and keeps the inserts.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use super::app::{App, ChangedBy, Effect, InsertResult, Overlay, Screen};
use crate::fix::{HunkState, InsertKind, Proposal, ProposedHunk};
use crate::keymap::Action;
use crate::review::model::{Decision, DiveKind, Prep, Unit};

/// One insert in the list: its unit (by index into `review.units`), its
/// hunk (by index into the unit's `Improve` proposal), and its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Insert {
    pub unit: usize,
    pub hunk: usize,
    pub kind: InsertKind,
}

/// How many inserts stand in each state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InsertCounts {
    pub proposed: usize,
    pub to_add: usize,
    pub put_in: usize,
    pub suggested: usize,
    pub left_out: usize,
}

/// The inserts of `u`, once the agent answered: none yet, or a proposal
/// with no hunk (nothing to add).
pub fn inserts_of(u: &Unit) -> Option<&Proposal> {
    super::fix::proposal_of(u, DiveKind::Improve)
}

/// A hunk's kind as the list files it: an insert the agent gave no kind
/// is polish.
pub fn kind_of(h: &ProposedHunk) -> InsertKind {
    h.kind.unwrap_or(InsertKind::Polish)
}

/// `1 insert`, `3 inserts`.
fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

impl App {
    /// The inserts in the list's order: by kind, then by the queue's order
    /// of their units (a unit not listed last), then by hunk.
    pub fn inserts(&self) -> Vec<Insert> {
        let rank: HashMap<usize, usize> = self
            .order
            .iter()
            .enumerate()
            .map(|(p, &i)| (i, p))
            .collect();
        let mut keyed: Vec<(InsertKind, usize, usize, usize)> = Vec::new();
        for (ui, u) in self.review.units.iter().enumerate() {
            let Some(p) = inserts_of(u) else {
                continue;
            };
            let at = rank.get(&ui).copied().unwrap_or(usize::MAX);
            for (hi, h) in p.hunks.iter().enumerate() {
                keyed.push((kind_of(h), at, ui, hi));
            }
        }
        keyed.sort();
        keyed
            .into_iter()
            .map(|(kind, _, unit, hunk)| Insert { unit, hunk, kind })
            .collect()
    }

    /// The insert under the cursor, when there is one.
    pub fn insert_at(&self) -> Option<Insert> {
        let all = self.inserts();
        all.get(self.isel.min(all.len().saturating_sub(1))).copied()
    }

    /// The unit of the insert under the cursor.
    pub fn insert_unit(&self) -> Option<&Unit> {
        self.insert_at().map(|i| &self.review.units[i.unit])
    }

    /// Insert `i`'s hunk, as its proposal has it.
    pub fn insert_hunk(&self, i: Insert) -> Option<&ProposedHunk> {
        inserts_of(&self.review.units[i.unit])?.hunks.get(i.hunk)
    }

    /// The insert a key acts on, by unit id and hunk: in the unit view the
    /// current hunk of the unit on show, else the one under the list's
    /// cursor.
    fn insert_target(&self) -> Option<(String, usize)> {
        if self.screen == Screen::Unit {
            let hunk = self.current_fix()?;
            return Some((self.unit.id.clone(), hunk));
        }
        let i = self.insert_at()?;
        Some((self.review.units[i.unit].id.clone(), i.hunk))
    }

    /// The listed units an agent is asked about, by index.
    fn askable(&self) -> Vec<usize> {
        self.order
            .iter()
            .copied()
            .filter(|&i| self.review.units[i].needs_agent())
            .collect()
    }

    /// How many units the agent answered about (inserts, nothing to add,
    /// or a failure), and how many it is asked about in all.
    pub fn improve_progress(&self) -> (usize, usize) {
        let asked = self.askable();
        let done = asked
            .iter()
            .filter(|&&i| {
                let u = &self.review.units[i];
                u.dive.results.contains_key(&DiveKind::Improve)
                    || self.improve_failed.contains(&u.id)
            })
            .count();
        (done, asked.len())
    }

    /// How many units the agent is looking at now.
    pub fn improve_pending(&self) -> usize {
        self.review
            .units
            .iter()
            .filter(|u| u.dive_pending.contains(&DiveKind::Improve))
            .count()
    }

    /// How many units the agent answered with nothing to add.
    pub fn nothing_to_add(&self) -> usize {
        self.review
            .units
            .iter()
            .filter(|u| inserts_of(u).is_some_and(|p| p.hunks.is_empty()))
            .count()
    }

    /// The inserts by state.
    pub fn insert_counts(&self) -> InsertCounts {
        let mut n = InsertCounts::default();
        for i in self.inserts() {
            match self.insert_hunk(i).map(|h| h.state) {
                Some(HunkState::Proposed) => n.proposed += 1,
                Some(HunkState::Picked) => n.to_add += 1,
                Some(HunkState::Applied) => n.put_in += 1,
                Some(HunkState::Suggested) => n.suggested += 1,
                Some(HunkState::LeftOut) => n.left_out += 1,
                None => {}
            }
        }
        n
    }

    /// The inserts marked to add, by unit in the queue's order, each
    /// unit's hunks in order: what `:w` puts in.
    pub fn picks(&self) -> Vec<(String, Vec<usize>)> {
        let mut out: Vec<(String, Vec<usize>)> = Vec::new();
        let mut by_unit: Vec<Insert> = self.inserts();
        by_unit.sort_by_key(|i| {
            (
                self.position_of(&self.review.units[i.unit].id)
                    .unwrap_or(usize::MAX),
                i.unit,
                i.hunk,
            )
        });
        for i in by_unit {
            if !self
                .insert_hunk(i)
                .is_some_and(|h| h.state == HunkState::Picked)
            {
                continue;
            }
            let id = &self.review.units[i.unit].id;
            match out.last_mut() {
                Some((last, hunks)) if last == id => hunks.push(i.hunk),
                _ => out.push((id.clone(), vec![i.hunk])),
            }
        }
        out
    }

    /// How many inserts are marked to add.
    pub fn picked_count(&self) -> usize {
        self.insert_counts().to_add
    }

    /// Ask the agent what to add to every unit not asked yet, in the
    /// queue's order: in improve mode with an agent, the listed units that
    /// need one and are described (one still being described is asked
    /// once it is). Each once; a failed question isn't asked again.
    pub(crate) fn ask_inserts(&mut self) -> Vec<Effect> {
        if !self.mode.improves() || !self.can_ask() {
            return Vec::new();
        }
        let mut effects = Vec::new();
        for i in self.askable() {
            let u = &self.review.units[i];
            if matches!(u.prep, Prep::Pending | Prep::NotRequested)
                || self.improve_failed.contains(&u.id)
            {
                continue;
            }
            let id = u.id.clone();
            effects.extend(self.dive_on(&id, DiveKind::Improve));
        }
        effects
    }

    // ---- The cursor ------------------------------------------------------

    /// Put the cursor on insert `to` (the last one when there are fewer);
    /// the queue's cursor follows to its unit.
    pub fn select_insert(&mut self, to: usize) -> Vec<Effect> {
        let n = self.inserts().len();
        self.isel = to.min(n.saturating_sub(1));
        self.insert_sync();
        self.ensure_prepared()
    }

    /// The queue's cursor on the unit of the insert under the cursor, so
    /// everything that goes by the selected unit agrees.
    fn insert_sync(&mut self) {
        if let Some(i) = self.insert_at()
            && let Some(pos) = self.position_of(&self.review.units[i.unit].id.clone())
        {
            self.cursor_to(pos);
        }
    }

    /// The cursor on the first insert of the queue's selected unit, when it
    /// has one; else where it was, within the list.
    pub(crate) fn insert_follow_unit(&mut self) {
        let all = self.inserts();
        if let Some(&ui) = self.order.get(self.sel)
            && let Some(k) = all.iter().position(|i| i.unit == ui)
        {
            self.isel = k;
        } else {
            self.isel = self.isel.min(all.len().saturating_sub(1));
        }
    }

    fn move_insert(&mut self, by: isize) -> Vec<Effect> {
        let to = self.isel.saturating_add_signed(by);
        self.select_insert(to)
    }

    /// The next insert still proposed below `from`, by its place.
    fn next_open(&self, from: usize) -> Option<usize> {
        let all = self.inserts();
        (from + 1..all.len()).find(|&k| {
            self.insert_hunk(all[k])
                .is_some_and(|h| h.state == HunkState::Proposed)
        })
    }

    /// After a mark on the list: the cursor on to the next insert still
    /// proposed, when there is one below.
    fn advance_insert(&mut self) {
        if self.screen == Screen::Queue
            && let Some(k) = self.next_open(self.isel)
        {
            self.isel = k;
            self.insert_sync();
        }
    }

    /// `n` (`N`): the `times`-th next (previous) insert still proposed.
    fn jump_insert(&mut self, forward: bool, times: usize) -> Vec<Effect> {
        let all = self.inserts();
        let open = |k: &usize| {
            self.insert_hunk(all[*k])
                .is_some_and(|h| h.state == HunkState::Proposed)
        };
        let mut at = self.isel.min(all.len().saturating_sub(1));
        for _ in 0..times.max(1) {
            let next = if forward {
                (at + 1..all.len()).find(open)
            } else {
                (0..at).rev().find(open)
            };
            match next {
                Some(k) => at = k,
                None => break,
            }
        }
        if at == self.isel {
            self.status = if forward {
                "No insert proposed below."
            } else {
                "No insert proposed above."
            }
            .into();
            return Vec::new();
        }
        self.select_insert(at)
    }

    /// `]f` (`[f`): the first insert of the `times`-th next (previous)
    /// file.
    fn jump_insert_file(&mut self, forward: bool, times: usize) -> Vec<Effect> {
        let all = self.inserts();
        let file = |k: usize| &self.review.units[all[k].unit].file;
        let mut at = self.isel.min(all.len().saturating_sub(1));
        for _ in 0..times.max(1) {
            let next = if forward {
                (at + 1..all.len()).find(|&k| file(k) != file(at))
            } else {
                (0..at).rev().find(|&k| file(k) != file(at)).map(|k| {
                    // The first insert of that file.
                    (0..=k).find(|&j| file(j) == file(k)).unwrap_or(k)
                })
            };
            match next {
                Some(k) => at = k,
                None => break,
            }
        }
        self.select_insert(at)
    }

    // ---- The keys --------------------------------------------------------

    /// The list's keys: moves over the inserts, `a` marks the insert to
    /// add, `f` flags its unit, `l` looks closer. `None` for what the
    /// queue handles as usual.
    pub(crate) fn improve_action(&mut self, a: Action, count: Option<u32>) -> Option<Vec<Effect>> {
        let times = count.unwrap_or(1) as usize;
        Some(match a {
            Action::MoveDown => self.move_insert(times as isize),
            Action::MoveUp => self.move_insert(-(times as isize)),
            Action::MoveFirst | Action::MoveLast if count.is_some() => {
                self.select_insert(times.saturating_sub(1))
            }
            Action::MoveFirst => self.select_insert(0),
            Action::MoveLast => self.select_insert(usize::MAX),
            Action::MoveNextPending => self.jump_insert(true, times),
            Action::MovePrevPending => self.jump_insert(false, times),
            Action::MoveNextFile => self.jump_insert_file(true, times),
            Action::MovePrevFile => self.jump_insert_file(false, times),
            Action::DecideAccept => self.pick_insert(false),
            Action::DecideFlag => self.flag_insert(),
            Action::ViewOpen => self.open_insert(),
            _ => return None,
        })
    }

    /// `a`, `p` (`P`: `all`): mark the insert to add, and on to the next
    /// one still proposed; on one to add, take it back. `all`: every open
    /// insert of its unit. Nothing is written: `:w` puts them in.
    pub(crate) fn pick_insert(&mut self, all: bool) -> Vec<Effect> {
        let Some((id, hunk)) = self.insert_target() else {
            self.status = "No insert under the cursor yet.".into();
            return Vec::new();
        };
        let finish = self.keys.label(Action::FinishOpen);
        let Some(p) = self.review.proposal_mut(&id, DiveKind::Improve) else {
            self.status = "No insert here.".into();
            return Vec::new();
        };
        if all {
            let open: Vec<usize> = (0..p.hunks.len())
                .filter(|&i| matches!(p.hunks[i].state, HunkState::Proposed | HunkState::LeftOut))
                .collect();
            for &i in &open {
                p.hunks[i].state = HunkState::Picked;
            }
            let n = p
                .hunks
                .iter()
                .filter(|h| h.state == HunkState::Picked)
                .count();
            self.status = if open.is_empty() {
                "Every insert of this unit is to add, put in or suggested already.".into()
            } else {
                format!(
                    "{} of this unit to add: {finish} puts them in.",
                    plural(n, "insert")
                )
            };
            self.advance_insert();
            return vec![Effect::Save];
        }
        let Some(h) = p.hunks.get_mut(hunk) else {
            return Vec::new();
        };
        let picked = match h.state {
            HunkState::Proposed | HunkState::LeftOut => {
                h.state = HunkState::Picked;
                true
            }
            HunkState::Picked => {
                h.state = HunkState::Proposed;
                self.status = "Taken back: proposed again.".into();
                return vec![Effect::Save];
            }
            HunkState::Applied => {
                self.status = "This insert is put in already.".into();
                return Vec::new();
            }
            HunkState::Suggested => {
                self.status = format!(
                    "This insert is suggested already: it goes into the review on {finish}."
                );
                return Vec::new();
            }
        };
        debug_assert!(picked);
        let n = self.picked_count();
        self.status = format!("To add: {finish} puts {} in.", plural(n, "insert"));
        self.advance_insert();
        vec![Effect::Save]
    }

    /// `x`: leave the insert out (under the cursor on the list; the current
    /// hunk in the unit view), and on to the next one still proposed; on
    /// one left out, take it back.
    pub(crate) fn leave_insert(&mut self) -> Vec<Effect> {
        let Some((id, hunk)) = self.insert_target() else {
            self.status = "No insert under the cursor yet.".into();
            return Vec::new();
        };
        let back = self.keys.label(Action::InsertLeave);
        let Some(p) = self.review.proposal_mut(&id, DiveKind::Improve) else {
            self.status = "No insert here.".into();
            return Vec::new();
        };
        let Some(h) = p.hunks.get_mut(hunk) else {
            return Vec::new();
        };
        match h.state {
            HunkState::Proposed | HunkState::Picked | HunkState::Suggested => {
                h.state = HunkState::LeftOut;
                self.status = format!("Left out: not put in, not sent. {back} takes it back.");
                self.advance_insert();
            }
            HunkState::LeftOut => {
                h.state = HunkState::Proposed;
                self.status = "Taken back: proposed again.".into();
            }
            HunkState::Applied => {
                self.status = "Put in already: nothing to leave out.".into();
                return Vec::new();
            }
        }
        vec![Effect::Save]
    }

    /// `f`: flag the insert's unit, so its inserts to add go to your agent
    /// with the notes (`:w`).
    fn flag_insert(&mut self) -> Vec<Effect> {
        let Some(u) = self.insert_unit() else {
            self.status = "No insert under the cursor yet.".into();
            return Vec::new();
        };
        let id = u.id.clone();
        let name = crate::util::sanitize(&u.symbol);
        let finish = self.keys.label(Action::FinishOpen);
        if u.decision == Decision::Investigate {
            self.status = format!(
                "{name} is flagged already: its inserts to add go to your agent with the notes \
({finish})."
            );
            return Vec::new();
        }
        self.decide_ids(&[id], Decision::Investigate);
        self.status = format!(
            "Flagged {name}: its inserts to add go to your agent with the notes ({finish})."
        );
        vec![Effect::Save]
    }

    /// `l`, `enter`: the unit view on the insert's unit, its text at the
    /// insert.
    fn open_insert(&mut self) -> Vec<Effect> {
        let Some(i) = self.insert_at() else {
            // Nothing listed yet: the unit the cursor was left on.
            return match self
                .order
                .get(self.sel)
                .map(|&ui| self.review.units[ui].id.clone())
            {
                Some(id) => self.open_unit(id),
                None => Vec::new(),
            };
        };
        let id = self.review.units[i.unit].id.clone();
        let effects = self.open_unit(id);
        self.show_fix(i.hunk);
        effects
    }

    // ---- `:w` --------------------------------------------------------------

    /// The sheet's "put N inserts in": write every insert to add to the
    /// working tree at once, unit by unit ([`Effect::ApplyInserts`]).
    pub(crate) fn put_inserts(&mut self) -> Vec<Effect> {
        let picks = self.picks();
        if picks.is_empty() {
            self.status = format!(
                "No insert to add: {} marks one.",
                self.keys.label(Action::DecideAccept)
            );
            return Vec::new();
        }
        let n: usize = picks.iter().map(|(_, h)| h.len()).sum();
        self.overlay = Overlay::None;
        self.status = format!("Putting {} in…", plural(n, "insert"));
        vec![Effect::ApplyInserts { picks }]
    }

    /// The inserts to add were written (or not), unit by unit: mark the
    /// written ones put in, say what was done, and ask whether to check
    /// the change again. Nothing re-runs on its own.
    pub(crate) fn inserts_applied(&mut self, results: Vec<InsertResult>) -> Vec<Effect> {
        let mut files: BTreeSet<PathBuf> = BTreeSet::new();
        let mut put = 0;
        let mut failed: Vec<String> = Vec::new();
        for (id, hunks, result) in results {
            match result {
                Ok(written) => {
                    if let Some(p) = self.review.proposal_mut(&id, DiveKind::Improve) {
                        for &i in &hunks {
                            if let Some(h) = p.hunks.get_mut(i) {
                                h.state = HunkState::Applied;
                                put += 1;
                            }
                        }
                    }
                    files.extend(written);
                }
                Err(e) => {
                    let name = self
                        .review
                        .unit(&id)
                        .map(|u| crate::util::sanitize(&u.symbol))
                        .unwrap_or(id);
                    failed.push(format!("{name}: {e}"));
                }
            }
        }
        let mut said = if put > 0 {
            format!(
                "Put {} into {}.",
                plural(put, "insert"),
                plural(files.len(), "file")
            )
        } else {
            "Nothing put in.".to_string()
        };
        if !failed.is_empty() {
            said.push_str(&format!(" Not put in: {}.", failed.join("; ")));
        }
        self.status = said.clone();
        if put == 0 {
            return Vec::new();
        }
        // The re-run's status says what the change did, if one is asked for.
        self.fix_by = Some(ChangedBy::Inserts);
        self.overlay = Overlay::ConfirmCheck {
            question: format!(
                "{said} Check the change again? The agent looks at what changed, and at what to \
add to it."
            ),
        };
        vec![Effect::Save]
    }
}
