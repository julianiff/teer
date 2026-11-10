//! The review as a front end draws it: [`snapshot`] turns an [`App`] into
//! a [`View`], plain data any front end can render, in Rust or as JSON
//! (serde). It says what is on show and what it means: the queue's level
//! boxes and rows, the unit under the cursor, the unit view's text and
//! tiles, the flagged list, the overlay, the bar. It never says how it
//! looks: no widths, no colours, no wrapping. Text the agent wrote stays
//! plain text; the front end must not take it for markup.
//!
//! Code comes as diff rows cut into pieces by syntax class
//! ([`crate::syntax::Class`]); the front end's theme colours them.

use std::collections::BTreeMap;

use serde::Serialize;

use super::app::{App, DraftState, Grouping, JumpKind, NOTE_MAX, Overlay, Screen};
use super::finish::{Reviewing, Step, head_check};
use super::unit::Panel;
use super::{help, names, tools};
use crate::approve;
use crate::commit::{self, Branch};
use crate::config::Role;
use crate::keymap::{Action, Trigger};
use crate::review::attention::Level;
use crate::review::diff::{DIFF_LEVEL, Kind, Row, anchor, expand_tabs, rows_around};
use crate::review::model::{
    Commit, Decision, DiveKind, DiveResult, LineRef, Marker, PrMeta, Prep, Risk, Shown, Side, Unit,
    UnitKind, UnitStatus,
};
use crate::security::audit;
use crate::syntax::{Cache, Class};
use crate::util::sanitize;

/// Everything on show.
#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub screen: Screen,
    pub header: Header,
    pub bar: Bar,
    pub queue: Queue,
    /// The unit under the queue's cursor, as its preview shows it.
    pub preview: Option<Card>,
    /// The unit view, while it is on show.
    pub unit: Option<UnitScreen>,
    /// The flagged list, while it is on show.
    pub flagged: Option<FlaggedScreen>,
    pub overlay: OverlayView,
    /// What a front end puts up as buttons where they act (the unit's
    /// decisions on its card, the tiles beside it, the grouping over the
    /// list), by action id: each one's key, what it does, whether it can.
    /// Every key is in the help.
    pub actions: BTreeMap<&'static str, ActionView>,
    /// What the open panel's keys do (the notes, a jump list, the
    /// commits, the sheet), for buttons on it; empty without one.
    pub panel: Vec<Hint>,
}

/// The top line: what is reviewed and what teer is doing.
#[derive(Debug, Clone, Serialize)]
pub struct Header {
    /// The repository reviewed (a pull request's checkout for one).
    pub repo: String,
    /// The repository the review was started in: for a pull request, the
    /// clone it was fetched into. Its pull requests and changes open from here.
    pub home: String,
    /// The pull request reviewed, by number.
    pub pr: Option<u32>,
    /// `your changes`, `feature since main, 6 commits`, `PR #482 by ana`.
    pub subject: String,
    /// What the change is called: the PR's title, the only commit's subject.
    pub title: Option<String>,
    /// The agent's summary of the whole change, on the queue.
    pub summary: Option<String>,
    /// Running with `--no-agent`.
    pub no_agent: bool,
    /// Why there is no agent, when it isn't `--no-agent`.
    pub agent_note: Option<String>,
    /// Units described so far, while prepare runs.
    pub preparing: Option<Progress>,
    /// The commits reviewed: with a pull request or one of them, the
    /// commits view has something to show.
    pub commits: usize,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

/// The bottom bar: the counts, what the keys wait for, the status.
#[derive(Debug, Clone, Serialize)]
pub struct Bar {
    pub pending: usize,
    pub flagged: usize,
    /// Accepted, by the reviewer or for them.
    pub accepted: usize,
    /// Of those, accepted for the reviewer (no behavior change).
    pub auto_accepted: usize,
    /// Units with a note.
    pub notes: usize,
    /// The key that opens the `:w` sheet, said once nothing is pending.
    pub finish_key: String,
    /// The key that lists the keys.
    pub help_key: String,
    /// What the last action said; empty when nothing.
    pub status: String,
    /// A sequence or count that waits for its next key (`g`, `3`), and
    /// each key that can follow.
    pub typed: Option<String>,
    pub follow: Vec<Hint>,
    /// The command being typed after `:`, and the commands it can become.
    pub command: Option<String>,
    pub commands: Vec<Hint>,
    /// Visual mode: how many units the selection holds, and what applies.
    pub visual: Option<usize>,
    pub visual_actions: Vec<Hint>,
    /// The note being typed on the bar (not from the notes view).
    pub note: Option<NoteEditor>,
    /// Waiting for the changes to notes copied out of teer.
    pub waiting: Option<String>,
}

pub use super::help::Hint;

#[derive(Debug, Clone, Serialize)]
pub struct NoteEditor {
    pub id: String,
    /// The unit's name.
    pub name: String,
    pub text: String,
    pub max: usize,
}

/// The queue: units in level boxes, most attention first.
#[derive(Debug, Clone, Serialize)]
pub struct Queue {
    pub grouping: Grouping,
    pub boxes: Vec<LevelBox>,
    /// Changed files teer can't show, in a muted box nobody selects.
    pub skipped: Vec<Skipped>,
    /// How many units were accepted for you and the key that shows them.
    pub auto_line: Option<String>,
    /// The cursor, by position in the queue's order.
    pub sel: usize,
    /// The units accepted for you are listed.
    pub show_auto: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LevelBox {
    pub level: Level,
    /// `Worth a look · 3 files`.
    pub title: String,
    pub files: Vec<FileGroup>,
}

/// One file's units in a box. Listed by unit, the front end leaves the
/// file's own row out; each row names its file.
#[derive(Debug, Clone, Serialize)]
pub struct FileGroup {
    pub path: String,
    /// Units of it decided, and how many it has here.
    pub done: usize,
    pub total: usize,
    /// One of its units is being described.
    pub busy: bool,
    /// One of its units is flagged.
    pub flagged: bool,
    pub rows: Vec<QueueRow>,
}

/// A unit's row.
#[derive(Debug, Clone, Serialize)]
pub struct QueueRow {
    /// Its position in the queue's order: what the cursor moves over.
    pub pos: usize,
    pub id: String,
    pub level: Level,
    pub icon: &'static str,
    pub path: String,
    pub name: String,
    /// A whole file: named by its path already.
    pub whole_file: bool,
    /// How far it is read beneath the unit that calls it (0 at the top).
    pub depth: usize,
    pub decision: Decision,
    /// `pending`, `accepted`, `flagged`.
    pub state: &'static str,
    /// The start of a flagged unit's note.
    pub note: Option<String>,
    /// A second model disputes it (`⚠`).
    pub disputed: bool,
    /// It has a note its state doesn't show (`✎`).
    pub noted: bool,
    /// The unit was removed.
    pub removed: bool,
    pub preparing: bool,
    pub selected: bool,
    pub in_visual: bool,
    /// The last re-run found it changed or new.
    pub changed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub path: String,
    pub why: String,
}

/// One unit as the preview, the unit view and the flagged list show it.
#[derive(Debug, Clone, Serialize)]
pub struct Card {
    pub id: String,
    pub name: String,
    /// `path:lines`, or for a whole file its folder.
    pub place: String,
    pub file: String,
    /// Where `gf` opens it.
    pub line: u32,
    pub kind: UnitKind,
    pub status: UnitStatus,
    pub level: Level,
    pub icon: &'static str,
    pub level_title: &'static str,
    pub decision: Decision,
    pub accepted_for_you: bool,
    /// Where it stands, for its title: `pending`, `✓ accepted`, `⚑ flagged`.
    pub state: String,
    /// Why it has its level; empty when nothing says more than the level.
    pub why: Vec<String>,
    /// Sentences beyond the rules: expensive to undo, a second model
    /// disputes it, the commits don't mention it.
    pub reasons: Vec<String>,
    /// The commits that changed it, oldest first.
    pub commits: Vec<CommitRef>,
    /// Who wrote the summary and the statements: `agent (haiku)`.
    pub agent_label: Option<String>,
    pub summary: Option<String>,
    /// Why it has no behavior change, from the code.
    pub nbc_reason: Option<String>,
    pub prep: Prep,
    /// What stands in for the statements while there are none.
    pub prep_note: Option<String>,
    pub statements: Vec<Statement>,
    pub code: Option<Code>,
    /// The reviewer's note.
    pub note: Option<NoteCard>,
    /// Answers to questions asked about it, newest first.
    pub answers: Vec<AnswerCard>,
    pub disputed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitRef {
    pub short: String,
    pub subject: String,
}

/// A change statement the agent wrote.
#[derive(Debug, Clone, Serialize)]
pub struct Statement {
    pub marker: Marker,
    /// `+`, `~`, `−`.
    pub symbol: &'static str,
    pub text: String,
    pub risk: Risk,
    /// `L168-172, old L192`.
    pub cites: String,
    pub evidence: Vec<LineRef>,
}

/// The code change: diff rows, and where statements are written in.
#[derive(Debug, Clone, Serialize)]
pub struct Code {
    pub added: usize,
    pub removed: usize,
    pub rows: Vec<CodeRow>,
    /// Each statement written into the code: under row `after`, or after
    /// the last row when `None`; in the statements' order.
    pub notes: Vec<CodeNote>,
    /// The rows each hunk (a run of added and removed rows) starts at.
    pub hunks: Vec<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeRow {
    pub kind: Kind,
    /// Line in the new file; removed lines have none.
    pub line: Option<u32>,
    /// Line in the old file; added lines have none.
    pub old: Option<u32>,
    /// The text, tabs expanded, in pieces of one syntax class each.
    pub pieces: Vec<Piece>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Piece {
    pub text: String,
    pub class: Option<Class>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeNote {
    pub after: Option<usize>,
    /// Into [`Card::statements`].
    pub statement: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteCard {
    pub text: String,
    /// The code changed since the note was written.
    pub stale: bool,
}

/// An answered question, as a card on its unit.
#[derive(Debug, Clone, Serialize)]
pub struct AnswerCard {
    pub question: String,
    pub answer: String,
    /// Pieces of code the answer shows (the ask tile has them).
    pub shown: usize,
}

/// The unit view: the unit's text and the tiles beside it.
#[derive(Debug, Clone, Serialize)]
pub struct UnitScreen {
    pub card: Card,
    /// The second model, when it disputes the description.
    pub second: Option<SecondModel>,
    pub tiles: Vec<Tile>,
    /// The tile in focus; `None` is the unit's text.
    pub focus: Option<TileId>,
    pub zoom: bool,
    /// Keys go into the ask tile's question.
    pub typing: bool,
}

/// Which tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
pub enum TileId {
    Advice,
    Ask,
    Tests,
    Behavior,
    Visualize,
    Security,
    Outside,
}

impl TileId {
    pub fn of(p: Panel) -> TileId {
        match p {
            Panel::Recommend => TileId::Advice,
            Panel::Ask => TileId::Ask,
            Panel::Outside => TileId::Outside,
            Panel::Tool(DiveKind::Tests) => TileId::Tests,
            Panel::Tool(DiveKind::Behavior) => TileId::Behavior,
            Panel::Tool(DiveKind::Security) => TileId::Security,
            Panel::Tool(_) => TileId::Visualize,
        }
    }

    pub fn panel(self) -> Panel {
        match self {
            TileId::Advice => Panel::Recommend,
            TileId::Ask => Panel::Ask,
            TileId::Outside => Panel::Outside,
            TileId::Tests => Panel::Tool(DiveKind::Tests),
            TileId::Behavior => Panel::Tool(DiveKind::Behavior),
            TileId::Visualize => Panel::Tool(DiveKind::Visualize),
            TileId::Security => Panel::Tool(DiveKind::Security),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Tile {
    pub id: TileId,
    pub title: String,
    pub body: TileBody,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum TileBody {
    /// The advice, or a tool's answer: what came, what is on its way.
    Answer(Box<AnswerTile>),
    /// The conversation about the unit.
    Ask(AskTile),
    /// What the unit calls outside the review (`gd`), read-only.
    Outside { shown: Vec<Shown> },
}

#[derive(Debug, Clone, Serialize)]
pub struct AnswerTile {
    /// What was asked.
    pub question: DiveKind,
    /// Who wrote it: `agent (opus)`, `found by teer`.
    pub label: String,
    pub result: Option<DiveResult>,
    /// The diagram of a Visualize answer.
    pub svg: Option<Svg>,
    /// The answer is on its way, and what it is doing.
    pub waiting: Option<String>,
    /// The key that asks, and what it asks: before advice is asked, or
    /// for the agent to read the tests teer found.
    pub ask: Option<Hint>,
    /// Nothing came: how to ask again.
    pub none: Option<String>,
    /// Under the advice: the second model, when it disputes the description.
    pub second: Option<SecondModel>,
    /// In the Security tile: the facts teer found in the change itself,
    /// there before the agent answers (and without one).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<crate::security::Fact>,
}

/// A diagram: SVG whose colours are CSS variables (`var(--fg)`), and the
/// declarations that set them for light and for dark mode.
#[derive(Debug, Clone, Serialize)]
pub struct Svg {
    pub svg: String,
    pub light: String,
    pub dark: String,
}

/// What a second model said about the description.
#[derive(Debug, Clone, Serialize)]
pub struct SecondModel {
    pub label: String,
    /// It agrees, or on how many points it doesn't, and both risks.
    pub head: String,
    pub verdicts: Vec<VerdictView>,
    pub missed: Vec<MissedView>,
    /// Why there is no second opinion.
    pub failed: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerdictView {
    pub agree: bool,
    pub statement: String,
    pub why: String,
    pub cites: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MissedView {
    pub risk: Risk,
    pub statement: String,
    pub cites: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AskTile {
    /// Oldest first.
    pub exchanges: Vec<Exchange>,
    pub draft: String,
    pub max: usize,
    /// The link in focus, into the links numbered in the exchanges.
    pub link: Option<usize>,
    /// Asks once before a question may use the web.
    pub web_may_ask: bool,
    /// Who answers: `agent (opus)`.
    pub label: String,
    /// What else brings light into the unit, offered under an empty
    /// conversation: each tool's key, name and what it does.
    pub tools: Vec<Hint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Exchange {
    /// Into `review.questions`.
    pub index: usize,
    pub question: String,
    pub state: AskingState,
    /// The answer's text: as far as it has come while it is written.
    pub answer: Option<String>,
    pub related: Vec<RelatedView>,
    pub shown: Vec<ShownView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum AskingState {
    /// On its way: the agent's latest step and how many it took.
    Asking {
        step: Option<String>,
        steps: usize,
    },
    /// The answer is being written.
    Writing,
    Answered,
    Failed {
        error: String,
    },
    NotAnswered,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelatedView {
    pub id: String,
    pub why: String,
    pub name: String,
    pub place: String,
    pub level: Option<Level>,
    /// Its link, when it is in the list.
    pub link: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShownView {
    pub shown: Shown,
    pub link: usize,
}

/// An action a button presses: its key (empty when unbound) and what it
/// does, in the screen's words.
#[derive(Debug, Clone, Serialize)]
pub struct ActionView {
    pub keys: String,
    pub what: String,
    /// Needs an agent, and there is none.
    pub off: bool,
    /// A toggle's state (listed by file, zoomed); `None` for the rest.
    pub on: Option<bool>,
}

/// The flagged units one after another.
#[derive(Debug, Clone, Serialize)]
pub struct FlaggedScreen {
    pub entries: Vec<Card>,
    pub sel: usize,
}

/// What is open over the screen.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum OverlayView {
    None,
    Help(HelpView),
    ConfirmReset {
        decisions: usize,
    },
    /// The note editor opened from the notes view (`back`); one opened from
    /// a screen is on the bar.
    Note(NoteEditor),
    Notes {
        sel: usize,
        notes: Vec<NoteItem>,
    },
    ConfirmDeleteNote {
        id: String,
        name: String,
    },
    ConfirmWeb,
    Commits(CommitsView),
    Finish(FinishView),
    Jump(JumpView),
}

#[derive(Debug, Clone, Serialize)]
pub struct HelpView {
    /// What the keys are of: `queue`, `unit`, `finish`.
    pub of: &'static str,
    pub groups: Vec<HelpGroup>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HelpGroup {
    pub name: &'static str,
    pub rows: Vec<help::HelpRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteItem {
    pub id: String,
    pub name: String,
    pub place: String,
    pub level: Level,
    pub decision: Decision,
    pub text: String,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitsView {
    pub sel: usize,
    /// The selected one of the units the selected commit changed, while
    /// the cursor is in that list.
    pub unit: Option<usize>,
    pub pr: Option<PrMeta>,
    pub commits: Vec<Commit>,
    /// The units the selected commit changed, in list order.
    pub units: Vec<JumpItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JumpItem {
    pub id: String,
    pub name: String,
    pub place: String,
    pub level: Level,
    pub decision: Decision,
}

#[derive(Debug, Clone, Serialize)]
pub struct JumpView {
    /// What it lists.
    pub list: JumpKind,
    pub query: String,
    pub sel: usize,
    pub items: Vec<JumpItem>,
}

/// The `:w` sheet.
#[derive(Debug, Clone, Serialize)]
pub struct FinishView {
    pub reviewing: Reviewing,
    pub subject: String,
    pub sel: usize,
    pub steps: Vec<StepView>,
    /// Changed files teer couldn't show, said before anything is done.
    pub not_shown: Option<String>,
    /// A step waiting for a yes, since units are still pending.
    pub confirm: Option<String>,
    /// The one-line editor for the commit subject or the summary.
    pub editing: Option<String>,
    pub details: Details,
    pub notes: Vec<NoteItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepView {
    pub step: Step,
    pub label: String,
    /// The verb on the `⏎` hint.
    pub verb: &'static str,
}

/// What the selected step does.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum Details {
    None,
    Text {
        text: String,
    },
    Commit {
        /// `+`, `−`, `~` and the path.
        files: Vec<(char, String)>,
        branch: Option<String>,
        detached: bool,
        /// Changed paths outside the review, left out.
        others: usize,
        message: String,
        draft: Option<DraftState>,
    },
    Post {
        head_check: String,
        /// Approving with a comment waits for the comment.
        needs_comment: bool,
        body: String,
        inline: Vec<InlineView>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct InlineView {
    pub path: String,
    pub line: u32,
    pub old: bool,
    pub body: String,
}

/// What is on show in `app`; code coloured from `syntax` when given.
pub fn snapshot(app: &App, syntax: Option<&Cache>) -> View {
    View {
        screen: app.screen,
        header: header(app),
        bar: bar(app),
        queue: queue(app),
        preview: (app.screen == Screen::Queue)
            .then(|| app.selected().map(|u| card(app, u, Notes::Inline, syntax)))
            .flatten(),
        unit: (app.screen == Screen::Unit)
            .then(|| unit_screen(app, syntax))
            .flatten(),
        flagged: (app.screen == Screen::Flagged).then(|| flagged(app, syntax)),
        overlay: overlay(app),
        actions: actions(app),
        panel: panel_keys(app),
    }
}

fn header(app: &App) -> Header {
    let r = &app.review;
    let named = r.pr.is_some() || (r.head != crate::git::WORKTREE && r.intent.commits.len() == 1);
    let title = named
        .then(|| crate::review::intent::headline(r).0)
        .flatten()
        .map(|t| sanitize(&t));
    let summary = r
        .intent
        .summary
        .as_deref()
        .map(|s| sanitize(s).split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty());
    let no_agent = !app.has_agent && app.agent_note.as_deref() == Some(super::app::NO_AGENT);
    let agent_note = app
        .agent_note
        .clone()
        .filter(|n| !app.has_agent && n != super::app::NO_AGENT);
    let (done, total) = app.prep_progress();
    Header {
        repo: r.repo_root.display().to_string(),
        home: r
            .pr
            .as_ref()
            .and_then(|p| p.repo_dir.as_ref())
            .unwrap_or(&r.repo_root)
            .display()
            .to_string(),
        pr: r.pr.as_ref().map(|p| p.number),
        subject: r.subject(),
        title,
        summary,
        no_agent,
        agent_note,
        preparing: (done < total).then_some(Progress { done, total }),
        commits: r.intent.commits.len(),
    }
}

fn hint(app: &App, a: Action, what: impl Into<String>) -> Hint {
    Hint {
        keys: app.keys.label(a),
        action: a.id(),
        what: what.into(),
    }
}

fn bar(app: &App) -> Bar {
    let (accepted, flagged, pending) = app.decision_counts();
    let unit_tiles = app.screen == Screen::Unit && help::help_panel(app).is_none();
    const TILES: [Action; 3] = [
        Action::MoveNextTile,
        Action::MovePrevTile,
        Action::ToggleZoom,
    ];
    let typed = app.keys.pending();
    let follow = if typed.is_some() {
        app.keys
            .hint()
            .into_iter()
            .filter(|(_, a)| unit_tiles || !TILES.contains(a))
            .map(|(keys, a)| Hint {
                keys,
                action: a.id(),
                what: a.describe().into(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let command = app.keys.command_line().map(sanitize);
    let commands = match &command {
        Some(text) => app
            .keys
            .bindings()
            .filter_map(|(a, trigger)| match trigger {
                Trigger::Command(c) if c.starts_with(text.as_str()) => Some(Hint {
                    keys: trigger.label(),
                    action: a.id(),
                    what: a.describe().into(),
                }),
                _ => None,
            })
            .collect(),
        None => Vec::new(),
    };
    let visual = app.visual_rows().map(|r| r.count());
    let visual_actions = if visual.is_some() {
        vec![
            hint(app, Action::DecideAccept, "accept"),
            hint(app, Action::DecideFlag, "flag"),
            hint(app, Action::ViewBack, "leave"),
        ]
    } else {
        Vec::new()
    };
    let note = match &app.overlay {
        Overlay::Note {
            id,
            text,
            back: None,
        } => Some(note_editor(app, id, text)),
        _ => None,
    };
    Bar {
        pending,
        flagged,
        accepted,
        auto_accepted: app.auto_accepted(),
        notes: crate::notes::noted(&app.review).len(),
        finish_key: app.keys.label(Action::FinishOpen),
        help_key: app.keys.label(Action::HelpOpen),
        status: app.status.clone(),
        typed,
        follow,
        command,
        commands,
        visual,
        visual_actions,
        note,
        waiting: app.waiting.as_ref().map(|_| app.waiting_line()),
    }
}

fn note_editor(app: &App, id: &str, text: &str) -> NoteEditor {
    NoteEditor {
        id: id.to_string(),
        name: app.review.unit(id).map(names::row_name).unwrap_or_default(),
        text: text.to_string(),
        max: NOTE_MAX,
    }
}

/// `1 file`, `3 units`.
fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// What a level's box is called. Only units accepted for you are listed at
/// [`Level::Same`].
fn box_name(level: Level) -> &'static str {
    match level {
        Level::Same => "Accepted for you",
        _ => level.title(),
    }
}

/// The line under the boxes about the units accepted for you.
pub fn auto_line(app: &App) -> Option<String> {
    let n = app.auto_accepted();
    if n == 0 {
        return None;
    }
    let does = if app.show_auto { "hides" } else { "shows" };
    let key = app.keys.label(Action::ToggleAutoAccepted);
    Some(format!(
        "{} accepted for you, {key} {does} them",
        plural(n, "unit")
    ))
}

fn queue(app: &App) -> Queue {
    let mut boxes: Vec<LevelBox> = Vec::new();
    let visual = app.visual_rows();
    for start in (0..app.order.len()).filter(|&p| app.starts_file(p)) {
        let level = app.file_level(start);
        let range = app.file_range(start);
        let units: Vec<&Unit> = range
            .clone()
            .map(|p| &app.review.units[app.order[p]])
            .collect();
        let rows = range
            .map(|pos| {
                let ui = app.order[pos];
                let u = &app.review.units[ui];
                let level = app.shown_level(ui);
                QueueRow {
                    pos,
                    id: u.id.clone(),
                    level,
                    icon: level.icon(),
                    path: sanitize(&u.file.display().to_string()),
                    name: names::row_name(u),
                    whole_file: u.kind == UnitKind::File,
                    depth: app.depth.get(ui).copied().unwrap_or(0),
                    decision: u.decision,
                    state: names::state_word(u),
                    note: (u.decision == Decision::Investigate)
                        .then(|| names::note_start(u))
                        .flatten(),
                    disputed: !app.level_of(ui).doubts.is_empty(),
                    noted: u.decision != Decision::Investigate && !u.dive.note.trim().is_empty(),
                    removed: u.status == UnitStatus::Removed,
                    preparing: u.prep == Prep::Pending,
                    selected: pos == app.sel,
                    in_visual: visual.as_ref().is_some_and(|r| r.contains(&pos)),
                    changed: app.changed.contains(&u.id),
                }
            })
            .collect();
        let group = FileGroup {
            path: sanitize(&units[0].file.display().to_string()),
            done: units
                .iter()
                .filter(|u| u.decision != Decision::Pending)
                .count(),
            total: units.len(),
            busy: units.iter().any(|u| u.prep == Prep::Pending),
            flagged: units.iter().any(|u| u.decision == Decision::Investigate),
            rows,
        };
        match boxes.last_mut() {
            Some(b) if b.level == level => b.files.push(group),
            _ => boxes.push(LevelBox {
                level,
                title: String::new(),
                files: vec![group],
            }),
        }
    }
    for b in &mut boxes {
        let n = match app.grouping {
            Grouping::File => plural(b.files.len(), "file"),
            Grouping::Unit => plural(b.files.iter().map(|f| f.total).sum(), "unit"),
        };
        b.title = format!("{} · {n}", box_name(b.level));
    }
    Queue {
        grouping: app.grouping,
        boxes,
        skipped: app
            .review
            .skipped
            .iter()
            .map(|f| Skipped {
                path: sanitize(&f.path.display().to_string()),
                why: sanitize(&f.why),
            })
            .collect(),
        auto_line: auto_line(app),
        sel: app.sel,
        show_auto: app.show_auto,
    }
}

/// How a card's statements go with its code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Notes {
    /// Written into the code, which shows only what they cite.
    Inline,
    /// Written into the code, which shows every change too.
    Full,
}

fn level_of(app: &App, u: &Unit) -> Level {
    app.assessment(&u.id)
        .map_or(Level::Routine, |a| a.level.shown(u.auto_accepted()))
}

fn prep_note(app: &App, u: &Unit) -> Option<String> {
    match u.prep {
        Prep::Ready | Prep::Skipped => None,
        Prep::Pending => Some("preparing…".into()),
        Prep::Unavailable => Some(format!("{}, details unavailable", u.status_word())),
        Prep::NotRequested if app.large && app.can_ask() => {
            Some("details are prepared when you open this file".into())
        }
        Prep::NotRequested => Some(u.status_word().to_string()),
    }
}

fn card_state(u: &Unit) -> String {
    match u.decision {
        Decision::Pending => "pending".into(),
        Decision::Accepted if u.auto_accepted() => "✓ accepted for you".into(),
        Decision::Accepted => "✓ accepted".into(),
        Decision::Investigate => "⚑ flagged".into(),
    }
}

fn card(app: &App, u: &Unit, notes: Notes, syntax: Option<&Cache>) -> Card {
    let level = level_of(app, u);
    let a = app.assessment(&u.id);
    let statements: Vec<Statement> = if u.prep == Prep::Ready {
        u.changes
            .iter()
            .map(|c| Statement {
                marker: c.marker,
                symbol: c.marker.symbol(),
                text: c.statement.clone(),
                risk: c.risk,
                cites: names::cite_text(&c.evidence),
                evidence: c.evidence.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let said = u.summary.is_some() || !statements.is_empty();
    let note = u.dive.note.trim();
    Card {
        id: u.id.clone(),
        name: names::row_name(u),
        place: names::place(u),
        file: sanitize(&u.file.display().to_string()),
        line: crate::review::diff::first_change(&app.review, u).unwrap_or(u.display_line()),
        kind: u.kind,
        status: u.status,
        level,
        icon: level.icon(),
        level_title: level.title(),
        decision: u.decision,
        accepted_for_you: u.auto_accepted(),
        state: card_state(u),
        why: a.map(|a| a.reasons.clone()).unwrap_or_default(),
        reasons: u.reasons.clone(),
        commits: app
            .review
            .commits_of(u)
            .into_iter()
            .map(|c| CommitRef {
                short: c.short.clone(),
                subject: sanitize(&c.subject),
            })
            .collect(),
        agent_label: said.then(|| names::agent_label(app, u)),
        summary: u.summary.clone(),
        nbc_reason: u.nbc_reason.clone(),
        prep: u.prep,
        prep_note: prep_note(app, u),
        code: code(app, u, notes, syntax),
        statements,
        note: (!note.is_empty()).then(|| NoteCard {
            text: sanitize(note),
            stale: u.note_is_stale(),
        }),
        answers: app
            .answers_for(&u.id)
            .into_iter()
            .filter_map(|q| {
                let a = q.answer.as_ref()?;
                Some(AnswerCard {
                    question: q.text.trim().to_string(),
                    answer: a.text.clone(),
                    shown: a.shown.len(),
                })
            })
            .collect(),
        disputed: a.is_some_and(|a| !a.doubts.is_empty()),
    }
}

/// `text` cut where its syntax class changes.
fn pieces(text: &str, classes: &[Option<Class>]) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for (i, c) in text.chars().enumerate() {
        let class = classes.get(i).copied().flatten();
        match out.last_mut() {
            Some(p) if p.class == class => p.text.push(c),
            _ => out.push(Piece {
                text: c.to_string(),
                class,
            }),
        }
    }
    out
}

fn code_row(r: &Row) -> CodeRow {
    let (text, classes) = expand_tabs(&r.text, &r.classes);
    CodeRow {
        kind: r.kind,
        line: r.line,
        old: r.old,
        pieces: pieces(&text, &classes),
    }
}

fn code(app: &App, u: &Unit, notes: Notes, syntax: Option<&Cache>) -> Option<Code> {
    if app.assessment(&u.id).is_none_or(|a| a.level < DIFF_LEVEL) {
        return None;
    }
    let said = u.prep == Prep::Ready;
    let cites: Vec<LineRef> = if said {
        u.changes
            .iter()
            .flat_map(|c| c.evidence.iter().copied())
            .collect()
    } else {
        Vec::new()
    };
    let (rows, added, removed) = rows_around(&app.review, u, &cites, notes == Notes::Full, syntax);
    if rows.is_empty() {
        return None;
    }
    let changes = |k: usize| matches!(rows[k].kind, Kind::Added | Kind::Removed);
    let hunks = (0..rows.len())
        .filter(|&k| changes(k) && (k == 0 || !changes(k - 1)))
        .collect();
    let notes = if said {
        let mut at: Vec<CodeNote> = u
            .changes
            .iter()
            .enumerate()
            .map(|(n, c)| CodeNote {
                after: anchor(c, &rows),
                statement: n,
            })
            .collect();
        at.sort_by_key(|n| (n.after.unwrap_or(usize::MAX), n.statement));
        at
    } else {
        Vec::new()
    };
    Some(Code {
        added,
        removed,
        rows: rows.iter().map(code_row).collect(),
        notes,
        hunks,
    })
}

fn second_model(app: &App, u: &Unit) -> Option<SecondModel> {
    let label = tools::answer_label(app, u, DiveKind::Disagree);
    let Some(o) = &u.checks.second else {
        return u.checks.second_failed.as_ref().map(|e| SecondModel {
            label,
            head: String::new(),
            verdicts: Vec::new(),
            missed: Vec::new(),
            failed: Some(format!("No second opinion: {e}")),
        });
    };
    let first = u.highest_risk().map_or("none", Risk::word);
    let doubts = o.disagreements(&u.changes);
    let head = if doubts.is_empty() {
        format!(
            "A second model agrees with the description. Its risk: {}, the description's: {first}.",
            o.risk.word()
        )
    } else {
        format!(
            "A second model disagrees on {}. Its risk: {}, the description's: {first}.",
            plural(doubts.len(), "point"),
            o.risk.word()
        )
    };
    let mut verdicts: Vec<_> = o.verdicts.iter().collect();
    verdicts.sort_by_key(|v| v.agree);
    Some(SecondModel {
        label,
        head,
        verdicts: verdicts
            .into_iter()
            .map(|v| VerdictView {
                agree: v.agree,
                statement: v.statement.clone(),
                why: v.why.clone(),
                cites: names::cite_text(&v.evidence),
            })
            .collect(),
        missed: o
            .missed
            .iter()
            .map(|m| MissedView {
                risk: m.risk,
                statement: m.statement.clone(),
                cites: names::cite_text(&m.evidence),
            })
            .collect(),
        failed: None,
    })
}

fn disputed(app: &App, u: &Unit) -> bool {
    app.assessment(&u.id).is_some_and(|a| !a.doubts.is_empty())
}

fn answer_tile(app: &App, u: &Unit, kind: DiveKind) -> AnswerTile {
    let result = u.dive.results.get(&kind).cloned();
    let svg = result.as_ref().and_then(|r| r.visual.as_ref()).map(|v| {
        use crate::review::visual::{DARK, LIGHT, Paint};
        Svg {
            svg: crate::review::visual::svg(v, Paint::Css),
            light: LIGHT.css_vars(),
            dark: DARK.css_vars(),
        }
    });
    let audit = result.as_ref().and_then(|r| r.audit.as_ref());
    let waiting = if tools::pending(u, kind) {
        Some(tools::waiting(app, u, kind))
    } else if audit.is_some_and(|a| a.checking) {
        Some(audit::CHECKING.to_string())
    } else if audit.is_some_and(|a| a.proving) {
        Some(audit::PROVING.to_string())
    } else {
        None
    };
    let ask = if kind == DiveKind::Recommend && result.is_none() && waiting.is_none() {
        Some(hint(
            app,
            Action::ToolAdvice,
            "ask the agent what to do about it: concrete steps, most important first, each \
with its reason",
        ))
    } else if kind == DiveKind::Tests && tools::may_read_tests(app, u) {
        let found = result
            .as_ref()
            .and_then(|r| r.coverage.as_ref())
            .and_then(|c| c.search.as_ref())
            .is_some_and(|s| !s.tests.is_empty());
        let what = if found {
            "the agent reads them"
        } else {
            "the agent looks for tests itself"
        };
        Some(hint(app, Action::ToolTests, what))
    } else if audit.is_some_and(|a| !a.findings.is_empty() && !a.proving) {
        Some(hint(
            app,
            Action::ToolProve,
            "prove it: each finding's attacker case, old against new",
        ))
    } else {
        None
    };
    let none = (result.is_none() && waiting.is_none() && ask.is_none()).then(|| {
        match tools::tool_key(app, kind) {
            _ if kind == DiveKind::Security && !app.can_ask() => audit::NO_AGENT.to_string(),
            key if key.is_empty() => "No answer. Opening the unit again asks again.".to_string(),
            key => format!("No answer. {key} asks again."),
        }
    });
    AnswerTile {
        question: kind,
        label: tools::answer_label(app, u, kind),
        result,
        svg,
        waiting,
        ask,
        none,
        second: (kind == DiveKind::Recommend && app.can_ask() && disputed(app, u))
            .then(|| second_model(app, u))
            .flatten(),
        facts: if kind == DiveKind::Security {
            u.security.clone()
        } else {
            Vec::new()
        },
    }
}

fn ask_tile(app: &App, u: &Unit) -> AskTile {
    let mut next_link = 0;
    let exchanges = app
        .chat_of(&u.id)
        .into_iter()
        .map(|n| {
            let q = &app.review.questions[n];
            let live = app.live_text(n);
            let state = if app.ask.pending.contains(&n) {
                match app.ask.live.get(&n) {
                    _ if live.is_some() => AskingState::Writing,
                    Some(l) => AskingState::Asking {
                        step: l.step.clone(),
                        steps: l.steps,
                    },
                    None => AskingState::Asking {
                        step: None,
                        steps: 0,
                    },
                }
            } else if let Some(e) = &q.error {
                AskingState::Failed { error: sanitize(e) }
            } else if q.answer.is_some() {
                AskingState::Answered
            } else {
                AskingState::NotAnswered
            };
            let mut related = Vec::new();
            let mut shown = Vec::new();
            let answer = match (live, &q.answer) {
                (Some(text), _) => Some(text.to_string()),
                (None, Some(a)) => {
                    for r in &a.related {
                        let unit = app.review.unit(&r.id);
                        let link = app.listed(&r.id).then(|| {
                            next_link += 1;
                            next_link - 1
                        });
                        related.push(RelatedView {
                            id: r.id.clone(),
                            why: r.why.clone(),
                            name: unit.map(names::row_name).unwrap_or_else(|| r.id.clone()),
                            place: unit.map(names::place).unwrap_or_default(),
                            level: unit.map(|u| level_of(app, u)),
                            link,
                        });
                    }
                    for s in &a.shown {
                        shown.push(ShownView {
                            shown: s.clone(),
                            link: next_link,
                        });
                        next_link += 1;
                    }
                    Some(a.text.clone())
                }
                (None, None) => None,
            };
            Exchange {
                index: n,
                question: q.text.clone(),
                state,
                answer,
                related,
                shown,
            }
        })
        .collect();
    AskTile {
        exchanges,
        draft: app.ask.draft.clone(),
        max: super::app::QUESTION_MAX,
        link: app.ask.link,
        web_may_ask: app.web_may_ask(),
        label: format!("agent ({})", app.cfg.model_label(Role::Deep)),
        tools: tools::TOOLS
            .iter()
            .map(|(_, a, name, what)| hint(app, *a, format!("{name}: {what}")))
            .collect(),
    }
}

fn tile(app: &App, u: &Unit, p: Panel) -> Tile {
    let id = TileId::of(p);
    let (title, body) = match p {
        Panel::Recommend => (
            "Advice".to_string(),
            TileBody::Answer(Box::new(answer_tile(app, u, DiveKind::Recommend))),
        ),
        Panel::Ask => ("Ask".to_string(), TileBody::Ask(ask_tile(app, u))),
        Panel::Tool(kind) => (
            tools::light(kind).0.to_string(),
            TileBody::Answer(Box::new(answer_tile(app, u, kind))),
        ),
        Panel::Outside => (
            "Outside the review".to_string(),
            TileBody::Outside {
                shown: app.unit.outside.clone(),
            },
        ),
    };
    Tile { id, title, body }
}

/// The actions a window puts up as buttons where they act.
pub const BUTTONS: &[Action] = &[
    Action::DecideAccept,
    Action::DecideFlag,
    Action::NoteEdit,
    Action::HistoryUndo,
    Action::DecideAcceptFile,
    Action::ViewOpen,
    Action::ViewBack,
    Action::AskOpen,
    Action::ToolAdvice,
    Action::ToolTests,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
    Action::ToolSecurity,
    Action::ToolProve,
    Action::GotoDefinition,
    Action::GotoCallers,
    Action::GotoEditor,
    Action::FindUnit,
    Action::ToggleByFile,
    Action::ToggleSecurityOrder,
    Action::ToggleAutoAccepted,
    Action::ToggleZoom,
    Action::MoveNextPending,
    Action::MovePrevPending,
    Action::MoveNextHunk,
    Action::MovePrevHunk,
    Action::ToolFlagged,
    Action::ToolNotes,
    Action::ToolHistory,
    Action::FinishOpen,
    Action::HelpOpen,
];

fn actions(app: &App) -> BTreeMap<&'static str, ActionView> {
    BUTTONS
        .iter()
        .map(|&a| {
            let what = match app.screen {
                Screen::Flagged => help::flagged_describe(a),
                _ => None,
            }
            .unwrap_or(a.describe());
            let view = ActionView {
                keys: app.keys.label(a),
                what: what.into(),
                off: help::AGENT_KEYS.contains(&a) && !app.can_ask(),
                on: toggled(app, a),
            };
            (a.id(), view)
        })
        .collect()
}

/// `actions` as a window lists them: without `:app`, the way from the
/// terminal to the window this is.
fn window_actions(actions: &[Action]) -> Vec<Action> {
    actions
        .iter()
        .copied()
        .filter(|&a| a != Action::FinishApp)
        .collect()
}

/// Whether the toggle `a` is on; `None` for what isn't one.
fn toggled(app: &App, a: Action) -> Option<bool> {
    match a {
        Action::ToggleByFile => Some(app.grouping == Grouping::File),
        Action::ToggleAutoAccepted => Some(app.show_auto),
        Action::ToggleZoom => Some(app.unit.zoom),
        _ => None,
    }
}

/// The keys of the panel open (not the help over it): what it does
/// besides moving through its list, which a click does, and its way back.
fn panel_keys(app: &App) -> Vec<Hint> {
    let find = matches!(
        app.overlay,
        Overlay::Jump {
            kind: JumpKind::Find,
            ..
        }
    );
    let open = matches!(
        app.overlay,
        Overlay::Notes { .. }
            | Overlay::Commits { .. }
            | Overlay::Jump { .. }
            | Overlay::Finish { .. }
    );
    let Some((_, actions)) = help::help_panel(app).filter(|_| open) else {
        return Vec::new();
    };
    actions
        .iter()
        .filter(|&&a| help::group_of(a) != 0 || a == Action::ViewBack)
        .filter(|&&a| a != Action::ViewClose)
        .filter_map(|&a| {
            // Finding, the keys are the query's: ⏎ goes, esc closes.
            let keys = match (find, a) {
                (false, _) => app.keys.labels(a),
                (true, Action::ViewOpen) => "⏎".into(),
                (true, Action::ViewBack) => "esc".into(),
                (true, _) => return None,
            };
            (!keys.is_empty()).then(|| Hint {
                keys,
                action: a.id(),
                what: help::describe(app, a).into(),
            })
        })
        .collect()
}

fn unit_screen(app: &App, syntax: Option<&Cache>) -> Option<UnitScreen> {
    let u = app.unit_on_show()?;
    let advice_open = app.unit.tiles.contains(&Panel::Recommend);
    Some(UnitScreen {
        card: card(app, u, Notes::Full, syntax),
        second: (!advice_open && app.can_ask() && disputed(app, u))
            .then(|| second_model(app, u))
            .flatten(),
        tiles: app.unit.tiles.iter().map(|&p| tile(app, u, p)).collect(),
        focus: app.unit.panel.map(TileId::of),
        zoom: app.unit.zoom,
        typing: app.unit.typing,
    })
}

fn flagged(app: &App, syntax: Option<&Cache>) -> FlaggedScreen {
    let ids = app.flagged_ids();
    FlaggedScreen {
        sel: app.flagged_index(&ids),
        entries: ids
            .iter()
            .filter_map(|id| app.review.unit(id))
            .map(|u| card(app, u, Notes::Inline, syntax))
            .collect(),
    }
}

fn jump_item(app: &App, u: &Unit) -> JumpItem {
    JumpItem {
        id: u.id.clone(),
        name: names::row_name(u),
        place: names::place(u),
        level: level_of(app, u),
        decision: u.decision,
    }
}

fn note_item(app: &App, u: &Unit) -> NoteItem {
    NoteItem {
        id: u.id.clone(),
        name: names::row_name(u),
        place: crate::notes::place(u),
        level: level_of(app, u),
        decision: u.decision,
        text: sanitize(u.dive.note.trim()),
        stale: u.note_is_stale(),
    }
}

fn overlay(app: &App) -> OverlayView {
    match &app.overlay {
        Overlay::None => OverlayView::None,
        Overlay::Help => OverlayView::Help(HelpView {
            of: help::screen_name(app),
            groups: help::help_groups(app, &window_actions(help::screen_actions(app)))
                .into_iter()
                .map(|(name, rows)| HelpGroup { name, rows })
                .collect(),
        }),
        Overlay::ConfirmReset => OverlayView::ConfirmReset {
            decisions: app.reset_count(),
        },
        Overlay::Note {
            id,
            text,
            back: Some(_),
        } => OverlayView::Note(note_editor(app, id, text)),
        // On the bar.
        Overlay::Note { .. } => OverlayView::None,
        Overlay::Notes { sel } => OverlayView::Notes {
            sel: *sel,
            notes: crate::notes::noted(&app.review)
                .into_iter()
                .map(|u| note_item(app, u))
                .collect(),
        },
        Overlay::ConfirmDeleteNote { id, .. } => OverlayView::ConfirmDeleteNote {
            id: id.clone(),
            name: app.review.unit(id).map(names::row_name).unwrap_or_default(),
        },
        Overlay::ConfirmWeb => OverlayView::ConfirmWeb,
        Overlay::Commits { sel, unit } => OverlayView::Commits(CommitsView {
            sel: *sel,
            unit: *unit,
            pr: app.review.intent.pr.clone(),
            commits: app.review.intent.commits.clone(),
            units: app
                .commit_units(*sel)
                .into_iter()
                .filter_map(|pos| app.order.get(pos))
                .map(|&i| jump_item(app, &app.review.units[i]))
                .collect(),
        }),
        Overlay::Finish { sel } => OverlayView::Finish(finish(app, *sel)),
        Overlay::Jump { kind, query, sel } => OverlayView::Jump(JumpView {
            list: kind.clone(),
            query: query.clone(),
            sel: *sel,
            items: app
                .jump_items()
                .into_iter()
                .map(|i| jump_item(app, &app.review.units[i]))
                .collect(),
        }),
    }
}

fn finish(app: &App, sel: usize) -> FinishView {
    let api = app.cfg.provider() == crate::config::Provider::Api;
    let steps = app.finish_steps();
    let step = app.finish_step();
    let details = match step {
        None => Details::None,
        Some(Step::Commit) => {
            let (branch, detached) = match &app.finish.branch {
                Some(Branch::Named(b)) => (Some(sanitize(b)), false),
                Some(Branch::Detached) => (None, true),
                _ => (None, false),
            };
            Details::Commit {
                files: commit::file_rows(&app.review),
                branch,
                detached,
                others: app.finish.others,
                message: app.finish.message.clone(),
                draft: app.finish.draft,
            }
        }
        Some(s) => match app.step_text(s) {
            Some(text) => Details::Text { text },
            None => match app.review.pr.as_ref() {
                None => Details::None,
                Some(pr) => {
                    let needs_comment =
                        s == Step::ApproveWithComment && app.finish.summary.is_none();
                    let notes = approve::collect_notes(&app.review, &app.order);
                    let plan = approve::build_plan(&notes, &app.finish_summary(), true);
                    Details::Post {
                        head_check: head_check(pr),
                        needs_comment,
                        body: plan.body,
                        inline: plan
                            .inline
                            .into_iter()
                            .map(|c| InlineView {
                                path: sanitize(&c.path),
                                line: c.line,
                                old: c.side == Side::Old,
                                body: c.body,
                            })
                            .collect(),
                    }
                }
            },
        },
    };
    FinishView {
        reviewing: app.reviewing(),
        subject: app.review.subject(),
        sel: sel.min(steps.len().saturating_sub(1)),
        steps: steps
            .iter()
            .map(|&s| StepView {
                step: s,
                label: app.step_label(s),
                verb: s.verb(api),
            })
            .collect(),
        not_shown: app
            .review
            .not_shown(|f| sanitize(&f.path.display().to_string())),
        confirm: app.finish.confirm.map(|s| app.step_label(s)),
        editing: app.finish.editing.clone(),
        details,
        notes: crate::notes::listed(&app.review)
            .into_iter()
            .map(|u| note_item(app, u))
            .collect(),
    }
}

/// What the front end hands back: an action by its id, or a key by its
/// notation (`<space>a`, `G`, `<c-u>`), so `[keys]` rebinding applies.
pub fn action_of(id: &str) -> Option<Action> {
    Action::from_id(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Key;
    use crate::session::app::tests::app;

    fn press(a: &mut App, keys: &str) {
        for k in crate::keymap::parse_keys(keys).unwrap() {
            a.on_key(k);
        }
    }

    #[test]
    fn the_queue_lists_every_unit_once_in_its_levels_box() {
        let a = app(5);
        let v = snapshot(&a, None);
        let rows: Vec<&QueueRow> = v
            .queue
            .boxes
            .iter()
            .flat_map(|b| &b.files)
            .flat_map(|f| &f.rows)
            .collect();
        assert_eq!(rows.len(), a.order.len());
        for (k, r) in rows.iter().enumerate() {
            assert_eq!(r.pos, k);
            assert_eq!(r.selected, k == a.sel);
        }
        for b in &v.queue.boxes {
            assert!(
                b.files
                    .iter()
                    .flat_map(|f| &f.rows)
                    .all(|r| r.level == b.level)
            );
            assert!(b.title.starts_with(box_name(b.level)), "{}", b.title);
        }
        assert_eq!(v.bar.pending, 5);
        assert!(v.preview.is_some());
        assert!(v.unit.is_none());
    }

    #[test]
    fn deciding_shows_in_the_rows_and_the_counts() {
        let mut a = app(3);
        a.sel = 0;
        press(&mut a, "a");
        let v = snapshot(&a, None);
        let first = &v.queue.boxes[0].files[0].rows[0];
        assert_eq!(first.decision, Decision::Accepted);
        assert_eq!(first.state, "accepted");
        assert_eq!((v.bar.accepted, v.bar.pending), (1, 2));
    }

    #[test]
    fn the_unit_view_has_its_card_tiles_and_keys() {
        let mut a = app(3);
        a.sel = 0;
        let id = a.selected_id().unwrap();
        a.open_unit(id.clone());
        let v = snapshot(&a, None);
        assert_eq!(v.screen, Screen::Unit);
        let u = v.unit.expect("the unit view");
        assert_eq!(u.card.id, id);
        assert_eq!(v.actions["decide.accept"].keys, "a");
        assert_eq!(v.actions["toggle.zoom"].on, Some(false));
        assert!(v.preview.is_none());
    }

    #[test]
    fn a_waiting_prefix_lists_what_can_follow() {
        let mut a = app(3);
        a.on_key(Key::new(
            crate::keymap::KeyCode::Char('g'),
            Default::default(),
        ));
        let v = snapshot(&a, None);
        assert_eq!(v.bar.typed.as_deref(), Some("g"));
        assert!(
            v.bar.follow.iter().any(|h| h.action == "move.first"),
            "{:?}",
            v.bar.follow
        );
    }

    #[test]
    fn the_help_and_the_sheet_are_overlays() {
        let mut a = app(2);
        press(&mut a, "?");
        let OverlayView::Help(h) = snapshot(&a, None).overlay else {
            panic!("the help");
        };
        assert_eq!(h.of, "queue");
        assert!(h.groups.iter().any(|g| g.name == "Decide"));
        press(&mut a, "<esc>");
        a.open_finish();
        let OverlayView::Finish(f) = snapshot(&a, None).overlay else {
            panic!("the sheet");
        };
        assert_eq!(f.steps[0].step, Step::KeepReviewing);
        assert!(matches!(f.details, Details::Text { .. }));
    }

    #[test]
    fn code_is_cut_into_pieces_by_class() {
        let p = pieces(
            "let x",
            &[
                Some(Class::Keyword),
                Some(Class::Keyword),
                Some(Class::Keyword),
                None,
                Some(Class::Variable),
            ],
        );
        assert_eq!(
            p,
            vec![
                Piece {
                    text: "let".into(),
                    class: Some(Class::Keyword)
                },
                Piece {
                    text: " ".into(),
                    class: None
                },
                Piece {
                    text: "x".into(),
                    class: Some(Class::Variable)
                },
            ]
        );
        assert_eq!(pieces("ab", &[]).len(), 1);
    }

    #[test]
    fn the_view_is_json() {
        let a = app(3);
        let json = serde_json::to_value(snapshot(&a, None)).unwrap();
        assert_eq!(json["screen"], "Queue");
        assert_eq!(json["overlay"]["kind"], "None");
        assert!(json["queue"]["boxes"].is_array());
    }
}
