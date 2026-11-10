//! The flagged list's state and what its keys do (`<space>f`): the flagged
//! units one after another, one of them selected. `j` `k` (and `n` `N`)
//! select the next and previous one and scroll it to the top, `]c` `[c`
//! go from hunk to hunk, the scroll keys scroll; a selected unit scrolled
//! out of view hands the selection to the one at the top. `l` or `enter`
//! looks closer (the unit view; `h` there comes back here), `a` accepts it
//! (it leaves the list), `i` writes its note, `h` goes back. How it looks
//! is the front end's. A unit with standing security findings, or facts
//! worth a look, says so on one line ([`security_line`]).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ops::Range;

use super::app::{App, Effect, Screen};
use crate::keymap::Action;
use crate::review::model::{Decision, Risk, Unit};
use crate::security::order::{class_name, facts_from, one_line, severity_of, standing};
use crate::security::{Class, Weight};

/// What a flagged unit's entry says about its security, on one line:
/// `▲ security: high · authorization: the asset id reaches the query
/// without the owner check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityLine {
    /// A finding's severity, or what a fact's weight stands for (a fact
    /// that needs a close look is high).
    pub severity: Risk,
    pub class: Class,
    /// The finding's title or the fact's sentence, on one line.
    pub text: String,
    /// It is the agent's finding, not a fact teer found.
    pub finding: bool,
    /// How many more standing findings and facts worth a look it has.
    pub more: usize,
}

impl SecurityLine {
    /// `security: high · authorization: the text (+1 more)`, without the
    /// level's glyph.
    pub fn line(&self) -> String {
        format!("security: {} · {}", self.severity.word(), self.about())
    }

    /// What it is about: `authorization: the text (+1 more)`.
    pub fn about(&self) -> String {
        let more = match self.more {
            0 => String::new(),
            n => format!(" (+{n} more)"),
        };
        format!("{}: {}{more}", class_name(self.class), self.text)
    }
}

/// The security line of `u`: its most severe standing finding, else its
/// heaviest fact worth a look or more. `None` without either.
pub fn security_line(u: &Unit) -> Option<SecurityLine> {
    let findings = standing(u);
    let facts = facts_from(u, Weight::Look);
    let more = (findings.len() + facts.len()).saturating_sub(1);
    let finding = findings.first().map(|f| SecurityLine {
        severity: f.severity,
        class: f.class,
        text: one_line(&f.title),
        finding: true,
        more,
    });
    let fact = facts.first().map(|f| SecurityLine {
        severity: severity_of(f.weight),
        class: f.class,
        text: one_line(&f.text),
        finding: false,
        more,
    });
    // A finding unless a fact is more severe.
    match (finding, fact) {
        (Some(a), Some(b)) => Some(if b.severity > a.severity { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// The flagged list's state.
#[derive(Debug, Default)]
pub struct FlaggedView {
    /// The screen `h` goes back to.
    pub from: Option<Screen>,
    /// The selected unit.
    pub at: String,
    /// How far the list is scrolled, in lines.
    pub offset: Cell<usize>,
    /// The next draw scrolls the selected unit to the top.
    pub reveal: Cell<bool>,
    /// Each box as last drawn: its unit and the lines it spans.
    pub boxes: RefCell<Vec<(String, Range<usize>)>>,
    /// Where each box and each hunk in it starts, in lines, as last drawn.
    pub hunks: RefCell<Vec<usize>>,
    /// How far the list can scroll and its rows, as last drawn.
    pub max: Cell<usize>,
    pub rows: Cell<usize>,
}

impl App {
    /// The units flagged (`f`), in the queue's order; any the queue
    /// doesn't list come after, in the review's.
    pub fn flagged_ids(&self) -> Vec<String> {
        let flagged = |u: &&Unit| u.decision == Decision::Investigate;
        let mut seen = HashSet::new();
        let mut ids: Vec<String> = self
            .order
            .iter()
            .map(|&i| &self.review.units[i])
            .filter(flagged)
            .map(|u| {
                seen.insert(u.id.clone());
                u.id.clone()
            })
            .collect();
        ids.extend(
            self.review
                .units
                .iter()
                .filter(flagged)
                .filter(|u| !seen.contains(&u.id))
                .map(|u| u.id.clone()),
        );
        ids
    }

    /// Where the selected unit is among `ids`: the unit [`FlaggedView::at`]
    /// names; once it left the list, the one that took its place (the
    /// next, or the last).
    pub fn flagged_index(&self, ids: &[String]) -> usize {
        if let Some(i) = ids.iter().position(|id| *id == self.flagged.at) {
            return i;
        }
        let was = self
            .flagged
            .boxes
            .borrow()
            .iter()
            .position(|(id, _)| *id == self.flagged.at);
        // The units before it that are still flagged.
        let before = match was {
            Some(w) => self.flagged.boxes.borrow()[..w]
                .iter()
                .filter(|(id, _)| ids.contains(id))
                .count(),
            None => 0,
        };
        before.min(ids.len().saturating_sub(1))
    }

    /// `<space>f`: the flagged list, the unit on show selected when it
    /// is flagged, else the first. Pressed in the list, back.
    pub(crate) fn open_flagged(&mut self) -> Vec<Effect> {
        if self.screen == Screen::Flagged {
            self.flagged_back();
            return Vec::new();
        }
        let ids = self.flagged_ids();
        let Some(first) = ids.first() else {
            self.status = format!(
                "Nothing is flagged yet: {} flags a unit.",
                self.keys.label(Action::DecideFlag)
            );
            return Vec::new();
        };
        let here = (self.current_unit().map(|u| u.id.clone())).filter(|id| ids.contains(id));
        self.flagged.at = here.unwrap_or_else(|| first.clone());
        self.flagged.from = Some(self.screen);
        self.flagged.reveal.set(true);
        self.flagged.boxes.borrow_mut().clear();
        self.screen = Screen::Flagged;
        Vec::new()
    }

    /// Back to the screen the list was opened from; in the queue, its
    /// cursor on the unit selected here.
    fn flagged_back(&mut self) {
        let to = self.flagged.from.take().unwrap_or(Screen::Queue);
        if to == Screen::Queue
            && let Some(p) = self.position_of(&self.flagged.at.clone())
        {
            self.sel = p;
        }
        self.screen = to;
    }

    /// Select the unit at `i` of `ids` and scroll it to the top.
    fn flagged_focus(&mut self, ids: &[String], i: usize) {
        if let Some(id) = ids.get(i) {
            self.flagged.at = id.clone();
            self.flagged.reveal.set(true);
        }
    }

    /// A click on the unit at `i`, as last drawn, selects it.
    pub fn flagged_click(&mut self, i: usize) {
        let id = self.flagged.boxes.borrow().get(i).map(|(id, _)| id.clone());
        if let Some(id) = id {
            self.flagged.at = id;
        }
    }

    /// The actions of the flagged list (after those every screen shares).
    pub(crate) fn flagged_action(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        let times = count.unwrap_or(1) as usize;
        let ids = self.flagged_ids();
        if ids.is_empty() {
            if matches!(a, Action::ViewBack | Action::ViewClose) {
                self.flagged_back();
            }
            return Vec::new();
        }
        let at = self.flagged_index(&ids);
        let id = ids[at].clone();
        match a {
            Action::MoveNextPending
            | Action::MovePrevPending
            | Action::MoveDown
            | Action::MoveUp => {
                let next = matches!(a, Action::MoveNextPending | Action::MoveDown);
                let to = if next {
                    (at + times).min(ids.len() - 1)
                } else {
                    at.saturating_sub(times)
                };
                if to == at {
                    self.status = "No more flagged units that way.".into();
                }
                self.flagged_focus(&ids, to);
            }
            Action::MoveNextHunk | Action::MovePrevHunk => {
                self.flagged_hunk(a == Action::MoveNextHunk, times);
            }
            Action::MoveFirst => self.flagged_focus(&ids, 0),
            Action::MoveLast => self.flagged_focus(&ids, ids.len() - 1),
            Action::ViewOpen => return self.open_unit(id),
            Action::ViewBack | Action::ViewClose => self.flagged_back(),
            Action::DecideAccept => {
                let mut effects = match self.position_of(&id) {
                    Some(p) => {
                        self.sel = p;
                        self.queue_action(a, None)
                    }
                    None => {
                        self.decide_ids(std::slice::from_ref(&id), Decision::Accepted);
                        vec![Effect::Save]
                    }
                };
                // The next one takes its place.
                let rest = self.flagged_ids();
                let next = ids[at + 1..].iter().find(|i| rest.contains(i));
                let next = next.or(rest.last()).cloned();
                if let Some(next) = next {
                    self.flagged.at = next;
                    self.flagged.reveal.set(true);
                }
                effects.extend(self.ensure_prepared());
                return effects;
            }
            Action::DecideAcceptFile | Action::DecideFlagFile => {
                self.status = "Whole files are decided in the queue or the unit view.".into();
            }
            _ => {}
        }
        Vec::new()
    }

    /// Scroll the list by `by` lines. A selected unit scrolled out of view
    /// hands the selection to the one at the top.
    pub fn flagged_scroll(&mut self, by: isize) -> Vec<Effect> {
        let to = self
            .flagged
            .offset
            .get()
            .saturating_add_signed(by)
            .min(self.flagged.max.get());
        self.flagged.offset.set(to);
        let rows = self.flagged.rows.get().max(1);
        let view = to..to + rows;
        let boxes = self.flagged.boxes.borrow();
        let seen = |r: &Range<usize>| r.start < view.end && view.start < r.end;
        let out_of_view = boxes
            .iter()
            .find(|(id, _)| *id == self.flagged.at)
            .is_none_or(|(_, r)| !seen(r));
        let top = boxes
            .iter()
            .rev()
            .find(|(_, r)| r.start <= to)
            .or(boxes.first())
            .map(|(id, _)| id.clone());
        drop(boxes);
        if out_of_view && let Some(top) = top {
            self.flagged.at = top;
        }
        Vec::new()
    }

    /// `]c` (`[c`): scroll to the `times`-th box or hunk below (above) the
    /// top, as far as there are.
    fn flagged_hunk(&mut self, forward: bool, times: usize) {
        let hunks = self.flagged.hunks.borrow().clone();
        let at = self.flagged.offset.get();
        let mut to = at;
        for _ in 0..times.max(1) {
            let next = if forward {
                hunks.iter().copied().find(|&h| h > to)
            } else {
                hunks.iter().rev().copied().find(|&h| h < to)
            };
            match next {
                Some(h) => to = h,
                None => break,
            }
        }
        let to = to.min(self.flagged.max.get());
        if to == at {
            self.status = if forward {
                "No more hunks below."
            } else {
                "No more hunks above."
            }
            .into();
            return;
        }
        self.flagged_scroll(to as isize - at as isize);
    }

    /// `u` or `ctrl+r` changed unit `first` first: when it is flagged, the
    /// selection on it.
    pub(crate) fn flagged_show_step(&mut self, first: &str) -> Vec<Effect> {
        let ids = self.flagged_ids();
        if let Some(i) = ids.iter().position(|id| id == first) {
            self.flagged_focus(&ids, i);
        }
        Vec::new()
    }
}
