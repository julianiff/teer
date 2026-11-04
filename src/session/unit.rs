//! The unit view's state and what its keys do (v2 Phase 5): any unit,
//! its text, and the tiles open beside it, tmux style: the advice (open as
//! the unit opens, with an agent; nothing is asked before `<space>a`), the
//! ask conversation (`K`), the tools (`<space>t` `<space>r` `<space>v`)
//! and what the unit calls outside the review (`gd`). `tab` and
//! `shift+tab` move the focus, `zz` zooms, `h` closes the tile in focus,
//! then goes back. Deciding here moves on to the next pending unit and
//! stays in the view. How it looks is the front end's.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use super::app::{App, Effect, JumpKind, Overlay, QUESTION_MAX, SCROLL_STEP, Screen, typed};
use super::{names, tools};
use crate::keymap::{Action, Edit, Key};
use crate::review::model::{Cite, Decision, DiveKind, Shown, Unit};

/// Most lines of one definition `gd` shows from outside the review.
const OUTSIDE_LINES: usize = 60;
/// Most definitions `gd` shows from outside the review.
const OUTSIDE_MOST: usize = 8;
/// Largest file `gd` reads a definition from.
const OUTSIDE_BYTES: usize = 512 * 1024;

/// A tile beside the unit's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum Panel {
    /// What the agent says to do about the unit: open as the unit opens.
    Recommend,
    /// `K`: the conversation about the unit and the question being typed.
    Ask,
    /// A leader tool's answer: the tests (`Tests`), old against new
    /// (`Behavior`) or the diagram (`Visualize`).
    Tool(DiveKind),
    /// `gd`: what the unit calls outside the review, read-only.
    Outside,
}

/// How far a tile is scrolled, how far it can, and its rows, as last drawn.
#[derive(Debug, Clone, Copy, Default)]
pub struct TileScroll {
    pub offset: usize,
    pub max: usize,
    pub rows: usize,
}

/// The unit view's state.
#[derive(Debug, Default)]
pub struct UnitView {
    /// The unit on show.
    pub id: String,
    /// The screen `h` goes back to.
    pub from: Option<Screen>,
    /// The unit whose row `h` puts the queue's cursor on: the one opened,
    /// or the one moving or deciding here went on to. A unit gone to to
    /// look (`gd`, `gr`, a link, `/`) leaves it.
    pub home: String,
    /// The tiles open beside the text, in the order they opened.
    pub tiles: Vec<Panel>,
    /// The tile in focus; `None` is the unit's text.
    pub panel: Option<Panel>,
    /// The tile in focus fills the view (`zz`).
    pub zoom: bool,
    /// Keys go into the ask tile's question; else, with a link in focus
    /// (`App::ask.link`), to the links ([`App::ask_links`]); else, with the
    /// ask tile in focus, to the view as on any tile.
    pub typing: bool,
    /// The definitions `gd` found outside the review.
    pub outside: Vec<Shown>,
    /// Where each hunk of the diff starts, in lines of the text, as last
    /// drawn.
    pub hunks: RefCell<Vec<usize>>,
    /// How far the text can scroll and its rows, as last drawn.
    pub text_max: Cell<usize>,
    pub text_rows: Cell<usize>,
    /// Each tile's scroll but the ask tile's (that is the conversation's,
    /// `App::ask.chat_scroll`).
    pub scroll: RefCell<HashMap<Panel, TileScroll>>,
    /// Asked this session whether questions may use the web (Q7).
    pub web_asked: bool,
    /// The question waiting for that answer, by index.
    pub web_question: Option<usize>,
}

impl UnitView {
    /// Where tile `p` is scrolled to.
    pub fn tile_offset(&self, p: Panel) -> usize {
        self.scroll.borrow().get(&p).map_or(0, |s| s.offset)
    }
}

impl App {
    /// Open the unit view on unit `id` (from the queue: `l`, `enter`; in
    /// the view, a unit gone to to look), with only the advice tile open.
    pub fn open_unit(&mut self, id: String) -> Vec<Effect> {
        if self.review.unit(&id).is_none() {
            return Vec::new();
        }
        let entering = self.screen != Screen::Unit;
        if entering {
            // From the flagged list opened from here: back there, then on
            // back to where this view was opened from.
            if self.screen == Screen::Flagged && self.flagged.from == Some(Screen::Unit) {
                self.flagged.from = self.unit.from;
            }
            self.unit.from = Some(self.screen);
            self.screen = Screen::Unit;
        }
        let home = std::mem::take(&mut self.unit.home);
        self.close_tiles();
        let effects = self.show_unit(id);
        if !entering {
            self.unit.home = home;
        }
        effects
    }

    /// The unit the view shows: the one whose note is being written while
    /// the note line is open (from the notes view it may be another), else
    /// the one on show.
    pub fn unit_on_show(&self) -> Option<&Unit> {
        if let Overlay::Note { id, .. } = &self.overlay
            && let Some(u) = self.review.unit(id)
        {
            return Some(u);
        }
        self.review.unit(&self.unit.id)
    }

    /// May a question use the web: the config allows it, it isn't a pull
    /// request, and the reviewer said yes this session.
    pub fn web_allowed(&self) -> bool {
        self.cfg.dive_web && self.review.pr.is_none() && self.dive_web_ok
    }

    /// The next question that could use the web asks first.
    pub fn web_may_ask(&self) -> bool {
        self.cfg.dive_web && self.review.pr.is_none() && !self.dive_web_ok && !self.unit.web_asked
    }

    /// Show unit `id`, the queue's cursor on it, the advice tile open (with
    /// an agent). Nothing is asked: the tile says which key asks.
    fn show_unit(&mut self, id: String) -> Vec<Effect> {
        if id != self.unit.id {
            self.close_tiles();
            self.unit.outside.clear();
        }
        if self.can_ask() && !self.unit.tiles.contains(&Panel::Recommend) {
            self.unit.tiles.insert(0, Panel::Recommend);
        }
        self.unit.id = id.clone();
        self.unit.home = id.clone();
        let mut effects = Vec::new();
        let pos = self.position_of(&id);
        if let Some(p) = pos
            && p != self.sel
        {
            effects.extend(self.move_sel(p));
        }
        effects
    }

    /// Close every tile, the focus back on the text; the conversation stays
    /// with the unit.
    fn close_tiles(&mut self) {
        self.unit.tiles.clear();
        self.unit.scroll.borrow_mut().clear();
        self.unit.zoom = false;
        self.focus_tile(None);
        self.ask.chat_scroll.set(None);
    }

    /// Give tile `p` the focus (`None`: the text); typing in the ask tile
    /// stops.
    fn focus_tile(&mut self, p: Option<Panel>) {
        self.unit.panel = p;
        self.unit.typing = false;
        self.ask.link = None;
    }

    /// Open tile `p` after the ones open, its scroll at the top, and give it
    /// the focus; open already, it only gets the focus. A zoom ends, so it
    /// shows.
    fn open_tile(&mut self, p: Panel) {
        if !self.unit.tiles.contains(&p) {
            self.unit.tiles.push(p);
            self.unit.scroll.borrow_mut().remove(&p);
            if p == Panel::Ask {
                self.ask.chat_scroll.set(None);
            }
        }
        self.unit.zoom = false;
        self.focus_tile(Some(p));
    }

    /// Close tile `p`; the focus goes to the tile opened before it, else
    /// to the text. The advice tile, open from the start, never
    /// gets it this way: `h` again goes on to the text, then back.
    fn close_tile(&mut self, p: Panel) {
        let Some(i) = self.unit.tiles.iter().position(|&t| t == p) else {
            return;
        };
        self.unit.tiles.remove(i);
        self.unit.scroll.borrow_mut().remove(&p);
        if self.unit.tiles.is_empty() {
            self.unit.zoom = false;
        }
        if self.unit.panel == Some(p) {
            let before = self.unit.tiles[..i]
                .iter()
                .rev()
                .copied()
                .find(|&t| t != Panel::Recommend);
            self.focus_tile(before);
        }
    }

    /// `tab` (`shift+tab`): the focus on the next (previous) tile, the text
    /// first, `times` over.
    fn tile_step(&mut self, forward: bool, times: usize) {
        let mut ring: Vec<Option<Panel>> = vec![None];
        ring.extend(self.unit.tiles.iter().copied().map(Some));
        let n = ring.len();
        if n == 1 {
            return;
        }
        let i = ring.iter().position(|&p| p == self.unit.panel).unwrap_or(0);
        let step = times.max(1) % n;
        let to = if forward {
            (i + step) % n
        } else {
            (i + n - step) % n
        };
        self.focus_tile(ring[to]);
    }

    /// `K`: the ask tile, typing, at the end of the conversation.
    pub(crate) fn open_ask_panel(&mut self) {
        self.open_tile(Panel::Ask);
        self.unit.typing = true;
        self.ask.chat_scroll.set(None);
    }

    /// Is the ask tile in focus, with nothing over it?
    pub(crate) fn unit_ask_focused(&self) -> bool {
        self.screen == Screen::Unit
            && self.overlay == Overlay::None
            && self.unit.panel == Some(Panel::Ask)
    }

    /// Whether the ask tile takes the keys: it is in focus, typing or on
    /// its links.
    pub(crate) fn unit_panel_has_keys(&self) -> bool {
        self.unit_ask_focused() && (self.unit.typing || self.ask.link.is_some())
    }

    /// Keys of the ask tile. Typing, a text input ([`Keymap::edit`]):
    /// letters go into the question, `enter` asks it, `ctrl+r` asks the
    /// last again, `tab` (`shift+tab`) moves to the first (last) link,
    /// `↑ ↓` and the page keys scroll, `esc` stops typing (the tile keeps
    /// the focus, and the view's keys work again). On the links see
    /// [`App::unit_link_key`].
    ///
    /// [`Keymap::edit`]: crate::keymap::Keymap::edit
    pub(crate) fn unit_panel_key(&mut self, k: Key) -> Vec<Effect> {
        if let Some(i) = self.ask.link {
            return self.unit_link_key(i, k);
        }
        let page = self.detail_rows.get().saturating_sub(2).max(1) as isize;
        match self.keys.edit(k) {
            Edit::Cancel => self.unit.typing = false,
            Edit::Submit => return self.unit_send(),
            Edit::Tab => {
                // The newest answer's first link.
                let links = self.ask_links();
                let newest = links.last().map(|(q, _)| *q);
                self.ask.link = links.iter().position(|(q, _)| Some(*q) == newest);
                self.unit.typing = self.ask.link.is_none();
            }
            Edit::BackTab => {
                self.ask.link = self.ask_links().len().checked_sub(1);
                self.unit.typing = self.ask.link.is_none();
            }
            Edit::Down => return self.chat_scroll(SCROLL_STEP),
            Edit::Up => return self.chat_scroll(-SCROLL_STEP),
            Edit::PageDown => return self.chat_scroll(page),
            Edit::PageUp => return self.chat_scroll(-page),
            Edit::Retry => {
                let last = self.chat_of(&self.unit.id).last().copied();
                if let Some(n) = last {
                    return self.ask_question(n);
                }
            }
            e => {
                typed(&mut self.ask.draft, e, QUESTION_MAX);
            }
        }
        Vec::new()
    }

    /// Keys on the ask tile's links (link `i` in focus), a panel
    /// ([`App::panel_action`]): `tab` and `j` move to the next, `shift+tab`
    /// and `k` to the one before (`tab` past the last, or `shift+tab`
    /// before the first, back to the question; `tab` and `shift+tab` are
    /// the question's own keys, [`Keymap::edit`]), `view.open` follows one
    /// (a unit opens here, code in the editor), `i` or `K` types again,
    /// `h`/`esc`/`q` leave the links (the tile keeps the focus), `gt` `gT`
    /// move to another tile and `zz` zooms; the scroll keys scroll.
    ///
    /// [`Keymap::edit`]: crate::keymap::Keymap::edit
    fn unit_link_key(&mut self, i: usize, k: Key) -> Vec<Effect> {
        let n = self.ask_links().len();
        let last = n.saturating_sub(1);
        let page = self.detail_rows.get().saturating_sub(2).max(1) as isize;
        let typing = |app: &mut App| {
            app.ask.link = None;
            app.unit.typing = true;
        };
        match self.keys.edit(k) {
            Edit::Tab if i + 1 < n => self.ask.link = Some(i + 1),
            Edit::BackTab if i > 0 => self.ask.link = Some(i - 1),
            Edit::Tab | Edit::BackTab => typing(self),
            _ => match self.panel_action(k) {
                Some(Action::MoveDown) => self.ask.link = Some((i + 1).min(last)),
                Some(Action::MoveUp) => self.ask.link = Some(i.saturating_sub(1)),
                Some(Action::NoteEdit | Action::AskOpen) => typing(self),
                Some(Action::MoveFirst) => self.ask.link = Some(0),
                Some(Action::MoveLast) => self.ask.link = Some(last),
                Some(Action::ViewOpen) => return self.follow_ask_link(i),
                Some(Action::ViewBack | Action::ViewClose) => self.ask.link = None,
                Some(a @ (Action::MoveNextTile | Action::MovePrevTile)) => {
                    self.tile_step(a == Action::MoveNextTile, 1);
                }
                Some(Action::ToggleZoom) => self.toggle_zoom(),
                Some(Action::ScrollPageDown | Action::ScrollHalfDown) => {
                    return self.chat_scroll(page);
                }
                Some(Action::ScrollPageUp | Action::ScrollHalfUp) => {
                    return self.chat_scroll(-page);
                }
                _ => {}
            },
        }
        Vec::new()
    }

    /// Ask what was typed, about the unit on show. The first question that
    /// could use the web asks first whether it may (once a session, Q7);
    /// the question waits for the answer.
    fn unit_send(&mut self) -> Vec<Effect> {
        let text = std::mem::take(&mut self.ask.draft);
        self.ask.chat_scroll.set(None);
        let id = self.unit.id.clone();
        if self.has_agent && self.web_may_ask() {
            let Some(index) = self.add_question(&text, Some(id)) else {
                return Vec::new();
            };
            self.unit.web_asked = true;
            self.unit.web_question = Some(index);
            self.overlay = Overlay::ConfirmWeb;
            return vec![Effect::Save];
        }
        match self.add_question(&text, Some(id)) {
            Some(index) => self.ask_question(index),
            None => Vec::new(),
        }
    }

    /// The answer to the web question, when a question of the ask panel
    /// waits for it: it is asked now, with the web or without.
    pub(crate) fn unit_answer_web(&mut self, yes: bool) -> Option<Vec<Effect>> {
        let index = self.unit.web_question.take()?;
        self.dive_web_ok = yes;
        if !yes {
            self.status = "No web search this session: the agent answers from the code.".into();
        }
        Some(self.ask_question(index))
    }

    /// The actions of the unit view (after those every screen shares).
    pub(crate) fn unit_action(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        let times = count.unwrap_or(1) as usize;
        match a {
            Action::DecideAccept
            | Action::DecideFlag
            | Action::DecideAcceptFile
            | Action::DecideFlagFile => return self.unit_decide(a, count),
            Action::MoveNextPending | Action::MovePrevPending => {
                let id = self.unit.id.clone();
                if let Some(p) = self.position_of(&id) {
                    self.sel = p;
                }
                let mut effects = self.jump_pending(a == Action::MoveNextPending, times);
                effects.extend(self.follow_sel());
                return effects;
            }
            Action::MoveNextFile | Action::MovePrevFile => {
                let id = self.unit.id.clone();
                if let Some(p) = self.position_of(&id) {
                    self.sel = p;
                }
                let mut effects = self.jump_file(a == Action::MoveNextFile, times);
                effects.extend(self.follow_sel());
                return effects;
            }
            Action::MoveNextHunk => self.jump_hunk(true, times),
            Action::MovePrevHunk => self.jump_hunk(false, times),
            Action::MoveDown => return self.unit_scroll(times as isize),
            Action::MoveUp => return self.unit_scroll(-(times as isize)),
            Action::MoveFirst => return self.unit_scroll(isize::MIN / 2),
            Action::MoveLast => return self.unit_scroll(isize::MAX / 2),
            Action::MoveNextTile | Action::MovePrevTile => {
                self.tile_step(a == Action::MoveNextTile, times);
            }
            Action::ToggleZoom => self.toggle_zoom(),
            // On the advice tile, the agent is asked.
            Action::ViewOpen if self.unit.panel == Some(Panel::Recommend) => {
                return self.ask_advice();
            }
            // On the Tests tile, the agent reads what teer found.
            Action::ViewOpen if self.unit.panel == Some(Panel::Tool(DiveKind::Tests)) => {
                return self.read_tests();
            }
            // A zoom ends first, then the tile in focus closes; on the text,
            // back to the queue.
            Action::ViewBack | Action::ViewClose if self.unit.zoom => self.unit.zoom = false,
            Action::ViewBack | Action::ViewClose => match self.unit.panel {
                Some(p) => self.close_tile(p),
                None => self.unit_back(),
            },
            _ => {}
        }
        Vec::new()
    }

    /// Back to the screen the view was opened from (the queue), its
    /// cursor on the row the view was opened on, or moved on to (see
    /// [`UnitView::home`]).
    fn unit_back(&mut self) {
        self.close_tiles();
        if let Some(p) = self.position_of(&self.unit.home.clone()) {
            self.sel = p;
        }
        self.screen = self.unit.from.take().unwrap_or(Screen::Queue);
    }

    /// The queue's cursor moved (`n`, `N`, `]f`, `[f`): show its unit.
    fn follow_sel(&mut self) -> Vec<Effect> {
        match self.selected_id() {
            Some(id) if id != self.unit.id => self.show_unit(id),
            _ => Vec::new(),
        }
    }

    /// Decide as the queue does (`a`, `f`, `A`, `F`, with a count), on the
    /// unit on show; then show the next pending unit. A unit the queue
    /// doesn't list is decided alone.
    fn unit_decide(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        let id = self.unit.id.clone();
        let mut effects = match self.position_of(&id) {
            Some(p) => {
                self.sel = p;
                self.queue_action(a, count)
            }
            None => {
                let flag = matches!(a, Action::DecideFlag | Action::DecideFlagFile);
                let d = if flag {
                    Decision::Investigate
                } else {
                    Decision::Accepted
                };
                self.decide_ids(std::slice::from_ref(&id), d);
                vec![Effect::Save]
            }
        };
        effects.extend(self.unit_follow(&id));
        effects
    }

    /// After deciding unit `was`: the unit the queue moved on to; when it
    /// stayed on a decided unit, the next pending one after it, else
    /// before it. With nothing pending the view stays.
    fn unit_follow(&mut self, was: &str) -> Vec<Effect> {
        if let Some(id) = self.selected_id()
            && id != was
            && self.position_of(was).is_some()
        {
            return self.show_unit(id);
        }
        let decided = self
            .review
            .unit(was)
            .is_some_and(|u| u.decision != Decision::Pending);
        if !decided {
            return Vec::new();
        }
        let from = self.position_of(was).unwrap_or(self.sel);
        let next = self
            .next_pending_pos(from)
            .or_else(|| self.prev_pending_pos(from));
        match next {
            Some(p) => {
                let id = self.review.units[self.order[p]].id.clone();
                self.show_unit(id)
            }
            None => Vec::new(),
        }
    }

    /// `u` or `ctrl+r` changed unit `first` first: show it.
    pub(crate) fn unit_show_step(&mut self, first: &str) -> Vec<Effect> {
        if self.review.unit(first).is_none() {
            return Vec::new();
        }
        self.show_unit(first.to_string())
    }

    /// Scroll with the keys: the tile in focus (the ask tile's
    /// conversation), else the text.
    pub(crate) fn unit_scroll(&mut self, by: isize) -> Vec<Effect> {
        self.unit_wheel(self.unit.panel, by)
    }

    /// Scroll tile `over` (the text with none) by `by`: the wheel scrolls
    /// what it is over, in focus or not.
    pub fn unit_wheel(&mut self, over: Option<Panel>, by: isize) -> Vec<Effect> {
        match over.filter(|p| self.unit.tiles.contains(p)) {
            Some(Panel::Ask) => return self.chat_scroll(by),
            Some(p) => self.scroll_tile(p, by),
            None => self.scroll_text(by),
        }
        Vec::new()
    }

    /// A click on tile `p` (`None`: the text) gives it the focus; on the
    /// ask tile, typing.
    pub fn unit_click_tile(&mut self, p: Option<Panel>) {
        if p.is_some_and(|p| !self.unit.tiles.contains(&p)) {
            return;
        }
        self.focus_tile(p);
        if p == Some(Panel::Ask) {
            self.unit.typing = true;
        }
    }

    /// A click on the unit view's way back: back to where it was opened
    /// from, whatever tile has the focus (`h` would close that first).
    pub fn unit_click_back(&mut self) {
        if self.screen == Screen::Unit {
            self.unit_back();
        }
    }

    /// A click on a tile's close button: tile `p` closes, and a zoom ends.
    pub fn unit_click_close(&mut self, p: Panel) {
        if self.unit.tiles.contains(&p) {
            self.unit.zoom = false;
            self.close_tile(p);
        }
    }

    /// `zz`: the tile in focus (or the text) fills the view, or no longer
    /// does. The keys tile is always beside the text, so there is always
    /// something to hide.
    fn toggle_zoom(&mut self) {
        self.unit.zoom = !self.unit.zoom;
    }

    fn scroll_text(&mut self, by: isize) {
        let to = self
            .detail_offset()
            .saturating_add_signed(by)
            .min(self.unit.text_max.get());
        self.detail_scroll.borrow_mut().1 = to;
    }

    fn scroll_tile(&mut self, p: Panel, by: isize) {
        let mut scroll = self.unit.scroll.borrow_mut();
        let s = scroll.entry(p).or_default();
        s.offset = s.offset.saturating_add_signed(by).min(s.max);
    }

    /// `]c` (`[c`): scroll the text to the `times`-th hunk below (above)
    /// the top, as far as there are.
    fn jump_hunk(&mut self, forward: bool, times: usize) {
        let hunks = self.unit.hunks.borrow().clone();
        if hunks.is_empty() {
            self.status = "No code change to jump in.".into();
            return;
        }
        let at = self.detail_offset();
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
        let to = to.min(self.unit.text_max.get());
        if self.unit.text_max.get() == 0 {
            self.status = "The whole change is on screen.".into();
            return;
        }
        if to == at {
            self.status = if forward {
                "No more hunks below."
            } else {
                "No more hunks above."
            }
            .into();
            return;
        }
        self.detail_scroll.borrow_mut().1 = to;
    }

    /// A leader tool (`<space>a` `<space>t` `<space>r` `<space>v`): its tile opens
    /// beside the ones open and its answer is asked for unless it has one.
    /// Open already, the same key gives it the focus, and in focus closes
    /// it. `<space>t` runs only teer's test search; the tile then asks
    /// whether the agent reads what it found ([`App::read_tests`]).
    pub(crate) fn unit_tool(&mut self, a: Action) -> Vec<Effect> {
        let id = self.unit.id.clone();
        if self.review.unit(&id).is_none() {
            return Vec::new();
        }
        if a == Action::ToolAdvice {
            return self.toggle_advice();
        }
        let Some(kind) = tools::tool_kind(a) else {
            return Vec::new();
        };
        let tile = Panel::Tool(kind);
        if self.unit.panel == Some(tile) {
            self.close_tile(tile);
            return Vec::new();
        }
        if self.unit.tiles.contains(&tile) {
            self.focus_tile(Some(tile));
            return Vec::new();
        }
        if kind != DiveKind::Tests && !self.can_ask() {
            let (name, _) = tools::light(kind);
            self.status = format!("No agent: {name} needs one (`teer setup` picks it).");
            return Vec::new();
        }
        self.open_tile(tile);
        self.dive_on(&id, kind)
    }

    /// `<space>a`: the advice tile, asked when it hasn't been. In focus
    /// with an answer (or one on its way), the same key closes it.
    fn toggle_advice(&mut self) -> Vec<Effect> {
        let tile = Panel::Recommend;
        if self.unit.panel == Some(tile)
            && self
                .review
                .unit(&self.unit.id)
                .is_some_and(|u| !tools::may_advise(self, u))
        {
            self.close_tile(tile);
            return Vec::new();
        }
        self.ask_advice()
    }

    /// Ask the agent for advice on the unit on show: what to do about it.
    /// The advice tile opens first, in focus; answered already or on its
    /// way, nothing more is asked.
    pub(crate) fn ask_advice(&mut self) -> Vec<Effect> {
        if !self.can_ask() {
            self.status = "No agent: advice needs one (`teer setup` picks it).".into();
            return Vec::new();
        }
        let tile = Panel::Recommend;
        if !self.unit.tiles.contains(&tile) {
            self.unit.tiles.insert(0, tile);
            self.unit.scroll.borrow_mut().remove(&tile);
        }
        self.unit.zoom = false;
        self.focus_tile(Some(tile));
        let id = self.unit.id.clone();
        self.dive_on(&id, DiveKind::Recommend)
    }

    /// The Tests tile's question answered yes (`enter` on it, or a click):
    /// the agent reads what teer's search found for the unit on show.
    pub fn read_tests(&mut self) -> Vec<Effect> {
        let tile = Panel::Tool(DiveKind::Tests);
        if !self.unit.tiles.contains(&tile) {
            return Vec::new();
        }
        self.focus_tile(Some(tile));
        let id = self.unit.id.clone();
        let Some(u) = self.review.unit(&id) else {
            return Vec::new();
        };
        if !tools::may_read_tests(self, u) {
            if !self.can_ask() {
                self.status =
                    "No agent: reading the tests needs one (`teer setup` picks it).".into();
            }
            return Vec::new();
        }
        self.interpret_tests_on(&id)
    }

    /// `gd` in the unit view: the jump list of what the unit calls in the
    /// review (Q5), and what it calls outside the review, read-only in a
    /// tile.
    pub(crate) fn unit_goto_definition(&mut self) -> Vec<Effect> {
        let id = self.unit.id.clone();
        let outside = self.outside_definitions(&id);
        let found = !outside.is_empty();
        if found {
            self.open_tile(Panel::Outside);
            self.unit.outside = outside;
        }
        let effects = self.open_jump(JumpKind::Definition { of: id.clone() });
        if found && !matches!(self.overlay, Overlay::Jump { .. }) {
            let symbol = self
                .review
                .unit(&id)
                .map(names::row_name)
                .unwrap_or_default();
            self.status = format!(
                "{symbol} calls nothing else in this review; what it calls outside is shown below."
            );
        }
        effects
    }

    /// What unit `id` calls outside the review, from the call graph: each
    /// definition's lines read from the repository (at most
    /// [`OUTSIDE_LINES`] of each, [`OUTSIDE_MOST`] of them).
    fn outside_definitions(&self, id: &str) -> Vec<Shown> {
        let g = &self.review.graph;
        let Some(from) = g.nodes.iter().position(|n| n.id == id) else {
            return Vec::new();
        };
        let root = &self.review.repo_root;
        let mut seen = HashSet::new();
        g.edges
            .iter()
            .filter(|(caller, _)| *caller == from)
            .filter_map(|&(_, to)| g.nodes.get(to))
            .filter(|n| !n.in_review && seen.insert(n.id.clone()))
            .filter_map(|n| {
                let text = crate::git::read_text_file(root, &n.file, OUTSIDE_BYTES)?;
                let start = n.range.start.max(1);
                let code: Vec<String> = text
                    .lines()
                    .skip(start as usize - 1)
                    .take((n.range.len() as usize).clamp(1, OUTSIDE_LINES))
                    .map(String::from)
                    .collect();
                (!code.is_empty()).then(|| Shown {
                    cite: Cite {
                        file: n.file.clone(),
                        start,
                        end: start + code.len() as u32 - 1,
                    },
                    why: crate::util::sanitize(&n.symbol),
                    code,
                    in_change: false,
                    changed: Vec::new(),
                })
            })
            .take(OUTSIDE_MOST)
            .collect()
    }
}
