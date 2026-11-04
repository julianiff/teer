//! A review's state and what keys, actions and background events do to
//! it. Handlers are synchronous and return [`Effect`]s for the runtime to
//! carry out, so all of this is testable without a screen or an agent. No
//! front end lives here: keys come in as [`Key`]s or as [`Action`]s with a
//! count, and what a front end needs to remember of its own drawing (the
//! mouse's targets, colours) stays with it.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use crate::agent::SessionInfo;
use crate::approve::{Inline, Verdict};
use crate::commit;
use crate::config::{Config, Order, Provider};
use crate::keymap::{Action, Edit, Key, Keymap, Outcome};
use crate::review::ask::Progress;
use crate::review::attention::{Assessment, Level, accepted_for_you, assess_all, settle};
use crate::review::dive::{DiveEvent, uses_web};
use crate::review::history::{Change, Note, NoteChange, Step};
use crate::review::model::{
    AcceptedBy, Answer, Decision, DiveKind, PrInfo, Prep, Question, Review, SessionRef, Unit,
    UnitKind,
};
use crate::review::prepare::PrepareEvent;
use crate::review::story;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Screen {
    /// The queue (Phase 4): what teer opens on.
    Queue,
    /// One unit, any unit, with its answers (Phase 5).
    Unit,
    /// The flagged units one after another (`<space>f`, see
    /// [`super::flagged_list`]).
    Flagged,
}

/// What the queue's level boxes hold (`toggle.by_file` switches).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum Grouping {
    /// Files, each in the box of the most attention one of its units
    /// needs, its units under its name.
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
    },
    /// Every note of the review (see `crate::notes`) and the selected one:
    /// to go to, edit, delete or copy (`:w` takes them out of teer).
    Notes {
        sel: usize,
    },
    /// Asks before deleting the note on unit `id`; then back to the notes at `sel`.
    ConfirmDeleteNote {
        id: String,
        sel: usize,
    },
    /// Asks once per session before the first question of the unit view's
    /// ask panel may use the web.
    ConfirmWeb,
    /// The commits between base and head and the PR's description: the
    /// selected commit, and the selected one of the units it changed while
    /// the cursor is in that list.
    Commits {
        sel: usize,
        unit: Option<usize>,
    },
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum JumpKind {
    /// Every listed unit whose file or name matches the query.
    Find,
    /// What unit `of` calls, in this review.
    Definition { of: String },
    /// What calls unit `of`, in this review.
    Callers { of: String },
}

/// The unit view's ask panel: the question being typed, the link in focus
/// while the keys move between links instead, the questions on their way,
/// and the scroll.
#[derive(Debug, Default)]
pub struct AskState {
    /// Index into [`App::ask_links`]; `None` while typing.
    pub link: Option<usize>,
    /// Questions, by index, the agent is answering.
    pub pending: BTreeSet<usize>,
    /// What came of those so far, by index.
    pub live: BTreeMap<usize, LiveAnswer>,
    /// The question being typed.
    pub draft: String,
    /// The conversation's first line shown; `None` follows its end.
    pub chat_scroll: Cell<Option<usize>>,
    /// The conversation's last line that can be first, as last drawn.
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

/// Where a link in an answer goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskLink {
    /// A unit of this change, in the unit view.
    Unit(String),
    /// Shown code, in the editor.
    Code { file: PathBuf, line: u32 },
}

/// Longest question, in characters.
pub const QUESTION_MAX: usize = 500;

/// Edit `text` with `e`, a key of a line being typed ([`Keymap::edit`]): a
/// character goes in while it is shorter than `max`, backspace takes the
/// last off, [`Edit::ClearLine`] clears it. False for any other key.
pub(crate) fn typed(text: &mut String, e: Edit, max: usize) -> bool {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
pub(crate) fn units(n: usize) -> String {
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

/// "5 s", "0.5 s".
fn seconds(d: std::time::Duration) -> String {
    let s = d.as_secs_f64();
    if s.fract() == 0.0 {
        format!("{} s", d.as_secs())
    } else {
        format!("{s:.1} s")
    }
}

/// What the queue still does while it waits for the changes to copied
/// notes: it is read-only, so it moves and scrolls, shows the help and its
/// other views, and re-runs (`q` stops waiting).
fn waiting_allows(a: Action) -> bool {
    let id = a.id();
    id.starts_with("move.")
        || id.starts_with("scroll.")
        || matches!(
            a,
            Action::HelpOpen
                | Action::ReviewRerun
                | Action::ReviewReset
                | Action::ToggleAutoAccepted
                | Action::ToggleByFile
        )
}

/// Whether note change `n` of `step` put a note away: the step accepted its
/// unit and the note went (see [`App::decide_each`]). Notes typed on a note
/// line only ever join a flag's step.
fn archives(step: &Step, n: &NoteChange) -> bool {
    n.after == Note::default()
        && step.changes.iter().any(|c| {
            c.id == n.id && c.after == Decision::Accepted && c.before != Decision::Accepted
        })
}

/// Lines the scroll keys and the mouse wheel move the detail area by.
pub const SCROLL_STEP: isize = 3;

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
    /// Ask the agent a question about a unit: its recommendation, or a
    /// tool's answer.
    // Named for the question (`DiveKind`), not the screen that asks it.
    Dive {
        unit_id: String,
        kind: DiveKind,
        /// Whether the agent may search the web for this question.
        web: bool,
    },
    /// Have the agent read the tests teer's search found for a unit: which
    /// cover it and what dangerous cases none does (the Tests panel).
    InterpretTests {
        unit_id: String,
    },
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
    /// An agent answer to a question about a unit (see `DiveKind`).
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
    /// The working tree as teer looked at it at `at`, while waiting for the
    /// changes to copied notes (see [`App::watch_due`]), or why it couldn't.
    Tree {
        seen: Result<crate::watch::Snapshot, String>,
        at: Instant,
    },
}

/// Who changed the code since the notes went out, as the status line says
/// it after the re-run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangedBy {
    /// The agent CLI the notes were handed to: its exit is the signal.
    Agent(Provider),
    /// Whoever got them from the clipboard (`y` on the sheet, or the model
    /// API's copy): the files going quiet is the signal.
    Copied,
}

impl ChangedBy {
    /// "by Claude Code", "since the notes were copied".
    fn words(self) -> String {
        match self {
            ChangedBy::Agent(p) => format!("by {}", p.title()),
            ChangedBy::Copied => "since the notes were copied".into(),
        }
    }
}

/// Waiting for the changes to notes that were copied out of teer: the
/// queue stays on show and takes no decisions, and teer looks at the
/// working tree until it has changed and settled (see [`crate::watch`]).
#[derive(Debug, Clone)]
pub struct Waiting {
    /// How many notes (and flagged units without one) went out.
    pub notes: usize,
    /// Whether the tree changed and has been quiet for long enough.
    pub quiet: crate::watch::Quiet,
    /// When to look at the tree next; `None` while a look is on its way.
    pub next: Option<Instant>,
}

/// One review as the reviewer goes through it: the review itself, the
/// screen and overlay on show, the queue's order and cursor, each view's
/// state, and what is on its way back from the agent.
pub struct App {
    pub review: Review,
    pub cfg: Config,
    pub keys: Keymap,
    pub screen: Screen,
    pub overlay: Overlay,
    /// The panel the help (`?`) was opened over: what it lists the keys of,
    /// and what comes back when it closes.
    pub help_over: Option<Overlay>,

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
    /// Visual mode (`V`): the unit the selection started on, by id; it
    /// runs from where that unit is listed now to `sel` (see
    /// [`App::visual_rows`]), however a re-listing moved it.
    pub visual: Option<String>,
    /// The last decision action and its count, for `.`.
    pub last_decision: Option<(Action, Option<u32>)>,
    /// How far each unit is indented under the units it is read beneath in
    /// its file and level, by index into `review.units` (0 in line order).
    pub depth: Vec<usize>,
    /// Attention level per unit, by index into `review.units`.
    pub levels: Vec<Assessment>,
    pub sel: usize,
    pub large: bool,

    /// The unit view (see [`super::unit`]).
    pub unit: super::unit::UnitView,
    /// The flagged list's state (see [`super::flagged_list`]).
    pub flagged: super::flagged_list::FlaggedView,
    /// The user agreed to web use for questions this session.
    pub dive_web_ok: bool,
    pub status: String,
    pub tick: u64,
    /// An approval is being sent.
    pub approving: bool,
    /// What a long question is doing now, by unit and kind (Behavior).
    pub progress: std::collections::HashMap<(String, DiveKind), String>,

    /// How far the detail area (the queue's preview, the unit view's text)
    /// is scrolled, in lines, and what it showed then: another unit starts
    /// at the top.
    pub detail_scroll: RefCell<(String, usize)>,
    /// Furthest the detail area can scroll, and its rows (written while drawing).
    pub detail_max_scroll: Cell<usize>,
    pub detail_rows: Cell<usize>,
    /// The short id of the commit made from this review in this session.
    pub committed: Option<String>,
    /// The `:w` sheet's message, summary and result (see [`super::finish`]).
    pub finish: super::finish::FinishState,
    /// The summary of the whole change was asked for this session.
    pub summary_asked: bool,
    pub ask: AskState,
    /// Waiting for the changes to copied notes (Phase 7's waiting screen).
    pub waiting: Option<Waiting>,
    /// Who the notes that went out last are with, until the re-run that
    /// brings their changes says so.
    pub fix_by: Option<ChangedBy>,
    /// The units the last re-run found changed or new, by id: listed at the
    /// top of their level until the next one.
    pub changed: HashSet<String>,
}

impl App {
    pub fn new(review: Review, cfg: Config, keys: Keymap) -> App {
        let mut app = App {
            review,
            keys,
            screen: Screen::Queue,
            overlay: Overlay::None,
            help_over: None,
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
            unit: Default::default(),
            flagged: Default::default(),
            dive_web_ok: false,
            status: String::new(),
            tick: 0,
            approving: false,
            progress: Default::default(),
            detail_scroll: RefCell::new((String::new(), 0)),
            detail_max_scroll: Cell::new(0),
            detail_rows: Cell::new(0),
            committed: None,
            finish: Default::default(),
            summary_asked: false,
            ask: AskState::default(),
            waiting: None,
            fix_by: None,
            changed: HashSet::new(),
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
    /// listed last, and only with `show_auto`. Units the last re-run changed
    /// ([`App::changed`]) lead their box (listed by file, their files do,
    /// and they lead their file). The selection stays on the same unit.
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
        let fresh = |i: usize| self.changed.contains(&units[i].id);
        let fresh_files: HashSet<&PathBuf> = listed
            .iter()
            .filter(|&&i| fresh(i))
            .map(|&i| &units[i].file)
            .collect();
        // What goes to the top of a box: a changed unit, or its file.
        let leads = |i: usize| {
            if by_file {
                fresh_files.contains(&units[i].file)
            } else {
                fresh(i)
            }
        };
        let mut order = listed;
        order.sort_by(|&a, &b| {
            let (fa, fb) = (&units[a].file, &units[b].file);
            (boxed(b) as u8)
                .cmp(&(boxed(a) as u8))
                .then_with(|| leads(b).cmp(&leads(a)))
                .then_with(|| fa.cmp(fb))
                .then_with(|| (level(b) as u8).cmp(&(level(a) as u8)))
                .then_with(|| fresh(b).cmp(&fresh(a)))
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
        // The selection runs from its unit: no longer listed, it ends.
        if let Some(id) = &self.visual
            && self.position_of(id).is_none()
        {
            self.visual = None;
        }
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
    /// explains why there is no agent, if there isn't one.
    pub fn start(
        &mut self,
        has_agent: bool,
        note: Option<String>,
        sessions: Vec<SessionInfo>,
        session: Option<SessionRef>,
    ) -> Vec<Effect> {
        self.has_agent = has_agent;
        self.sessions = sessions;
        self.agent_note = note;
        let mut effects = Vec::new();
        self.review.set_session(session.filter(|_| self.has_agent));
        self.screen = Screen::Queue;
        effects.extend(self.prepare_start());
        // A review described in an earlier run that has no summary yet.
        if self.can_ask() && crate::review::intent::wants_summary(&self.review) {
            self.summary_asked = true;
            effects.push(Effect::Summarize);
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

    /// Large reviews prepare a file's units when the user gets to them.
    pub(crate) fn ensure_prepared(&mut self) -> Vec<Effect> {
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

    /// What the detail area shows: the screen and the unit.
    fn detail_key(&self) -> String {
        match self.screen {
            Screen::Queue => {
                format!("list {}", self.selected().map_or("", |u| u.id.as_str()))
            }
            Screen::Unit => format!("unit {}", self.unit.id),
            Screen::Flagged => "flagged".into(),
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
    /// allowed: in the unit view, its text or the panel open under it.
    pub fn scroll_detail(&mut self, by: isize) -> Vec<Effect> {
        match self.screen {
            Screen::Unit => return self.unit_scroll(by),
            Screen::Flagged => return self.flagged_scroll(by),
            Screen::Queue => {}
        }
        let to = self
            .detail_offset()
            .saturating_add_signed(by)
            .min(self.detail_max_scroll.get());
        self.detail_scroll.borrow_mut().1 = to;
        Vec::new()
    }

    // ---- Input -----------------------------------------------------------

    /// A key pressed: the overlay or panel on show takes it, else the
    /// keymap turns it into an action.
    pub fn on_key(&mut self, k: Key) -> Vec<Effect> {
        if self.keys.is_interrupt(k) {
            return vec![Effect::Save, Effect::Quit];
        }
        // A status message answers the last action; the next key dismisses it.
        // Actions that report something set it again after this.
        self.status.clear();
        let effects = match self.overlay_key(k) {
            Some(effects) => effects,
            None => self.normal_key(k),
        };
        // While waiting, the status line says so unless a key said more.
        if self.waiting.is_some() && self.status.is_empty() {
            self.status = self.waiting_line();
        }
        self.save_cursor(effects)
    }

    /// Keep the unit under the cursor in the review (`Review.cursor`) and
    /// save it when it moved, so `q` and a crash both resume on it. A full
    /// save writes it too.
    pub fn save_cursor(&mut self, mut effects: Vec<Effect>) -> Vec<Effect> {
        let at = self.selected_id();
        if self.review.cursor != at {
            self.review.cursor = at;
            if !effects.contains(&Effect::Save) {
                effects.push(Effect::SaveCursor);
            }
        }
        effects
    }

    /// Keys of the overlay on show, or of the unit view's ask panel: its
    /// text input, prompt or sheet. `None` passes the key on to the screen.
    fn overlay_key(&mut self, k: Key) -> Option<Vec<Effect>> {
        match self.overlay.clone() {
            Overlay::Jump { .. } => return Some(self.jump_key(k)),
            Overlay::Finish { .. } => return Some(self.finish_key(k)),
            Overlay::Note { .. } => return Some(self.note_key(k)),
            Overlay::Notes { sel } => return Some(self.notes_key(sel, k)),
            Overlay::ConfirmDeleteNote { id, sel } => {
                self.overlay = Overlay::Notes { sel };
                if self.keys.confirm(k)
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
                let yes = self.keys.confirm(k);
                return Some(self.unit_answer_web(yes).unwrap_or_default());
            }
            Overlay::ConfirmReset => {
                self.overlay = Overlay::None;
                if self.keys.confirm(k) {
                    return Some(self.reset_and_rerun());
                }
                return Some(Vec::new());
            }
            Overlay::Help => {
                self.overlay = self.help_over.take().unwrap_or(Overlay::None);
                return Some(Vec::new());
            }
            Overlay::Commits { sel, unit } => return Some(self.commits_key(sel, unit, k)),
            Overlay::None => {}
        }
        if self.unit_panel_has_keys() {
            return Some(self.unit_panel_key(k));
        }
        None
    }

    /// Normal mode: the keymap turns keys into actions for the screen.
    fn normal_key(&mut self, k: Key) -> Vec<Effect> {
        match self.keys.feed(k, std::time::Instant::now()) {
            Outcome::Run { action, count } => self.run_action(action, count),
            Outcome::Error(e) => {
                self.status = e;
                Vec::new()
            }
            Outcome::None | Outcome::Pending | Outcome::Command => Vec::new(),
        }
    }

    /// The action `k` completes in a panel that moves through a list (the
    /// notes, the commits, the ask panel's links, the `gd`/`gr` list), if
    /// any. Sequences and the command line work as in normal mode; counts
    /// are dropped, and an unknown command is reported.
    pub(crate) fn panel_action(&mut self, k: Key) -> Option<Action> {
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
        if self.waiting.is_some() {
            if a == Action::ViewClose {
                self.stop_waiting();
                return Vec::new();
            }
            if !waiting_allows(a) {
                return Vec::new();
            }
        }
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
        if a.is_decision() {
            self.last_decision = Some((a, count));
        }
        self.act(a, count)
    }

    /// The help (`?`) for the screen, or for the panel open on it.
    pub fn open_help(&mut self) {
        let over = std::mem::replace(&mut self.overlay, Overlay::Help);
        self.help_over = (over != Overlay::None).then_some(over);
    }

    /// Run `a` once, without a count.
    pub fn on_action(&mut self, a: Action) -> Vec<Effect> {
        self.run_action(a, None)
    }

    /// Actions that work the same on every screen, then the screen's own.
    fn act(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        match a {
            Action::HelpOpen => {
                self.open_help();
                return Vec::new();
            }
            Action::ReviewReset => {
                self.overlay = Overlay::ConfirmReset;
                return Vec::new();
            }
            Action::ReviewRerun => return self.rerun_now(),
            Action::AskOpen if self.screen == Screen::Unit => {
                self.open_ask_panel();
                return Vec::new();
            }
            // From the queue or the flagged list: the unit view on the
            // unit, asking.
            Action::AskOpen => {
                let Some(id) = self.current_unit().map(|u| u.id.clone()) else {
                    return Vec::new();
                };
                let effects = self.open_unit(id);
                self.open_ask_panel();
                return effects;
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
            // On the ask tile, `i` types the question.
            Action::NoteEdit if self.unit_ask_focused() => {
                self.open_ask_panel();
                return Vec::new();
            }
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
            Action::ToolFlagged => return self.open_flagged(),
            Action::ToolHistory => {
                self.open_commits();
                return Vec::new();
            }
            Action::ToolSettings => return vec![Effect::Settings],
            Action::FinishOpen => return self.open_finish(),
            Action::ToggleAutoAccepted => {
                self.toggle_auto();
                return Vec::new();
            }
            Action::HistoryUndo => return self.undo(),
            Action::HistoryRedo => return self.redo(),
            Action::MoveNextHunk | Action::MovePrevHunk if self.screen == Screen::Queue => {
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
            Action::ToolAdvice
            | Action::ToolTests
            | Action::ToolRunOldNew
            | Action::ToolDiagram => {
                return self.open_tool(a);
            }
            _ => {}
        }
        match self.screen {
            Screen::Queue => self.queue_action(a, count),
            Screen::Unit => self.unit_action(a, count),
            Screen::Flagged => self.flagged_action(a, count),
        }
    }

    /// A tool (`<space>a` `<space>t` `<space>r` `<space>v`): in the unit
    /// view, its panel; from the queue, the unit view on the unit with that
    /// panel.
    fn open_tool(&mut self, a: Action) -> Vec<Effect> {
        if self.screen == Screen::Unit {
            return self.unit_tool(a);
        }
        let Some(id) = self.current_unit().map(|u| u.id.clone()) else {
            return Vec::new();
        };
        let mut effects = self.open_unit(id);
        effects.extend(self.unit_tool(a));
        effects
    }

    /// Ask `kind` about unit `id` unless it is answered or in flight. The
    /// unit view never stops to ask: the web is used once allowed (see
    /// [`App::web_allowed`]).
    pub(crate) fn dive_on(&mut self, id: &str, kind: DiveKind) -> Vec<Effect> {
        let web_ok = self.web_allowed();
        let Some(u) = self.review.unit_mut(id) else {
            return Vec::new();
        };
        if u.dive.results.contains_key(&kind) || u.dive_pending.contains(&kind) {
            return Vec::new();
        }
        u.dive_pending.insert(kind);
        vec![Effect::Dive {
            unit_id: id.to_string(),
            kind,
            web: web_ok && uses_web(kind),
        }]
    }

    /// Have the agent read what the Tests search found for unit `id`.
    pub(crate) fn interpret_tests_on(&mut self, id: &str) -> Vec<Effect> {
        let kind = DiveKind::Tests;
        let Some(u) = self.review.unit_mut(id) else {
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
        vec![Effect::InterpretTests {
            unit_id: id.to_string(),
        }]
    }

    /// Open the picture of the unit on show, or say how to draw one.
    pub(crate) fn open_visual(&mut self) -> Vec<Effect> {
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

    /// Keys while the note line is open, a text input ([`Keymap::edit`]):
    /// every letter is text, `enter` saves, `esc` closes it and keeps the
    /// note as it was. Saving marks the note as written on the code as it
    /// is now; an empty note is no note.
    fn note_key(&mut self, k: Key) -> Vec<Effect> {
        let Overlay::Note { id, mut text, back } = self.overlay.clone() else {
            return Vec::new();
        };
        let after = back.map_or(Overlay::None, |sel| Overlay::Notes { sel });
        match self.keys.edit(k) {
            Edit::Cancel => self.overlay = after,
            Edit::Submit => {
                self.overlay = after;
                let note: String = text.chars().filter(|c| !c.is_control()).collect();
                let note: String = note.trim().chars().take(NOTE_MAX).collect();
                if !self.write_note(&id, &note) {
                    return Vec::new();
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

    /// Put `text` as the note on unit `id`, written on its code as it is
    /// now. Whether the unit is there.
    fn write_note(&mut self, id: &str, text: &str) -> bool {
        let Some(u) = self.review.unit_mut(id) else {
            return false;
        };
        u.dive.note_on = (!text.is_empty()).then(|| u.fingerprint.clone());
        u.dive.note = text.to_string();
        true
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
    pub(crate) fn edit_note(&mut self, id: String, back: Option<usize>) {
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
    /// every note; `finish.open`, `v1.notes_to_agent` and `v1.notes_post`
    /// open the `:w` sheet; `view.back`, `view.close` or `tool.notes` closes.
    fn notes_key(&mut self, sel: usize, k: Key) -> Vec<Effect> {
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
        let Some(a) = self.panel_action(k) else {
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
                return self.go_to_unit(&ids[sel]).unwrap_or_default();
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
                self.overlay = Overlay::Notes { sel };
                return self.yank_notes();
            }
            // The ways out of teer are the `:w` sheet's.
            Action::FinishOpen => return self.open_finish(),
            Action::HelpOpen => {
                self.open_help();
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

    /// Go to unit `id`, closing any overlay: in the unit view or the
    /// flagged list, it opens in the unit view; in the queue, the cursor
    /// goes to it. `None` when the queue doesn't list it.
    pub(crate) fn go_to_unit(&mut self, id: &str) -> Option<Vec<Effect>> {
        if self.screen != Screen::Queue && self.review.unit(id).is_some() {
            self.overlay = Overlay::None;
            return Some(self.open_unit(id.to_string()));
        }
        let pos = self.position_of(id)?;
        self.overlay = Overlay::None;
        Some(self.move_sel(pos))
    }

    /// Select the unit at `to`.
    pub fn move_sel(&mut self, to: usize) -> Vec<Effect> {
        self.sel = to.min(self.order.len().saturating_sub(1));
        self.ensure_prepared()
    }

    /// The queue's actions: moves, `a`/`f` (then on to the next pending
    /// unit), `A`/`F` on the file's pending units at once, `V`, `l` into
    /// the unit view on any unit, `zf`, and `q` to quit (the review is
    /// saved, cursor and all).
    pub(crate) fn queue_action(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
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
            Action::VisualStart => self.visual = self.selected_id(),
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
    /// through the commits, `view.open` into the units the selected one
    /// changed, `view.open` on one goes to it; `view.back` back out,
    /// `view.back` again, `view.close` or `tool.history` closes.
    fn commits_key(&mut self, mut sel: usize, mut unit: Option<usize>, k: Key) -> Vec<Effect> {
        let Some(a) = self.panel_action(k) else {
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
            (None, Action::ViewOpen) => {
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
                    return self.go_to_unit(&id).unwrap_or_default();
                }
            }
            (Some(_), Action::ViewBack) => unit = None,
            _ => {}
        }
        self.overlay = Overlay::Commits { sel, unit };
        Vec::new()
    }

    // ---- Ask --------------------------------------------------------------

    /// The questions about unit `id`, by index, oldest first: its chat.
    pub fn chat_of(&self, id: &str) -> Vec<usize> {
        let questions = self.review.questions.iter().enumerate();
        questions
            .filter(|(_, q)| q.unit.as_deref() == Some(id))
            .map(|(n, _)| n)
            .collect()
    }

    /// The links of the unit view's conversation in the order they are
    /// drawn, with the question each belongs to: per answer, the related
    /// units that are in the list, then the code it shows.
    pub fn ask_links(&self) -> Vec<(usize, AskLink)> {
        let mut out = Vec::new();
        for n in self.chat_of(&self.unit.id) {
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

    /// Note `text` as a question about `unit`, not asked yet; its index.
    /// `None` when it is blank.
    pub(crate) fn add_question(&mut self, text: &str, unit: Option<String>) -> Option<usize> {
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
    pub(crate) fn ask_question(&mut self, index: usize) -> Vec<Effect> {
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

    /// Follow link `i` of [`App::ask_links`]: a unit opens in the unit
    /// view, code in the editor.
    pub fn follow_ask_link(&mut self, i: usize) -> Vec<Effect> {
        let Some((_, link)) = self.ask_links().into_iter().nth(i) else {
            return Vec::new();
        };
        match link {
            AskLink::Code { file, line } => vec![Effect::Edit { file, line }],
            AskLink::Unit(id) => self.go_to_unit(&id).unwrap_or_default(),
        }
    }

    /// Scroll the ask panel's conversation `by` lines; at the end it
    /// follows new lines.
    pub(crate) fn chat_scroll(&mut self, by: isize) -> Vec<Effect> {
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
    pub(crate) fn jump_pending(&mut self, forward: bool, times: usize) -> Vec<Effect> {
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
    pub(crate) fn jump_file(&mut self, forward: bool, times: usize) -> Vec<Effect> {
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

    /// `A`, `F` in the queue: decide the pending units of the file under the
    /// cursor at once, as one undo step, and move on to the next pending
    /// unit. Decided units are left alone. Listed by level, the file is its
    /// units in the cursor's box.
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
        effects
    }

    /// Apply `d` to the selected unit and move on to the next pending one,
    /// as one undo step.
    pub fn decide(&mut self, d: Decision) -> Vec<Effect> {
        self.decide_n(d, None)
    }

    /// Apply `d` to the selected unit and, with a count, the next
    /// `count - 1` pending ones, as one undo step; then move on to the next
    /// pending unit (see [`App::advance`]). A flag is only a mark: `i`
    /// writes a note.
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
        let n = self.decide_ids(&ids, d);
        if count.is_some() {
            self.status = decided_status(d, n);
        }
        let mut effects = vec![Effect::Save];
        effects.extend(self.advance(self.sel));
        effects
    }

    /// `a` or `f` in visual mode: decide every unit of the selection as one
    /// undo step, leave visual mode and move on to the next pending unit
    /// after it.
    /// `.` repeats it on as many units.
    fn decide_visual(&mut self, a: Action) -> Vec<Effect> {
        let rows = self.visual_rows();
        self.visual = None;
        let Some(rows) = rows else {
            return Vec::new();
        };
        let hi = *rows.end();
        let ids: Vec<String> = rows
            .map(|p| self.review.units[self.order[p]].id.clone())
            .collect();
        let d = if a == Action::DecideFlag {
            Decision::Investigate
        } else {
            Decision::Accepted
        };
        self.last_decision = Some((a, Some(ids.len() as u32)));
        let n = self.decide_ids(&ids, d);
        self.status = decided_status(d, n);
        // A unit that was accepted for you may have moved: go by its id.
        let hi = ids.last().and_then(|id| self.position_of(id)).unwrap_or(hi);
        self.sel = hi;
        let mut effects = vec![Effect::Save];
        effects.extend(self.advance(hi));
        effects
    }

    /// The rows of the visual selection: from where the unit it started on
    /// is listed now to the cursor. `None` outside visual mode.
    pub fn visual_rows(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let anchor = self.position_of(self.visual.as_deref()?)?;
        let sel = self.sel.min(self.order.len().checked_sub(1)?);
        Some(anchor.min(sel)..=anchor.max(sel))
    }

    /// Where unit `id` is listed, if it is.
    pub(crate) fn position_of(&self, id: &str) -> Option<usize> {
        self.order
            .iter()
            .position(|&i| self.review.units[i].id == id)
    }

    /// Set each of `ids` to `d` and record that as one undo step. Accepting
    /// a unit whose note was written on code that changed since puts the
    /// note away (Phase 7). How many changed.
    pub(crate) fn decide_ids(&mut self, ids: &[String], d: Decision) -> usize {
        let each: Vec<(String, Decision)> = ids.iter().map(|id| (id.clone(), d)).collect();
        self.decide_each(&each, d == Decision::Accepted)
    }

    /// Set each unit to its decision as one undo step, and list the units
    /// again (who accepted them; see [`settle`]). With `archive`, a unit
    /// this accepts puts away its note when the code changed since it was
    /// written ([`Unit::archive_note`]); the step holds that, so `u` brings
    /// the note back. How many changed.
    fn decide_each(&mut self, each: &[(String, Decision)], archive: bool) -> usize {
        let mut changes = Vec::new();
        let mut notes = Vec::new();
        for (id, d) in each {
            if let Some(u) = self.review.unit_mut(id) {
                let before = std::mem::replace(&mut u.decision, *d);
                let accepts = *d == Decision::Accepted && before != Decision::Accepted;
                if archive && accepts && u.note_is_stale() {
                    let note = Note {
                        text: u.dive.note.clone(),
                        on: u.dive.note_on.clone(),
                    };
                    u.archive_note();
                    notes.push(NoteChange {
                        id: id.clone(),
                        before: note,
                        after: Note::default(),
                    });
                }
                changes.push(Change {
                    id: id.clone(),
                    before,
                    after: *d,
                });
            }
        }
        let n = changes.iter().filter(|c| c.before != c.after).count();
        self.review.history.record(Step { changes, notes });
        self.reorder();
        n
    }

    /// `u`: put back what the last step changed (the decisions and the
    /// notes its note line wrote), and go to its first unit. On a unit
    /// accepted for you (listed with `za`), take that back instead: it is
    /// pending again, as a step of its own.
    pub fn undo(&mut self) -> Vec<Effect> {
        if self.screen == Screen::Queue
            && let Some(id) = self
                .selected()
                .filter(|u| u.auto_accepted())
                .map(|u| u.id.clone())
        {
            self.decide_ids(std::slice::from_ref(&id), Decision::Pending);
            let name = self
                .review
                .unit(&id)
                .map(super::names::row_name)
                .unwrap_or_default();
            self.status = format!("{name} is pending again, no longer accepted for you.");
            return vec![Effect::Save];
        }
        let Some(step) = self.review.history.undo() else {
            self.status = "Nothing to undo.".into();
            return Vec::new();
        };
        for n in step.notes.iter().rev() {
            if archives(&step, n) {
                if let Some(u) = self.review.unit_mut(&n.id) {
                    u.unarchive_note(&n.before.text, n.before.on.as_deref());
                }
            } else {
                self.put_note(&n.id, &n.before);
            }
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
            if archives(&step, n) {
                if let Some(u) = self.review.unit_mut(&n.id) {
                    u.archive_note();
                }
            } else {
                self.put_note(&n.id, &n.after);
            }
        }
        self.reorder();
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
            Screen::Queue => match self.position_of(&first) {
                Some(pos) => self.move_sel(pos),
                None => Vec::new(),
            },
            Screen::Unit => self.unit_show_step(&first),
            Screen::Flagged => self.flagged_show_step(&first),
        }
    }

    // ---- Editor and reset ------------------------------------------------

    /// The unit the current screen is about.
    pub(crate) fn current_unit(&self) -> Option<&Unit> {
        match self.screen {
            Screen::Queue => self.selected(),
            Screen::Unit => self.review.unit(&self.unit.id),
            Screen::Flagged => {
                let ids = self.flagged_ids();
                ids.get(self.flagged_index(&ids))
                    .and_then(|id| self.review.unit(id))
            }
        }
    }

    /// Open the current unit's file in the editor, at the unit's first line.
    fn edit(&mut self) -> Vec<Effect> {
        match self.current_unit() {
            Some(u) => {
                // A whole file opens at its first change, not at its top;
                // in the unit view, every unit does.
                let first = (u.kind == UnitKind::File || self.screen == Screen::Unit)
                    .then(|| crate::review::diff::first_change(&self.review, u))
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
        self.decide_each(&each, false);
        self.status = "Reset: nothing is marked as looked at.".into();
        vec![Effect::Save]
    }

    /// How many decisions `:e!` resets: one per unit, pending ones and
    /// those accepted for you too.
    pub fn reset_count(&self) -> usize {
        self.review.units.len()
    }

    /// `:e!`, confirmed: forget every decision ([`App::reset`], one undo
    /// step), then re-run the review now; the units accepted for you are
    /// weighed again on the new code.
    fn reset_and_rerun(&mut self) -> Vec<Effect> {
        let mut effects = self.reset();
        effects.extend(self.rerun_now());
        self.status = "Reset every decision. Re-running the review…".into();
        effects
    }

    // ---- Fix loop (Phase 7) ----------------------------------------------

    /// `:e`: re-run the review now, decisions kept; on a pull request, on
    /// the head it has now, fetched again. Waiting for copied notes ends.
    fn rerun_now(&mut self) -> Vec<Effect> {
        self.waiting = None;
        self.status = match &self.review.pr {
            Some(pr) => format!("Fetching PR #{} and re-running the review…", pr.number),
            None => "Re-running the review…".into(),
        };
        vec![Effect::Rerun]
    }

    /// The notes went to the agent CLI `cli` (see [`Effect::OpenAgent`])
    /// and it exited: that is the signal its changes are in, so the review
    /// re-runs now, without a key.
    pub fn agent_returned(&mut self, cli: Provider) -> Vec<Effect> {
        self.fix_by = Some(ChangedBy::Agent(cli));
        let effects = self.rerun_now();
        self.status = format!("Back from {}. Re-running the review…", cli.title());
        effects
    }

    /// `notes` notes were copied for an agent teer doesn't run: wait for
    /// their changes on the queue, read-only, until the files have changed
    /// and settled (see [`App::watch_due`]), or `q`.
    pub fn start_waiting(&mut self, notes: usize) {
        self.overlay = Overlay::None;
        self.visual = None;
        self.screen = Screen::Queue;
        self.fix_by = Some(ChangedBy::Copied);
        self.waiting = Some(Waiting {
            notes,
            quiet: crate::watch::Quiet::new(self.cfg.fix_loop.quiet()),
            next: Some(Instant::now()),
        });
        self.status = self.waiting_line();
    }

    /// `q` while waiting: the queue as before; `:e` re-runs later.
    fn stop_waiting(&mut self) {
        self.waiting = None;
        self.status = format!(
            "Stopped waiting. {} re-runs the review once the changes are in.",
            self.keys.label(Action::ReviewRerun)
        );
    }

    /// What the status line says while waiting.
    pub fn waiting_line(&self) -> String {
        let Some(w) = &self.waiting else {
            return String::new();
        };
        let q = self.keys.label(Action::ViewClose);
        if w.quiet.changing() {
            return format!(
                "Files are changing… the review re-runs once they're quiet for {} ({q} to stop waiting)",
                seconds(w.quiet.period())
            );
        }
        let notes = if w.notes == 1 {
            "the note".to_string()
        } else {
            format!("the {} notes", w.notes)
        };
        format!("Waiting for the changes to {notes}… ({q} to stop waiting)")
    }

    /// While waiting, whether to look at the working tree at `now`: the
    /// repository to look at. The look is then on its way, and what it saw
    /// comes back as [`AppEvent::Tree`]. The runtime asks on every tick.
    pub fn watch_due(&mut self, now: Instant) -> Option<PathBuf> {
        let w = self.waiting.as_mut()?;
        if w.next.is_none_or(|t| now < t) {
            return None;
        }
        w.next = None;
        Some(self.review.repo_root.clone())
    }

    /// The tree as a look saw it at `at`: once it changed and then stayed
    /// the same for `fix_loop.quiet_seconds`, waiting ends and the review
    /// re-runs. Else the next look is due a little later.
    fn tree_seen(
        &mut self,
        seen: Result<crate::watch::Snapshot, String>,
        at: Instant,
    ) -> Vec<Effect> {
        let Some(w) = self.waiting.as_mut() else {
            return Vec::new();
        };
        w.next = Some(at + crate::watch::every(w.quiet.period()));
        match seen {
            Ok(s) => {
                let was = w.quiet.changing();
                if w.quiet.see(s, at) {
                    self.waiting = None;
                    self.status = "The files are quiet. Re-running the review…".into();
                    return vec![Effect::Rerun];
                }
                if !was && w.quiet.changing() {
                    self.status = self.waiting_line();
                }
            }
            Err(e) => self.status = format!("Couldn't look at the files: {e}"),
        }
        Vec::new()
    }

    /// A re-run's review is in. Decisions carry over by unit and code
    /// (`store::merge_previous`); the units it changed or added lead their
    /// level, the cursor goes to the first of them, and the status says how
    /// many changed and by whom.
    fn reran(&mut self, mut new: Review) -> Vec<Effect> {
        crate::review::store::merge_previous(&mut new, &self.review);
        new.agent_session = self.review.agent_session.clone();
        let changed = crate::review::store::changed_units(&new, &self.review);
        // Positions in the old order mean nothing in the new review:
        // remember ids, drop the positions, restore by id.
        let keep = self.selected().map(|u| u.id.clone());
        self.review = new;
        self.committed = None;
        self.finish = Default::default();
        self.summary_asked = false;
        self.order.clear();
        self.sel = 0;
        self.visual = None;
        self.waiting = None;
        self.changed = changed.into_iter().collect();
        self.rebuild_order();
        // Units accepted for you are no news.
        self.changed
            .retain(|id| self.review.unit(id).is_some_and(|u| !u.auto_accepted()));
        let first_changed = (0..self.order.len())
            .find(|&p| self.changed.contains(&self.review.units[self.order[p]].id));
        if let Some(pos) = first_changed.or_else(|| keep.and_then(|id| self.position_of(&id))) {
            self.sel = pos;
        }
        self.screen = Screen::Queue;
        let by = self
            .fix_by
            .take()
            .map_or("since the last run".to_string(), ChangedBy::words);
        self.status = match self.changed.len() {
            0 => format!("Nothing changed {by}."),
            n => format!("{} changed {by}.", units(n)),
        };
        let mut effects = vec![Effect::Save];
        effects.extend(self.prepare_start());
        effects
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
            AppEvent::PrReviewed { verdict, result } => return self.posted(verdict, result),
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
            AppEvent::Rerun(Ok(new)) => return self.reran(*new),
            AppEvent::Rerun(Err(e)) => self.status = format!("Re-run failed: {e}"),
            AppEvent::Tree { seen, at } => return self.tree_seen(seen, at),
            AppEvent::Dive(ev) => {
                // About code a re-run changed since it was asked: dropped
                // without a word (see `Review::apply_dive`).
                let current = self.review.is_current(&ev);
                match &ev {
                    DiveEvent::Progress { id, kind, text } => {
                        self.progress.insert((id.clone(), *kind), text.clone());
                    }
                    DiveEvent::Ready { id, kind, .. } => {
                        self.progress.remove(&(id.clone(), *kind));
                    }
                    DiveEvent::Failed {
                        id, kind, error, ..
                    } => {
                        self.progress.remove(&(id.clone(), *kind));
                        if current {
                            self.status = format!("Question ({kind:?}) failed: {error}");
                        }
                    }
                }
                if !current {
                    return Vec::new();
                }
                if self.review.apply_dive(&ev) {
                    return vec![Effect::Save];
                }
                return Vec::new();
            }
        }
        Vec::new()
    }
}

/// Why there is no agent when the user asked for none; the review list
/// doesn't repeat it as a banner.
pub const NO_AGENT: &str = "running with --no-agent";

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::review::model::{UnitKind, UnitStatus};

    pub(crate) fn app(n: usize) -> App {
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

    /// Type `keys` (Neovim notation).
    fn press(a: &mut App, keys: &str) -> Vec<Effect> {
        crate::keymap::parse_keys(keys)
            .unwrap()
            .into_iter()
            .flat_map(|k| a.on_key(k))
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
    fn the_queue_opens_by_level_and_l_opens_any_unit() {
        let mut a = app(3);
        assert_eq!((a.screen, a.grouping), (Screen::Queue, Grouping::Unit));
        // `s` was the v1 list's way into swipe.
        press(&mut a, "s");
        assert_eq!(a.screen, Screen::Queue);
        press(&mut a, "j<cr>");
        assert_eq!(a.screen, Screen::Unit, "a pending unit opens too");
        assert_eq!(a.sel, 1);
        let mut a = app(3);
        a.start(false, None, Vec::new(), None);
        assert_eq!(a.screen, Screen::Queue);
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
    fn undo_selects_the_undone_unit() {
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
    fn f_flags_and_moves_on_asking_for_no_note() {
        let mut a = app(3);
        let effects = press(&mut a, "f");
        assert!(effects.contains(&Effect::Save));
        assert_eq!(dec(&a, 0), Flag);
        assert_eq!(a.sel, 1);
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.review.history.undo.len(), 1);
    }

    #[test]
    fn visual_mode_decides_the_selection_as_one_step() {
        let mut a = app(6);
        press(&mut a, "Vjj");
        assert_eq!(a.visual.as_deref(), Some("u0"));
        assert_eq!(a.visual_rows(), Some(0..=2));
        assert_eq!(a.sel, 2);
        assert_eq!(decisions(&a), vec![Pend; 6], "moving only selects");
        press(&mut a, "a");
        assert_eq!(a.visual, None, "deciding leaves visual mode");
        assert_eq!(decisions(&a), vec![Acc, Acc, Acc, Pend, Pend, Pend]);
        assert_eq!(a.sel, 3);
        assert_eq!(a.review.history.undo.len(), 1);
        // Upwards works too.
        press(&mut a, "jjVkf");
        assert_eq!(decisions(&a), vec![Acc, Acc, Acc, Pend, Flag, Flag]);
        assert_eq!(a.overlay, Overlay::None);
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
    fn upper_f_flags_the_files_pending_units() {
        let mut a = app(6);
        a.review.units[1].decision = Acc;
        let effects = press(&mut a, "F");
        assert!(effects.contains(&Effect::Save));
        assert_eq!(decisions(&a), vec![Flag, Acc, Flag, Pend, Pend, Pend]);
        assert_eq!(a.status, "Flagged 2 units.");
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.sel, 3, "on to the next pending unit");
        assert_eq!(a.review.history.undo.len(), 1);
        press(&mut a, "u");
        assert_eq!(decisions(&a), vec![Pend, Acc, Pend, Pend, Pend, Pend]);
        // Nothing pending: nothing to flag.
        a.review.units[0].decision = Acc;
        a.review.units[2].decision = Acc;
        press(&mut a, "ggF");
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.status, "Nothing pending in this file.");
    }

    #[test]
    fn w_opens_the_finish_sheet_whatever_is_left() {
        // Something left: the sheet, on keeping on reviewing.
        let mut a = app(3);
        a.review.units[0].decision = Acc;
        press(&mut a, ":w<cr>");
        assert_eq!(a.overlay, Overlay::Finish { sel: 0 });
        assert_eq!(
            a.finish_step(),
            Some(crate::session::finish::Step::KeepReviewing)
        );
        // From the notes view too.
        press(&mut a, "<esc>");
        a.review.units[1].dive.note = "why?".into();
        press(&mut a, "<space>n");
        assert!(matches!(a.overlay, Overlay::Notes { .. }));
        press(&mut a, ":w<cr>");
        assert!(matches!(a.overlay, Overlay::Finish { .. }));
        // Everything accepted: the same sheet; a branch is done.
        let mut a = app(2);
        a.review.units[0].decision = Acc;
        a.review.units[1].decision = Acc;
        press(&mut a, ":w<cr>");
        assert_eq!(a.finish_step(), Some(crate::session::finish::Step::Done));
    }

    #[test]
    fn q_quits_the_queue_but_leaves_the_unit_view() {
        let mut a = app(2);
        assert_eq!(press(&mut a, "q"), vec![Effect::Save, Effect::Quit]);
        press(&mut a, "l");
        assert_eq!(a.screen, Screen::Unit);
        let effects = press(&mut a, "q");
        assert!(!effects.contains(&Effect::Quit));
        assert_eq!(a.screen, Screen::Queue);
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
    fn a_tool_key_opens_the_unit_view_with_its_panel_on_any_unit() {
        let mut a = app(2);
        let effects = press(&mut a, "<space>t");
        assert_eq!(a.screen, Screen::Unit);
        assert_eq!(
            a.unit.panel,
            Some(super::super::unit::Panel::Tool(DiveKind::Tests))
        );
        assert!(effects.contains(&Effect::Dive {
            unit_id: "u0".into(),
            kind: DiveKind::Tests,
            web: false,
        }));
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

    // ---- Fix loop (Phase 7) ------------------------------------------------

    /// What a re-run builds after the code of units `ids` changed: their
    /// fingerprints differ, and they come undecided, without a note.
    fn rebuilt(a: &App, ids: &[usize]) -> Box<Review> {
        let mut r = a.review.clone();
        r.history = Default::default();
        for u in &mut r.units {
            u.decision = Pend;
            u.accepted_by = None;
            u.dive = Default::default();
        }
        for &i in ids {
            r.units[i].fingerprint = "new".into();
        }
        Box::new(r)
    }

    fn listed_ids(a: &App) -> Vec<String> {
        a.order
            .iter()
            .map(|&i| a.review.units[i].id.clone())
            .collect()
    }

    #[test]
    fn back_from_the_agent_the_review_reruns_and_lands_on_what_changed() {
        let mut a = app(6);
        press(&mut a, "aaaa");
        a.review.units[1].dive.note = "rename it".into();
        a.review.units[1].dive.note_on = Some(String::new());
        assert_eq!(a.agent_returned(Provider::Claude), vec![Effect::Rerun]);
        assert_eq!(a.status, "Back from Claude Code. Re-running the review…");
        let new = rebuilt(&a, &[1, 4]);
        let effects = a.on_app_event(AppEvent::Rerun(Ok(new)));
        assert!(effects.contains(&Effect::Save), "{effects:?}");
        assert_eq!(a.status, "2 units changed by Claude Code.");
        // The changed units lead their level, the cursor on the first.
        assert_eq!(&listed_ids(&a)[..2], ["u1", "u4"]);
        assert_eq!(a.selected_id().as_deref(), Some("u1"));
        // Changed: pending, the note kept and marked. The rest keep theirs.
        assert_eq!(decisions(&a), vec![Acc, Pend, Acc, Acc, Pend, Pend]);
        assert_eq!(a.review.units[1].dive.note, "rename it");
        assert!(a.review.units[1].note_is_stale());
        // The next re-run, with no notes out, changes nothing.
        let same = Box::new(a.review.clone());
        a.on_app_event(AppEvent::Rerun(Ok(same)));
        assert_eq!(a.status, "Nothing changed since the last run.");
        assert!(a.changed.is_empty());
    }

    #[test]
    fn copied_notes_wait_read_only_until_the_files_settle() {
        use crate::watch::Snapshot;
        use std::time::Duration;
        let mut a = app(4);
        a.cfg.fix_loop.quiet_seconds = 2.0;
        press(&mut a, "fx<cr>");
        a.start_waiting(1);
        assert_eq!(
            a.status,
            "Waiting for the changes to the note… (q to stop waiting)"
        );
        // Read-only: no decision, no note line; moves still move.
        let before = decisions(&a);
        press(&mut a, "afiAV");
        assert_eq!(decisions(&a), before);
        assert_eq!((a.overlay.clone(), a.visual.clone()), (Overlay::None, None));
        let sel = a.sel;
        press(&mut a, "j");
        assert_eq!(a.sel, sel + 1);
        assert!(
            a.status.starts_with("Waiting for the changes"),
            "{}",
            a.status
        );

        // The first look is the baseline; one look at a time.
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let seen = |snap: &str, at| AppEvent::Tree {
            seen: Ok(Snapshot(snap.into())),
            at,
        };
        assert_eq!(a.watch_due(t0), Some(a.review.repo_root.clone()));
        assert_eq!(a.watch_due(t0), None);
        assert!(a.on_app_event(seen("a", t0)).is_empty());
        assert_eq!(a.watch_due(t0), None, "the next look comes later");
        // A change, then two seconds the same.
        assert!(a.watch_due(t0 + s(1)).is_some());
        assert!(a.on_app_event(seen("b", t0 + s(1))).is_empty());
        assert!(a.status.starts_with("Files are changing…"), "{}", a.status);
        assert!(a.status.contains("quiet for 2 s"), "{}", a.status);
        assert!(a.watch_due(t0 + s(2)).is_some());
        assert!(a.on_app_event(seen("b", t0 + s(2))).is_empty());
        assert!(a.watch_due(t0 + s(3)).is_some());
        assert_eq!(a.on_app_event(seen("b", t0 + s(3))), vec![Effect::Rerun]);
        assert!(a.waiting.is_none());
        assert_eq!(a.watch_due(t0 + s(4)), None);
        let new = rebuilt(&a, &[0]);
        a.on_app_event(AppEvent::Rerun(Ok(new)));
        assert_eq!(a.status, "1 unit changed since the notes were copied.");
    }

    #[test]
    fn q_stops_waiting_and_colon_e_reruns_instead() {
        let mut a = app(3);
        a.start_waiting(2);
        assert!(a.status.contains("the 2 notes"), "{}", a.status);
        let e = press(&mut a, "q");
        assert!(!e.contains(&Effect::Quit), "{e:?}");
        assert!(a.waiting.is_none());
        assert_eq!(
            a.status,
            "Stopped waiting. :e re-runs the review once the changes are in."
        );
        // A look that was on its way changes nothing now.
        let late = AppEvent::Tree {
            seen: Ok(crate::watch::Snapshot("x".into())),
            at: Instant::now(),
        };
        assert!(a.on_app_event(late).is_empty());
        // `q` again quits, as in the queue.
        assert!(press(&mut a, "q").contains(&Effect::Quit));

        let mut b = app(3);
        b.start_waiting(1);
        assert_eq!(press(&mut b, ":e<cr>"), vec![Effect::Rerun]);
        assert!(b.waiting.is_none());
        // Still the notes' changes.
        b.on_app_event(AppEvent::Rerun(Ok(rebuilt(&b, &[2]))));
        assert_eq!(b.status, "1 unit changed since the notes were copied.");
    }

    #[test]
    fn a_puts_away_a_note_on_changed_code_and_u_brings_it_back() {
        use crate::review::model::ArchivedNote;
        let mut a = app(4);
        let stale = |a: &mut App, i: usize, note: &str| {
            a.review.units[i].dive.note = note.into();
            a.review.units[i].dive.note_on = Some("old".into());
        };
        stale(&mut a, 0, "check the limit");
        a.sel = 0;
        press(&mut a, "a");
        let archived = vec![ArchivedNote {
            text: "check the limit".into(),
            on: Some("old".into()),
        }];
        let u = &a.review.units[0];
        assert_eq!((u.decision, u.dive.note.as_str()), (Acc, ""));
        assert_eq!(u.dive.archived, archived);
        assert!(crate::notes::listed(&a.review).is_empty());
        press(&mut a, "u");
        let u = &a.review.units[0];
        assert_eq!(
            (u.decision, u.dive.note.as_str()),
            (Pend, "check the limit")
        );
        assert!(u.dive.archived.is_empty() && u.note_is_stale());
        press(&mut a, "<c-r>");
        assert_eq!(a.review.units[0].dive.archived, archived);

        // A note on code that didn't change stays when its unit is accepted.
        a.review.units[1].dive.note = "fine".into();
        a.review.units[1].dive.note_on = Some(String::new());
        a.sel = a.position_of("u1").unwrap();
        press(&mut a, "a");
        assert_eq!(a.review.units[1].dive.note, "fine");

        // `i` edits it without deciding; the edit is on the code as it is.
        stale(&mut a, 2, "old note");
        a.sel = a.position_of("u2").unwrap();
        press(&mut a, "i<c-u>new note<cr>");
        let u = &a.review.units[2];
        assert_eq!((u.decision, u.dive.note.as_str()), (Pend, "new note"));
        assert!(!u.note_is_stale());

        // `f` flags it and leaves the note as it is, stale.
        stale(&mut a, 3, "still wrong");
        a.sel = a.position_of("u3").unwrap();
        press(&mut a, "f");
        assert_eq!(a.overlay, Overlay::None);
        let u = &a.review.units[3];
        assert_eq!((u.decision, u.dive.note.as_str()), (Flag, "still wrong"));
        assert!(u.note_is_stale());
    }

    #[test]
    fn colon_e_bang_resets_every_decision_then_reruns() {
        let mut a = app(4);
        press(&mut a, "aaf<esc>");
        press(&mut a, ":e!<cr>");
        assert_eq!(a.overlay, Overlay::ConfirmReset);
        assert_eq!(a.reset_count(), 4);
        assert_eq!(press(&mut a, "y"), vec![Effect::Save, Effect::Rerun]);
        assert_eq!(decisions(&a), vec![Pend; 4]);
        // No is no: nothing reset, nothing re-run.
        a.sel = 0;
        press(&mut a, "a");
        press(&mut a, ":e!<cr>");
        assert!(press(&mut a, "n").is_empty());
        assert_eq!(dec(&a, 0), Acc);
    }
}
