//! App state and input handling. Handlers are synchronous and return
//! [`Effect`]s for the runtime to carry out, so all of this is testable
//! without a terminal or an agent.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use super::select;
use super::swipe::{Dir, DragTracker};
use super::theme::Theme;
use crate::agent::SessionInfo;
use crate::approve::{self, EditOutcome, Inline, NoteEntry, Verdict};
use crate::commit::{self, Branch};
use crate::config::{Action, Config, KeyMap, Order, Provider};
use crate::review::attention::{Assessment, Level, assess};
use crate::review::dive::{DiveEvent, uses_web};
use crate::review::model::{
    Answer, Decision, DiveKind, PrInfo, Prep, Question, Resolution, Review, SessionRef, Unit,
    UnitKind, UnitStatus,
};
use crate::review::prepare::PrepareEvent;
use crate::review::story;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Slim,
    Swipe,
    /// Every unit marked investigate, with the agent questions.
    Flagged,
}

/// What the list's level boxes hold (`f` switches).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grouping {
    /// Files, each in the box of the most attention one of its units
    /// needs, its units under its name.
    #[default]
    File,
    /// Units, each in the box of its own level, its file named on its row.
    Unit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    None,
    Help,
    /// Asks before clearing every decision.
    ConfirmReset,
    /// Single-line editor for the note on unit `id`; `back` is the notes
    /// view's selection to go back to when it was opened from there.
    Note {
        id: String,
        text: String,
        back: Option<usize>,
    },
    /// Every note of the review (see `crate::notes`) and the selected one:
    /// to copy, hand to Claude Code, or post on the PR.
    Notes {
        sel: usize,
    },
    /// Asks before deleting the note on unit `id`; then back to the notes at `sel`.
    ConfirmDeleteNote {
        id: String,
        sel: usize,
    },
    /// Asks once per session before a Flagged-screen question may use the web.
    ConfirmWeb,
    /// Asks before accepting every pending unit of a file in the list: the
    /// file and the ids that would change.
    ConfirmAcceptFile {
        file: PathBuf,
        ids: Vec<String>,
    },
    /// Shown once when the last unit that needed a look gets accepted.
    AllDone,
    /// Asks before posting a review of the PR on GitHub: the verdict, the
    /// summary comment and the inline comments that would be posted.
    PostReview {
        verdict: Verdict,
        /// The user's notes, in list order, with their inline targets.
        notes: Vec<NoteEntry>,
        /// The editable first part of the summary.
        intro: String,
        /// Post notes as inline comments (off: all go into the summary).
        inline_on: bool,
        /// First visible line of the scrolling text.
        scroll: usize,
        /// One-line editor for `intro`, when the external editor is not available.
        editing: Option<String>,
    },
    /// Asks before committing the reviewed files (plain `teer` review of
    /// uncommitted changes): the files, the message, the keys.
    Commit {
        /// The message that would be committed.
        message: String,
        draft: DraftState,
        /// First visible line of the scrolling text.
        scroll: usize,
        /// One-line editor for the subject, when the external editor is not available.
        editing: Option<String>,
        branch: Branch,
        /// Changed paths in the working tree that are not part of the review.
        others: usize,
    },
    /// The commits between base and head and the PR's description: the
    /// selected commit, and the selected one of the units it changed while
    /// the cursor is in that list.
    Commits {
        sel: usize,
        unit: Option<usize>,
    },
    /// The Ask view: questions about the code and their answers (see
    /// [`AskState`]).
    Ask,
}

/// The Ask view: the question being typed, the link in focus while the keys
/// move between links instead, the questions on their way, and the scroll.
/// The chat of a flagged unit (the Chat tile) keeps its own question and
/// scroll.
#[derive(Debug, Default)]
pub struct AskState {
    pub input: String,
    /// Index into [`App::ask_links`]; `None` while typing.
    pub link: Option<usize>,
    /// Questions, by index, the agent is answering.
    pub pending: BTreeSet<usize>,
    /// First line shown; `None` follows the end of the conversation.
    pub scroll: Cell<Option<usize>>,
    /// The last line that can be first, as last drawn.
    pub max_scroll: Cell<usize>,
    /// The question being typed in the chat.
    pub draft: String,
    /// The chat's first line shown; `None` follows its end.
    pub chat_scroll: Cell<Option<usize>>,
    /// The chat's last line that can be first, as last drawn.
    pub chat_max: Cell<usize>,
}

/// Where a link in the Ask view goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskLink {
    /// A unit of this change, in the list.
    Unit(String),
    /// Shown code, in the editor.
    Code { file: PathBuf, line: u32 },
}

/// Longest question, in characters.
pub const QUESTION_MAX: usize = 500;

/// Edit `text` with key `k` as a line being typed: a character goes in
/// while it is shorter than `max`, backspace takes the last off, `ctrl+u`
/// clears it. False for any other key.
fn typed(text: &mut String, k: &KeyEvent, max: usize) -> bool {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Backspace => {
            text.pop();
        }
        KeyCode::Char('u') if ctrl => text.clear(),
        KeyCode::Char(c)
            if !c.is_control()
                && !k
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            if text.chars().count() < max {
                text.push(c);
            }
        }
        _ => return false,
    }
    true
}

/// Where the commit message came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftState {
    /// The agent is drafting; the template is shown meanwhile.
    Pending,
    /// The agent's message is shown.
    Drafted,
    /// The template stays (no agent, or the draft failed).
    Template,
    /// The user has taken over the text; a late draft is dropped.
    Edited,
}

/// Longest note, in characters.
pub const NOTE_MAX: usize = 1000;

/// Cards swipe asks about ahead of the one on show, in a large review.
pub const SWIPE_AHEAD: usize = 8;

/// Lines the scroll keys and the mouse wheel move the detail area by.
pub const SCROLL_STEP: isize = 3;

/// The questions, in menu order.
pub const DIVE_KINDS: [DiveKind; 5] = [
    DiveKind::Recommend,
    DiveKind::Visualize,
    DiveKind::Tests,
    DiveKind::Behavior,
    DiveKind::Disagree,
];

/// The questions with a tile on the Flagged screen, in the order they were
/// asked. An answer from before that order was kept follows, in menu order;
/// the second opinion, which also runs on its own, only once asked for.
pub fn tile_kinds(u: &Unit) -> Vec<DiveKind> {
    let mut kinds: Vec<DiveKind> = Vec::new();
    for k in &u.dive.asked {
        if !kinds.contains(k) {
            kinds.push(*k);
        }
    }
    for k in DIVE_KINDS {
        let there = u.dive.results.contains_key(&k) || u.dive_pending.contains(&k);
        if k != DiveKind::Disagree && there && !kinds.contains(&k) {
            kinds.push(k);
        }
    }
    kinds
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Prepare(Vec<String>),
    Rerun,
    /// Open `file` (relative to the repo root) in the editor at `line`.
    Edit {
        file: PathBuf,
        line: u32,
    },
    Save,
    Quit,
    /// Ask the agent a question about a flagged unit.
    // Named for the question (`DiveKind`), not the screen that asks it.
    Dive {
        unit_id: String,
        kind: DiveKind,
        /// Whether the agent may search the web for this question.
        web: bool,
    },
    /// Have the agent read the tests teer's search found for a unit: which
    /// cover it and what dangerous cases none does (the Tests tile).
    InterpretTests {
        unit_id: String,
    },
    /// Ask the second model about these units again (the Disagreement tab).
    Second(Vec<String>),
    /// Open the unit's Visualize page in the browser (written again from the
    /// saved picture if the file is gone).
    OpenVisual {
        unit_id: String,
    },
    /// Post the review (see `approve::post`). `body` and `inline` are what
    /// the user confirmed; `flat_body` is the summary with every note listed,
    /// used if GitHub rejects the inline comments.
    PostReview {
        pr: Box<PrInfo>,
        verdict: Verdict,
        body: String,
        inline: Vec<Inline>,
        flat_body: String,
    },
    /// Edit the approval summary in the external editor (the runtime hands
    /// the result to [`App::comment_edited`]).
    EditComment {
        text: String,
    },
    /// Put `text` on the clipboard; `what` names it for the status line.
    Copy {
        text: String,
        what: String,
    },
    /// Hand the terminal to the agent CLI (Claude Code or Codex) in `cwd`
    /// with `prompt` as its first message, resuming a fork of Claude Code
    /// session `resume` when there is one.
    OpenAgent {
        cli: Provider,
        cwd: PathBuf,
        prompt: String,
        resume: Option<String>,
    },
    /// Ask the agent to draft a commit message (see `commit::draft_message`);
    /// the answer comes back as [`AppEvent::CommitDrafted`].
    DraftCommitMessage(Box<commit::DraftInput>),
    /// Stage `paths` and commit them with `message` (see `commit::run_commit`).
    /// The runtime hands the terminal to git while it runs and passes the
    /// outcome to [`App::commit_finished`].
    Commit {
        repo: PathBuf,
        paths: Vec<PathBuf>,
        message: String,
    },
    /// Ask for one or two sentences about the whole change; the answer comes
    /// back as [`AppEvent::Summary`].
    Summarize,
    /// Ask the agent question `index` of `review.questions`; the answer comes
    /// back as [`AppEvent::Answer`].
    Ask {
        index: usize,
    },
}

#[derive(Debug)]
pub enum AppEvent {
    /// An agent answer to a Flagged-screen question (see `DiveKind`).
    Dive(DiveEvent),
    Prep(PrepareEvent),
    Rerun(Result<Box<Review>, String>),
    /// Posting the review finished: the status line to show, or why it failed.
    PrReviewed {
        verdict: Verdict,
        result: Result<String, String>,
    },
    /// The agent's commit message, or why there is none.
    CommitDrafted(Result<String, String>),
    /// The summary of the whole change, or why there is none.
    Summary(Result<String, String>),
    /// The answer to question `index`, or why there is none.
    Answer {
        index: usize,
        result: Result<Answer, String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Undo {
    /// Each unit touched by one step and the decision it had before, in the
    /// order they were changed. Most steps hold one; a group accept holds all.
    pub entries: Vec<(String, Decision)>,
}

#[derive(Debug, Clone, Default)]
pub struct SwipeState {
    /// The cards of this session, in list order.
    pub cards: Vec<String>,
    /// The card on show; `cards.len()` means the summary.
    pub cursor: usize,
    pub history: Vec<Undo>,
    pub drag: DragTracker,
}

/// Flagged screen state: the selected flagged unit, the tile in focus,
/// whether the tile in focus fills the screen, and whether keys go into the
/// chat's question (the Chat tile in focus).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FlaggedState {
    pub sel: usize,
    pub pane: Pane,
    pub zoom: bool,
    pub typing: bool,
}

/// A tile of the Flagged screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pane {
    /// The code change, with the statements in it.
    #[default]
    Code,
    /// One question's answer.
    Answer(DiveKind),
    /// The conversation about the unit: the questions asked about it in
    /// the reviewer's words, the answers, and a line to ask the next.
    Chat,
    /// The questions not asked yet, to bring more light into it.
    More,
}

/// Where the scroll of unit `id`'s tile `pane` is kept.
pub fn pane_key(id: &str, pane: Pane) -> String {
    format!("{id} {pane:?}")
}

/// Clickable regions registered while drawing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Action(Action),
    Unit(usize),
    /// The detail area next to a list (the list preview, the Flagged unit):
    /// the wheel scrolls it, a click does nothing.
    Detail,
    /// A Flagged tile: a click gives it the focus, the wheel scrolls it.
    Tile(Pane),
    /// A link in the Ask view, by index into [`App::ask_links`].
    AskLink(usize),
    /// A key named on screen (`esc close`): a click presses it.
    Key(KeyEvent),
    /// A note in the notes view, by its place there: a click selects it.
    Note(usize),
}

pub struct App {
    pub review: Review,
    pub cfg: Config,
    pub keys: KeyMap,
    pub theme: Theme,
    pub screen: Screen,
    pub return_to: Vec<Screen>,
    pub overlay: Overlay,

    pub has_agent: bool,
    pub agent_note: Option<String>,
    pub sessions: Vec<SessionInfo>,

    /// Unit indexes in display order, file by file: files by the most
    /// attention one of their units needs (then by path), inside a file most
    /// attention first, then by story (see `review::story`) or, with
    /// `order = "line"`, by line. Listed by unit, units by their own level
    /// first, then the same.
    pub order: Vec<usize>,
    pub grouping: Grouping,
    /// How far each unit is indented under the units it is read beneath in
    /// its file and level, by index into `review.units` (0 in line order).
    pub depth: Vec<usize>,
    /// Attention level per unit, by index into `review.units`.
    pub levels: Vec<Assessment>,
    pub sel: usize,
    pub large: bool,

    pub swipe: SwipeState,
    pub flagged: FlaggedState,
    /// The user agreed to web use for Flagged-screen questions this session.
    pub dive_web_ok: bool,
    /// A web-using request waiting for the user's answer to `ConfirmWeb`.
    pub dive_web_pending: Option<(String, DiveKind)>,
    pub status: String,
    pub tick: u64,
    /// Draws Visualize pictures when the terminal can show images (set by
    /// the runtime after asking it); `None` keeps the text list.
    pub pictures: Option<super::picture::Pictures>,
    /// An approval is being sent.
    pub approving: bool,
    /// What a long question is doing now, by unit and kind (Behavior).
    pub progress: std::collections::HashMap<(String, DiveKind), String>,

    // Written while drawing.
    pub targets: RefCell<Vec<(Rect, Target)>>,
    /// Where a mouse selection may run (see [`super::select`]).
    pub text_areas: RefCell<Vec<super::select::Area>>,
    /// The screen as last drawn, to copy from.
    pub shown: RefCell<ratatui::buffer::Buffer>,
    /// Text marked with the mouse, being selected or just copied.
    pub selection: Option<super::select::Selection>,
    /// When and where the last click was, to tell a double click.
    pub last_click: Option<(std::time::Instant, u16, u16)>,
    pub slim_scroll: Cell<usize>,
    pub swipe_card: Cell<Rect>,
    /// Furthest the approval text can scroll (written while drawing).
    pub approve_max_scroll: Cell<usize>,
    /// Furthest the commit overlay can scroll (written while drawing).
    pub commit_max_scroll: Cell<usize>,
    /// How far the detail area (the list preview, the swipe card, the open
    /// tab on the Flagged screen) is scrolled, in lines, and what it showed
    /// then: another unit or tab starts at the top.
    pub detail_scroll: RefCell<(String, usize)>,
    /// Furthest the detail area can scroll, and its rows (written while drawing).
    pub detail_max_scroll: Cell<usize>,
    pub detail_rows: Cell<usize>,
    /// How far each Flagged tile is scrolled, by [`pane_key`].
    pub pane_scroll: RefCell<HashMap<String, usize>>,
    /// Syntax colours of the files drawn so far (see [`super::syntax`]).
    pub syntax: super::syntax::Cache,
    /// The short id of the commit made from this review in this session.
    pub committed: Option<String>,
    /// The summary of the whole change was asked for this session.
    pub summary_asked: bool,
    pub ask: AskState,
}

impl App {
    pub fn new(review: Review, cfg: Config, keys: KeyMap) -> App {
        let theme = Theme::new(cfg.theme);
        let mut app = App {
            review,
            keys,
            theme,
            screen: Screen::Slim,
            return_to: Vec::new(),
            overlay: Overlay::None,
            has_agent: false,
            agent_note: None,
            sessions: Vec::new(),
            order: Vec::new(),
            grouping: Grouping::File,
            depth: Vec::new(),
            levels: Vec::new(),
            sel: 0,
            // Set by `rebuild_order`, as the rest of the order.
            large: false,
            swipe: SwipeState::default(),
            flagged: FlaggedState::default(),
            dive_web_ok: false,
            dive_web_pending: None,
            status: String::new(),
            tick: 0,
            pictures: None,
            targets: RefCell::new(Vec::new()),
            text_areas: RefCell::new(Vec::new()),
            shown: RefCell::default(),
            selection: None,
            last_click: None,
            slim_scroll: Cell::new(0),
            swipe_card: Cell::new(Rect::default()),
            approving: false,
            progress: Default::default(),
            approve_max_scroll: Cell::new(0),
            commit_max_scroll: Cell::new(0),
            detail_scroll: RefCell::new((String::new(), 0)),
            detail_max_scroll: Cell::new(0),
            pane_scroll: RefCell::default(),
            detail_rows: Cell::new(0),
            syntax: Default::default(),
            committed: None,
            summary_asked: false,
            ask: AskState::default(),
            cfg,
        };
        app.rebuild_order();
        app
    }

    /// List units file by file: files by the most attention one of their
    /// units needs, then by path; inside a file most attention first, then
    /// by story (callers right before what they call) or line. Listed by
    /// unit, each unit goes by its own level instead of its file's. Units
    /// without a behavior change aren't listed as units. The selection stays
    /// on the same unit as levels change.
    pub fn rebuild_order(&mut self) {
        let keep = self.selected().map(|u| u.id.clone());
        self.levels = self
            .review
            .units
            .iter()
            .map(|u| assess(&self.review, u))
            .collect();
        let units = &self.review.units;
        let level = |i: usize| self.levels[i].level;
        let listed: Vec<usize> = (0..units.len())
            .filter(|&i| level(i) != Level::Same)
            .collect();
        self.large = units.len() > self.cfg.large_review;
        let mut most: HashMap<&PathBuf, Level> = HashMap::new();
        for &i in &listed {
            let m = most.entry(&units[i].file).or_insert(Level::Same);
            *m = (*m).max(level(i));
        }
        let placed = if self.cfg.order == Order::Story {
            story::place(units, &listed)
        } else {
            Default::default()
        };
        // The level of the box a unit is listed in.
        let by_file = self.grouping == Grouping::File;
        let boxed = |i: usize| {
            if by_file {
                most[&units[i].file]
            } else {
                level(i)
            }
        };
        let mut order = listed;
        order.sort_by(|&a, &b| {
            let (fa, fb) = (&units[a].file, &units[b].file);
            (boxed(b) as u8)
                .cmp(&(boxed(a) as u8))
                .then_with(|| fa.cmp(fb))
                .then_with(|| (level(b) as u8).cmp(&(level(a) as u8)))
                .then_with(|| match (placed.get(&a), placed.get(&b)) {
                    (Some(x), Some(y)) => x.rank.cmp(&y.rank),
                    _ => units[a].display_line().cmp(&units[b].display_line()),
                })
        });
        self.depth = (0..units.len())
            .map(|i| {
                story::depth(&placed, i, |p| {
                    units[p].file == units[i].file && level(p) == level(i)
                })
            })
            .collect();
        self.order = order;
        if let Some(id) = keep
            && let Some(pos) = self
                .order
                .iter()
                .position(|&i| self.review.units[i].id == id)
        {
            self.sel = pos;
        }
        self.sel = self.sel.min(self.order.len().saturating_sub(1));
    }

    // ---- Files -------------------------------------------------------------

    pub fn file_at(&self, pos: usize) -> &PathBuf {
        &self.review.units[self.order[pos]].file
    }

    /// Does a new file start at `pos`? Listed by unit, a file starts again
    /// in each level's box it has units in.
    pub fn starts_file(&self, pos: usize) -> bool {
        let level = |p: usize| self.levels[self.order[p]].level;
        pos == 0
            || self.file_at(pos - 1) != self.file_at(pos)
            || (self.grouping == Grouping::Unit && level(pos - 1) != level(pos))
    }

    /// Position of the first unit of the file `pos` is in.
    pub fn file_start(&self, pos: usize) -> usize {
        (0..=pos).rev().find(|&p| self.starts_file(p)).unwrap_or(0)
    }

    /// The positions of the file `pos` is in.
    pub fn file_range(&self, pos: usize) -> std::ops::Range<usize> {
        let end = (pos + 1..self.order.len())
            .find(|&p| self.starts_file(p))
            .unwrap_or(self.order.len());
        self.file_start(pos)..end
    }

    /// The most attention a unit of the file `pos` is in needs: the box the
    /// file is listed in (listed by unit, the level of the box's units).
    pub fn file_level(&self, pos: usize) -> Level {
        self.file_range(pos)
            .map(|p| self.levels[self.order[p]].level)
            .max()
            .unwrap_or(Level::Routine)
    }

    /// List by unit, or by file again; the cursor stays on its unit.
    fn switch_grouping(&mut self) {
        let (to, word) = match self.grouping {
            Grouping::File => (Grouping::Unit, "unit"),
            Grouping::Unit => (Grouping::File, "file"),
        };
        self.grouping = to;
        self.rebuild_order();
        self.status = format!("listed by {word}");
    }

    /// Move `step` units up or down the list.
    fn move_row(&mut self, step: isize) -> Vec<Effect> {
        let Some(last) = self.order.len().checked_sub(1) else {
            return Vec::new();
        };
        let to = (self.sel as isize)
            .saturating_add(step)
            .clamp(0, last as isize);
        self.move_sel(to as usize)
    }

    pub fn level_of(&self, unit_index: usize) -> &Assessment {
        &self.levels[unit_index]
    }

    /// Position of a unit in the review by id, for looking up its level.
    pub fn assessment(&self, id: &str) -> Option<&Assessment> {
        self.review
            .units
            .iter()
            .position(|u| u.id == id)
            .map(|i| &self.levels[i])
    }

    // ---- Startup ---------------------------------------------------------

    /// Start on the review. `session` is the agent to ask: a fresh headless
    /// run, or the session given with `--session`. `note` explains why there
    /// is no agent, if there isn't one.
    pub fn start(
        &mut self,
        has_agent: bool,
        note: Option<String>,
        sessions: Vec<SessionInfo>,
        session: Option<SessionRef>,
        swipe: bool,
    ) -> Vec<Effect> {
        self.has_agent = has_agent;
        self.sessions = sessions;
        self.agent_note = note;
        let mut effects = Vec::new();
        self.review.set_session(session.filter(|_| self.has_agent));
        self.screen = Screen::Slim;
        if swipe {
            self.enter_swipe();
        }
        effects.extend(self.prepare_start());
        effects.extend(self.prepare_cards());
        // A review described in an earlier run that has no summary yet.
        if self.can_ask() && crate::review::intent::wants_summary(&self.review) {
            self.summary_asked = true;
            effects.push(Effect::Summarize);
        }
        // The one time the popup shows on load: a finished review of
        // uncommitted changes can be committed right away.
        if self.can_commit() && !self.review.files.is_empty() && self.all_reviewed() {
            self.overlay = Overlay::AllDone;
        }
        effects
    }

    pub fn can_ask(&self) -> bool {
        self.has_agent && self.review.agent_session.is_some()
    }

    fn prepare_effect(&mut self, ids: Option<Vec<String>>) -> Vec<Effect> {
        if !self.can_ask() {
            return Vec::new();
        }
        let todo: Vec<String> = self
            .review
            .to_prepare(ids.as_deref())
            .into_iter()
            .map(|(u, _)| u.id)
            .collect();
        if todo.is_empty() {
            return Vec::new();
        }
        for id in &todo {
            if let Some(u) = self.review.unit_mut(id) {
                u.prep = Prep::Pending;
            }
        }
        vec![Effect::Prepare(todo)]
    }

    /// What to prepare when the review opens: everything, or for a large
    /// review the units that need a close look and the selected unit's file.
    fn prepare_start(&mut self) -> Vec<Effect> {
        if !self.can_ask() {
            return Vec::new();
        }
        if !self.large {
            return self.prepare_effect(None);
        }
        let close: Vec<String> = self
            .order
            .iter()
            .filter(|&&i| self.levels[i].level == Level::Attention)
            .map(|&i| self.review.units[i].id.clone())
            .collect();
        let mut effects = self.prepare_effect(Some(close));
        effects.extend(self.ensure_prepared());
        effects
    }

    /// Swipe in a large review: ask about the card on show and the next
    /// [`SWIPE_AHEAD`] that nobody asked about yet, so each is described by
    /// the time it comes up. (The list asks per file as the cursor moves.)
    fn prepare_cards(&mut self) -> Vec<Effect> {
        if !self.large || self.screen != Screen::Swipe {
            return Vec::new();
        }
        let ids: Vec<String> = self
            .swipe
            .cards
            .iter()
            .skip(self.swipe.cursor)
            .take(SWIPE_AHEAD + 1)
            .filter(|id| {
                self.review
                    .unit(id)
                    .is_some_and(|u| u.prep == Prep::NotRequested)
            })
            .cloned()
            .collect();
        if ids.is_empty() {
            return Vec::new();
        }
        self.prepare_effect(Some(ids))
    }

    /// Large reviews prepare a file's units when the user gets to them.
    fn ensure_prepared(&mut self) -> Vec<Effect> {
        if !self.large {
            return Vec::new();
        }
        let Some(u) = self.selected() else {
            return Vec::new();
        };
        let file = u.file.clone();
        // Units asked about already (the close looks) aren't asked twice.
        let ids: Vec<String> = self
            .review
            .units
            .iter()
            .filter(|u| u.file == file && u.prep == Prep::NotRequested)
            .map(|u| u.id.clone())
            .collect();
        if ids.is_empty() {
            return Vec::new();
        }
        self.prepare_effect(Some(ids))
    }

    // ---- Queries ---------------------------------------------------------

    /// The selected unit; `None` when nothing is listed.
    pub fn selected(&self) -> Option<&Unit> {
        self.order.get(self.sel).map(|&i| &self.review.units[i])
    }

    pub fn selected_id(&self) -> Option<String> {
        self.selected().map(|u| u.id.clone())
    }

    pub fn prep_progress(&self) -> (usize, usize) {
        let asked: Vec<&Unit> = self
            .review
            .units
            .iter()
            .filter(|u| matches!(u.prep, Prep::Pending | Prep::Ready | Prep::Unavailable))
            .collect();
        let done = asked.iter().filter(|u| u.prep != Prep::Pending).count();
        (done, asked.len())
    }

    /// Units flagged for a closer look, in display order.
    pub fn flagged_units(&self) -> Vec<&Unit> {
        self.review.flagged()
    }

    /// (accepted, flagged, pending), counting only units with a behavior change.
    pub fn decision_counts(&self) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for u in self
            .review
            .units
            .iter()
            .filter(|u| u.status != UnitStatus::NoBehaviorChange)
        {
            match u.decision {
                Decision::Accepted => c.0 += 1,
                Decision::Investigate => c.1 += 1,
                Decision::Pending => c.2 += 1,
            }
        }
        c
    }

    /// Every unit with a behavior change is accepted (and there is one).
    pub fn all_reviewed(&self) -> bool {
        let (accepted, flagged, pending) = self.decision_counts();
        accepted > 0 && flagged == 0 && pending == 0
    }

    /// What the detail area shows: the screen, the unit, the tab.
    fn detail_key(&self) -> String {
        match self.screen {
            Screen::Slim => format!("list {}", self.selected().map_or("", |u| u.id.as_str())),
            Screen::Swipe => format!("swipe {}", self.swipe_current().map_or("", String::as_str)),
            // Per unit; its tiles scroll on their own (`pane_scroll`).
            Screen::Flagged => format!("flagged {}", self.flagged_unit_id().unwrap_or_default()),
        }
    }

    /// Lines the detail area is scrolled down: 0 once it shows something else.
    pub fn detail_offset(&self) -> usize {
        let key = self.detail_key();
        let mut s = self.detail_scroll.borrow_mut();
        if s.0 != key {
            *s = (key, 0);
        }
        s.1
    }

    /// Scroll the detail area `by` lines, no further than the last frame
    /// allowed: on the Flagged screen, the tile in focus.
    fn scroll_detail(&mut self, by: isize) -> Vec<Effect> {
        if self.screen == Screen::Flagged {
            if let Some(u) = self.flagged_unit() {
                if self.flagged_focus(u) == Pane::Chat {
                    return self.chat_scroll(by);
                }
                let key = pane_key(&u.id, self.flagged_focus(u));
                let mut m = self.pane_scroll.borrow_mut();
                let at = m.get(&key).copied().unwrap_or(0);
                let to = at
                    .saturating_add_signed(by)
                    .min(self.detail_max_scroll.get());
                m.insert(key, to);
            }
            return Vec::new();
        }
        let to = self
            .detail_offset()
            .saturating_add_signed(by)
            .min(self.detail_max_scroll.get());
        self.detail_scroll.borrow_mut().1 = to;
        Vec::new()
    }

    /// Open the "all done" popup if `was_done` was false and it is now true.
    fn check_all_done(&mut self, was_done: bool) {
        if !was_done && self.all_reviewed() {
            self.overlay = Overlay::AllDone;
        }
    }

    // ---- Input -----------------------------------------------------------

    pub fn on_event(&mut self, ev: Event) -> Vec<Effect> {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            Event::Mouse(m) => self.on_mouse(m),
            _ => Vec::new(),
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            return vec![Effect::Save, Effect::Quit];
        }
        // A status message answers the last action; the next key dismisses it.
        // Actions that report something set it again after this.
        self.status.clear();
        self.selection = None;
        match self.overlay.clone() {
            Overlay::Ask => return self.ask_key(k),
            Overlay::Note { id, text, back } => return self.note_key(id, text, back, k),
            Overlay::Notes { sel } => return self.notes_key(sel, k),
            Overlay::ConfirmDeleteNote { id, sel } => {
                self.overlay = Overlay::Notes { sel };
                if matches!(k.code, KeyCode::Enter | KeyCode::Char('y' | 'Y'))
                    && let Some(u) = self.review.unit_mut(&id)
                {
                    u.dive.note.clear();
                    u.dive.note_on = None;
                    if crate::notes::noted(&self.review).is_empty() {
                        self.overlay = Overlay::None;
                    }
                    self.status = "Note deleted.".into();
                    return vec![Effect::Save];
                }
                return Vec::new();
            }
            Overlay::ConfirmWeb => {
                self.overlay = Overlay::None;
                return self
                    .answer_web(matches!(k.code, KeyCode::Enter | KeyCode::Char('y' | 'Y')));
            }
            Overlay::ConfirmReset => {
                self.overlay = Overlay::None;
                if matches!(k.code, KeyCode::Enter | KeyCode::Char('y' | 'Y')) {
                    return self.reset();
                }
                return Vec::new();
            }
            Overlay::ConfirmAcceptFile { ids, .. } => {
                self.overlay = Overlay::None;
                if matches!(k.code, KeyCode::Enter | KeyCode::Char('y' | 'Y')) {
                    return self.accept_file(ids);
                }
                return Vec::new();
            }
            Overlay::Help => {
                self.overlay = Overlay::None;
                return Vec::new();
            }
            Overlay::AllDone => {
                self.overlay = Overlay::None;
                if self.keys.is(Action::Quit, &k) {
                    return vec![Effect::Save, Effect::Quit];
                }
                if self.committed.is_some() {
                    return Vec::new();
                }
                if self.keys.is(Action::Reset, &k) {
                    self.overlay = Overlay::ConfirmReset;
                } else if self.keys.is(Action::ApprovePr, &k) {
                    self.ask_post(Verdict::Approve);
                } else if self.keys.is(Action::Commit, &k) {
                    return self.ask_commit();
                }
                return Vec::new();
            }
            Overlay::Commit { .. } => return self.commit_key(k),
            Overlay::Commits { sel, unit } => return self.commits_key(sel, unit, k),
            Overlay::PostReview {
                verdict,
                notes,
                intro,
                inline_on,
                scroll,
                editing,
            } => return self.post_key(verdict, notes, intro, inline_on, scroll, editing, k),
            Overlay::None => {}
        }
        if self.chat_typing() {
            return self.chat_key(k);
        }
        // PageDown and PageUp scroll the detail area a page at a time.
        let page = self.detail_rows.get().saturating_sub(2).max(1) as isize;
        match k.code {
            KeyCode::PageDown => return self.scroll_detail(page),
            KeyCode::PageUp => return self.scroll_detail(-page),
            _ => {}
        }
        let action = self.action_for(&k);
        match action {
            Some(a) => self.on_action(a),
            None => Vec::new(),
        }
    }

    /// Map a key to an action for the current screen. Arrow keys always work.
    /// In the list `h` and Left also investigate; in swipe only Left does
    /// (`h` goes back a card).
    fn action_for(&self, k: &KeyEvent) -> Option<Action> {
        if self.screen == Screen::Flagged {
            return self.flagged_action_for(k);
        }
        let in_swipe = self.screen == Screen::Swipe;
        match k.code {
            KeyCode::Down => return Some(Action::Down),
            KeyCode::Up => return Some(Action::Up),
            KeyCode::Right if in_swipe => return Some(Action::Accept),
            KeyCode::Left => return Some(Action::Investigate),
            KeyCode::Home => return Some(Action::Top),
            KeyCode::End => return Some(Action::Bottom),
            _ => {}
        }
        if self.screen == Screen::Slim {
            const LIST: &[Action] = &[
                Action::Down,
                Action::Up,
                Action::Accept,
                Action::Investigate,
                Action::Undo,
                Action::NextPending,
                Action::PrevPending,
                Action::NextGroup,
                Action::PrevGroup,
                Action::AcceptFile,
                Action::Grouping,
                Action::Commits,
            ];
            if let Some(a) = LIST.iter().copied().find(|a| self.keys.is(*a, k)) {
                return Some(a);
            }
        }
        let candidates: &[Action] = if in_swipe {
            &[
                Action::Accept,
                Action::Investigate,
                Action::Undo,
                Action::PrevCard,
            ]
        } else {
            &[Action::Down, Action::Up]
        };
        for a in candidates {
            if self.keys.is(*a, k) {
                return Some(*a);
            }
        }
        const REST: &[Action] = &[
            Action::Quit,
            Action::Top,
            Action::Bottom,
            Action::Edit,
            Action::Swipe,
            Action::Rerun,
            Action::Reset,
            Action::Back,
            Action::Help,
            Action::Flagged,
            Action::PreviewDown,
            Action::PreviewUp,
            Action::Ask,
            Action::EditNote,
            Action::Notes,
        ];
        if let Some(a) = REST.iter().copied().find(|a| self.keys.is(*a, k)) {
            return Some(a);
        }
        // Muscle memory: `h` investigates unless it was rebound.
        if self.screen == Screen::Slim && k.code == KeyCode::Char('h') && k.modifiers.is_empty() {
            return Some(Action::Investigate);
        }
        None
    }

    /// Keys of the Flagged screen. Plain letters never reach the global actions
    /// here, so `r` re-asks the tab instead of re-running the review.
    fn flagged_action_for(&self, k: &KeyEvent) -> Option<Action> {
        match k.code {
            KeyCode::Down => return Some(Action::Down),
            KeyCode::Up => return Some(Action::Up),
            KeyCode::Tab => return Some(Action::NextTile),
            KeyCode::BackTab => return Some(Action::PrevTile),
            KeyCode::Home => return Some(Action::Top),
            KeyCode::End => return Some(Action::Bottom),
            KeyCode::Enter if self.flagged.pane == Pane::Chat => return Some(Action::Ask),
            _ => {}
        }
        const FLAGGED: &[Action] = &[
            Action::Down,
            Action::Up,
            Action::PrevTile,
            Action::NextTile,
            Action::DiveRecommend,
            Action::DiveVisualize,
            Action::DiveBehavior,
            Action::DiveDisagree,
            Action::DiveTests,
            Action::InterpretTests,
            Action::OpenVisual,
            Action::ResolveOk,
            Action::ResolveFix,
            Action::EditNote,
            Action::DiveRerun,
            Action::Zoom,
            Action::Quit,
            Action::Top,
            Action::Bottom,
            Action::Edit,
            Action::Reset,
            Action::Back,
            Action::Help,
            Action::PreviewDown,
            Action::PreviewUp,
            Action::Ask,
            Action::Notes,
        ];
        FLAGGED.iter().copied().find(|a| self.keys.is(*a, k))
    }

    pub fn on_action(&mut self, a: Action) -> Vec<Effect> {
        // Actions that work everywhere.
        match a {
            Action::Help => {
                self.overlay = Overlay::Help;
                return Vec::new();
            }
            Action::Reset => {
                self.overlay = Overlay::ConfirmReset;
                return Vec::new();
            }
            Action::Ask if self.screen == Screen::Flagged => {
                self.open_chat();
                return Vec::new();
            }
            Action::Ask => {
                self.open_ask();
                return Vec::new();
            }
            Action::Edit => return self.edit(),
            Action::EditNote => {
                match self.current_unit().map(|u| u.id.clone()) {
                    Some(id) => self.edit_note(id, None),
                    None => self.status = "Select a unit to write a note on it.".into(),
                }
                return Vec::new();
            }
            Action::Notes => {
                self.open_notes();
                return Vec::new();
            }
            Action::PreviewDown => return self.scroll_detail(SCROLL_STEP),
            Action::PreviewUp => return self.scroll_detail(-SCROLL_STEP),
            Action::Rerun => {
                self.status = "Re-running the review…".into();
                return vec![Effect::Rerun];
            }
            _ => {}
        }
        match self.screen {
            Screen::Slim => self.slim_action(a),
            Screen::Swipe => self.swipe_action(a),
            Screen::Flagged => self.flagged_action(a),
        }
    }

    // ---- Flagged screen ---------------------------------------------------

    /// Open the Flagged screen on the first flagged unit, remembering where we
    /// came from. With nothing flagged, stay put and say so.
    fn start_flagged(&mut self) {
        if self.screen == Screen::Flagged {
            return;
        }
        if self.flagged_units().is_empty() {
            self.status = "Nothing flagged yet. Investigate a unit first.".into();
            return;
        }
        self.return_to.push(self.screen);
        self.screen = Screen::Flagged;
        self.flagged.sel = 0;
        self.flagged_focus_last();
    }

    /// Id of the flagged unit the Flagged screen is on.
    pub fn flagged_unit_id(&self) -> Option<String> {
        let flagged = self.flagged_units();
        flagged
            .get(self.flagged.sel.min(flagged.len().saturating_sub(1)))
            .map(|u| u.id.clone())
    }

    /// The flagged unit the Flagged screen is on.
    pub fn flagged_unit(&self) -> Option<&Unit> {
        let id = self.flagged_unit_id()?;
        self.review.unit(&id)
    }

    /// The Flagged tiles of `u`, in order: the code change (when it shows),
    /// each answer in the order it was asked for, the chat once something
    /// was asked in it (or while it has the focus), and the tile to ask more.
    pub fn flagged_panes(&self, u: &Unit) -> Vec<Pane> {
        let mut panes = Vec::new();
        if super::diff::shows(self, u) {
            panes.push(Pane::Code);
        }
        panes.extend(tile_kinds(u).into_iter().map(Pane::Answer));
        if self.flagged.pane == Pane::Chat || !self.chat_of(&u.id).is_empty() {
            panes.push(Pane::Chat);
        }
        panes.push(Pane::More);
        panes
    }

    /// The tile in focus: the one chosen, or the first when it is gone.
    pub fn flagged_focus(&self, u: &Unit) -> Pane {
        let panes = self.flagged_panes(u);
        if panes.contains(&self.flagged.pane) {
            self.flagged.pane
        } else {
            panes[0]
        }
    }

    /// Give `pane` the focus; typing in the chat stops.
    fn focus(&mut self, pane: Pane) {
        self.flagged.pane = pane;
        self.flagged.typing = false;
    }

    /// Give the next (or previous) tile the focus.
    fn flagged_tile_step(&mut self, forward: bool) {
        let Some(u) = self.flagged_unit() else {
            return;
        };
        let panes = self.flagged_panes(u);
        let n = panes.len();
        let i = panes
            .iter()
            .position(|p| *p == self.flagged_focus(u))
            .unwrap_or(0);
        let next = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        self.focus(panes[next]);
    }

    /// Another unit: the focus on its latest answer, else on its code.
    fn flagged_focus_last(&mut self) {
        self.flagged.zoom = false;
        self.ask.chat_scroll.set(None);
        if let Some(u) = self.flagged_unit() {
            let panes = self.flagged_panes(u);
            let last = panes
                .iter()
                .rev()
                .find(|p| matches!(p, Pane::Answer(_)))
                .unwrap_or(&panes[0]);
            self.focus(*last);
        }
    }

    fn flagged_action(&mut self, a: Action) -> Vec<Effect> {
        let n = self.flagged_units().len();
        match a {
            Action::Down | Action::Up | Action::Top | Action::Bottom => {
                self.flagged.sel = match a {
                    Action::Down => (self.flagged.sel + 1).min(n.saturating_sub(1)),
                    Action::Up => self.flagged.sel.saturating_sub(1),
                    Action::Top => 0,
                    _ => n.saturating_sub(1),
                };
                self.flagged_focus_last();
            }
            Action::NextTile => self.flagged_tile_step(true),
            Action::PrevTile => self.flagged_tile_step(false),
            Action::Zoom => self.flagged.zoom = !self.flagged.zoom,
            Action::DiveRecommend => return self.request_dive(DiveKind::Recommend),
            Action::DiveVisualize => return self.request_dive(DiveKind::Visualize),
            Action::DiveBehavior => return self.request_dive(DiveKind::Behavior),
            Action::DiveDisagree => return self.request_dive(DiveKind::Disagree),
            Action::DiveTests => return self.request_dive(DiveKind::Tests),
            Action::InterpretTests => return self.interpret_tests(),
            Action::OpenVisual => return self.open_visual(),
            Action::DiveRerun => {
                let Some(id) = self.flagged_unit_id() else {
                    return Vec::new();
                };
                // Again only the answer in focus.
                let focus = self.flagged_unit().map(|u| self.flagged_focus(u));
                if focus == Some(Pane::Chat) {
                    return self.ask_chat_again();
                }
                let Some(Pane::Answer(kind)) = focus else {
                    self.status = format!(
                        "Give an answer the focus ({} {}) to ask it again.",
                        self.keys.label(Action::PrevTile),
                        self.keys.label(Action::NextTile)
                    );
                    return Vec::new();
                };
                self.pane_scroll
                    .borrow_mut()
                    .remove(&pane_key(&id, Pane::Answer(kind)));
                if let Some(u) = self.review.unit_mut(&id) {
                    if u.dive_pending.contains(&kind) || u.checks.second_pending {
                        return Vec::new();
                    }
                    if kind == DiveKind::Disagree {
                        u.checks.second = None;
                        u.checks.second_failed = None;
                    }
                    u.dive.results.remove(&kind);
                }
                return self.request_dive(kind);
            }
            Action::ResolveOk => return self.resolve(Resolution::Ok),
            Action::ResolveFix => return self.resolve(Resolution::NeedsFix),
            Action::Back if self.flagged.zoom => self.flagged.zoom = false,
            Action::Back => self.back(),
            Action::Quit => return vec![Effect::Save, Effect::Quit],
            _ => {}
        }
        Vec::new()
    }

    /// Give `kind` a tile and the focus, asking for it unless it is
    /// answered or in flight. The first web-using ask of the session
    /// confirms first.
    fn request_dive(&mut self, kind: DiveKind) -> Vec<Effect> {
        let Some(id) = self.flagged_unit_id() else {
            return Vec::new();
        };
        let Some(u) = self.review.unit_mut(&id) else {
            return Vec::new();
        };
        let mark_asked = |u: &mut Unit| {
            if !u.dive.asked.contains(&kind) {
                u.dive.asked.push(kind);
            }
        };
        if kind == DiveKind::Disagree {
            // The second opinion runs after the description; ask now only
            // when there is a description and nothing is on its way.
            if u.checks.second.is_some() || u.checks.second_pending {
                mark_asked(u);
                self.focus(Pane::Answer(kind));
                return Vec::new();
            }
            if u.changes.is_empty() {
                self.status = "Nothing to check yet: the unit has no description.".into();
                return Vec::new();
            }
            mark_asked(u);
            u.checks.second_pending = true;
            self.focus(Pane::Answer(kind));
            return vec![Effect::Second(vec![id])];
        }
        mark_asked(u);
        self.focus(Pane::Answer(kind));
        let Some(u) = self.review.unit_mut(&id) else {
            return Vec::new();
        };
        if u.dive.results.contains_key(&kind) || u.dive_pending.contains(&kind) {
            return Vec::new();
        }
        u.dive_pending.insert(kind);
        // Never for a pull request: its code is untrusted, and web tools could
        // carry text out of the machine. (The agent adapter enforces this too.)
        let is_pr = self.review.pr.is_some();
        if is_pr && self.cfg.dive_web && uses_web(kind) {
            self.status =
                "Web search is disabled for PR reviews; answering from the code alone.".into();
        }
        let web = self.cfg.dive_web && !is_pr;
        if web && uses_web(kind) && !self.dive_web_ok {
            self.dive_web_pending = Some((id, kind));
            self.overlay = Overlay::ConfirmWeb;
            return Vec::new();
        }
        vec![Effect::Dive {
            unit_id: id,
            kind,
            web,
        }]
    }

    /// Have the agent read what the Tests search found for the flagged unit
    /// (again, if it already has).
    fn interpret_tests(&mut self) -> Vec<Effect> {
        let kind = DiveKind::Tests;
        let Some(id) = self.flagged_unit_id() else {
            return Vec::new();
        };
        let Some(u) = self.review.unit_mut(&id) else {
            return Vec::new();
        };
        let searched = u
            .dive
            .results
            .get(&kind)
            .and_then(|r| r.coverage.as_ref())
            .is_some_and(|c| c.search.is_some());
        if !searched {
            self.status = format!(
                "Nothing found yet: {} searches the tests first.",
                self.keys.label(Action::DiveTests)
            );
            return Vec::new();
        }
        if u.dive_pending.contains(&kind) {
            return Vec::new();
        }
        u.dive_pending.insert(kind);
        self.focus(Pane::Answer(kind));
        vec![Effect::InterpretTests { unit_id: id }]
    }

    /// Open the selected unit's picture, or say how to draw one.
    fn open_visual(&mut self) -> Vec<Effect> {
        let Some(u) = self.flagged_unit() else {
            return Vec::new();
        };
        let drawn = u
            .dive
            .results
            .get(&DiveKind::Visualize)
            .is_some_and(|r| r.visual.is_some());
        if !drawn {
            self.status = format!(
                "No picture yet: press {} to draw one.",
                self.keys.label(Action::DiveVisualize)
            );
            return Vec::new();
        }
        vec![Effect::OpenVisual {
            unit_id: u.id.clone(),
        }]
    }

    /// The answer to the web confirmation.
    fn answer_web(&mut self, yes: bool) -> Vec<Effect> {
        let Some((id, kind)) = self.dive_web_pending.take() else {
            return Vec::new();
        };
        if yes {
            self.dive_web_ok = true;
            return vec![Effect::Dive {
                unit_id: id,
                kind,
                web: true,
            }];
        }
        if let Some(u) = self.review.unit_mut(&id) {
            u.dive_pending.remove(&kind);
            if !u.dive.results.contains_key(&kind) {
                u.dive.asked.retain(|k| *k != kind);
            }
        }
        self.status = "Web search not allowed; question not asked.".into();
        Vec::new()
    }

    /// Set the selected unit's resolution; pressing the same one clears it.
    fn resolve(&mut self, r: Resolution) -> Vec<Effect> {
        let Some(id) = self.flagged_unit_id() else {
            return Vec::new();
        };
        let Some(u) = self.review.unit_mut(&id) else {
            return Vec::new();
        };
        u.dive.resolution = if u.dive.resolution == Some(r) {
            None
        } else {
            Some(r)
        };
        vec![Effect::Save]
    }

    /// Can the PR be approved from here: a `teer pr` review whose head has
    /// not been approved yet.
    pub fn can_approve(&self) -> bool {
        self.review.pr.as_ref().is_some_and(|p| !p.is_approved()) && !self.approving
    }

    /// A review of the PR can be posted now: nothing is being posted.
    pub fn can_post(&self) -> bool {
        self.review.pr.is_some() && !self.approving
    }

    /// The verdicts the post confirmation offers, in the order `v` goes
    /// through them: approving only once everything was reviewed.
    pub fn verdicts(&self) -> Vec<Verdict> {
        let mut v = Vec::new();
        if self.can_approve() && self.all_reviewed() {
            v.push(Verdict::Approve);
        }
        v.extend([Verdict::RequestChanges, Verdict::Comment]);
        v
    }

    /// Open the confirmation for posting a review with `verdict`: `a` in the
    /// all-reviewed popup approves, `p` in the notes requests changes.
    fn ask_post(&mut self, verdict: Verdict) {
        if !self.can_post() || (verdict == Verdict::Approve && !self.can_approve()) {
            return;
        }
        let Some(pr) = self.review.pr.as_ref() else {
            return;
        };
        if pr.remote.is_none() || pr.head_sha.is_none() || pr.repo_dir.is_none() {
            self.status = format!(
                "Can't post to PR #{}: teer doesn't know its remote or head.",
                pr.number
            );
            return;
        }
        let notes = approve::collect_notes(&self.review, &self.order);
        self.overlay = Overlay::PostReview {
            verdict,
            notes,
            intro: approve::intro_for(&self.review),
            inline_on: true,
            scroll: 0,
            editing: None,
        };
    }

    /// Keys while the post confirmation is open. Only the keys below do
    /// anything; ctrl+c is handled before.
    #[allow(clippy::too_many_arguments)]
    fn post_key(
        &mut self,
        verdict: Verdict,
        notes: Vec<NoteEntry>,
        mut intro: String,
        mut inline_on: bool,
        mut scroll: usize,
        editing: Option<String>,
        k: KeyEvent,
    ) -> Vec<Effect> {
        let put = |app: &mut App, notes, intro, inline_on, scroll, editing| {
            app.overlay = Overlay::PostReview {
                verdict,
                notes,
                intro,
                inline_on,
                scroll,
                editing,
            };
        };
        if let Some(mut text) = editing {
            match k.code {
                KeyCode::Esc => put(self, notes, intro, inline_on, scroll, None),
                KeyCode::Enter => {
                    let t = approve::clamp_text(&text.replace('\n', " "), approve::BODY_MAX_CHARS);
                    if !t.trim().is_empty() {
                        intro = t.trim().to_string();
                    }
                    put(self, notes, intro, inline_on, scroll, None);
                }
                KeyCode::Backspace => {
                    text.pop();
                    put(self, notes, intro, inline_on, scroll, Some(text));
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !k
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    if text.chars().count() < approve::BODY_MAX_CHARS {
                        text.push(c);
                    }
                    put(self, notes, intro, inline_on, scroll, Some(text));
                }
                _ => put(self, notes, intro, inline_on, scroll, Some(text)),
            }
            return Vec::new();
        }
        let max = self.approve_max_scroll.get();
        match k.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                return Vec::new();
            }
            KeyCode::Enter | KeyCode::Char('y' | 'Y') => {
                self.overlay = Overlay::None;
                let Some(pr) = self.review.pr.clone() else {
                    return Vec::new();
                };
                if !self.verdicts().contains(&verdict) || !self.can_post() {
                    return Vec::new();
                }
                let plan = approve::build_plan(&notes, &intro, inline_on);
                let flat_body = approve::build_plan(&notes, &intro, false).body;
                self.approving = true;
                self.status = format!("{} on GitHub…", verdict.doing(pr.number));
                return vec![Effect::PostReview {
                    pr: Box::new(pr),
                    verdict,
                    body: plan.body,
                    inline: plan.inline,
                    flat_body,
                }];
            }
            KeyCode::Char('e' | 'E') => {
                let text = intro.clone();
                put(self, notes, intro, inline_on, scroll, None);
                return vec![Effect::EditComment { text }];
            }
            KeyCode::Char('i' | 'I') => inline_on = !inline_on,
            KeyCode::Char('v' | 'V') => {
                let all = self.verdicts();
                let at = all.iter().position(|v| *v == verdict).map_or(0, |i| i + 1);
                self.overlay = Overlay::PostReview {
                    verdict: all[at % all.len()],
                    notes,
                    intro,
                    inline_on,
                    scroll,
                    editing: None,
                };
                return Vec::new();
            }
            KeyCode::Down => scroll = (scroll + 1).min(max),
            KeyCode::Up => scroll = scroll.saturating_sub(1),
            KeyCode::PageDown => scroll = (scroll + 8).min(max),
            KeyCode::PageUp => scroll = scroll.saturating_sub(8),
            _ => {}
        }
        put(self, notes, intro, inline_on, scroll, None);
        Vec::new()
    }

    // ---- Committing -------------------------------------------------------

    /// Can the reviewed changes be committed from here: a plain review of
    /// uncommitted changes (not `teer pr`, not `--base`) not yet committed.
    pub fn can_commit(&self) -> bool {
        self.review.pr.is_none()
            && self.review.head == crate::git::WORKTREE
            && self.committed.is_none()
    }

    /// `c` in the all-reviewed popup: open the commit confirmation with a
    /// template message, and ask the agent for a better one.
    fn ask_commit(&mut self) -> Vec<Effect> {
        if !self.can_commit() {
            return Vec::new();
        }
        if !self.all_reviewed() {
            self.status = "Not everything is accepted yet; nothing to commit.".into();
            return Vec::new();
        }
        let paths = match commit::reviewed_paths(&self.review) {
            Ok(p) => p,
            Err(why) => {
                self.status = why;
                return Vec::new();
            }
        };
        if paths.is_empty() {
            self.status = "Nothing to commit: the review has no files.".into();
            return Vec::new();
        }
        let repo = self.review.repo_root.clone();
        let message = commit::template_message(
            &commit::file_names(&self.review),
            &commit::accepted_summaries(&self.review, &self.order),
        );
        let ask = self.can_ask();
        self.overlay = Overlay::Commit {
            message,
            draft: if ask {
                DraftState::Pending
            } else {
                DraftState::Template
            },
            scroll: 0,
            editing: None,
            branch: commit::current_branch(&repo),
            others: commit::other_changed(&repo, &paths),
        };
        if ask {
            vec![Effect::DraftCommitMessage(Box::new(commit::draft_input(
                &self.review,
                &self.order,
            )))]
        } else {
            self.status = "No agent to draft a message; using the template. e edits it.".into();
            Vec::new()
        }
    }

    /// Keys while the commit confirmation is open. Only the keys below do
    /// anything; ctrl+c is handled before.
    fn commit_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        enum Act {
            Stay,
            Close,
            Commit,
            Edit(String),
        }
        let max = self.commit_max_scroll.get();
        let Overlay::Commit {
            message,
            draft,
            scroll,
            editing,
            ..
        } = &mut self.overlay
        else {
            return Vec::new();
        };
        if let Some(text) = editing.as_mut() {
            match k.code {
                KeyCode::Esc => *editing = None,
                KeyCode::Enter => {
                    let line: String = text.chars().filter(|c| !c.is_control()).collect();
                    let line = line.trim();
                    if !line.is_empty() {
                        let body = message.split_once("\n\n").map(|(_, b)| b.to_string());
                        let new = match body {
                            Some(b) => format!("{line}\n\n{b}"),
                            None => line.to_string(),
                        };
                        *message = commit::normalize_message(&new);
                    }
                    *draft = DraftState::Edited;
                    *editing = None;
                }
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !k
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && text.chars().count() < commit::SUBJECT_MAX * 2 =>
                {
                    text.push(c);
                }
                _ => {}
            }
            return Vec::new();
        }
        let act = match k.code {
            KeyCode::Esc => Act::Close,
            KeyCode::Enter | KeyCode::Char('y' | 'Y') => Act::Commit,
            KeyCode::Char('e' | 'E') => {
                *draft = DraftState::Edited;
                Act::Edit(message.clone())
            }
            KeyCode::Down => {
                *scroll = (*scroll + 1).min(max);
                Act::Stay
            }
            KeyCode::Up => {
                *scroll = scroll.saturating_sub(1);
                Act::Stay
            }
            KeyCode::PageDown => {
                *scroll = (*scroll + 8).min(max);
                Act::Stay
            }
            KeyCode::PageUp => {
                *scroll = scroll.saturating_sub(8);
                Act::Stay
            }
            _ => Act::Stay,
        };
        match act {
            Act::Stay => Vec::new(),
            Act::Close => {
                self.overlay = Overlay::None;
                Vec::new()
            }
            Act::Commit => self.commit_now(),
            Act::Edit(text) => vec![Effect::EditComment { text }],
        }
    }

    /// `y`: check that nothing changed since the review, then commit. A
    /// refusal closes the overlay (the reason is in the status line) and runs
    /// no git command that changes anything.
    fn commit_now(&mut self) -> Vec<Effect> {
        let Overlay::Commit { message, draft, .. } = &mut self.overlay else {
            return Vec::new();
        };
        // What the user is about to commit is final: a late draft must not replace it.
        *draft = DraftState::Edited;
        let message = commit::normalize_message(message);
        if !self.can_commit() {
            self.overlay = Overlay::None;
            return Vec::new();
        }
        if message.is_empty() {
            self.status = "The commit message is empty. Press e to write one.".into();
            return Vec::new();
        }
        match commit::check(&self.review) {
            Err(refusal) => {
                self.overlay = Overlay::None;
                self.status = refusal.text(&self.keys.label(Action::Rerun));
                Vec::new()
            }
            Ok(paths) => {
                self.status = "Committing…".into();
                vec![Effect::Commit {
                    repo: self.review.repo_root.clone(),
                    paths,
                    message,
                }]
            }
        }
    }

    /// git finished (see `Effect::Commit`).
    pub fn commit_finished(&mut self, outcome: commit::Outcome) -> Vec<Effect> {
        match outcome {
            commit::Outcome::Committed { short } => {
                let subject = match &self.overlay {
                    Overlay::Commit { message, .. } => commit::subject_of(message).to_string(),
                    _ => String::new(),
                };
                self.status = format!("Committed {short}: {subject}");
                self.committed = Some(short);
                // The review is stale now: the popup says so and offers quit.
                self.overlay = Overlay::AllDone;
            }
            commit::Outcome::Failed { code, output } => {
                // The overlay stays open for a retry or an edit.
                self.status = commit::failure_status(code, &output);
            }
            commit::Outcome::Refused(why) => {
                self.status = approve::tidy_output(&why);
            }
        }
        Vec::new()
    }

    /// The agent's draft arrived. It never replaces text the user has touched.
    fn commit_drafted(&mut self, r: Result<String, String>) {
        let Overlay::Commit { message, draft, .. } = &mut self.overlay else {
            return;
        };
        if *draft != DraftState::Pending {
            return;
        }
        match r {
            Ok(m) if !commit::normalize_message(&m).is_empty() => {
                *message = commit::normalize_message(&m);
                *draft = DraftState::Drafted;
                self.status = "Drafted a commit message. e edits it.".into();
            }
            Ok(_) => {
                *draft = DraftState::Template;
                self.status = "The agent's draft was empty; kept the template message.".into();
            }
            Err(why) => {
                *draft = DraftState::Template;
                self.status = format!(
                    "Couldn't draft a message ({}); kept the template.",
                    approve::tidy_output(&why)
                );
            }
        }
    }

    /// The external editor finished (or could not start) for the commit message.
    fn commit_edited(&mut self, outcome: EditOutcome) {
        let Overlay::Commit {
            message,
            draft,
            editing,
            ..
        } = &mut self.overlay
        else {
            return;
        };
        *draft = DraftState::Edited;
        match outcome {
            EditOutcome::Edited(t) => {
                let t = commit::normalize_message(&t);
                if !t.is_empty() {
                    *message = t;
                }
            }
            EditOutcome::Cancelled => {}
            EditOutcome::Unavailable(why) => {
                *editing = Some(commit::subject_of(message).to_string());
                self.status = format!(
                    "Couldn't start the editor ({}); editing the subject on one line.",
                    approve::tidy_output(&why)
                );
            }
        }
    }

    /// The external editor finished (or could not start) for the approval summary.
    pub fn comment_edited(&mut self, outcome: EditOutcome) {
        if matches!(self.overlay, Overlay::Commit { .. }) {
            self.commit_edited(outcome);
            return;
        }
        let Overlay::PostReview {
            verdict,
            notes,
            intro,
            inline_on,
            scroll,
            ..
        } = self.overlay.clone()
        else {
            return;
        };
        let (intro, editing) = match outcome {
            EditOutcome::Edited(t) if !t.trim().is_empty() => (t, None),
            EditOutcome::Edited(_) | EditOutcome::Cancelled => (intro, None),
            EditOutcome::Unavailable(why) => {
                self.status = format!(
                    "Couldn't start the editor ({}); editing on one line.",
                    approve::tidy_output(&why)
                );
                let flat = intro.replace('\n', " ");
                (intro, Some(flat))
            }
        };
        self.overlay = Overlay::PostReview {
            verdict,
            notes,
            intro,
            inline_on,
            scroll,
            editing,
        };
    }

    /// Keys while the note editor is open: raw text, Backspace, Enter, Esc.
    /// Saving marks the note as written on the code as it is now; an empty
    /// note is no note.
    fn note_key(
        &mut self,
        id: String,
        mut text: String,
        back: Option<usize>,
        k: KeyEvent,
    ) -> Vec<Effect> {
        let after = back.map_or(Overlay::None, |sel| Overlay::Notes { sel });
        match k.code {
            KeyCode::Esc => self.overlay = after,
            KeyCode::Enter => {
                self.overlay = after;
                let note: String = text.chars().filter(|c| !c.is_control()).collect();
                let note: String = note.trim().chars().take(NOTE_MAX).collect();
                if let Some(u) = self.review.unit_mut(&id) {
                    u.dive.note_on = (!note.is_empty()).then(|| u.fingerprint.clone());
                    u.dive.note = note;
                    if crate::notes::noted(&self.review).is_empty() {
                        self.overlay = Overlay::None;
                    }
                    return vec![Effect::Save];
                }
            }
            _ => {
                if typed(&mut text, &k, NOTE_MAX) {
                    self.overlay = Overlay::Note { id, text, back };
                }
            }
        }
        Vec::new()
    }

    // ---- Notes ------------------------------------------------------------

    /// Open the note editor on unit `id`, with its note.
    fn edit_note(&mut self, id: String, back: Option<usize>) {
        let text = self
            .review
            .unit(&id)
            .map(|u| u.dive.note.clone())
            .unwrap_or_default();
        self.overlay = Overlay::Note { id, text, back };
    }

    /// Open the notes view, on the note of the unit on show when it has one.
    fn open_notes(&mut self) {
        let noted = crate::notes::noted(&self.review);
        if noted.is_empty() {
            self.status = format!(
                "No notes yet: {} writes one on the unit on show.",
                self.keys.label(Action::EditNote)
            );
            return;
        }
        let here = self.current_unit().map(|u| u.id.clone());
        let sel = noted
            .iter()
            .position(|u| Some(&u.id) == here.as_ref())
            .unwrap_or(0);
        self.overlay = Overlay::Notes { sel };
    }

    /// Keys of the notes view: move with j/k, `enter` goes to the unit, the
    /// note key edits its note, `d` deletes it; `y` copies every note, `c`
    /// hands them to Claude Code, `p` posts them on the PR; `esc` (or `q`,
    /// the notes key) closes.
    fn notes_key(&mut self, sel: usize, k: KeyEvent) -> Vec<Effect> {
        let ids: Vec<String> = crate::notes::noted(&self.review)
            .iter()
            .map(|u| u.id.clone())
            .collect();
        if ids.is_empty() {
            self.overlay = Overlay::None;
            return Vec::new();
        }
        let last = ids.len() - 1;
        let mut sel = sel.min(last);
        let is = |a: Action| self.keys.is(a, &k);
        if k.code == KeyCode::Down || is(Action::Down) {
            sel = (sel + 1).min(last);
        } else if k.code == KeyCode::Up || is(Action::Up) {
            sel = sel.saturating_sub(1);
        } else if k.code == KeyCode::Home || is(Action::Top) {
            sel = 0;
        } else if k.code == KeyCode::End || is(Action::Bottom) {
            sel = last;
        } else if k.code == KeyCode::Enter {
            self.overlay = Overlay::None;
            return self.go_to_unit(&ids[sel], true).unwrap_or_default();
        } else if is(Action::EditNote) {
            self.edit_note(ids[sel].clone(), Some(sel));
            return Vec::new();
        } else if k.code == KeyCode::Char('d') {
            self.overlay = Overlay::ConfirmDeleteNote {
                id: ids[sel].clone(),
                sel,
            };
            return Vec::new();
        } else if k.code == KeyCode::Char('y') {
            let what = match ids.len() {
                1 => "1 note as Markdown".to_string(),
                n => format!("{n} notes as Markdown"),
            };
            self.overlay = Overlay::Notes { sel };
            return vec![Effect::Copy {
                text: crate::notes::markdown(&self.review),
                what,
            }];
        } else if k.code == KeyCode::Char('c') {
            self.overlay = Overlay::Notes { sel };
            return self.notes_to_agent();
        } else if k.code == KeyCode::Char('p') {
            self.overlay = Overlay::Notes { sel };
            if self.review.pr.is_none() {
                self.status = "Posting is for a pull request (teer pr).".into();
            } else if self.can_post() {
                self.ask_post(Verdict::RequestChanges);
            }
            return Vec::new();
        } else if k.code == KeyCode::Esc
            || is(Action::Back)
            || is(Action::Quit)
            || is(Action::Notes)
        {
            self.overlay = Overlay::None;
            return Vec::new();
        }
        self.overlay = Overlay::Notes { sel };
        Vec::new()
    }

    /// Hand the notes to the agent CLI in the repository, as the first
    /// message of a session: a fork of the review's Claude Code session when
    /// there is one. Not for a pull request: its code is untrusted, and the
    /// session could change files and run commands. Not with a model API,
    /// which has no session to hand over to.
    fn notes_to_agent(&mut self) -> Vec<Effect> {
        if self.review.pr.is_some() {
            self.status =
                "Not for a pull request: its code is untrusted. y copies the notes, p posts them on the PR."
                    .into();
            return Vec::new();
        }
        let cli = self.cfg.provider();
        if cli.program().is_none() {
            self.status =
                "Handing over needs Claude Code or Codex (teer setup). y copies the notes.".into();
            return Vec::new();
        }
        let prompt = crate::notes::agent_prompt(&self.review);
        // Only Claude Code has sessions to fork.
        let session = (self.review.agent_session.as_ref())
            .filter(|s| !s.is_fresh() && cli == Provider::Claude);
        // Claude Code finds a session by the directory it was started in.
        let cwd = session.map_or(self.review.repo_root.clone(), |s| s.cwd.clone());
        let resume = session.map(|s| s.id.clone());
        self.overlay = Overlay::None;
        vec![Effect::OpenAgent {
            cli,
            cwd,
            prompt,
            resume,
        }]
    }

    /// Go to unit `id`: on the Flagged screen when it is flagged there and
    /// `stay` allows, else in the list, closing any overlay. `None` when the
    /// list doesn't have it.
    fn go_to_unit(&mut self, id: &str, stay: bool) -> Option<Vec<Effect>> {
        if stay
            && self.screen == Screen::Flagged
            && let Some(sel) = self.flagged_units().iter().position(|u| u.id == id)
        {
            self.flagged.sel = sel;
            self.flagged_focus_last();
            return Some(Vec::new());
        }
        let pos = self
            .order
            .iter()
            .position(|&u| self.review.units[u].id == id)?;
        self.overlay = Overlay::None;
        if self.screen != Screen::Slim {
            self.return_to.clear();
            self.screen = Screen::Slim;
        }
        Some(self.move_sel(pos))
    }

    /// Select the unit at `to`.
    fn move_sel(&mut self, to: usize) -> Vec<Effect> {
        self.sel = to.min(self.order.len().saturating_sub(1));
        self.ensure_prepared()
    }

    fn slim_action(&mut self, a: Action) -> Vec<Effect> {
        match a {
            Action::Down => return self.move_row(1),
            Action::Up => return self.move_row(-1),
            Action::Top => return self.move_row(isize::MIN / 2),
            Action::Bottom => return self.move_row(isize::MAX / 2),
            Action::Swipe => {
                self.enter_swipe();
                return self.prepare_cards();
            }
            Action::Accept => return self.decide(Decision::Accepted),
            Action::Investigate => return self.decide(Decision::Investigate),
            Action::Flagged => self.start_flagged(),
            Action::Undo => return self.undo(),
            Action::NextPending => return self.jump_pending(true),
            Action::PrevPending => return self.jump_pending(false),
            Action::NextGroup => return self.jump_file(true),
            Action::PrevGroup => return self.jump_file(false),
            Action::AcceptFile => self.ask_accept_file(),
            Action::Grouping => self.switch_grouping(),
            Action::Commits => self.open_commits(),
            Action::Quit => return vec![Effect::Save, Effect::Quit],
            _ => {}
        }
        Vec::new()
    }

    // ---- Commits ------------------------------------------------------------

    /// Show the commits and the PR's description, or say there are none.
    fn open_commits(&mut self) {
        let i = &self.review.intent;
        if i.commits.is_empty() && i.pr.is_none() {
            self.status = if self.review.head == crate::git::WORKTREE {
                "No commits: these changes are not committed yet.".into()
            } else {
                "No commits between the base and the head.".into()
            };
            return;
        }
        self.overlay = Overlay::Commits { sel: 0, unit: None };
    }

    /// Positions in the list of the units commit `n` changed, in list order.
    pub fn commit_units(&self, n: usize) -> Vec<usize> {
        let Some(c) = self.review.intent.commits.get(n) else {
            return Vec::new();
        };
        (0..self.order.len())
            .filter(|&p| self.review.changed_by(&self.review.units[self.order[p]], c))
            .collect()
    }

    /// Keys of the commits view: move through the commits, `enter` (or
    /// `tab`, `→`) into the units the selected one changed, `enter` on one
    /// goes to it in the list; `esc` (or `←`, `tab`) back, `esc` again or
    /// `q` closes.
    fn commits_key(&mut self, mut sel: usize, mut unit: Option<usize>, k: KeyEvent) -> Vec<Effect> {
        let n = self.review.intent.commits.len();
        let units = self.commit_units(sel);
        let is = |a: Action| self.keys.is(a, &k);
        let down = k.code == KeyCode::Down || is(Action::Down);
        let up = k.code == KeyCode::Up || is(Action::Up);
        let close = is(Action::Quit) || is(Action::Commits);
        match unit {
            None => {
                if down {
                    sel = (sel + 1).min(n.saturating_sub(1));
                } else if up {
                    sel = sel.saturating_sub(1);
                } else if k.code == KeyCode::Home || is(Action::Top) {
                    sel = 0;
                } else if k.code == KeyCode::End || is(Action::Bottom) {
                    sel = n.saturating_sub(1);
                } else if matches!(k.code, KeyCode::Enter | KeyCode::Tab | KeyCode::Right) {
                    unit = (!units.is_empty()).then_some(0);
                } else if close || k.code == KeyCode::Esc || is(Action::Back) {
                    self.overlay = Overlay::None;
                    return Vec::new();
                }
            }
            Some(i) => {
                if down {
                    unit = Some((i + 1).min(units.len().saturating_sub(1)));
                } else if up {
                    unit = Some(i.saturating_sub(1));
                } else if k.code == KeyCode::Enter {
                    if let Some(&pos) = units.get(i) {
                        self.overlay = Overlay::None;
                        return self.move_sel(pos);
                    }
                } else if matches!(k.code, KeyCode::Esc | KeyCode::Left | KeyCode::Tab)
                    || is(Action::Back)
                {
                    unit = None;
                } else if close {
                    self.overlay = Overlay::None;
                    return Vec::new();
                }
            }
        }
        self.overlay = Overlay::Commits { sel, unit };
        Vec::new()
    }

    // ---- Ask --------------------------------------------------------------

    /// Open the Ask view, typing, at the end of the conversation.
    fn open_ask(&mut self) {
        self.overlay = Overlay::Ask;
        self.ask.link = None;
        self.ask.scroll.set(None);
    }

    /// The unit "this" means in a question: the one on show.
    fn ask_context(&self) -> Option<String> {
        self.current_unit().map(|u| u.id.clone())
    }

    /// The questions of the conversation on show, by index: the flagged
    /// unit's chat on the Flagged screen, else all of them (the Ask view).
    pub fn talk(&self) -> Vec<usize> {
        if self.overlay != Overlay::Ask
            && self.screen == Screen::Flagged
            && let Some(id) = self.flagged_unit_id()
        {
            return self.chat_of(&id);
        }
        (0..self.review.questions.len()).collect()
    }

    /// The questions about unit `id`, by index, oldest first: its chat.
    pub fn chat_of(&self, id: &str) -> Vec<usize> {
        let questions = self.review.questions.iter().enumerate();
        questions
            .filter(|(_, q)| q.unit.as_deref() == Some(id))
            .map(|(n, _)| n)
            .collect()
    }

    /// The links of the conversation on show ([`App::talk`]) in the order
    /// they are drawn, with the question each belongs to: per answer, the
    /// related units that are in the list, then the code it shows.
    pub fn ask_links(&self) -> Vec<(usize, AskLink)> {
        let mut out = Vec::new();
        for n in self.talk() {
            let Some(a) = &self.review.questions[n].answer else {
                continue;
            };
            for r in a.related.iter().filter(|r| self.listed(&r.id)) {
                out.push((n, AskLink::Unit(r.id.clone())));
            }
            for s in &a.shown {
                out.push((
                    n,
                    AskLink::Code {
                        file: s.cite.file.clone(),
                        line: s.cite.start,
                    },
                ));
            }
        }
        out
    }

    /// Whether unit `id` has a row in the list.
    pub fn listed(&self, id: &str) -> bool {
        self.order.iter().any(|&i| self.review.units[i].id == id)
    }

    /// Answered questions whose answer belongs with unit `id`, newest first.
    pub fn answers_for(&self, id: &str) -> Vec<&Question> {
        self.review
            .questions
            .iter()
            .rev()
            .filter(|q| q.answer.is_some() && q.unit.as_deref() == Some(id))
            .collect()
    }

    /// Keys of the Ask view. Typing: letters go into the question, `enter`
    /// asks it, `ctrl+r` asks the last question again, `ctrl+u` clears,
    /// `tab` moves to the links, `esc` closes. On the links: `↑ ↓ j k` move,
    /// `enter` follows (a unit: to it in the list; code: the editor),
    /// `tab` `esc` `/` go back to typing. `↑ ↓` (typing) and PageUp PageDown
    /// scroll.
    fn ask_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        const PAGE: isize = 10;
        match k.code {
            KeyCode::PageDown => return self.ask_scroll(PAGE),
            KeyCode::PageUp => return self.ask_scroll(-PAGE),
            _ => {}
        }
        if let Some(i) = self.ask.link {
            let n = self.ask_links().len();
            match k.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.ask.link = Some((i + 1).min(n.saturating_sub(1)));
                }
                KeyCode::Up | KeyCode::Char('k') => self.ask.link = Some(i.saturating_sub(1)),
                KeyCode::Enter => return self.follow_ask_link(i),
                KeyCode::Esc | KeyCode::Tab | KeyCode::Char('/') => self.ask.link = None,
                _ => {}
            }
            return Vec::new();
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Enter => return self.send_question(),
            KeyCode::Tab => {
                // The newest answer's first link.
                let links = self.ask_links();
                let newest = links.last().map(|(q, _)| *q);
                self.ask.link = links.iter().position(|(q, _)| Some(*q) == newest);
            }
            KeyCode::Down => return self.ask_scroll(SCROLL_STEP),
            KeyCode::Up => return self.ask_scroll(-SCROLL_STEP),
            KeyCode::Char('r') if ctrl => {
                return match self.review.questions.len() {
                    0 => Vec::new(),
                    n => self.ask_question(n - 1),
                };
            }
            _ => {
                typed(&mut self.ask.input, &k, QUESTION_MAX);
            }
        }
        Vec::new()
    }

    fn ask_mouse(&mut self, m: MouseEvent) -> Vec<Effect> {
        match m.kind {
            MouseEventKind::ScrollDown => self.ask_scroll(SCROLL_STEP),
            MouseEventKind::ScrollUp => self.ask_scroll(-SCROLL_STEP),
            MouseEventKind::Up(MouseButton::Left) => match self.hit(m.column, m.row) {
                Some(Target::AskLink(i)) => {
                    self.ask.link = Some(i);
                    self.follow_ask_link(i)
                }
                Some(Target::Key(k)) => self.on_key(k),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Scroll the Ask view `by` lines; at the end it follows new lines.
    fn ask_scroll(&mut self, by: isize) -> Vec<Effect> {
        let max = self.ask.max_scroll.get();
        let to = self
            .ask
            .scroll
            .get()
            .unwrap_or(max)
            .saturating_add_signed(by)
            .min(max);
        self.ask.scroll.set((to < max).then_some(to));
        Vec::new()
    }

    /// Ask what was typed, about the unit on show.
    fn send_question(&mut self) -> Vec<Effect> {
        let text = std::mem::take(&mut self.ask.input);
        self.ask.scroll.set(None);
        self.push_question(&text, self.ask_context())
    }

    /// Ask `text` about `unit`; nothing when it is blank.
    fn push_question(&mut self, text: &str, unit: Option<String>) -> Vec<Effect> {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        let text = text.trim().to_string();
        if text.is_empty() {
            return Vec::new();
        }
        self.review.questions.push(Question {
            text,
            unit,
            ..Default::default()
        });
        self.ask_question(self.review.questions.len() - 1)
    }

    /// Have the agent answer question `index` (again), unless it is on its
    /// way. An earlier answer stays until the new one comes.
    fn ask_question(&mut self, index: usize) -> Vec<Effect> {
        let has_agent = self.has_agent;
        let Some(q) = self.review.questions.get_mut(index) else {
            return Vec::new();
        };
        if !has_agent {
            q.error = Some("no agent: asking needs one (`teer setup` picks it)".into());
            return vec![Effect::Save];
        }
        if !self.ask.pending.insert(index) {
            return Vec::new();
        }
        q.error = None;
        vec![Effect::Save, Effect::Ask { index }]
    }

    /// The answer to question `index` came (or failed). A question asked
    /// with no unit on show goes with the unit the answer relates to most.
    fn answered(&mut self, index: usize, result: Result<Answer, String>) -> Vec<Effect> {
        self.ask.pending.remove(&index);
        let Some(q) = self.review.questions.get_mut(index) else {
            return Vec::new();
        };
        match result {
            Ok(a) => {
                if q.unit.is_none() {
                    q.unit = a.related.first().map(|r| r.id.clone());
                }
                q.answer = Some(a);
                q.error = None;
            }
            Err(e) => {
                crate::log::write(&format!("question {} failed: {e}", index + 1));
                q.error = Some(e);
            }
        }
        vec![Effect::Save]
    }

    /// Follow link `i` of [`App::ask_links`]: a unit is selected in the
    /// list (the view closes), or from a chat, among the flagged units when
    /// it is one; code opens in the editor.
    fn follow_ask_link(&mut self, i: usize) -> Vec<Effect> {
        let Some((_, link)) = self.ask_links().into_iter().nth(i) else {
            return Vec::new();
        };
        match link {
            AskLink::Code { file, line } => vec![Effect::Edit { file, line }],
            AskLink::Unit(id) => {
                // From the chat on the Flagged screen, stay there.
                let stay = self.overlay != Overlay::Ask;
                let Some(effects) = self.go_to_unit(&id, stay) else {
                    return Vec::new();
                };
                if self.screen == Screen::Slim {
                    self.ask.link = None;
                }
                effects
            }
        }
    }

    // ---- Chat (a flagged unit's conversation) --------------------------------

    /// Whether keys go into the chat's question.
    fn chat_typing(&self) -> bool {
        self.screen == Screen::Flagged
            && self.flagged.typing
            && self.flagged_unit().map(|u| self.flagged_focus(u)) == Some(Pane::Chat)
    }

    /// Give the flagged unit's chat the focus, typing, at its end.
    fn open_chat(&mut self) {
        if self.flagged_unit().is_none() {
            return;
        }
        self.focus(Pane::Chat);
        self.flagged.typing = true;
        self.ask.chat_scroll.set(None);
    }

    /// Keys while typing in the chat: letters go into the question, `enter`
    /// asks it, `ctrl+r` asks the chat's last question again, `ctrl+u`
    /// clears, `↑ ↓` PageUp PageDown scroll, `esc` stops typing (the keys of
    /// the screen work again), `tab` `shift+tab` move to another tile.
    fn chat_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let page = self.detail_rows.get().saturating_sub(2).max(1) as isize;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => self.flagged.typing = false,
            KeyCode::Tab => self.flagged_tile_step(true),
            KeyCode::BackTab => self.flagged_tile_step(false),
            KeyCode::Enter => return self.send_chat(),
            KeyCode::Down => return self.chat_scroll(SCROLL_STEP),
            KeyCode::Up => return self.chat_scroll(-SCROLL_STEP),
            KeyCode::PageDown => return self.chat_scroll(page),
            KeyCode::PageUp => return self.chat_scroll(-page),
            KeyCode::Char('r') if ctrl => return self.ask_chat_again(),
            _ => {
                typed(&mut self.ask.draft, &k, QUESTION_MAX);
            }
        }
        Vec::new()
    }

    /// Scroll the chat `by` lines; at the end it follows new lines.
    fn chat_scroll(&mut self, by: isize) -> Vec<Effect> {
        let max = self.ask.chat_max.get();
        let to = self
            .ask
            .chat_scroll
            .get()
            .unwrap_or(max)
            .saturating_add_signed(by)
            .min(max);
        self.ask.chat_scroll.set((to < max).then_some(to));
        Vec::new()
    }

    /// Ask what was typed in the chat, about the flagged unit.
    fn send_chat(&mut self) -> Vec<Effect> {
        let Some(id) = self.flagged_unit_id() else {
            return Vec::new();
        };
        let text = std::mem::take(&mut self.ask.draft);
        self.ask.chat_scroll.set(None);
        self.push_question(&text, Some(id))
    }

    /// Ask the chat's last question again.
    fn ask_chat_again(&mut self) -> Vec<Effect> {
        let last = self
            .flagged_unit_id()
            .and_then(|id| self.chat_of(&id).last().copied());
        match last {
            Some(n) => self.ask_question(n),
            None => Vec::new(),
        }
    }

    // ---- Triage in the list ---------------------------------------------

    /// Position of the next pending unit after `from` in `order`.
    pub fn next_pending_pos(&self, from: usize) -> Option<usize> {
        (from + 1..self.order.len()).find(|&p| self.pending_at(p))
    }

    /// Position of the previous pending unit before `from` in `order`.
    pub fn prev_pending_pos(&self, from: usize) -> Option<usize> {
        (0..from.min(self.order.len()))
            .rev()
            .find(|&p| self.pending_at(p))
    }

    fn pending_at(&self, pos: usize) -> bool {
        self.review.units[self.order[pos]].decision == Decision::Pending
    }

    fn jump_pending(&mut self, forward: bool) -> Vec<Effect> {
        let to = if forward {
            self.next_pending_pos(self.sel)
        } else {
            self.prev_pending_pos(self.sel)
        };
        match to {
            Some(p) => self.move_sel(p),
            None => {
                self.status = "No more pending units that way.".into();
                Vec::new()
            }
        }
    }

    /// Go to the first unit of the next file, or (not `forward`) of the one
    /// above. No wrapping: from the last (first) file the cursor stays.
    fn jump_file(&mut self, forward: bool) -> Vec<Effect> {
        if self.sel >= self.order.len() {
            return Vec::new();
        }
        let range = self.file_range(self.sel);
        let to = if forward {
            Some(range.end).filter(|&p| p < self.order.len())
        } else {
            range.start.checked_sub(1)
        };
        match to {
            Some(p) => self.move_sel(self.file_start(p)),
            None => {
                self.status = if forward { "last file" } else { "first file" }.into();
                Vec::new()
            }
        }
    }

    /// Ask to accept every pending unit in the file the cursor is in.
    /// Flagged and already accepted units are left alone; with nothing
    /// pending there is nothing to confirm.
    fn ask_accept_file(&mut self) {
        if self.sel >= self.order.len() {
            return;
        }
        let ids: Vec<String> = self
            .file_range(self.sel)
            .map(|p| &self.review.units[self.order[p]])
            .filter(|u| u.decision == Decision::Pending)
            .map(|u| u.id.clone())
            .collect();
        if ids.is_empty() {
            self.status = "nothing pending in this file".into();
            return;
        }
        let file = self.file_at(self.sel).clone();
        self.overlay = Overlay::ConfirmAcceptFile { file, ids };
    }

    /// Accept the confirmed units as one undo step. The selection stays.
    fn accept_file(&mut self, ids: Vec<String>) -> Vec<Effect> {
        let was_done = self.all_reviewed();
        let mut entries = Vec::new();
        for id in ids {
            if let Some(u) = self.review.unit_mut(&id)
                && u.decision == Decision::Pending
            {
                entries.push((id, std::mem::replace(&mut u.decision, Decision::Accepted)));
            }
        }
        if entries.is_empty() {
            return Vec::new();
        }
        self.status = format!("Accepted {} units.", entries.len());
        self.swipe.history.push(Undo { entries });
        self.check_all_done(was_done);
        vec![Effect::Save]
    }

    /// Apply `d` to the selected unit and move on to the next pending one.
    /// Recorded in the shared undo history.
    pub fn decide(&mut self, d: Decision) -> Vec<Effect> {
        let Some(id) = self.selected_id() else {
            return Vec::new();
        };
        self.apply_decision(id, d);
        if let Some(p) = self.next_pending_pos(self.sel) {
            self.sel = p;
        }
        let mut effects = vec![Effect::Save];
        effects.extend(self.ensure_prepared());
        effects
    }

    /// Set one unit's decision and record it for undo.
    fn apply_decision(&mut self, id: String, d: Decision) {
        let was_done = self.all_reviewed();
        self.set_decision(id, d);
        self.check_all_done(was_done);
    }

    fn set_decision(&mut self, id: String, d: Decision) {
        let Some(u) = self.review.unit_mut(&id) else {
            return;
        };
        let prev = std::mem::replace(&mut u.decision, d);
        self.swipe.history.push(Undo {
            entries: vec![(id, prev)],
        });
    }

    fn back(&mut self) {
        self.screen = self.return_to.pop().unwrap_or(Screen::Slim);
    }

    // ---- Editor and reset ------------------------------------------------

    /// The unit the current screen is about.
    fn current_unit(&self) -> Option<&Unit> {
        match self.screen {
            Screen::Swipe => self.swipe_current().and_then(|id| self.review.unit(id)),
            Screen::Flagged => self.flagged_unit(),
            _ => self.selected(),
        }
    }

    /// Open the current unit's file in the editor, at the unit's first line.
    fn edit(&mut self) -> Vec<Effect> {
        match self.current_unit() {
            Some(u) => {
                // A whole file opens at its first change, not at its top.
                let first = (u.kind == UnitKind::File)
                    .then(|| super::diff::first_change(&self.review, u))
                    .flatten();
                vec![Effect::Edit {
                    file: u.file.clone(),
                    line: first.unwrap_or(u.display_line()).max(1),
                }]
            }
            None => Vec::new(),
        }
    }

    /// Forget every decision, as if nothing had been looked at.
    fn reset(&mut self) -> Vec<Effect> {
        for u in &mut self.review.units {
            u.decision = Decision::Pending;
        }
        if self.screen == Screen::Swipe {
            self.swipe = self.fresh_swipe();
        } else {
            self.swipe = SwipeState::default();
        }
        self.status = "Reset: nothing is marked as looked at.".into();
        vec![Effect::Save]
    }

    // ---- Swipe -----------------------------------------------------------

    /// The cards on entering swipe: units with a behavior change that are
    /// flagged or not yet looked at (accepted ones are left out), most
    /// attention first, then in list order.
    fn swipe_cards(&self) -> Vec<String> {
        let mut cards: Vec<usize> = self
            .order
            .iter()
            .copied()
            .filter(|&i| self.review.units[i].status != UnitStatus::NoBehaviorChange)
            .filter(|&i| self.review.units[i].decision != Decision::Accepted)
            .collect();
        cards.sort_by_key(|&i| std::cmp::Reverse(self.levels[i].level));
        cards
            .into_iter()
            .map(|i| self.review.units[i].id.clone())
            .collect()
    }

    /// Cards with the cursor on the first one (the summary when empty).
    /// Cards are never removed during a session, so `h` can return to one
    /// just accepted.
    fn fresh_swipe(&self) -> SwipeState {
        let cards = self.swipe_cards();
        SwipeState {
            cards,
            cursor: 0,
            history: Vec::new(),
            drag: DragTracker::default(),
        }
    }

    /// The card on show, `None` on the summary.
    pub fn swipe_current(&self) -> Option<&String> {
        self.swipe.cards.get(self.swipe.cursor)
    }

    pub fn enter_swipe(&mut self) {
        self.swipe = self.fresh_swipe();
        if self.screen != Screen::Swipe {
            if self.screen == Screen::Slim {
                self.return_to.clear();
            }
            self.return_to.push(Screen::Slim);
        }
        self.screen = Screen::Swipe;
    }

    pub fn swipe_dir(&mut self, dir: Dir) -> Vec<Effect> {
        let d = match dir {
            Dir::Right => Decision::Accepted,
            Dir::Left => Decision::Investigate,
        };
        let Some(id) = self.swipe_current().cloned() else {
            return Vec::new();
        };
        self.apply_decision(id, d);
        self.swipe.cursor += 1;
        let mut effects = vec![Effect::Save];
        effects.extend(self.prepare_cards());
        effects
    }

    pub fn undo(&mut self) -> Vec<Effect> {
        let Some(step) = self.swipe.history.pop() else {
            self.status = "Nothing to undo.".into();
            return Vec::new();
        };
        for (id, prev) in &step.entries {
            if let Some(unit) = self.review.unit_mut(id) {
                unit.decision = *prev;
            }
        }
        let Some((first, _)) = step.entries.first() else {
            return vec![Effect::Save];
        };
        if self.screen == Screen::Swipe
            && let Some(pos) = self.swipe.cards.iter().position(|id| id == first)
        {
            self.swipe.cursor = pos;
        }
        let mut effects = vec![Effect::Save];
        if self.screen == Screen::Slim
            && let Some(pos) = self
                .order
                .iter()
                .position(|&i| self.review.units[i].id == *first)
        {
            effects.extend(self.move_sel(pos));
        }
        effects
    }

    /// Back one card, decisions untouched. From the summary: the last card.
    fn prev_card(&mut self) {
        if self.swipe.cursor == 0 {
            self.status = "Already at the first card.".into();
        } else {
            self.swipe.cursor -= 1;
        }
    }

    fn swipe_action(&mut self, a: Action) -> Vec<Effect> {
        match a {
            Action::Accept => self.swipe_dir(Dir::Right),
            Action::Investigate => self.swipe_dir(Dir::Left),
            Action::Flagged => {
                self.start_flagged();
                Vec::new()
            }
            Action::Undo => self.undo(),
            Action::PrevCard => {
                self.prev_card();
                Vec::new()
            }
            Action::Back | Action::Quit | Action::Swipe => {
                self.back();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    // ---- Mouse -----------------------------------------------------------

    fn hit(&self, col: u16, row: u16) -> Option<Target> {
        self.targets
            .borrow()
            .iter()
            .rev()
            .find(|(r, _)| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height)
            .map(|(_, t)| *t)
    }

    /// The text area under (`col`, `row`) as last drawn.
    fn text_area_at(&self, col: u16, row: u16) -> select::Area {
        select::area_at(
            &self.text_areas.borrow(),
            self.shown.borrow().area,
            col,
            row,
        )
    }

    /// A second click on the same cell soon after the first.
    fn double_click(&mut self, col: u16, row: u16) -> bool {
        let now = std::time::Instant::now();
        let double = self.last_click.is_some_and(|(at, c, r)| {
            (c, r) == (col, row) && now.duration_since(at) <= select::DOUBLE_CLICK
        });
        self.last_click = (!double).then_some((now, col, row));
        double
    }

    /// Copy what the selection marks; it stays marked.
    fn copy_selection(&mut self, mut s: select::Selection) -> Vec<Effect> {
        let text = select::text(&self.shown.borrow(), &s);
        if text.trim().is_empty() {
            self.selection = None;
            return Vec::new();
        }
        s.copied = true;
        self.selection = Some(s);
        vec![Effect::Copy {
            what: select::name(&text),
            text,
        }]
    }

    /// Copy the word under (`col`, `row`), marked like a selection.
    fn copy_word(&mut self, col: u16, row: u16) -> Vec<Effect> {
        let area = self.text_area_at(col, row);
        let Some((word, x0, x1)) = select::word_at(&self.shown.borrow(), area, col, row) else {
            return Vec::new();
        };
        let mut s = select::Selection::start(area, x0, row);
        s.extend(x1, row);
        s.copied = true;
        self.selection = Some(s);
        vec![Effect::Copy {
            what: select::name(&word),
            text: word,
        }]
    }

    pub fn on_mouse(&mut self, m: MouseEvent) -> Vec<Effect> {
        // The button going down starts a selection, a drag extends it, and
        // the release copies it; a release where it went down is a click.
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let area = self.text_area_at(m.column, m.row);
                self.selection = Some(select::Selection::start(area, m.column, m.row));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(s) = self.selection.as_mut() {
                    s.extend(m.column, m.row);
                }
            }
            // The text moves away under it.
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => self.selection = None,
            _ => {}
        }
        if self.screen == Screen::Swipe
            && self.overlay == Overlay::None
            && self.swipe_current().is_some()
        {
            let card = self.swipe_card.get();
            let sideways = self.swipe.drag.sideways();
            if let Some(dir) = self.swipe.drag.on_mouse(m, card) {
                self.selection = None;
                return self.swipe_dir(dir);
            }
            // A swipe that didn't reach the threshold moved the card, and
            // isn't a selection or a click.
            if sideways && matches!(m.kind, MouseEventKind::Up(_)) {
                self.selection = None;
                return Vec::new();
            }
        }
        if m.kind == MouseEventKind::Up(MouseButton::Left) {
            match self.selection.take() {
                Some(s) if s.moved() && !s.copied => return self.copy_selection(s),
                _ => {}
            }
            // A double click copies the word, except on what a click does something with.
            let button = matches!(
                self.hit(m.column, m.row),
                Some(Target::Action(_) | Target::Key(_) | Target::AskLink(_) | Target::Note(_))
            );
            if self.double_click(m.column, m.row) && !button {
                return self.copy_word(m.column, m.row);
            }
        }
        if self.overlay == Overlay::Ask {
            return self.ask_mouse(m);
        }
        match m.kind {
            // Over the detail area, and anywhere in swipe, the wheel scrolls
            // it; elsewhere it moves the selection.
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = m.kind == MouseEventKind::ScrollDown;
                // A link here is in the chat.
                let hit = match self.hit(m.column, m.row) {
                    Some(Target::AskLink(_)) => Some(Target::Tile(Pane::Chat)),
                    hit => hit,
                };
                if let Some(Target::Tile(pane)) = hit
                    && pane != self.flagged.pane
                {
                    self.focus(pane);
                }
                if self.screen == Screen::Swipe
                    || matches!(hit, Some(Target::Detail | Target::Tile(_)))
                {
                    return self.scroll_detail(if down { SCROLL_STEP } else { -SCROLL_STEP });
                }
                self.on_action(if down { Action::Down } else { Action::Up })
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.overlay != Overlay::None {
                    match (self.hit(m.column, m.row), &self.overlay) {
                        (Some(Target::Key(k)), _) => return self.on_key(k),
                        (Some(Target::Note(i)), Overlay::Notes { .. }) => {
                            self.overlay = Overlay::Notes { sel: i };
                        }
                        (_, Overlay::AllDone) => self.overlay = Overlay::None,
                        _ => {}
                    }
                    return Vec::new();
                }
                match self.hit(m.column, m.row) {
                    Some(Target::Key(k)) => self.on_key(k),
                    Some(Target::Note(_)) => Vec::new(),
                    Some(Target::Action(a)) => self.on_action(a),
                    Some(Target::Unit(pos)) => self.move_sel(pos),
                    // A click on the chat types in it.
                    Some(Target::Tile(Pane::Chat)) => {
                        self.open_chat();
                        Vec::new()
                    }
                    Some(Target::Tile(pane)) => {
                        self.focus(pane);
                        Vec::new()
                    }
                    Some(Target::AskLink(i)) => self.follow_ask_link(i),
                    Some(Target::Detail) | None => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    // ---- Background events ----------------------------------------------

    pub fn on_app_event(&mut self, ev: AppEvent) -> Vec<Effect> {
        match ev {
            AppEvent::Answer { index, result } => return self.answered(index, result),
            AppEvent::CommitDrafted(r) => self.commit_drafted(r),
            AppEvent::PrReviewed { verdict, result } => {
                self.approving = false;
                match result {
                    Ok(msg) => {
                        if verdict == Verdict::Approve
                            && let Some(p) = self.review.pr.as_mut()
                        {
                            p.approved_head = p.head_sha.clone();
                        }
                        self.status = msg;
                        return vec![Effect::Save];
                    }
                    Err(msg) => self.status = msg,
                }
            }
            AppEvent::Prep(p) => {
                self.review.apply(p);
                self.rebuild_order();
                if !self.review.units.iter().any(|u| u.prep == Prep::Pending) {
                    let mut effects = vec![Effect::Save];
                    // Every unit described: one more cheap call sums the change up.
                    if self.can_ask()
                        && !self.summary_asked
                        && crate::review::intent::wants_summary(&self.review)
                    {
                        self.summary_asked = true;
                        effects.push(Effect::Summarize);
                    }
                    return effects;
                }
            }
            AppEvent::Summary(r) => match r {
                Ok(s) => {
                    self.review.intent.summary = Some(s);
                    return vec![Effect::Save];
                }
                Err(e) => crate::log::write(&format!("summing up the change failed: {e}")),
            },
            AppEvent::Rerun(Ok(new)) => {
                let mut new = *new;
                crate::review::store::merge_previous(&mut new, &self.review);
                new.agent_session = self.review.agent_session.clone();
                // Positions in the old order mean nothing in the new review:
                // remember ids, drop the positions, restore by id.
                let keep = self.selected().map(|u| u.id.clone());
                self.review = new;
                self.syntax.clear();
                self.committed = None;
                self.summary_asked = false;
                self.order.clear();
                self.sel = 0;
                self.rebuild_order();
                let pos_of = |app: &App, id: &str| {
                    app.order
                        .iter()
                        .position(|&i| app.review.units.get(i).is_some_and(|u| u.id == id))
                };
                if let Some(pos) = keep.and_then(|id| pos_of(self, &id)) {
                    self.sel = pos;
                }
                self.screen = Screen::Slim;
                self.return_to.clear();
                self.status = format!("Review updated: {} units.", self.review.units.len());
                let mut effects = vec![Effect::Save];
                effects.extend(self.prepare_start());
                return effects;
            }
            AppEvent::Rerun(Err(e)) => self.status = format!("Re-run failed: {e}"),
            AppEvent::Dive(ev) => {
                match &ev {
                    DiveEvent::Progress { id, kind, text } => {
                        self.progress.insert((id.clone(), *kind), text.clone());
                    }
                    DiveEvent::Ready { id, kind, .. } => {
                        self.progress.remove(&(id.clone(), *kind));
                    }
                    DiveEvent::Failed { id, kind, error } => {
                        self.progress.remove(&(id.clone(), *kind));
                        self.status = format!("Question ({kind:?}) failed: {error}");
                    }
                }
                if self.review.apply_dive(&ev) {
                    return vec![Effect::Save];
                }
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::UnitKind;

    fn app(n: usize) -> App {
        let units = (0..n)
            .map(|i| Unit {
                id: format!("u{i}"),
                file: PathBuf::from(format!("f{}.rs", i / 3)),
                symbol: format!("s{i}"),
                kind: UnitKind::Function,
                status: UnitStatus::Changed,
                old_range: None,
                new_range: None,
                renamed_from: None,
                fingerprint: String::new(),
                changes: Vec::new(),
                prep: Prep::Skipped,
                callers: Vec::new(),
                callees: Vec::new(),
                decision: Decision::Pending,
                summary: None,
                nbc_reason: None,
                dive: Default::default(),
                dive_pending: Default::default(),
                checks: Default::default(),
            })
            .collect();
        let review = Review {
            id: "t".into(),
            repo_root: PathBuf::new(),
            base: String::new(),
            head: String::new(),
            head_commit: String::new(),
            agent_session: None,
            pr: None,
            intent: Default::default(),
            created_at: chrono::Utc::now(),
            files: Vec::new(),
            units,
            skipped: Vec::new(),
            graph: Default::default(),
            questions: Vec::new(),
        };
        let mut a = App::new(review, Config::default(), KeyMap::default());
        a.sel = 0;
        a
    }

    fn dec(a: &App, i: usize) -> Decision {
        a.review.units[i].decision
    }

    #[test]
    fn decide_selected_advances_and_undoes() {
        let mut a = app(3);
        a.decide(Decision::Accepted);
        assert_eq!(dec(&a, 0), Decision::Accepted);
        assert_eq!(a.sel, 1);
        a.undo();
        assert_eq!(dec(&a, 0), Decision::Pending);
    }

    fn click(a: &mut App, pos: usize, modifiers: KeyModifiers) {
        a.screen = Screen::Slim;
        *a.targets.borrow_mut() = vec![(Rect::new(0, pos as u16, 10, 1), Target::Unit(pos))];
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 1,
            row: pos as u16,
            modifiers,
        });
    }

    #[test]
    fn click_selects_a_row_and_a_second_click_opens_it() {
        let mut a = app(5);
        click(&mut a, 2, KeyModifiers::NONE);
        assert_eq!(a.sel, 2);
        assert_eq!(a.screen, Screen::Slim);
        // Modifiers change nothing: there is no multi-select.
        click(&mut a, 4, KeyModifiers::CONTROL);
        assert_eq!(a.sel, 4);
        click(&mut a, 1, KeyModifiers::SHIFT);
        assert_eq!(a.sel, 1);
        assert!(
            a.review
                .units
                .iter()
                .all(|u| u.decision == Decision::Pending)
        );
    }

    #[test]
    fn next_pending_skips_decided_and_files_jump() {
        let mut a = app(7);
        a.review.units[1].decision = Decision::Accepted;
        a.review.units[2].decision = Decision::Investigate;
        assert_eq!(a.next_pending_pos(0), Some(3));
        assert_eq!(a.prev_pending_pos(3), Some(0));
        assert_eq!(a.prev_pending_pos(0), None);
        a.on_action(Action::NextGroup);
        assert_eq!(a.sel, 3);
        a.on_action(Action::NextGroup);
        assert_eq!(a.sel, 6);
        a.on_action(Action::PrevGroup);
        assert_eq!(a.sel, 3);
    }

    fn review_of(app: &App, n: usize) -> Review {
        let mut r = app.review.clone();
        r.units.truncate(n);
        r
    }

    #[test]
    fn rerun_with_fewer_units_keeps_selection_sane() {
        let mut a = app(10);
        a.sel = 9;
        let new = review_of(&a, 6);
        a.on_app_event(AppEvent::Rerun(Ok(Box::new(new))));
        assert_eq!(a.order.len(), 6);
        assert!(a.sel < 6);
        // A surviving selection is kept by id.
        let mut a = app(10);
        a.sel = 4;
        let new = review_of(&a, 6);
        a.on_app_event(AppEvent::Rerun(Ok(Box::new(new))));
        assert_eq!(a.sel, 4);
    }

    #[test]
    fn k_is_plain_up_and_investigate_has_three_keys() {
        let key = |c| KeyEvent::from(KeyCode::Char(c));
        let mut a = app(3);
        for screen in [Screen::Slim, Screen::Swipe] {
            a.screen = screen;
            if screen != Screen::Swipe {
                assert_eq!(a.action_for(&key('k')), Some(Action::Up), "{screen:?}");
            }
            assert_eq!(a.action_for(&key('d')), Some(Action::Flagged), "{screen:?}");
        }
        a.screen = Screen::Swipe;
        assert_eq!(a.action_for(&key('h')), Some(Action::PrevCard));
        for screen in [Screen::Slim, Screen::Swipe] {
            a.screen = screen;
            assert_eq!(a.action_for(&key('i')), Some(Action::Investigate));
            if screen == Screen::Slim {
                assert_eq!(a.action_for(&key('h')), Some(Action::Investigate));
            }
            assert_eq!(
                a.action_for(&KeyEvent::from(KeyCode::Left)),
                Some(Action::Investigate)
            );
        }
        // `h` follows a rebinding instead of shadowing it.
        a.screen = Screen::Slim;
        let ov = [("reset".to_string(), "h".to_string())]
            .into_iter()
            .collect();
        a.keys = KeyMap::with_overrides(&ov).unwrap();
        assert_eq!(a.action_for(&key('h')), Some(Action::Reset));
    }

    #[test]
    fn swipe_has_two_directions() {
        let mut a = app(4);
        a.enter_swipe();
        assert_eq!(a.swipe.cards.len(), 4);
        a.swipe_dir(Dir::Right);
        a.swipe_dir(Dir::Left);
        assert_eq!(
            (dec(&a, 0), dec(&a, 1)),
            (Decision::Accepted, Decision::Investigate)
        );
        assert_eq!(a.swipe.cursor, 2);
        assert_eq!(a.decision_counts(), (1, 1, 2));
        // Up is plain navigation, never a decision.
        a.on_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(a.swipe.cursor, 2);
        assert_eq!(dec(&a, 2), Decision::Pending);
        // Keys: l accepts, i and Left investigate, undo restores the card.
        a.on_key(KeyEvent::from(KeyCode::Char('l')));
        a.on_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(
            (dec(&a, 2), dec(&a, 3)),
            (Decision::Accepted, Decision::Investigate)
        );
        assert!(a.swipe_current().is_none());
        a.undo();
        assert_eq!(a.swipe_current().map(String::as_str), Some("u3"));
        // Re-entering drops the accepted cards and starts on the first one.
        a.enter_swipe();
        assert_eq!(a.swipe.cards, vec!["u1".to_string(), "u3".to_string()]);
        assert_eq!(a.swipe_current().map(String::as_str), Some("u1"));
        assert_eq!(a.flagged_units().len(), 1);
    }

    #[test]
    fn d_with_nothing_flagged_stays_put() {
        let mut a = app(2);
        a.on_key(KeyEvent::from(KeyCode::Char('d')));
        assert_eq!(a.screen, Screen::Slim);
        assert!(a.status.contains("Nothing flagged"));
    }

    #[test]
    fn the_next_key_dismisses_a_stale_status_message() {
        let mut a = app(2);
        a.on_key(KeyEvent::from(KeyCode::Char('d')));
        assert!(a.status.contains("Nothing flagged"));
        a.on_key(KeyEvent::from(KeyCode::Char('j')));
        assert!(a.status.is_empty(), "status was {:?}", a.status);
        // An action that reports something still shows its message.
        a.on_key(KeyEvent::from(KeyCode::Char('d')));
        assert!(a.status.contains("Nothing flagged"));
    }

    #[test]
    fn swipe_starts_on_the_first_card_and_flagged_ones_are_shown_again() {
        let mut a = app(3);
        a.review.units[0].decision = Decision::Investigate;
        a.enter_swipe();
        assert_eq!(a.swipe.cards.len(), 3);
        assert_eq!(a.swipe.cursor, 0);
    }

    /// A large review with an agent and nothing asked yet: `n` units, three
    /// per file.
    fn large(n: usize) -> App {
        let mut a = app(n);
        a.large = true;
        a.has_agent = true;
        a.review.agent_session = Some(SessionRef::default());
        for u in &mut a.review.units {
            u.prep = Prep::NotRequested;
        }
        a.review.files = (0..n.div_ceil(3))
            .map(|i| crate::git::ChangedFile {
                path: PathBuf::from(format!("f{i}.rs")),
                old_path: None,
                status: crate::git::FileStatus::Modified,
                old_content: None,
                new_content: None,
            })
            .collect();
        a
    }

    fn asked(effects: &[Effect]) -> Vec<String> {
        let mut ids: Vec<String> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::Prepare(ids) => Some(ids.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn swipe_asks_ahead_for_the_cards_nobody_asked_about() {
        let mut a = large(40);
        let effects = a.on_action(Action::Swipe);
        assert_eq!(a.screen, Screen::Swipe);
        let mut window = a.swipe.cards[..=SWIPE_AHEAD].to_vec();
        window.sort();
        assert_eq!(
            asked(&effects),
            window,
            "the card on show and the next ones"
        );
        for id in &window {
            assert_eq!(a.review.unit(id).unwrap().prep, Prep::Pending);
        }
        // Each step asks for the one card that came into the window.
        let effects = a.swipe_dir(Dir::Right);
        assert_eq!(asked(&effects), [a.swipe.cards[SWIPE_AHEAD + 1].clone()]);
        // Nothing is asked twice: back a card and forward again.
        a.prev_card();
        assert!(asked(&a.swipe_dir(Dir::Right)).is_empty());
        // Near the end the window just gets shorter.
        a.swipe.cursor = a.swipe.cards.len() - 1;
        assert!(asked(&a.swipe_dir(Dir::Right)).len() <= 1);
    }

    #[test]
    fn swipe_in_a_small_review_or_without_an_agent_asks_nothing_more() {
        let mut small = large(10);
        small.large = false;
        assert!(asked(&small.on_action(Action::Swipe)).is_empty());
        let mut no_agent = large(40);
        no_agent.has_agent = false;
        assert!(asked(&no_agent.on_action(Action::Swipe)).is_empty());
        assert!(asked(&no_agent.swipe_dir(Dir::Right)).is_empty());
    }

    #[test]
    fn deciding_prepares_the_row_it_advances_to() {
        let mut a = large(40);
        // Decide the first file's three units; the last one moves on to the
        // next file, which is prepared.
        a.decide(Decision::Accepted);
        a.decide(Decision::Accepted);
        let effects = a.decide(Decision::Accepted);
        assert!(effects.iter().any(|e| matches!(e, Effect::Prepare(_))));
        assert_eq!(a.sel, 3);
    }

    #[test]
    fn slim_undo_selects_the_undone_unit() {
        let mut a = app(6);
        a.sel = 1;
        a.decide(Decision::Accepted);
        a.sel = 3;
        a.decide(Decision::Accepted);
        a.sel = 5;
        a.undo();
        assert_eq!(a.sel, 3);
        assert_eq!(dec(&a, 3), Decision::Pending);
        assert_eq!(dec(&a, 1), Decision::Accepted);
    }
}
