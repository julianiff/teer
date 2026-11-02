//! App state and input handling. Handlers are synchronous and return
//! [`Effect`]s for the runtime to carry out, so all of this is testable
//! without a terminal or an agent.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::select;
use super::swipe::{Dir, DragTracker};
use super::theme::Theme;
use crate::agent::SessionInfo;
use crate::approve::{self, EditOutcome, Inline, NoteEntry, Verdict};
use crate::commit::{self, Branch};
use crate::config::{Config, Order, Provider};
use crate::keymap::{Action, Edit, Keymap, Outcome, Sheet};
use crate::review::ask::Progress;
use crate::review::attention::{Assessment, Level, accepted_for_you, assess_all, settle};
use crate::review::dive::{DiveEvent, uses_web};
use crate::review::history::{Change, Note, NoteChange, Step};
use crate::review::model::{
    AcceptedBy, Answer, Decision, DiveKind, PrInfo, Prep, Question, Resolution, Review, SessionRef,
    Unit, UnitKind,
};
use crate::review::prepare::PrepareEvent;
use crate::review::story;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The queue (Phase 4): what teer opens on.
    Queue,
    /// One unit, any unit, with its answers (Phase 5).
    Unit,
    /// The v1 list, reachable with `--swipe` until Phase 8.
    Slim,
    Swipe,
    /// Every unit marked investigate, with the agent questions (v1, until
    /// Phase 8; the unit view replaces it).
    Flagged,
}

/// What the queue's level boxes hold (`toggle.by_file` switches).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grouping {
    /// Files, each in the box of the most attention one of its units
    /// needs, its units under its name (the v1 list's default).
    File,
    /// Units, each in the box of its own level, its file named on its row:
    /// the queue's order (level, then file, callers before callees).
    #[default]
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
        /// The other units the note goes on (`F`, `f` on a selection): the
        /// text is copied to each.
        also: Vec<String>,
        /// The flag that opened the line made the undo step at this index,
        /// and the note joins it, so `u` takes both back.
        joins: Option<usize>,
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
    /// Asks once per session before a Flagged-screen question, or the first
    /// question of the unit view's ask panel, may use the web.
    ConfirmWeb,
    /// Asks before accepting every pending unit of a file in the v1 list:
    /// the file and the ids that would change. (The queue acts at once.)
    ConfirmAcceptFile {
        file: PathBuf,
        ids: Vec<String>,
    },
    /// Asks before flagging every pending unit of a file (`F` in the v1 list).
    ConfirmFlagFile {
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
    /// The `:w` sheet (Phase 6, see [`super::finish`]): one default next
    /// step for the decisions made, its alternatives, the notes.
    Finish {
        sel: usize,
    },
    /// Units to jump to (see [`super::jump`]): found by file or name as
    /// `query` is typed (`/`), or what the unit calls (`gd`) or what calls
    /// it (`gr`); `sel` indexes [`App::jump_items`].
    Jump {
        kind: JumpKind,
        query: String,
        sel: usize,
    },
}

/// What a jump list lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JumpKind {
    /// Every listed unit whose file or name matches the query.
    Find,
    /// What unit `of` calls, in this review.
    Definition { of: String },
    /// What calls unit `of`, in this review.
    Callers { of: String },
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
    /// What came of those so far, by index.
    pub live: BTreeMap<usize, LiveAnswer>,
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

/// A question on its way: the agent's latest step, how many it took, and
/// the answer's text as far as it has come.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LiveAnswer {
    pub step: Option<String>,
    pub steps: usize,
    pub text: String,
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

/// Lines the page keys scroll the Ask view by.
const ASK_PAGE: isize = 10;

/// Edit `text` with `e`, a key of a line being typed ([`Keymap::edit`]): a
/// character goes in while it is shorter than `max`, backspace takes the
/// last off, [`Edit::ClearLine`] clears it. False for any other key.
pub(super) fn typed(text: &mut String, e: Edit, max: usize) -> bool {
    match e {
        Edit::Backspace => {
            text.pop();
        }
        Edit::ClearLine => text.clear(),
        Edit::Char(c) => {
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

/// "1 unit", "3 units".
fn units(n: usize) -> String {
    if n == 1 {
        "1 unit".into()
    } else {
        format!("{n} units")
    }
}

/// What deciding `n` units at once says: "Accepted 3 units."
fn decided_status(d: Decision, n: usize) -> String {
    let verb = match d {
        Decision::Investigate => "Flagged",
        _ => "Accepted",
    };
    format!("{verb} {}.", units(n))
}

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
    /// Save only `Review.cursor` (see `store::save_cursor`).
    SaveCursor,
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
    /// Suspend the review, run the settings screen (`teer setup`) and load
    /// the config and keys again (`<space>s`).
    Settings,
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
    /// What the agent is doing on question `index` while it answers.
    Asking {
        index: usize,
        progress: Progress,
    },
}

#[derive(Debug, Clone, Default)]
pub struct SwipeState {
    /// The cards of this session, in list order.
    pub cards: Vec<String>,
    /// The card on show; `cards.len()` means the summary.
    pub cursor: usize,
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
    /// The panel under the unit view's text: the wheel scrolls it, a click
    /// on the ask panel types.
    Panel,
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
    pub keys: Keymap,
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
    /// first, then the same. Units accepted for you are left out unless
    /// `show_auto`.
    pub order: Vec<usize>,
    pub grouping: Grouping,
    /// The units accepted for you are listed (`za`).
    pub show_auto: bool,
    /// Visual mode (`V`): the row the selection started on; it runs to
    /// `sel`.
    pub visual: Option<usize>,
    /// The last decision action and its count, for `.`.
    pub last_decision: Option<(Action, Option<u32>)>,
    /// How far each unit is indented under the units it is read beneath in
    /// its file and level, by index into `review.units` (0 in line order).
    pub depth: Vec<usize>,
    /// Attention level per unit, by index into `review.units`.
    pub levels: Vec<Assessment>,
    pub sel: usize,
    pub large: bool,

    pub swipe: SwipeState,
    pub flagged: FlaggedState,
    /// The unit view (see [`super::unit`]).
    pub unit: super::unit::UnitView,
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
    pub fn new(review: Review, cfg: Config, keys: Keymap) -> App {
        let theme = Theme::new(cfg.theme);
        let mut app = App {
            review,
            keys,
            theme,
            screen: Screen::Queue,
            return_to: Vec::new(),
            overlay: Overlay::None,
            has_agent: false,
            agent_note: None,
            sessions: Vec::new(),
            order: Vec::new(),
            grouping: Grouping::default(),
            show_auto: false,
            visual: None,
            last_decision: None,
            depth: Vec::new(),
            levels: Vec::new(),
            sel: 0,
            // Set by `rebuild_order`, as the rest of the order.
            large: false,
            swipe: SwipeState::default(),
            flagged: FlaggedState::default(),
            unit: Default::default(),
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
        app.sel = app.resume_at();
        app.review.cursor = app.selected_id();
        app
    }

    /// Where the review starts: on the unit it was left on
    /// (`Review.cursor`) while it is listed, else on the first pending unit
    /// that needs a close look, else the top.
    fn resume_at(&self) -> usize {
        let left_on = self.review.cursor.as_deref();
        left_on
            .and_then(|id| self.position_of(id))
            .or_else(|| {
                (0..self.order.len()).find(|&p| {
                    self.pending_at(p) && self.levels[self.order[p]].level == Level::Attention
                })
            })
            .unwrap_or(0)
    }

    /// Put the review on the v1 list (`--swipe`, until Phase 8): files in
    /// the box of their most critical unit, as v1 listed them.
    pub fn open_v1_list(&mut self) {
        self.screen = Screen::Slim;
        self.return_to.clear();
        self.grouping = Grouping::File;
        self.reorder();
    }

    /// The list the screen on show goes back to: the queue, or the v1 list
    /// from the v1 screens.
    fn list_screen(&self) -> Screen {
        match self.screen {
            Screen::Queue | Screen::Unit => Screen::Queue,
            Screen::Slim | Screen::Swipe | Screen::Flagged => Screen::Slim,
        }
    }

    /// Assess every unit again, then [`App::reorder`]. The selection stays
    /// on the same unit as levels change.
    pub fn rebuild_order(&mut self) {
        self.levels = assess_all(&self.review);
        self.large = self.review.units.len() > self.cfg.large_review;
        self.reorder();
    }

    /// The level unit `i` (index into `review.units`) is listed under (see
    /// [`Level::shown`]): units accepted for you in the box of their own.
    pub fn shown_level(&self, i: usize) -> Level {
        self.levels[i]
            .level
            .shown(self.review.units[i].auto_accepted())
    }

    /// Settle what the levels decide (reasons, units accepted for you: see
    /// [`settle`]) and list the units file by file: files by the most
    /// attention one of their units needs, then by path; inside a file most
    /// attention first, then by story (callers right before what they call)
    /// or line. Listed by unit, each unit goes by its own level instead of
    /// its file's: level, then path, then story. Units accepted for you are
    /// listed last, and only with `show_auto`. The selection stays on the
    /// same unit.
    fn reorder(&mut self) {
        let keep = self.selected().map(|u| u.id.clone());
        settle(&mut self.review, &self.levels, self.cfg.auto_accept);
        let units = &self.review.units;
        let level = |i: usize| self.shown_level(i);
        let listed: Vec<usize> = (0..units.len())
            .filter(|&i| self.show_auto || !units[i].auto_accepted())
            .collect();
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
        let level = |p: usize| self.shown_level(self.order[p]);
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
            .map(|p| self.shown_level(self.order[p]))
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

    /// `za`: list the units accepted for you, or hide them again; the
    /// cursor stays on its unit while it is listed.
    fn toggle_auto(&mut self) {
        let n = self.auto_accepted();
        if n == 0 && !self.show_auto {
            self.status = "No units are accepted for you.".into();
            return;
        }
        self.show_auto = !self.show_auto;
        self.reorder();
        self.status = if self.show_auto {
            format!("Showing {} accepted for you.", units(n))
        } else {
            format!("Hiding {} accepted for you.", units(n))
        };
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

    /// Start on the review, in the queue. `session` is the agent to ask: a
    /// fresh headless run, or the session given with `--session`. `note`
    /// explains why there is no agent, if there isn't one. `swipe` (the
    /// hidden `--swipe`) opens the v1 screens instead, on swipe.
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
        self.screen = Screen::Queue;
        if swipe {
            self.open_v1_list();
            self.enter_swipe();
        }
        effects.extend(self.prepare_start());
        effects.extend(self.prepare_cards());
        effects.extend(self.prefetch_start());
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

    /// (accepted, flagged, pending); units accepted for you count as accepted.
    pub fn decision_counts(&self) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for u in &self.review.units {
            match u.decision {
                Decision::Accepted => c.0 += 1,
                Decision::Investigate => c.1 += 1,
                Decision::Pending => c.2 += 1,
            }
        }
        c
    }

    /// How many units teer accepted for you.
    pub fn auto_accepted(&self) -> usize {
        self.review
            .units
            .iter()
            .filter(|u| u.auto_accepted())
            .count()
    }

    /// Every unit is accepted, by you or for you (and there is one).
    pub fn all_reviewed(&self) -> bool {
        let (accepted, flagged, pending) = self.decision_counts();
        accepted > 0 && flagged == 0 && pending == 0
    }

    /// What the detail area shows: the screen, the unit, the tab.
    fn detail_key(&self) -> String {
        match self.screen {
            Screen::Slim | Screen::Queue => {
                format!("list {}", self.selected().map_or("", |u| u.id.as_str()))
            }
            Screen::Swipe => format!("swipe {}", self.swipe_current().map_or("", String::as_str)),
            // Per unit; its tiles scroll on their own (`pane_scroll`).
            Screen::Flagged => {
                format!("flagged {}", self.flagged_unit_id().unwrap_or_default())
            }
            Screen::Unit => format!("unit {}", self.unit.id),
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
        if self.screen == Screen::Unit {
            return self.unit_scroll(by);
        }
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
        if self.keys.is_interrupt(&k) {
            return vec![Effect::Save, Effect::Quit];
        }
        // A status message answers the last action; the next key dismisses it.
        // Actions that report something set it again after this.
        self.status.clear();
        self.selection = None;
        let effects = match self.overlay_key(k) {
            Some(effects) => effects,
            None => self.normal_key(k),
        };
        self.save_cursor(effects)
    }

    /// Keep the unit under the cursor in the review (`Review.cursor`) and
    /// save it when it moved, so `q` and a crash both resume on it. A full
    /// save writes it too.
    fn save_cursor(&mut self, mut effects: Vec<Effect>) -> Vec<Effect> {
        let at = self.selected_id();
        if self.review.cursor != at {
            self.review.cursor = at;
            if !effects.contains(&Effect::Save) {
                effects.push(Effect::SaveCursor);
            }
        }
        effects
    }

    /// Keys of the overlay on show, or of the chat being typed: its text
    /// input, prompt or sheet. `None` passes the key on to the screen.
    fn overlay_key(&mut self, k: KeyEvent) -> Option<Vec<Effect>> {
        match self.overlay.clone() {
            Overlay::Ask => return Some(self.ask_key(k)),
            Overlay::Jump { .. } => return Some(self.jump_key(k)),
            Overlay::Finish { .. } => return Some(self.finish_key(k)),
            Overlay::Note { .. } => return Some(self.note_key(k)),
            Overlay::Notes { sel } => return Some(self.notes_key(sel, k)),
            Overlay::ConfirmDeleteNote { id, sel } => {
                self.overlay = Overlay::Notes { sel };
                if self.keys.confirm(&k)
                    && let Some(u) = self.review.unit_mut(&id)
                {
                    u.dive.note.clear();
                    u.dive.note_on = None;
                    if crate::notes::noted(&self.review).is_empty() {
                        self.overlay = Overlay::None;
                    }
                    self.status = "Note deleted.".into();
                    return Some(vec![Effect::Save]);
                }
                return Some(Vec::new());
            }
            Overlay::ConfirmWeb => {
                self.overlay = Overlay::None;
                let yes = self.keys.confirm(&k);
                return Some(self.answer_web(yes));
            }
            Overlay::ConfirmReset => {
                self.overlay = Overlay::None;
                if self.keys.confirm(&k) {
                    return Some(self.reset());
                }
                return Some(Vec::new());
            }
            Overlay::ConfirmAcceptFile { ids, .. } => {
                self.overlay = Overlay::None;
                if self.keys.confirm(&k) {
                    return Some(self.accept_file(ids));
                }
                return Some(Vec::new());
            }
            Overlay::ConfirmFlagFile { ids, .. } => {
                self.overlay = Overlay::None;
                if self.keys.confirm(&k) {
                    return Some(self.flag_file(ids));
                }
                return Some(Vec::new());
            }
            Overlay::Help => {
                self.overlay = Overlay::None;
                return Some(Vec::new());
            }
            Overlay::AllDone => {
                self.overlay = Overlay::None;
                match self.keys.sheet(&k) {
                    Some(Sheet::Quit) => return Some(vec![Effect::Save, Effect::Quit]),
                    Some(Sheet::Confirm) if self.committed.is_none() => {
                        if self.can_approve() {
                            self.ask_post(Verdict::Approve);
                        } else if self.can_commit() {
                            return Some(self.ask_commit());
                        }
                    }
                    _ => {}
                }
                return Some(Vec::new());
            }
            Overlay::Commit { .. } => return Some(self.commit_key(k)),
            Overlay::Commits { sel, unit } => return Some(self.commits_key(sel, unit, k)),
            Overlay::PostReview {
                verdict,
                notes,
                intro,
                inline_on,
                scroll,
                editing,
            } => return Some(self.post_key(verdict, notes, intro, inline_on, scroll, editing, k)),
            Overlay::None => {}
        }
        if self.chat_typing() {
            return Some(self.chat_key(k));
        }
        if self.unit_panel_has_keys() {
            return Some(self.unit_panel_key(k));
        }
        None
    }

    /// Normal mode: the keymap turns keys into actions for the screen.
    fn normal_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        match self.keys.feed(&k, std::time::Instant::now()) {
            Outcome::Run { action, count } => self.run_action(action, count),
            Outcome::Error(e) => {
                self.status = e;
                Vec::new()
            }
            Outcome::None | Outcome::Pending | Outcome::Command => Vec::new(),
        }
    }

    /// The action `k` completes in a panel that moves through a list (the
    /// notes, the commits, the Ask view's links, the `gd`/`gr` list), if
    /// any. Sequences and the command line work as in normal mode; counts
    /// are dropped, and an unknown command is reported.
    pub(super) fn panel_action(&mut self, k: &KeyEvent) -> Option<Action> {
        match self.keys.feed(k, std::time::Instant::now()) {
            Outcome::Run { action, .. } => Some(action),
            Outcome::Error(e) => {
                self.status = e;
                None
            }
            Outcome::None | Outcome::Pending | Outcome::Command => None,
        }
    }

    /// Run `a` with the count typed before it, if any. In visual mode moves
    /// extend the selection, a decision applies to all of it, and anything
    /// else leaves visual mode first. Decisions are remembered for `.`.
    pub fn run_action(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        let effects = self.action(a, count);
        self.save_cursor(effects)
    }

    /// [`App::run_action`], before the cursor is saved.
    fn action(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        if self.visual.is_some() {
            match a {
                _ if a.id().starts_with("move.") => {}
                Action::DecideAccept | Action::DecideFlag => return self.decide_visual(a),
                // `V` again leaves it, as in Neovim.
                Action::ViewBack | Action::VisualStart => {
                    self.visual = None;
                    return Vec::new();
                }
                _ => self.visual = None,
            }
        }
        if a == Action::HistoryRepeat {
            let Some((last, last_count)) = self.last_decision else {
                self.status = "Nothing to repeat yet.".into();
                return Vec::new();
            };
            // A count typed before `.` replaces the one it had.
            return self.run_action(last, count.or(last_count));
        }
        if self.decides_here(a) {
            self.last_decision = Some((a, count));
        }
        self.act(a, count)
    }

    /// Run `a` once, without a count.
    pub fn on_action(&mut self, a: Action) -> Vec<Effect> {
        self.run_action(a, None)
    }

    /// Whether `a` is a decision on this screen, for `.` to repeat.
    fn decides_here(&self, a: Action) -> bool {
        match self.screen {
            Screen::Slim | Screen::Queue => a.is_decision(),
            Screen::Swipe => matches!(a, Action::DecideAccept | Action::DecideFlag),
            // `f` on a flagged unit only opens its note.
            Screen::Flagged => a == Action::DecideAccept,
            Screen::Unit => a.is_decision(),
        }
    }

    /// Actions that work the same on every screen, then the screen's own.
    fn act(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        match a {
            Action::HelpOpen => {
                self.overlay = Overlay::Help;
                return Vec::new();
            }
            Action::ReviewReset => {
                self.overlay = Overlay::ConfirmReset;
                return Vec::new();
            }
            Action::ReviewRerun => {
                self.status = "Re-running the review…".into();
                return vec![Effect::Rerun];
            }
            Action::AskOpen if self.screen == Screen::Flagged => {
                self.open_chat();
                return Vec::new();
            }
            Action::AskOpen if self.screen == Screen::Unit => {
                self.open_ask_panel();
                return Vec::new();
            }
            // From the queue: the unit view on the unit, asking.
            Action::AskOpen if self.screen == Screen::Queue => {
                let Some(id) = self.selected_id() else {
                    return Vec::new();
                };
                let effects = self.open_unit(id);
                self.open_ask_panel();
                return effects;
            }
            Action::AskOpen => {
                self.open_ask();
                return Vec::new();
            }
            Action::FindUnit => return self.open_jump(JumpKind::Find),
            Action::GotoDefinition if self.screen == Screen::Unit => {
                return self.unit_goto_definition();
            }
            Action::GotoDefinition | Action::GotoCallers => {
                let Some(of) = self.current_unit().map(|u| u.id.clone()) else {
                    return Vec::new();
                };
                return self.open_jump(if a == Action::GotoDefinition {
                    JumpKind::Definition { of }
                } else {
                    JumpKind::Callers { of }
                });
            }
            Action::GotoEditor => return self.edit(),
            Action::GotoBrowser => return self.open_visual(),
            Action::NoteEdit => {
                match self.current_unit().map(|u| u.id.clone()) {
                    Some(id) => self.edit_note(id, None),
                    None => self.status = "Select a unit to write a note on it.".into(),
                }
                return Vec::new();
            }
            Action::ToolNotes => {
                self.open_notes();
                return Vec::new();
            }
            Action::ToolHistory => {
                self.open_commits();
                return Vec::new();
            }
            Action::ToolSettings => return vec![Effect::Settings],
            Action::FinishOpen => {
                self.finish();
                return Vec::new();
            }
            Action::ToggleAutoAccepted => {
                self.toggle_auto();
                return Vec::new();
            }
            Action::HistoryUndo => return self.undo(),
            Action::HistoryRedo => return self.redo(),
            Action::MoveNextHunk | Action::MovePrevHunk if self.screen != Screen::Unit => {
                self.status = "Hunk jumps work in the unit view.".into();
                return Vec::new();
            }
            Action::ScrollHalfDown | Action::ScrollHalfUp => {
                let half = (self.detail_rows.get() / 2).max(1) as isize;
                let by = if a == Action::ScrollHalfDown {
                    half
                } else {
                    -half
                };
                return self.scroll_detail(by);
            }
            Action::ScrollPageDown | Action::ScrollPageUp => {
                let page = self.detail_rows.get().saturating_sub(2).max(1) as isize;
                let by = if a == Action::ScrollPageDown {
                    page
                } else {
                    -page
                };
                return self.scroll_detail(by);
            }
            Action::ToolTests
            | Action::ToolRunOldNew
            | Action::ToolDiagram
            | Action::V1Recommend
            | Action::V1Disagree
            | Action::V1InterpretTests
                if self.screen != Screen::Flagged =>
            {
                return self.tool_from_list(a);
            }
            _ => {}
        }
        match self.screen {
            Screen::Queue => self.queue_action(a, count),
            Screen::Slim => self.slim_action(a, count),
            Screen::Swipe => self.swipe_action(a, count),
            Screen::Flagged => self.flagged_action(a, count),
            Screen::Unit => self.unit_action(a, count),
        }
    }

    /// `:w`: the all-done sheet once everything is accepted, else the notes
    /// when there are some, else what is left.
    fn finish(&mut self) {
        if self.all_reviewed() {
            self.overlay = Overlay::AllDone;
        } else if !crate::notes::noted(&self.review).is_empty() {
            self.open_notes();
        } else {
            let (_, flagged, pending) = self.decision_counts();
            self.status = if pending == 0 && flagged > 0 {
                format!("Nothing to finish yet: {} flagged.", units(flagged))
            } else {
                format!("Nothing to finish yet: {} pending.", units(pending))
            };
        }
    }

    /// A tool asked for outside the Flagged screen: in the unit view, its
    /// panel; from the queue, the unit view on the unit with that panel;
    /// from the v1 list or swipe on a flagged unit, the Flagged screen on
    /// it with the tile.
    fn tool_from_list(&mut self, a: Action) -> Vec<Effect> {
        if self.screen == Screen::Unit {
            return self.unit_tool(a);
        }
        let Some(id) = self.current_unit().map(|u| u.id.clone()) else {
            return Vec::new();
        };
        if self.screen == Screen::Queue {
            let mut effects = self.open_unit(id);
            effects.extend(self.unit_tool(a));
            return effects;
        }
        if !self.open_flagged_on(&id) {
            self.status = format!(
                "Flag the unit ({}) to use this.",
                self.keys.label(Action::DecideFlag)
            );
            return Vec::new();
        }
        self.flagged_action(a, None)
    }

    /// Open the Flagged screen on unit `id`, remembering where we came
    /// from. False, and nothing changes, when it isn't flagged.
    fn open_flagged_on(&mut self, id: &str) -> bool {
        let Some(pos) = self.flagged_units().iter().position(|u| u.id == id) else {
            return false;
        };
        if self.screen != Screen::Flagged {
            self.return_to.push(self.screen);
            self.screen = Screen::Flagged;
        }
        self.flagged.sel = pos;
        self.flagged_focus_last();
        true
    }

    /// The Flagged screen, the swipe cards: back to the list.
    fn back_to_list(&mut self) {
        self.return_to.clear();
        self.screen = Screen::Slim;
    }

    // ---- Flagged screen ---------------------------------------------------

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

    fn flagged_action(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        let n = self.flagged_units().len();
        let last = n.saturating_sub(1);
        let times = count.unwrap_or(1) as usize;
        match a {
            Action::MoveDown | Action::MoveUp | Action::MoveFirst | Action::MoveLast => {
                let at = self.flagged.sel.min(last);
                self.flagged.sel = match (a, count) {
                    (Action::MoveDown, _) => at.saturating_add(times).min(last),
                    (Action::MoveUp, _) => at.saturating_sub(times),
                    // `3gg`, `3G`: the third, as Neovim's line numbers.
                    (_, Some(row)) => (row as usize).saturating_sub(1).min(last),
                    (Action::MoveFirst, None) => 0,
                    _ => last,
                };
                self.flagged_focus_last();
            }
            Action::DecideAccept => return self.accept_flagged(times),
            Action::DecideFlag => {
                if let Some(id) = self.flagged_unit_id() {
                    self.edit_note(id, None);
                }
            }
            Action::ViewOpen => {
                if self.flagged_unit().map(|u| self.flagged_focus(u)) == Some(Pane::Chat) {
                    self.open_chat();
                }
            }
            Action::V1NextTile => self.flagged_tile_step(true),
            Action::V1PrevTile => self.flagged_tile_step(false),
            Action::V1Zoom => self.flagged.zoom = !self.flagged.zoom,
            Action::V1Recommend => return self.request_dive(DiveKind::Recommend),
            Action::ToolDiagram => return self.request_dive(DiveKind::Visualize),
            Action::ToolRunOldNew => return self.request_dive(DiveKind::Behavior),
            Action::V1Disagree => return self.request_dive(DiveKind::Disagree),
            Action::ToolTests => return self.request_dive(DiveKind::Tests),
            Action::V1InterpretTests => return self.interpret_tests(),
            Action::V1DiveRerun => {
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
                        self.keys.label(Action::V1PrevTile),
                        self.keys.label(Action::V1NextTile)
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
            Action::V1ResolveOk => return self.resolve(Resolution::Ok),
            Action::V1ResolveFix => return self.resolve(Resolution::NeedsFix),
            Action::ViewBack if self.flagged.zoom => self.flagged.zoom = false,
            Action::ViewBack => self.back(),
            Action::ViewClose => self.back_to_list(),
            _ => {}
        }
        Vec::new()
    }

    /// Accept the flagged unit on show and the `times - 1` after it, as one
    /// step; they leave the flagged list, and with none left, so does the
    /// screen.
    fn accept_flagged(&mut self, times: usize) -> Vec<Effect> {
        let flagged: Vec<String> = self.flagged_units().iter().map(|u| u.id.clone()).collect();
        let from = self.flagged.sel.min(flagged.len().saturating_sub(1));
        let ids: Vec<String> = flagged.into_iter().skip(from).take(times.max(1)).collect();
        if ids.is_empty() {
            return Vec::new();
        }
        self.decide_ids(&ids, Decision::Accepted);
        self.after_flagged_change();
        vec![Effect::Save]
    }

    /// The flagged list changed under the Flagged screen: the selection
    /// stays in it, and an empty list closes the screen.
    fn after_flagged_change(&mut self) {
        let n = self.flagged_units().len();
        if n == 0 {
            self.back();
            return;
        }
        self.flagged.sel = self.flagged.sel.min(n - 1);
        self.flagged_focus_last();
    }

    /// Give `kind` a tile and the focus, asking for it unless it is
    /// answered or in flight. The first web-using ask of the session
    /// confirms first.
    fn request_dive(&mut self, kind: DiveKind) -> Vec<Effect> {
        let Some(id) = self.flagged_unit_id() else {
            return Vec::new();
        };
        self.dive_on(&id, kind, true)
    }

    /// Ask `kind` about unit `id` unless it is answered or in flight. On
    /// the Flagged screen (`tile`) the answer gets a tile and the focus,
    /// and the first web-using ask of the session confirms first; the unit
    /// view never stops to ask: the web is used once allowed (see
    /// [`App::web_allowed`]).
    pub(super) fn dive_on(&mut self, id: &str, kind: DiveKind, tile: bool) -> Vec<Effect> {
        let id = id.to_string();
        let web_ok = self.web_allowed();
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
                if tile {
                    self.focus(Pane::Answer(kind));
                }
                return Vec::new();
            }
            if u.changes.is_empty() {
                self.status = "Nothing to check yet: the unit has no description.".into();
                return Vec::new();
            }
            mark_asked(u);
            u.checks.second_pending = true;
            if tile {
                self.focus(Pane::Answer(kind));
            }
            return vec![Effect::Second(vec![id])];
        }
        mark_asked(u);
        if tile {
            self.focus(Pane::Answer(kind));
        }
        let Some(u) = self.review.unit_mut(&id) else {
            return Vec::new();
        };
        if u.dive.results.contains_key(&kind) || u.dive_pending.contains(&kind) {
            return Vec::new();
        }
        u.dive_pending.insert(kind);
        if !tile {
            return vec![Effect::Dive {
                unit_id: id,
                kind,
                web: web_ok && uses_web(kind),
            }];
        }
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
        let Some(id) = self.flagged_unit_id() else {
            return Vec::new();
        };
        self.interpret_tests_on(&id, true)
    }

    /// Have the agent read what the Tests search found for unit `id`; on
    /// the Flagged screen (`tile`) its tile gets the focus.
    pub(super) fn interpret_tests_on(&mut self, id: &str, tile: bool) -> Vec<Effect> {
        let kind = DiveKind::Tests;
        let id = id.to_string();
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
                self.keys.label(Action::ToolTests)
            );
            return Vec::new();
        }
        if u.dive_pending.contains(&kind) {
            return Vec::new();
        }
        u.dive_pending.insert(kind);
        if tile {
            self.focus(Pane::Answer(kind));
        }
        vec![Effect::InterpretTests { unit_id: id }]
    }

    /// Open the picture of the unit on show, or say how to draw one.
    pub(super) fn open_visual(&mut self) -> Vec<Effect> {
        let Some(u) = self.current_unit() else {
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
                self.keys.label(Action::ToolDiagram)
            );
            return Vec::new();
        }
        vec![Effect::OpenVisual {
            unit_id: u.id.clone(),
        }]
    }

    /// The answer to the web confirmation.
    fn answer_web(&mut self, yes: bool) -> Vec<Effect> {
        if let Some(effects) = self.unit_answer_web(yes) {
            return effects;
        }
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

    /// Open the confirmation for posting a review with `verdict`: confirming
    /// the all-reviewed sheet approves, posting the notes requests changes.
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

    /// Keys of the post confirmation, a sheet ([`Keymap::sheet`]): `enter`
    /// posts, `esc` closes, `i` edits the summary, `v` cycles the verdict,
    /// `t` turns inline comments on or off, `j`/`k` and the page keys
    /// scroll. While the summary is edited on one line, a text input
    /// ([`Keymap::edit`]). ctrl+c is handled before.
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
            match self.keys.edit(&k) {
                Edit::Cancel => put(self, notes, intro, inline_on, scroll, None),
                Edit::Submit => {
                    let t = approve::clamp_text(&text.replace('\n', " "), approve::BODY_MAX_CHARS);
                    if !t.trim().is_empty() {
                        intro = t.trim().to_string();
                    }
                    put(self, notes, intro, inline_on, scroll, None);
                }
                e => {
                    typed(&mut text, e, approve::BODY_MAX_CHARS);
                    put(self, notes, intro, inline_on, scroll, Some(text));
                }
            }
            return Vec::new();
        }
        let max = self.approve_max_scroll.get();
        match self.keys.sheet(&k) {
            Some(Sheet::Cancel) => {
                self.overlay = Overlay::None;
                return Vec::new();
            }
            Some(Sheet::Confirm) => {
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
            Some(Sheet::Edit) => {
                let text = intro.clone();
                put(self, notes, intro, inline_on, scroll, None);
                return vec![Effect::EditComment { text }];
            }
            Some(Sheet::Inline) => inline_on = !inline_on,
            Some(Sheet::Verdict) => {
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
            Some(Sheet::Down) => scroll = (scroll + 1).min(max),
            Some(Sheet::Up) => scroll = scroll.saturating_sub(1),
            Some(Sheet::PageDown) => scroll = (scroll + 8).min(max),
            Some(Sheet::PageUp) => scroll = scroll.saturating_sub(8),
            Some(Sheet::Yank | Sheet::Quit) | None => {}
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

    /// Confirming the all-reviewed sheet: open the commit confirmation with a
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
            self.status = format!(
                "No agent to draft a message; using the template. {} edits it.",
                self.keys.label(Action::NoteEdit)
            );
            Vec::new()
        }
    }

    /// Keys of the commit confirmation, a sheet ([`Keymap::sheet`]): `enter`
    /// commits, `esc` closes, `i` edits the message, `j`/`k` and the page
    /// keys scroll. While the subject is edited on one line, a text input
    /// ([`Keymap::edit`]). ctrl+c is handled before.
    fn commit_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        enum Act {
            Stay,
            Close,
            Commit,
            Edit(String),
        }
        let max = self.commit_max_scroll.get();
        let edit = self.keys.edit(&k);
        let sheet = self.keys.sheet(&k);
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
            match edit {
                Edit::Cancel => *editing = None,
                Edit::Submit => {
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
                e => {
                    typed(text, e, commit::SUBJECT_MAX * 2);
                }
            }
            return Vec::new();
        }
        let act = match sheet {
            Some(Sheet::Cancel) => Act::Close,
            Some(Sheet::Confirm) => Act::Commit,
            Some(Sheet::Edit) => {
                *draft = DraftState::Edited;
                Act::Edit(message.clone())
            }
            Some(Sheet::Down) => {
                *scroll = (*scroll + 1).min(max);
                Act::Stay
            }
            Some(Sheet::Up) => {
                *scroll = scroll.saturating_sub(1);
                Act::Stay
            }
            Some(Sheet::PageDown) => {
                *scroll = (*scroll + 8).min(max);
                Act::Stay
            }
            Some(Sheet::PageUp) => {
                *scroll = scroll.saturating_sub(8);
                Act::Stay
            }
            Some(Sheet::Yank | Sheet::Verdict | Sheet::Inline | Sheet::Quit) | None => Act::Stay,
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
            self.status = format!(
                "The commit message is empty. Press {} to write one.",
                self.keys.label(Action::NoteEdit)
            );
            return Vec::new();
        }
        match commit::check(&self.review) {
            Err(refusal) => {
                self.overlay = Overlay::None;
                self.status = refusal.text(&self.keys.label(Action::ReviewRerun));
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
                self.status = format!(
                    "Drafted a commit message. {} edits it.",
                    self.keys.label(Action::NoteEdit)
                );
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

    /// Keys while the note line is open, a text input ([`Keymap::edit`]):
    /// every letter is text, `enter` saves, `esc` closes it and keeps the
    /// note as it was (a flag that opened it stays). Saving marks the note
    /// as written on the code as it is now; an empty note is no note. A line
    /// for several units (`F`, `f` on a selection) puts its text on each;
    /// left empty when their notes differ, it keeps each one's own. A flag's
    /// note joins the flag's undo step.
    fn note_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let Overlay::Note {
            id,
            mut text,
            back,
            also,
            joins,
        } = self.overlay.clone()
        else {
            return Vec::new();
        };
        let after = back.map_or(Overlay::None, |sel| Overlay::Notes { sel });
        match self.keys.edit(&k) {
            Edit::Cancel => self.overlay = after,
            Edit::Submit => {
                self.overlay = after;
                let note: String = text.chars().filter(|c| !c.is_control()).collect();
                let note: String = note.trim().chars().take(NOTE_MAX).collect();
                let ids: Vec<String> = std::iter::once(id).chain(also).collect();
                if note.is_empty() && self.shared_note(&ids).is_none() {
                    return Vec::new();
                }
                let changes = self.write_note(&ids, &note);
                if changes.is_empty() {
                    return Vec::new();
                }
                if ids.len() > 1 {
                    self.status = format!("Note on {}.", units(changes.len()));
                }
                // Only while the flag's step is still the last one.
                if joins.is_some_and(|i| i + 1 == self.review.history.undo.len()) {
                    self.review.history.add_notes(changes);
                }
                if crate::notes::noted(&self.review).is_empty() {
                    self.overlay = Overlay::None;
                }
                return vec![Effect::Save];
            }
            e => {
                if typed(&mut text, e, NOTE_MAX)
                    && let Overlay::Note { text: line, .. } = &mut self.overlay
                {
                    *line = text;
                }
            }
        }
        Vec::new()
    }

    /// The note every one of `ids` has, if they agree (the note of one unit).
    fn shared_note(&self, ids: &[String]) -> Option<String> {
        let mut notes = ids
            .iter()
            .filter_map(|id| self.review.unit(id))
            .map(|u| u.dive.note.as_str());
        let first = notes.next().unwrap_or_default();
        notes.all(|n| n == first).then(|| first.to_string())
    }

    /// Put `text` as the note on each of `ids`, written on their code as it
    /// is now. What each note was and became, for the undo step.
    fn write_note(&mut self, ids: &[String], text: &str) -> Vec<NoteChange> {
        let mut changes = Vec::new();
        for id in ids {
            let Some(u) = self.review.unit_mut(id) else {
                continue;
            };
            let after = Note {
                text: text.to_string(),
                on: (!text.is_empty()).then(|| u.fingerprint.clone()),
            };
            let before = Note {
                text: std::mem::replace(&mut u.dive.note, after.text.clone()),
                on: std::mem::replace(&mut u.dive.note_on, after.on.clone()),
            };
            changes.push(NoteChange {
                id: id.clone(),
                before,
                after,
            });
        }
        changes
    }

    /// Set the note on unit `id` back to `note` (undo, redo).
    fn put_note(&mut self, id: &str, note: &Note) {
        if let Some(u) = self.review.unit_mut(id) {
            u.dive.note = note.text.clone();
            u.dive.note_on = note.on.clone();
        }
    }

    // ---- Notes ------------------------------------------------------------

    /// Open the note editor on unit `id`, with its note.
    pub(super) fn edit_note(&mut self, id: String, back: Option<usize>) {
        let text = self
            .review
            .unit(&id)
            .map(|u| u.dive.note.clone())
            .unwrap_or_default();
        self.overlay = Overlay::Note {
            id,
            text,
            back,
            also: Vec::new(),
            joins: None,
        };
    }

    /// Open the note line for a flag on `ids` (`f`, `F`, `f` on a
    /// selection), prefilled with the note they share, else empty. With
    /// `step`, the flag made the last undo step and the note joins it.
    fn flag_note_line(&mut self, ids: Vec<String>, step: bool) {
        let Some((id, also)) = ids.split_first() else {
            return;
        };
        let undo = self.review.history.undo.len();
        self.overlay = Overlay::Note {
            id: id.clone(),
            text: self.shared_note(&ids).unwrap_or_default(),
            back: None,
            also: also.to_vec(),
            joins: (step && undo > 0).then(|| undo - 1),
        };
    }

    /// Open the notes view, on the note of the unit on show when it has one.
    fn open_notes(&mut self) {
        let noted = crate::notes::noted(&self.review);
        if noted.is_empty() {
            self.status = format!(
                "No notes yet: {} writes one on the unit on show.",
                self.keys.label(Action::NoteEdit)
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

    /// Keys of the notes view, a panel ([`App::panel_action`]): move with
    /// `j`/`k`/`gg`/`G`, `view.open` goes to the unit, `note.edit` edits its
    /// note, `note.delete` deletes it (after asking); `finish.yank` copies
    /// every note, `v1.notes_to_agent` hands them to Claude Code,
    /// `v1.notes_post` posts them on the PR; `view.back`, `view.close` or
    /// `tool.notes` closes.
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
        let Some(a) = self.panel_action(&k) else {
            self.overlay = Overlay::Notes { sel };
            return Vec::new();
        };
        match a {
            Action::MoveDown => sel = (sel + 1).min(last),
            Action::MoveUp => sel = sel.saturating_sub(1),
            Action::MoveFirst => sel = 0,
            Action::MoveLast => sel = last,
            Action::ViewOpen => {
                self.overlay = Overlay::None;
                return self.go_to_unit(&ids[sel], true).unwrap_or_default();
            }
            Action::NoteEdit => {
                self.edit_note(ids[sel].clone(), Some(sel));
                return Vec::new();
            }
            Action::NoteDelete => {
                self.overlay = Overlay::ConfirmDeleteNote {
                    id: ids[sel].clone(),
                    sel,
                };
                return Vec::new();
            }
            Action::FinishYank => {
                let what = match ids.len() {
                    1 => "1 note as Markdown".to_string(),
                    n => format!("{n} notes as Markdown"),
                };
                self.overlay = Overlay::Notes { sel };
                return vec![Effect::Copy {
                    text: crate::notes::markdown(&self.review),
                    what,
                }];
            }
            Action::V1NotesToAgent => {
                self.overlay = Overlay::Notes { sel };
                return self.notes_to_agent();
            }
            Action::V1NotesPost => {
                self.overlay = Overlay::Notes { sel };
                if self.review.pr.is_none() {
                    self.status = "Posting is for a pull request (teer pr).".into();
                } else if self.can_post() {
                    self.ask_post(Verdict::RequestChanges);
                }
                return Vec::new();
            }
            Action::HelpOpen => {
                self.overlay = Overlay::Help;
                return Vec::new();
            }
            Action::ViewBack | Action::ViewClose | Action::ToolNotes => {
                self.overlay = Overlay::None;
                return Vec::new();
            }
            _ => {}
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
            self.status = format!(
                "Not for a pull request: its code is untrusted. {} copies the notes, {} posts them on the PR.",
                self.keys.label(Action::FinishYank),
                self.keys.label(Action::V1NotesPost)
            );
            return Vec::new();
        }
        let cli = self.cfg.provider();
        if cli.program().is_none() {
            self.status = format!(
                "Handing over needs Claude Code or Codex (teer setup). {} copies the notes.",
                self.keys.label(Action::FinishYank)
            );
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

    /// Go to unit `id`: in the unit view, or on the Flagged screen when it
    /// is flagged there, when `stay` allows; else in the list (the queue, or
    /// the v1 list from the v1 screens), closing any overlay. `None` when
    /// the list doesn't have it.
    pub(super) fn go_to_unit(&mut self, id: &str, stay: bool) -> Option<Vec<Effect>> {
        // In the unit view, the unit opens there.
        if stay && self.screen == Screen::Unit && self.review.unit(id).is_some() {
            self.overlay = Overlay::None;
            return Some(self.open_unit(id.to_string()));
        }
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
        let list = self.list_screen();
        if self.screen != list {
            self.return_to.clear();
            self.screen = list;
        }
        Some(self.move_sel(pos))
    }

    /// Select the unit at `to`.
    pub(super) fn move_sel(&mut self, to: usize) -> Vec<Effect> {
        self.sel = to.min(self.order.len().saturating_sub(1));
        self.ensure_prepared()
    }

    /// The queue's actions: moves, `a`/`f` (then on to the next pending
    /// unit), `A`/`F` on the file's pending units at once, `V`, `l` into
    /// the unit view on any unit, `zf`, and `q` to quit (the review is
    /// saved, cursor and all).
    pub(super) fn queue_action(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        let times = count.unwrap_or(1) as usize;
        match a {
            Action::MoveDown => return self.move_row(times as isize),
            Action::MoveUp => return self.move_row(-(times as isize)),
            // `3gg`, `3G`: the third row, as Neovim's line numbers.
            Action::MoveFirst | Action::MoveLast if count.is_some() => {
                return self.move_sel(times.saturating_sub(1));
            }
            Action::MoveFirst => return self.move_row(isize::MIN / 2),
            Action::MoveLast => return self.move_row(isize::MAX / 2),
            Action::DecideAccept => return self.decide_n(Decision::Accepted, count),
            Action::DecideFlag => return self.decide_n(Decision::Investigate, count),
            Action::DecideAcceptFile => return self.decide_file_now(Decision::Accepted),
            Action::DecideFlagFile => return self.decide_file_now(Decision::Investigate),
            Action::VisualStart => {
                if self.sel < self.order.len() {
                    self.visual = Some(self.sel);
                }
            }
            Action::ViewOpen => {
                if let Some(id) = self.selected_id() {
                    return self.open_unit(id);
                }
            }
            Action::MoveNextPending => return self.jump_pending(true, times),
            Action::MovePrevPending => return self.jump_pending(false, times),
            Action::MoveNextFile => return self.jump_file(true, times),
            Action::MovePrevFile => return self.jump_file(false, times),
            Action::ToggleByFile => self.switch_grouping(),
            Action::ViewClose => return vec![Effect::Save, Effect::Quit],
            _ => {}
        }
        Vec::new()
    }

    /// The v1 list (`--swipe`, until Phase 8): the queue's actions, except
    /// that `l` opens only a flagged unit (on the Flagged screen), `A` and
    /// `F` ask first, and `s` swipes.
    fn slim_action(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        match a {
            Action::V1Swipe => {
                self.enter_swipe();
                return self.prepare_cards();
            }
            Action::ViewOpen => self.open_selected(),
            Action::DecideAcceptFile => self.ask_file(Decision::Accepted),
            Action::DecideFlagFile => self.ask_file(Decision::Investigate),
            _ => return self.queue_action(a, count),
        }
        Vec::new()
    }

    /// `l`: open the Flagged screen on the selected unit, which only a
    /// flagged unit has for now.
    fn open_selected(&mut self) {
        let Some(id) = self.selected_id() else {
            return;
        };
        if !self.open_flagged_on(&id) {
            self.status = format!(
                "Only flagged units open for now. {} flags this one.",
                self.keys.label(Action::DecideFlag)
            );
        }
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

    /// Keys of the commits view, a panel ([`App::panel_action`]): move
    /// through the commits, `view.open` (or `tab`, `→`) into the units the
    /// selected one changed, `view.open` on one goes to it; `view.back` (or
    /// `←`, `tab`) back out, `view.back` again, `view.close` or
    /// `tool.history` closes.
    fn commits_key(&mut self, mut sel: usize, mut unit: Option<usize>, k: KeyEvent) -> Vec<Effect> {
        let Some(a) = self.panel_action(&k) else {
            return Vec::new();
        };
        if matches!(a, Action::ViewClose | Action::ToolHistory) {
            self.overlay = Overlay::None;
            return Vec::new();
        }
        let n = self.review.intent.commits.len();
        let units = self.commit_units(sel);
        match (unit, a) {
            (None, Action::MoveDown) => sel = (sel + 1).min(n.saturating_sub(1)),
            (None, Action::MoveUp) => sel = sel.saturating_sub(1),
            (None, Action::MoveFirst) => sel = 0,
            (None, Action::MoveLast) => sel = n.saturating_sub(1),
            (None, Action::ViewOpen | Action::V1NextTile) => {
                unit = (!units.is_empty()).then_some(0);
            }
            (None, Action::ViewBack) => {
                self.overlay = Overlay::None;
                return Vec::new();
            }
            (Some(i), Action::MoveDown) => unit = Some((i + 1).min(units.len().saturating_sub(1))),
            (Some(i), Action::MoveUp) => unit = Some(i.saturating_sub(1)),
            (Some(_), Action::MoveFirst) => unit = Some(0),
            (Some(_), Action::MoveLast) => unit = Some(units.len().saturating_sub(1)),
            (Some(i), Action::ViewOpen) => {
                if let Some(&pos) = units.get(i) {
                    let id = self.review.units[self.order[pos]].id.clone();
                    self.overlay = Overlay::None;
                    return self.go_to_unit(&id, true).unwrap_or_default();
                }
            }
            (Some(_), Action::ViewBack | Action::V1NextTile | Action::V1PrevTile) => unit = None,
            _ => {}
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

    /// The questions of the conversation on show, by index: the unit's in
    /// the unit view, the flagged unit's chat on the Flagged screen, else
    /// all of them (the Ask view).
    pub fn talk(&self) -> Vec<usize> {
        if self.overlay != Overlay::Ask && self.screen == Screen::Unit {
            return self.chat_of(&self.unit.id);
        }
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
            // An answer being written takes the old one's place.
            if self.live_text(n).is_some() {
                continue;
            }
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

    /// The answer's text so far while question `n` is on its way, once
    /// some came.
    pub fn live_text(&self, n: usize) -> Option<&str> {
        let live = self.ask.live.get(&n)?;
        (!live.text.is_empty()).then_some(live.text.as_str())
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

    /// Keys of the Ask view. Typing, a text input ([`Keymap::edit`]):
    /// letters go into the question, `enter` asks it, `ctrl+r` asks the last
    /// question again, `ctrl+u` clears, `tab` moves to the links, `esc`
    /// closes; `↑ ↓` and the page keys scroll. On the links, a panel
    /// ([`App::panel_action`]): see [`App::ask_link_key`].
    fn ask_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        if let Some(i) = self.ask.link {
            return self.ask_link_key(i, &k);
        }
        match self.keys.edit(&k) {
            Edit::Cancel => self.overlay = Overlay::None,
            Edit::Submit => return self.send_question(),
            Edit::Tab => {
                // The newest answer's first link.
                let links = self.ask_links();
                let newest = links.last().map(|(q, _)| *q);
                self.ask.link = links.iter().position(|(q, _)| Some(*q) == newest);
            }
            Edit::Down => return self.ask_scroll(SCROLL_STEP),
            Edit::Up => return self.ask_scroll(-SCROLL_STEP),
            Edit::PageDown => return self.ask_scroll(ASK_PAGE),
            Edit::PageUp => return self.ask_scroll(-ASK_PAGE),
            Edit::Retry => {
                return match self.review.questions.len() {
                    0 => Vec::new(),
                    n => self.ask_question(n - 1),
                };
            }
            e => {
                typed(&mut self.ask.input, e, QUESTION_MAX);
            }
        }
        Vec::new()
    }

    /// Keys on the Ask view's links (link `i` in focus): `j`/`k`/`gg`/`G`
    /// move between them, `view.open` follows one (a unit: to it in the
    /// list; code: the editor), `view.back` or `tab` goes back to typing,
    /// `view.close` closes the view; the scroll keys scroll.
    fn ask_link_key(&mut self, i: usize, k: &KeyEvent) -> Vec<Effect> {
        let Some(a) = self.panel_action(k) else {
            return Vec::new();
        };
        let last = self.ask_links().len().saturating_sub(1);
        match a {
            Action::MoveDown => self.ask.link = Some((i + 1).min(last)),
            Action::MoveUp => self.ask.link = Some(i.saturating_sub(1)),
            Action::MoveFirst => self.ask.link = Some(0),
            Action::MoveLast => self.ask.link = Some(last),
            Action::ViewOpen => return self.follow_ask_link(i),
            // `tab` and `shift+tab` are the tile keys elsewhere; `K` opened
            // the view.
            Action::ViewBack | Action::V1NextTile | Action::V1PrevTile | Action::AskOpen => {
                self.ask.link = None;
            }
            Action::ViewClose => {
                self.ask.link = None;
                self.overlay = Overlay::None;
            }
            Action::ScrollPageDown => return self.ask_scroll(ASK_PAGE),
            Action::ScrollPageUp => return self.ask_scroll(-ASK_PAGE),
            Action::ScrollHalfDown => return self.ask_scroll(ASK_PAGE / 2),
            Action::ScrollHalfUp => return self.ask_scroll(-ASK_PAGE / 2),
            _ => {}
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
        match self.add_question(text, unit) {
            Some(index) => self.ask_question(index),
            None => Vec::new(),
        }
    }

    /// Note `text` as a question about `unit`, not asked yet; its index.
    /// `None` when it is blank.
    pub(super) fn add_question(&mut self, text: &str, unit: Option<String>) -> Option<usize> {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        let text = text.trim().to_string();
        if text.is_empty() {
            return None;
        }
        self.review.questions.push(Question {
            text,
            unit,
            ..Default::default()
        });
        Some(self.review.questions.len() - 1)
    }

    /// Have the agent answer question `index` (again), unless it is on its
    /// way. An earlier answer stays until the new one comes.
    pub(super) fn ask_question(&mut self, index: usize) -> Vec<Effect> {
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
        self.ask.live.remove(&index);
        q.error = None;
        vec![Effect::Save, Effect::Ask { index }]
    }

    /// What the agent does on question `index` while it answers, for as
    /// long as it is on its way.
    fn asking(&mut self, index: usize, progress: Progress) {
        if !self.ask.pending.contains(&index) {
            return;
        }
        let live = self.ask.live.entry(index).or_default();
        match progress {
            Progress::Step(s) => {
                live.step = Some(s);
                live.steps += 1;
            }
            Progress::Text(t) => live.text = t,
        }
    }

    /// The answer to question `index` came (or failed). A question asked
    /// with no unit on show goes with the unit the answer relates to most.
    fn answered(&mut self, index: usize, result: Result<Answer, String>) -> Vec<Effect> {
        self.ask.pending.remove(&index);
        self.ask.live.remove(&index);
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
    /// it is one, or from the unit view's ask panel, opens there; code opens
    /// in the editor.
    pub(super) fn follow_ask_link(&mut self, i: usize) -> Vec<Effect> {
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
                if matches!(self.screen, Screen::Slim | Screen::Queue) {
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

    /// Keys while typing in the chat, a text input ([`Keymap::edit`]):
    /// letters go into the question, `enter` asks it, `ctrl+r` asks the
    /// chat's last question again, `ctrl+u` clears, `↑ ↓` and the page keys
    /// scroll, `esc` stops typing (the keys of the screen work again), `tab`
    /// `shift+tab` move to another tile.
    fn chat_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let page = self.detail_rows.get().saturating_sub(2).max(1) as isize;
        match self.keys.edit(&k) {
            Edit::Cancel => self.flagged.typing = false,
            Edit::Tab => self.flagged_tile_step(true),
            Edit::BackTab => self.flagged_tile_step(false),
            Edit::Submit => return self.send_chat(),
            Edit::Down => return self.chat_scroll(SCROLL_STEP),
            Edit::Up => return self.chat_scroll(-SCROLL_STEP),
            Edit::PageDown => return self.chat_scroll(page),
            Edit::PageUp => return self.chat_scroll(-page),
            Edit::Retry => return self.ask_chat_again(),
            e => {
                typed(&mut self.ask.draft, e, QUESTION_MAX);
            }
        }
        Vec::new()
    }

    /// Scroll the chat `by` lines; at the end it follows new lines.
    pub(super) fn chat_scroll(&mut self, by: isize) -> Vec<Effect> {
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

    /// After a decision: on to the next pending unit after `from` in list
    /// order, else the first pending one from the top; with nothing pending,
    /// the cursor stays where it is (the bottom bar says so).
    fn advance(&mut self, from: usize) -> Vec<Effect> {
        let next = self
            .next_pending_pos(from)
            .or_else(|| (0..from.min(self.order.len())).find(|&p| self.pending_at(p)));
        if let Some(p) = next {
            self.sel = p;
        }
        self.ensure_prepared()
    }

    /// Go to the `times`-th pending unit after (or, not `forward`, before)
    /// the cursor, or as far as there are.
    pub(super) fn jump_pending(&mut self, forward: bool, times: usize) -> Vec<Effect> {
        let mut at = None;
        for _ in 0..times.max(1) {
            let from = at.unwrap_or(self.sel);
            let next = if forward {
                self.next_pending_pos(from)
            } else {
                self.prev_pending_pos(from)
            };
            match next {
                Some(p) => at = Some(p),
                None => break,
            }
        }
        match at {
            Some(p) => self.move_sel(p),
            None => {
                self.status = "No more pending units that way.".into();
                Vec::new()
            }
        }
    }

    /// Go to the first unit of the `times`-th file below, or (not
    /// `forward`) above, or as far as there are. No wrapping: from the last
    /// (first) file the cursor stays.
    pub(super) fn jump_file(&mut self, forward: bool, times: usize) -> Vec<Effect> {
        if self.sel >= self.order.len() {
            return Vec::new();
        }
        let mut at = None;
        for _ in 0..times.max(1) {
            let range = self.file_range(at.unwrap_or(self.sel));
            let next = if forward {
                Some(range.end).filter(|&p| p < self.order.len())
            } else {
                range.start.checked_sub(1).map(|p| self.file_start(p))
            };
            match next {
                Some(p) => at = Some(p),
                None => break,
            }
        }
        match at {
            Some(p) => self.move_sel(p),
            None => {
                self.status = if forward { "last file" } else { "first file" }.into();
                Vec::new()
            }
        }
    }

    /// Ask to accept (`A`) or flag (`F`) every pending unit in the file the
    /// cursor is in. Flagged and already accepted units are left alone; with
    /// nothing pending there is nothing to confirm.
    fn ask_file(&mut self, d: Decision) {
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
        self.overlay = match d {
            Decision::Investigate => Overlay::ConfirmFlagFile { file, ids },
            _ => Overlay::ConfirmAcceptFile { file, ids },
        };
    }

    /// Flag the confirmed units as one undo step (`F`). The selection stays.
    fn flag_file(&mut self, ids: Vec<String>) -> Vec<Effect> {
        self.decide_file(ids, Decision::Investigate)
    }

    /// Accept the confirmed units as one undo step (`A`). The selection stays.
    fn accept_file(&mut self, ids: Vec<String>) -> Vec<Effect> {
        self.decide_file(ids, Decision::Accepted)
    }

    /// Decide those of `ids` still pending as one undo step.
    fn decide_file(&mut self, ids: Vec<String>, d: Decision) -> Vec<Effect> {
        let ids: Vec<String> = ids
            .into_iter()
            .filter(|id| {
                self.review
                    .unit(id)
                    .is_some_and(|u| u.decision == Decision::Pending)
            })
            .collect();
        if ids.is_empty() {
            return Vec::new();
        }
        let n = self.decide_ids(&ids, d);
        self.status = decided_status(d, n);
        vec![Effect::Save]
    }

    /// `A`, `F` in the queue: decide the pending units of the file under the
    /// cursor at once, as one undo step, and move on to the next pending
    /// unit; `F` opens one note line for all of them. Decided units are left
    /// alone. Listed by level, the file is its units in the cursor's box.
    fn decide_file_now(&mut self, d: Decision) -> Vec<Effect> {
        if self.sel >= self.order.len() {
            return Vec::new();
        }
        let range = self.file_range(self.sel);
        let ids: Vec<String> = (range.clone())
            .map(|p| &self.review.units[self.order[p]])
            .filter(|u| u.decision == Decision::Pending)
            .map(|u| u.id.clone())
            .collect();
        if ids.is_empty() {
            self.status = "Nothing pending in this file.".into();
            return Vec::new();
        }
        let n = self.decide_ids(&ids, d);
        self.status = decided_status(d, n);
        // Deciding moves no row: pending units are never the hidden ones.
        let mut effects = vec![Effect::Save];
        effects.extend(self.advance(range.end - 1));
        if d == Decision::Investigate {
            self.flag_note_line(ids, true);
        }
        effects
    }

    /// Apply `d` to the selected unit and move on to the next pending one,
    /// as one undo step.
    pub fn decide(&mut self, d: Decision) -> Vec<Effect> {
        self.decide_n(d, None)
    }

    /// Apply `d` to the selected unit and, with a count, the next
    /// `count - 1` pending ones, as one undo step; then move on to the next
    /// pending unit (see [`App::advance`]). A single flag opens the note
    /// line on the flagged unit, prefilled with its note.
    fn decide_n(&mut self, d: Decision, count: Option<u32>) -> Vec<Effect> {
        let Some(id) = self.selected_id() else {
            return Vec::new();
        };
        let mut ids = vec![id.clone()];
        let mut at = self.sel;
        for _ in 1..count.unwrap_or(1) {
            let Some(p) = self.next_pending_pos(at) else {
                break;
            };
            ids.push(self.review.units[self.order[p]].id.clone());
            at = p;
        }
        let steps = self.review.history.undo.len();
        let n = self.decide_ids(&ids, d);
        if count.is_some() {
            self.status = decided_status(d, n);
        }
        let mut effects = vec![Effect::Save];
        effects.extend(self.advance(self.sel));
        if d == Decision::Investigate && count.is_none() {
            let step = self.review.history.undo.len() > steps;
            self.flag_note_line(vec![id], step);
        }
        effects
    }

    /// `a` or `f` in visual mode: decide every unit of the selection as one
    /// undo step, leave visual mode and move on to the next pending unit
    /// after it. In the queue, `f` opens one note line for all of them.
    /// `.` repeats it on as many units.
    fn decide_visual(&mut self, a: Action) -> Vec<Effect> {
        let Some(start) = self.visual.take() else {
            return Vec::new();
        };
        let Some(last) = self.order.len().checked_sub(1) else {
            return Vec::new();
        };
        let (lo, hi) = (start.min(self.sel), start.max(self.sel).min(last));
        let ids: Vec<String> = (lo..=hi)
            .map(|p| self.review.units[self.order[p]].id.clone())
            .collect();
        let d = if a == Action::DecideFlag {
            Decision::Investigate
        } else {
            Decision::Accepted
        };
        self.last_decision = Some((a, Some(ids.len() as u32)));
        let steps = self.review.history.undo.len();
        let n = self.decide_ids(&ids, d);
        self.status = decided_status(d, n);
        // A unit that was accepted for you may have moved: go by its id.
        let hi = ids.last().and_then(|id| self.position_of(id)).unwrap_or(hi);
        self.sel = hi;
        let mut effects = vec![Effect::Save];
        effects.extend(self.advance(hi));
        if d == Decision::Investigate && self.screen == Screen::Queue {
            let step = self.review.history.undo.len() > steps;
            self.flag_note_line(ids, step);
        }
        effects
    }

    /// Where unit `id` is listed, if it is.
    pub(super) fn position_of(&self, id: &str) -> Option<usize> {
        self.order
            .iter()
            .position(|&i| self.review.units[i].id == id)
    }

    /// Set each of `ids` to `d` and record that as one undo step; the "all
    /// done" sheet opens when this finished the review. How many changed.
    pub(super) fn decide_ids(&mut self, ids: &[String], d: Decision) -> usize {
        let each: Vec<(String, Decision)> = ids.iter().map(|id| (id.clone(), d)).collect();
        self.decide_each(&each)
    }

    /// Set each unit to its decision as one undo step, and list the units
    /// again (who accepted them; see [`settle`]); on the v1 screens the "all
    /// done" sheet opens when this finished the review (the queue's bottom
    /// bar says "Nothing pending" instead). How many changed.
    fn decide_each(&mut self, each: &[(String, Decision)]) -> usize {
        let was_done = self.all_reviewed();
        let mut changes = Vec::new();
        for (id, d) in each {
            if let Some(u) = self.review.unit_mut(id) {
                let before = std::mem::replace(&mut u.decision, *d);
                changes.push(Change {
                    id: id.clone(),
                    before,
                    after: *d,
                });
            }
        }
        let n = changes.iter().filter(|c| c.before != c.after).count();
        self.review.history.record(Step {
            changes,
            ..Default::default()
        });
        self.reorder();
        self.all_done_on_v1(was_done);
        n
    }

    /// The "all done" sheet, on the v1 screens only (see
    /// [`App::check_all_done`]).
    fn all_done_on_v1(&mut self, was_done: bool) {
        if self.list_screen() == Screen::Slim {
            self.check_all_done(was_done);
        }
    }

    /// `u`: put back what the last step changed (the decisions and the
    /// notes its note line wrote), and go to its first unit. On a unit
    /// accepted for you (listed with `za`), take that back instead: it is
    /// pending again, as a step of its own.
    pub fn undo(&mut self) -> Vec<Effect> {
        if matches!(self.screen, Screen::Slim | Screen::Queue)
            && let Some(id) = self
                .selected()
                .filter(|u| u.auto_accepted())
                .map(|u| u.id.clone())
        {
            self.decide_ids(std::slice::from_ref(&id), Decision::Pending);
            let name = self
                .review
                .unit(&id)
                .map(super::slim::row_name)
                .unwrap_or_default();
            self.status = format!("{name} is pending again, no longer accepted for you.");
            return vec![Effect::Save];
        }
        let Some(step) = self.review.history.undo() else {
            self.status = "Nothing to undo.".into();
            return Vec::new();
        };
        for n in step.notes.iter().rev() {
            self.put_note(&n.id, &n.before);
        }
        for c in step.changes.iter().rev() {
            if let Some(u) = self.review.unit_mut(&c.id) {
                u.decision = c.before;
            }
        }
        self.reorder();
        let mut effects = vec![Effect::Save];
        effects.extend(self.show_step(&step));
        effects
    }

    /// `ctrl+r`: make the last undone step again, notes and all, and go to
    /// its first unit.
    pub fn redo(&mut self) -> Vec<Effect> {
        let was_done = self.all_reviewed();
        let Some(step) = self.review.history.redo() else {
            self.status = "Nothing to redo.".into();
            return Vec::new();
        };
        for c in &step.changes {
            if let Some(u) = self.review.unit_mut(&c.id) {
                u.decision = c.after;
            }
        }
        for n in &step.notes {
            self.put_note(&n.id, &n.after);
        }
        self.reorder();
        self.all_done_on_v1(was_done);
        let mut effects = vec![Effect::Save];
        effects.extend(self.show_step(&step));
        effects
    }

    /// Put the cursor of the screen on show on the first unit `step` changed.
    fn show_step(&mut self, step: &Step) -> Vec<Effect> {
        let first = (step.changes.first().map(|c| &c.id))
            .or(step.notes.first().map(|n| &n.id))
            .cloned();
        let Some(first) = first else {
            return Vec::new();
        };
        match self.screen {
            Screen::Slim | Screen::Queue => {
                if let Some(pos) = self
                    .order
                    .iter()
                    .position(|&i| self.review.units[i].id == first)
                {
                    return self.move_sel(pos);
                }
            }
            Screen::Swipe => {
                if let Some(pos) = self.swipe.cards.iter().position(|id| *id == first) {
                    self.swipe.cursor = pos;
                }
            }
            Screen::Flagged => {
                if let Some(pos) = self.flagged_units().iter().position(|u| u.id == first) {
                    self.flagged.sel = pos;
                }
                self.after_flagged_change();
            }
            Screen::Unit => return self.unit_show_step(&first),
        }
        Vec::new()
    }

    fn back(&mut self) {
        self.screen = self.return_to.pop().unwrap_or(self.list_screen());
    }

    // ---- Editor and reset ------------------------------------------------

    /// The unit the current screen is about.
    fn current_unit(&self) -> Option<&Unit> {
        match self.screen {
            Screen::Swipe => self.swipe_current().and_then(|id| self.review.unit(id)),
            Screen::Flagged => self.flagged_unit(),
            Screen::Unit => self.review.unit(&self.unit.id),
            _ => self.selected(),
        }
    }

    /// Open the current unit's file in the editor, at the unit's first line.
    fn edit(&mut self) -> Vec<Effect> {
        match self.current_unit() {
            Some(u) => {
                // A whole file opens at its first change, not at its top;
                // in the unit view, every unit does.
                let first = (u.kind == UnitKind::File || self.screen == Screen::Unit)
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

    /// Forget every decision, as if nothing had been looked at: units with
    /// no behavior change are accepted for you again, the rest pending. One
    /// undo step, so `u` brings them back.
    fn reset(&mut self) -> Vec<Effect> {
        let auto = self.cfg.auto_accept;
        let mut each = Vec::new();
        for (u, a) in self.review.units.iter_mut().zip(&self.levels) {
            let d = if accepted_for_you(a.level, auto) {
                u.accepted_by = Some(AcceptedBy::Auto);
                Decision::Accepted
            } else {
                Decision::Pending
            };
            each.push((u.id.clone(), d));
        }
        self.visual = None;
        self.decide_each(&each);
        if self.screen == Screen::Swipe {
            self.swipe = self.fresh_swipe();
        } else {
            self.swipe = SwipeState::default();
        }
        self.status = "Reset: nothing is marked as looked at.".into();
        vec![Effect::Save]
    }

    // ---- Swipe -----------------------------------------------------------

    /// The cards on entering swipe: listed units that are flagged or not yet
    /// looked at (accepted ones, by you or for you, are left out), most
    /// attention first, then in list order.
    fn swipe_cards(&self) -> Vec<String> {
        let mut cards: Vec<usize> = self
            .order
            .iter()
            .copied()
            .filter(|&i| self.review.units[i].decision != Decision::Accepted)
            .collect();
        cards.sort_by_key(|&i| std::cmp::Reverse(self.levels[i].level));
        cards
            .into_iter()
            .map(|i| self.review.units[i].id.clone())
            .collect()
    }

    /// Cards with the cursor on the first one (the summary when empty).
    /// Cards are never removed during a session, so going back a card can
    /// return to one just accepted.
    fn fresh_swipe(&self) -> SwipeState {
        let cards = self.swipe_cards();
        SwipeState {
            cards,
            cursor: 0,
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

    /// Accept (right) or flag (left) the card on show and move on.
    pub fn swipe_dir(&mut self, dir: Dir) -> Vec<Effect> {
        let d = match dir {
            Dir::Right => Decision::Accepted,
            Dir::Left => Decision::Investigate,
        };
        self.swipe_decide(d, 1)
    }

    /// Decide the card on show and the `times - 1` after it as one undo
    /// step, and move on past them.
    fn swipe_decide(&mut self, d: Decision, times: usize) -> Vec<Effect> {
        let ids: Vec<String> = (self.swipe.cards.iter())
            .skip(self.swipe.cursor)
            .take(times.max(1))
            .cloned()
            .collect();
        if ids.is_empty() {
            return Vec::new();
        }
        self.decide_ids(&ids, d);
        self.swipe.cursor += ids.len();
        let mut effects = vec![Effect::Save];
        effects.extend(self.prepare_cards());
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

    fn swipe_action(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        let times = count.unwrap_or(1) as usize;
        match a {
            Action::DecideAccept => self.swipe_decide(Decision::Accepted, times),
            Action::DecideFlag => self.swipe_decide(Decision::Investigate, times),
            Action::MoveUp if self.swipe.cursor > 0 => {
                self.swipe.cursor = self.swipe.cursor.saturating_sub(times);
                Vec::new()
            }
            Action::MoveUp => {
                self.prev_card();
                Vec::new()
            }
            Action::ViewBack | Action::ViewClose | Action::V1Swipe => {
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
        let effects = self.mouse(m);
        self.save_cursor(effects)
    }

    /// [`App::on_mouse`], before the cursor is saved.
    fn mouse(&mut self, m: MouseEvent) -> Vec<Effect> {
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
                let by = if down { SCROLL_STEP } else { -SCROLL_STEP };
                if self.screen == Screen::Unit {
                    // A link is in the ask panel.
                    let hit = self.hit(m.column, m.row);
                    let panel = matches!(hit, Some(Target::Panel | Target::AskLink(_)));
                    return self.unit_wheel(panel, by);
                }
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
                    return self.scroll_detail(by);
                }
                let a = if down {
                    Action::MoveDown
                } else {
                    Action::MoveUp
                };
                self.run_action(a, None)
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
                    Some(Target::Action(a)) => self.run_action(a, None),
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
                    Some(Target::Panel) => {
                        self.unit_click_panel();
                        Vec::new()
                    }
                    Some(Target::Detail) | None => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    // ---- Background events ----------------------------------------------

    pub fn on_app_event(&mut self, ev: AppEvent) -> Vec<Effect> {
        let effects = self.app_event(ev);
        self.save_cursor(effects)
    }

    /// [`App::on_app_event`], before the cursor is saved.
    fn app_event(&mut self, ev: AppEvent) -> Vec<Effect> {
        match ev {
            AppEvent::Answer { index, result } => return self.answered(index, result),
            AppEvent::Asking { index, progress } => self.asking(index, progress),
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
                self.visual = None;
                self.rebuild_order();
                let pos_of = |app: &App, id: &str| {
                    app.order
                        .iter()
                        .position(|&i| app.review.units.get(i).is_some_and(|u| u.id == id))
                };
                if let Some(pos) = keep.and_then(|id| pos_of(self, &id)) {
                    self.sel = pos;
                }
                self.screen = self.list_screen();
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
                let mut effects = Vec::new();
                if self.review.apply_dive(&ev) {
                    effects.push(Effect::Save);
                }
                effects.extend(self.unit_tests_found(&ev));
                return effects;
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{UnitKind, UnitStatus};
    use crossterm::event::KeyModifiers;

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
                accepted_by: None,
                reasons: Vec::new(),
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
            version: 0,
            history: Default::default(),
            branch: None,
            cursor: None,
        };
        let mut a = App::new(review, Config::default(), Keymap::default());
        a.sel = 0;
        a
    }

    fn dec(a: &App, i: usize) -> Decision {
        a.review.units[i].decision
    }

    /// Type `keys` (Neovim notation) as the terminal would send them.
    fn press(a: &mut App, keys: &str) -> Vec<Effect> {
        crate::keymap::parse_keys(keys)
            .unwrap()
            .into_iter()
            .flat_map(|k| a.on_key(k.event()))
            .collect()
    }

    fn decisions(a: &App) -> Vec<Decision> {
        a.review.units.iter().map(|u| u.decision).collect()
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
        a.on_action(Action::MoveNextFile);
        assert_eq!(a.sel, 3);
        a.on_action(Action::MoveNextFile);
        assert_eq!(a.sel, 6);
        a.on_action(Action::MovePrevFile);
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
        // `k` goes back a card, never a decision; `j` does nothing here.
        press(&mut a, "k");
        assert_eq!(a.swipe.cursor, 1);
        assert_eq!(dec(&a, 1), Decision::Investigate);
        press(&mut a, "j");
        assert_eq!(a.swipe.cursor, 1);
        // `a` accepts, `2f` flags this card and the next as one step.
        press(&mut a, "a2f");
        assert_eq!(
            (dec(&a, 1), dec(&a, 2), dec(&a, 3)),
            (
                Decision::Accepted,
                Decision::Investigate,
                Decision::Investigate
            )
        );
        assert!(a.swipe_current().is_none());
        assert_eq!(a.overlay, Overlay::None, "no note line in swipe");
        // Undo puts both back and shows the first of them.
        press(&mut a, "u");
        assert_eq!(
            (dec(&a, 2), dec(&a, 3)),
            (Decision::Pending, Decision::Pending)
        );
        assert_eq!(a.swipe_current().map(String::as_str), Some("u2"));
        // Re-entering drops the accepted cards and starts on the first one.
        a.enter_swipe();
        assert_eq!(a.swipe.cards, vec!["u2".to_string(), "u3".to_string()]);
        assert_eq!(a.swipe_current().map(String::as_str), Some("u2"));
    }

    /// `n` units on the v1 list (`--swipe`), as [`app`].
    fn v1(n: usize) -> App {
        let mut a = app(n);
        a.open_v1_list();
        a
    }

    #[test]
    fn the_queue_opens_by_level_and_l_opens_any_unit() {
        let mut a = app(3);
        assert_eq!((a.screen, a.grouping), (Screen::Queue, Grouping::Unit));
        // `s` was the v1 list's way into swipe.
        press(&mut a, "s");
        assert_eq!(a.screen, Screen::Queue);
        press(&mut a, "j<cr>");
        assert_eq!(a.screen, Screen::Unit, "a pending unit opens too");
        assert_eq!(a.sel, 1);
        // `--swipe` opens the v1 screens: swipe, with the v1 list behind it.
        let mut a = app(3);
        a.start(false, None, Vec::new(), None, true);
        assert_eq!(a.screen, Screen::Swipe);
        assert_eq!(a.grouping, Grouping::File);
        press(&mut a, "h");
        assert_eq!(a.screen, Screen::Slim);
        let mut a = app(3);
        a.start(false, None, Vec::new(), None, false);
        assert_eq!(a.screen, Screen::Queue);
    }

    #[test]
    fn l_opens_only_a_flagged_unit_on_the_v1_list() {
        let mut a = v1(3);
        press(&mut a, "l");
        assert_eq!(a.screen, Screen::Slim);
        assert_eq!(
            a.status,
            "Only flagged units open for now. f flags this one."
        );
        a.review.units[1].decision = Decision::Investigate;
        a.review.units[2].decision = Decision::Investigate;
        press(&mut a, "j<cr>");
        assert_eq!(a.screen, Screen::Flagged);
        assert_eq!(a.flagged_unit_id().as_deref(), Some("u1"));
        // `h` goes back where it came from.
        press(&mut a, "h");
        assert_eq!(a.screen, Screen::Slim);
        press(&mut a, "jl");
        assert_eq!(a.flagged_unit_id().as_deref(), Some("u2"));
    }

    #[test]
    fn the_next_key_dismisses_a_stale_status_message() {
        let mut a = app(2);
        press(&mut a, "za");
        assert!(a.status.contains("No units are accepted for you"));
        press(&mut a, "j");
        assert!(a.status.is_empty(), "status was {:?}", a.status);
        // An action that reports something still shows its message.
        press(&mut a, "za");
        assert!(a.status.contains("No units are accepted for you"));
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
        a.open_v1_list();
        let effects = a.on_action(Action::V1Swipe);
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
        small.open_v1_list();
        assert!(asked(&small.on_action(Action::V1Swipe)).is_empty());
        assert_eq!(small.screen, Screen::Swipe);
        let mut no_agent = large(40);
        no_agent.has_agent = false;
        no_agent.open_v1_list();
        assert!(asked(&no_agent.on_action(Action::V1Swipe)).is_empty());
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

    use Decision::{Accepted as Acc, Investigate as Flag, Pending as Pend};

    #[test]
    fn counts_move_that_many_rows() {
        let mut a = app(7);
        press(&mut a, "5j");
        assert_eq!(a.sel, 5);
        press(&mut a, "2k");
        assert_eq!(a.sel, 3);
        press(&mut a, "20j");
        assert_eq!(a.sel, 6, "as far as the list goes");
        // `3gg` and `3G` go to the third row, as in Neovim.
        press(&mut a, "3gg");
        assert_eq!(a.sel, 2);
        press(&mut a, "G");
        assert_eq!(a.sel, 6);
        press(&mut a, "1G");
        assert_eq!(a.sel, 0);
        assert_eq!(decisions(&a), vec![Pend; 7], "moves decide nothing");
    }

    #[test]
    fn a_count_jumps_that_many_pending_units_or_files() {
        let mut a = app(9);
        a.review.units[1].decision = Acc;
        a.review.units[3].decision = Flag;
        // Pending after u0: u2, u4, u5.
        press(&mut a, "2n");
        assert_eq!(a.sel, 4);
        press(&mut a, "N");
        assert_eq!(a.sel, 2);
        press(&mut a, "9n");
        assert_eq!(a.sel, 8, "as far as there are");
        press(&mut a, "gg2]f");
        assert_eq!(a.sel, 6, "two files on: f2.rs");
        press(&mut a, "2[f");
        assert_eq!(a.sel, 0);
    }

    #[test]
    fn a_count_decides_that_many_pending_units_as_one_step() {
        let mut a = app(6);
        a.review.units[1].decision = Acc;
        let effects = press(&mut a, "3a");
        assert!(effects.contains(&Effect::Save));
        // u0 and the next two pending ones: u2, u3 (u1 was decided).
        assert_eq!(decisions(&a), vec![Acc, Acc, Acc, Acc, Pend, Pend]);
        assert_eq!(a.sel, 4, "on to the next pending unit");
        assert_eq!(a.status, "Accepted 3 units.");
        assert_eq!(a.review.history.undo.len(), 1);
        assert_eq!(a.review.history.undo[0].changes.len(), 3);
        press(&mut a, "u");
        assert_eq!(decisions(&a), vec![Pend, Acc, Pend, Pend, Pend, Pend]);
        assert_eq!(a.sel, 0, "undo goes to the step's first unit");
        // A flag with a count opens no note line.
        press(&mut a, "2f");
        assert_eq!(decisions(&a), vec![Flag, Acc, Flag, Pend, Pend, Pend]);
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn f_flags_moves_on_and_opens_the_note_line_on_the_flagged_unit() {
        let mut a = app(3);
        let effects = press(&mut a, "f");
        assert!(effects.contains(&Effect::Save));
        assert_eq!(dec(&a, 0), Flag);
        assert_eq!(a.sel, 1);
        assert!(matches!(&a.overlay, Overlay::Note { id, .. } if id == "u0"));
        press(&mut a, "<esc>");
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.review.history.undo.len(), 1);
    }

    #[test]
    fn visual_mode_decides_the_selection_as_one_step() {
        let mut a = app(6);
        press(&mut a, "Vjj");
        assert_eq!(a.visual, Some(0));
        assert_eq!(a.sel, 2);
        assert_eq!(decisions(&a), vec![Pend; 6], "moving only selects");
        press(&mut a, "a");
        assert_eq!(a.visual, None, "deciding leaves visual mode");
        assert_eq!(decisions(&a), vec![Acc, Acc, Acc, Pend, Pend, Pend]);
        assert_eq!(a.sel, 3);
        assert_eq!(a.review.history.undo.len(), 1);
        // Upwards works too; a flag opens one note line for the selection.
        press(&mut a, "jjVkf");
        assert_eq!(decisions(&a), vec![Acc, Acc, Acc, Pend, Flag, Flag]);
        assert!(matches!(
            &a.overlay,
            Overlay::Note { id, also, .. } if id == "u4" && also == &["u5"]
        ));
        press(&mut a, "<esc>");
        assert_eq!(a.last_decision, Some((Action::DecideFlag, Some(2))));
        // esc leaves it, deciding nothing.
        press(&mut a, "ggVj<esc>");
        assert_eq!(a.visual, None);
        assert_eq!(a.review.history.undo.len(), 2);
        // Any other action leaves it first, then runs.
        press(&mut a, "V?");
        assert_eq!(a.visual, None);
        assert_eq!(a.overlay, Overlay::Help);
    }

    #[test]
    fn dot_repeats_the_last_decision_with_its_count_or_a_new_one() {
        let mut a = app(10);
        press(&mut a, ".");
        assert_eq!(a.status, "Nothing to repeat yet.");
        press(&mut a, "a.");
        assert_eq!(&decisions(&a)[..3], &[Acc, Acc, Pend]);
        assert_eq!(a.sel, 2);
        // The count goes with it...
        press(&mut a, "2a.");
        assert_eq!(&decisions(&a)[..7], &[Acc, Acc, Acc, Acc, Acc, Acc, Pend]);
        // ...unless a new one is typed, which it keeps.
        press(&mut a, "3.");
        assert_eq!(&decisions(&a)[..9], &[Acc; 9]);
        assert_eq!(a.last_decision, Some((Action::DecideAccept, Some(3))));
        // Each one is a step of its own.
        assert_eq!(a.review.history.undo.len(), 5);
        // After visual mode it decides as many units.
        let mut a = app(6);
        press(&mut a, "Vja.");
        assert_eq!(decisions(&a), vec![Acc, Acc, Acc, Acc, Pend, Pend]);
    }

    #[test]
    fn dot_after_a_file_decision_decides_the_file_under_the_cursor() {
        let mut a = app(9);
        press(&mut a, "A");
        assert_eq!(&decisions(&a)[..6], &[Acc, Acc, Acc, Pend, Pend, Pend]);
        assert_eq!(a.sel, 3, "on to the next file's first pending unit");
        press(&mut a, ".");
        assert_eq!(decisions(&a), [vec![Acc; 6], vec![Pend; 3]].concat());
        assert_eq!(a.sel, 6);
        // On the v1 list it asks first.
        let mut a = v1(6);
        press(&mut a, "Ay");
        assert_eq!(decisions(&a), vec![Acc, Acc, Acc, Pend, Pend, Pend]);
        press(&mut a, "]f.");
        assert!(matches!(
            &a.overlay,
            Overlay::ConfirmAcceptFile { file, ids }
                if file == &PathBuf::from("f1.rs") && ids.len() == 3
        ));
    }

    #[test]
    fn undo_and_redo_go_through_the_reviews_history() {
        let mut a = app(4);
        press(&mut a, "a");
        press(&mut a, "f<esc>");
        assert_eq!(decisions(&a), vec![Acc, Flag, Pend, Pend]);
        assert_eq!(a.review.history.undo.len(), 2);
        let effects = press(&mut a, "u");
        assert!(effects.contains(&Effect::Save));
        assert_eq!(decisions(&a), vec![Acc, Pend, Pend, Pend]);
        assert_eq!(a.sel, 1);
        press(&mut a, "u");
        assert_eq!(decisions(&a), vec![Pend; 4]);
        assert_eq!(a.sel, 0);
        press(&mut a, "u");
        assert_eq!(a.status, "Nothing to undo.");
        let effects = press(&mut a, "<c-r>");
        assert!(effects.contains(&Effect::Save));
        assert_eq!(decisions(&a), vec![Acc, Pend, Pend, Pend]);
        assert_eq!(a.sel, 0);
        assert_eq!(
            (a.review.history.undo.len(), a.review.history.redo.len()),
            (1, 1)
        );
        // The steps are part of the review, so saving and loading keeps them.
        let json = serde_json::to_string(&a.review).unwrap();
        let loaded: Review = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.history, a.review.history);
        // A new decision forgets what was undone.
        press(&mut a, "ja");
        press(&mut a, "<c-r>");
        assert_eq!(a.status, "Nothing to redo.");
    }

    #[test]
    fn reset_is_one_step_that_undo_takes_back() {
        let mut a = app(4);
        press(&mut a, "aaf<esc>");
        press(&mut a, ":e!<cr>");
        assert_eq!(a.overlay, Overlay::ConfirmReset);
        press(&mut a, "y");
        assert_eq!(decisions(&a), vec![Pend; 4]);
        press(&mut a, "u");
        assert_eq!(decisions(&a), vec![Acc, Acc, Flag, Pend]);
    }

    #[test]
    fn upper_f_on_the_v1_list_flags_the_files_pending_units_after_a_confirm() {
        let mut a = v1(6);
        a.review.units[1].decision = Acc;
        press(&mut a, "F");
        assert!(matches!(
            &a.overlay,
            Overlay::ConfirmFlagFile { file, ids }
                if file == &PathBuf::from("f0.rs") && ids == &["u0", "u2"]
        ));
        // Anything but yes changes nothing.
        press(&mut a, "n");
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(decisions(&a), vec![Pend, Acc, Pend, Pend, Pend, Pend]);
        let effects = press(&mut a, "Fy");
        assert!(effects.contains(&Effect::Save));
        assert_eq!(decisions(&a), vec![Flag, Acc, Flag, Pend, Pend, Pend]);
        assert_eq!(a.status, "Flagged 2 units.");
        assert_eq!(a.sel, 0, "the selection stays");
        assert_eq!(a.review.history.undo.len(), 1);
        press(&mut a, "u");
        assert_eq!(decisions(&a), vec![Pend, Acc, Pend, Pend, Pend, Pend]);
        // Nothing pending: nothing to confirm.
        a.review.units[0].decision = Acc;
        a.review.units[2].decision = Acc;
        press(&mut a, "F");
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn w_finishes_in_three_ways() {
        // Something left and no notes: say what is left.
        let mut a = app(3);
        a.review.units[0].decision = Acc;
        press(&mut a, ":w<cr>");
        assert_eq!(a.status, "Nothing to finish yet: 2 units pending.");
        assert_eq!(a.overlay, Overlay::None);
        // Notes: the notes view.
        a.review.units[1].dive.note = "why?".into();
        press(&mut a, ":w<cr>");
        assert!(matches!(a.overlay, Overlay::Notes { .. }));
        // Everything accepted: the all-done sheet.
        let mut a = app(2);
        a.review.units[0].decision = Acc;
        a.review.units[1].decision = Acc;
        press(&mut a, ":w<cr>");
        assert_eq!(a.overlay, Overlay::AllDone);
    }

    #[test]
    fn q_quits_the_list_but_leaves_the_flagged_screen() {
        let mut a = v1(2);
        assert_eq!(press(&mut a, "q"), vec![Effect::Save, Effect::Quit]);
        a.review.units[0].decision = Flag;
        press(&mut a, "l");
        assert_eq!(a.screen, Screen::Flagged);
        let effects = press(&mut a, "q");
        assert!(!effects.contains(&Effect::Quit));
        assert_eq!(a.screen, Screen::Slim);
        assert!(a.return_to.is_empty());
    }

    #[test]
    fn accepting_on_the_flagged_screen_takes_the_unit_off_it() {
        let mut a = v1(3);
        a.review.units[0].decision = Flag;
        a.review.units[2].decision = Flag;
        press(&mut a, "l");
        press(&mut a, "a");
        assert_eq!(decisions(&a), vec![Acc, Pend, Flag]);
        assert_eq!(a.screen, Screen::Flagged);
        assert_eq!(a.flagged_unit_id().as_deref(), Some("u2"));
        // `f` on a flagged unit opens its note line.
        press(&mut a, "f");
        assert!(matches!(&a.overlay, Overlay::Note { id, .. } if id == "u2"));
        press(&mut a, "<esc>u");
        assert_eq!(decisions(&a), vec![Flag, Pend, Flag]);
        assert_eq!(a.flagged_unit_id().as_deref(), Some("u0"));
        // The last one accepted closes the screen.
        press(&mut a, "2a");
        assert_eq!(a.screen, Screen::Slim);
    }

    #[test]
    fn za_and_hunk_jumps_say_what_comes_later() {
        let mut a = app(2);
        press(&mut a, "za");
        assert_eq!(a.status, "No units are accepted for you.");
        assert!(!a.show_auto);
        press(&mut a, "]c");
        assert_eq!(a.status, "Hunk jumps work in the unit view.");
        assert_eq!(decisions(&a), vec![Pend; 2]);
    }

    #[test]
    fn a_tool_key_runs_on_a_flagged_unit_only() {
        let mut a = v1(2);
        press(&mut a, "<space>t");
        assert_eq!(a.status, "Flag the unit (f) to use this.");
        assert_eq!(a.screen, Screen::Slim);
        a.review.units[0].decision = Flag;
        press(&mut a, "<space>t");
        assert_eq!(a.screen, Screen::Flagged);
        assert!(a.review.units[0].dive.asked.contains(&DiveKind::Tests));
        assert_eq!(a.flagged.pane, Pane::Answer(DiveKind::Tests));
    }

    #[test]
    fn half_page_scrolls_go_by_half_the_detail_area() {
        let mut a = app(2);
        a.detail_rows.set(20);
        a.detail_max_scroll.set(100);
        press(&mut a, "<c-d>");
        assert_eq!(a.detail_offset(), 10);
        press(&mut a, "<c-d><c-u><c-u>");
        assert_eq!(a.detail_offset(), 0);
    }

    #[test]
    fn find_and_goto_open_the_jump_list() {
        let mut a = app(2);
        press(&mut a, "/");
        assert!(matches!(
            &a.overlay,
            Overlay::Jump {
                kind: JumpKind::Find,
                ..
            }
        ));
        a.overlay = Overlay::None;
        // Nothing here calls anything: the empty list doesn't open, the
        // status says why (lists with entries: see `super::jump`).
        press(&mut a, "gd");
        assert_eq!(a.overlay, Overlay::None);
        assert!(a.status.contains("calls nothing"), "{}", a.status);
        press(&mut a, "jgr");
        assert_eq!(a.overlay, Overlay::None);
        assert!(a.status.contains("calls s1"), "{}", a.status);
        assert_eq!(press(&mut a, "<space>s"), vec![Effect::Settings]);
    }

    #[test]
    fn an_unknown_command_changes_nothing_and_says_so() {
        let mut a = app(3);
        press(&mut a, "ja");
        let before = (decisions(&a), a.sel, a.screen, a.overlay.clone());
        let effects = press(&mut a, ":foo<cr>");
        assert!(effects.is_empty());
        assert_eq!(a.status, "Not a command: :foo");
        assert_eq!((decisions(&a), a.sel, a.screen, a.overlay.clone()), before);
    }

    #[test]
    fn a_click_on_a_key_runs_its_action() {
        let mut a = app(3);
        *a.targets.borrow_mut() =
            vec![(Rect::new(0, 0, 5, 1), Target::Action(Action::DecideAccept))];
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(dec(&a, 0), Acc);
        assert_eq!(a.review.history.undo.len(), 1);
    }
}
