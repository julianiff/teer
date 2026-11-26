//! The review as a front end draws it: [`snapshot`] turns an [`App`] into
//! a [`View`], plain data any front end can render, in Rust or as JSON
//! (serde). It says what is on show and what it means: the queue's tree
//! of files, the file under the cursor, the unit view's text and
//! tiles, the flagged list, the overlay, the bar. It never says how it
//! looks: no widths, no colours, no wrapping. Text the agent wrote stays
//! plain text; the front end must not take it for markup.
//!
//! Code comes as diff rows cut into pieces by syntax class
//! ([`crate::syntax::Class`]); the front end's theme colours them.

use std::collections::BTreeMap;

use serde::Serialize;

use super::app::{App, DraftState, JumpKind, NOTE_MAX, Overlay, Screen};
use super::finish::{Reviewing, Step, head_check};
use super::tree::{FileFacts, RowKind};
use super::unit::Panel;
use super::{help, names, tools};
use crate::approve;
use crate::commit::{self, Branch};
use crate::config::Role;
use crate::keymap::{Action, Trigger};
use crate::review::attention::Level;
use crate::review::comments::comments;
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
    /// The file under the queue's cursor, as its preview shows it; `None`
    /// on a directory.
    pub preview: Option<FilePreview>,
    /// The unit view, while it is on show.
    pub unit: Option<UnitScreen>,
    /// The flagged list, while it is on show.
    pub flagged: Option<FlaggedScreen>,
    pub overlay: OverlayView,
    /// What a front end puts up as buttons where they act (the unit's
    /// decisions on its card, the tiles beside it), by action id: each
    /// one's key, what it does, whether it can.
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
    /// The mode the review is in: `check`, `review`, `security`, `all`.
    pub mode: &'static str,
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
    /// Visual mode: how many rows the selection holds, and what applies.
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

/// The queue: the tree of the changed files.
#[derive(Debug, Clone, Serialize)]
pub struct Queue {
    /// The tree's rows as they are listed now, top to bottom.
    pub rows: Vec<TreeRow>,
    /// The attack-surface card over the tree, while the security lens is
    /// on; absent when it is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<SurfaceView>,
}

/// A row of the tree: a directory, a file, or a file teer can't show.
#[derive(Debug, Clone, Serialize)]
pub struct TreeRow {
    /// What a click on the row sends back (`host::Click::Row`), as it
    /// is: the same across re-runs and folds. Not for show.
    pub id: String,
    pub kind: RowKind,
    /// How many directory rows it is under.
    pub depth: usize,
    /// A file's name; a directory's, joined with those it alone holds
    /// (`src/tui`).
    pub name: String,
    /// From the repository's root.
    pub path: String,
    /// A directory whose rows are hidden.
    pub folded: bool,
    /// The most attention a unit of the file, or under the directory,
    /// needs; `None` where no unit is listed.
    pub level: Option<Level>,
    /// A file's units: done, flagged, being described, disputed, changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileState>,
    /// Why teer can't show the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// On a directory: every unit under it is accepted, so its level asks
    /// for nothing more.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub settled: bool,
    pub selected: bool,
    pub in_visual: bool,
}

/// What a file's row says about its units.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct FileState {
    /// Every one of its units is accepted: nothing is left to do on it.
    pub done: bool,
    /// One is flagged.
    pub flagged: bool,
    /// One is being described.
    pub preparing: bool,
    /// A second model disputes one (`⚠`).
    pub disputed: bool,
    /// The last re-run found one changed or new.
    pub changed: bool,
}

/// The file under the queue's cursor: its units in line order, each with
/// its code and the statements written into it.
#[derive(Debug, Clone, Serialize)]
pub struct FilePreview {
    pub path: String,
    /// The unit `view.open`, a note and the tools act on, by id: the
    /// file's lead unit, or the one a jump went to.
    pub selected: String,
    pub cards: Vec<Card>,
}

/// The attack-surface card: a row per class the change touches, most
/// severe first in each, then what was checked and what nobody did. No
/// rows: nothing found, which never means safe.
#[derive(Debug, Clone, Serialize)]
pub struct SurfaceView {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<crate::security::surface::Row>,
    /// `auth · entry points · injection · …`.
    pub checked: String,
    /// `denial of service · business logic · crypto details`.
    pub not: String,
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
    /// Hunks of a change the agent proposed that aren't put in yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed: Option<usize>,
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
    /// The row it is written under; `None` after the last.
    pub after: Option<usize>,
    /// On the card's head: `medium · disputed`, `sink · injection · high`.
    /// Empty for a statement as it is.
    pub title: String,
    pub risk: Option<Risk>,
    /// A statement's `+`, `~`, `−`.
    pub symbol: Option<&'static str>,
    /// A paragraph each.
    pub text: Vec<String>,
    /// `L168-172, old L192`.
    pub cites: String,
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
    /// The change the agent proposes, or that one is on its way: after
    /// the code change (and the second model).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<ProposalView>,
}

/// A change the agent proposes for the unit, as its text shows it.
#[derive(Debug, Clone, Serialize)]
pub struct ProposalView {
    /// What it answers: the finding's title, `your note`, `the advice`;
    /// with a finding, its severity.
    pub answers: Option<String>,
    pub severity: Option<Risk>,
    /// Who wrote it: `agent (opus)`.
    pub label: String,
    /// One line: what it changes, and what it leaves alone.
    pub summary: String,
    /// The second model's word: does it fix what it says, and nothing else.
    pub second: Option<crate::security::SecondCheck>,
    /// Hunks dropped: their text wasn't found once.
    pub dropped: usize,
    pub hunks: Vec<HunkView>,
    /// The hunk `fix.apply` puts in.
    pub current: Option<usize>,
    /// A change is on its way, and what it is doing.
    pub waiting: Option<String>,
    /// On a pull request: the hunks become suggestions for the review.
    pub suggest: bool,
}

/// One proposed hunk, in a box like the code change's.
#[derive(Debug, Clone, Serialize)]
pub struct HunkView {
    /// `src/x.php:3-5`.
    pub place: String,
    /// Why this hunk, one line.
    pub why: String,
    /// The file still parses with it.
    pub parses: bool,
    pub state: crate::fix::HunkState,
    /// Its rows: the lines it removes and adds, and the context they share.
    pub code: Code,
}

/// Which tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
pub enum TileId {
    Ask,
    Tests,
    Security,
    Outside,
    Fix,
}

impl TileId {
    pub fn of(p: Panel) -> TileId {
        match p {
            Panel::Ask => TileId::Ask,
            Panel::Outside => TileId::Outside,
            Panel::Fix => TileId::Fix,
            Panel::Tool(DiveKind::Tests) => TileId::Tests,
            Panel::Tool(DiveKind::Security) => TileId::Security,
            Panel::Tool(_) => TileId::Tests,
        }
    }

    pub fn panel(self) -> Panel {
        match self {
            TileId::Ask => Panel::Ask,
            TileId::Outside => Panel::Outside,
            TileId::Fix => Panel::Fix,
            TileId::Tests => Panel::Tool(DiveKind::Tests),
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
    /// A tool's answer: what came, what is on its way.
    Answer(Box<AnswerTile>),
    /// The conversation about the unit.
    Ask(AskTile),
    /// What the unit calls outside the review (`gd`), read-only.
    Outside { shown: Vec<Shown> },
    /// Asked for a change with nothing to do: why.
    Note { text: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct AnswerTile {
    /// What was asked.
    pub question: DiveKind,
    /// Who wrote it: `agent (opus)`, `found by teer`.
    pub label: String,
    pub result: Option<DiveResult>,
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
    /// Above the conversation: the advice (what to do about the unit),
    /// what came or is on its way, or the key that asks for it; with it
    /// the second model when it disputes the description. `None` where
    /// the mode has no advice.
    pub advice: Option<Box<AnswerTile>>,
    /// The questions the tile offers to ask next, the first one what
    /// `enter` on an empty question asks (see [`super::suggest`]).
    pub suggestions: Vec<super::suggest::Suggestion>,
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
    /// A toggle's state (zoomed, the units accepted for you listed);
    /// `None` for the rest.
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
    /// Asks before proposed hunks are written to the working tree:
    /// "Put this hunk into src/x.php? (+3 −1)".
    ConfirmApply {
        question: String,
    },
    /// After the inserts to add were put in: asks whether to check the
    /// change again.
    ConfirmCheck {
        question: String,
    },
    Commits(CommitsView),
    Finish(FinishView),
    Jump(JumpView),
    /// The agent sessions to send a tile's result to (`s`): `what` it is,
    /// the sessions, the one selected, and the message. `inside` is where
    /// each session runs, relative to the repository (empty at its root);
    /// `wrote` is the session that wrote the change, by index; with
    /// `pi_install` a last row (index `targets.len()`) installs teer's pi
    /// extension, and `confirm` is the question asked before it does.
    SendTo {
        what: String,
        targets: Vec<crate::send::Target>,
        sel: usize,
        text: String,
        inside: Vec<String>,
        wrote: Option<usize>,
        pi_install: bool,
        confirm: Option<String>,
    },
    /// The mode picker (`<space>m`): the modes, the one selected.
    Mode {
        sel: usize,
        modes: Vec<ModeItem>,
    },
}

/// A mode in the picker: its name, what it is for, the tools it has, and
/// whether the review is in it.
#[derive(Debug, Clone, Serialize)]
pub struct ModeItem {
    pub name: &'static str,
    pub about: &'static str,
    pub tools: &'static str,
    pub current: bool,
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
            .then(|| preview(app, syntax))
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
        mode: app.mode.name(),
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
            .filter(|(_, a)| app.allows(*a) && (unit_tiles || !TILES.contains(a)))
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

/// The attack-surface card while the security lens is on.
fn surface(app: &App) -> Option<SurfaceView> {
    use crate::security::surface::{CHECKED, NOT_CHECKED, surface};
    (app.security.enabled && app.security.surface).then(|| SurfaceView {
        rows: surface(&app.review).rows,
        checked: CHECKED.join(" · "),
        not: NOT_CHECKED.join(" · "),
    })
}

fn queue(app: &App) -> Queue {
    let at = app.cursor_row();
    let visual = app.visual_rows();
    let rows = app
        .rows()
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let facts = match r.kind {
                RowKind::File => app.file_facts(&r.path),
                _ => None,
            };
            TreeRow {
                id: r.id(),
                kind: r.kind,
                depth: r.depth,
                name: sanitize(&r.name),
                path: sanitize(&r.path.display().to_string()),
                folded: r.folded,
                level: match r.kind {
                    RowKind::Dir => app.dir_level(&r.path),
                    _ => facts.map(|f| f.level),
                },
                file: facts.map(file_state),
                why: app.skipped_why(&r.path).map(sanitize),
                settled: r.kind == RowKind::Dir && app.dir_done(&r.path),
                selected: at == Some(i),
                in_visual: visual.as_ref().is_some_and(|v| v.contains(&i)),
            }
        })
        .collect();
    Queue {
        rows,
        surface: surface(app),
    }
}

fn file_state(f: FileFacts) -> FileState {
    FileState {
        done: f.done,
        flagged: f.flagged,
        preparing: f.preparing,
        disputed: f.disputed,
        changed: f.changed,
    }
}

/// The file of the queue's selected unit: its listed units, by line.
fn preview(app: &App, syntax: Option<&Cache>) -> Option<FilePreview> {
    let selected = app.selected()?;
    let file = &selected.file;
    Some(FilePreview {
        path: sanitize(&file.display().to_string()),
        selected: selected.id.clone(),
        cards: app
            .file_units(file)
            .map(|p| card(app, &app.review.units[app.order[p]], Notes::Inline, syntax))
            .collect(),
    })
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
        proposed: app
            .shown_proposal(u)
            .map(|p| p.open().len())
            .filter(|&n| n > 0),
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
    // What the mode has to say about the unit (see `review::comments`).
    let said = comments(u, app.mode);
    let cites: Vec<LineRef> = said
        .iter()
        .flat_map(|c| c.evidence.iter().copied())
        .collect();
    let (rows, added, removed) = rows_around(&app.review, u, &cites, notes == Notes::Full, syntax);
    if rows.is_empty() {
        return None;
    }
    let changes = |k: usize| matches!(rows[k].kind, Kind::Added | Kind::Removed);
    let hunks = (0..rows.len())
        .filter(|&k| changes(k) && (k == 0 || !changes(k - 1)))
        .collect();
    // Sorted by row, comments under one row in their order.
    let mut notes: Vec<CodeNote> = said
        .into_iter()
        .map(|c| CodeNote {
            after: anchor(&c.evidence, &rows),
            cites: names::cite_text(&c.evidence),
            title: c.title,
            risk: c.risk,
            symbol: c.marker.map(Marker::symbol),
            text: c.text,
        })
        .collect();
    notes.sort_by_key(|n| n.after.unwrap_or(usize::MAX));
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
            failed: Some(format!("No check: {e}")),
        });
    };
    let first = u.highest_risk().map_or("none", Risk::word);
    let doubts = o.disagreements(&u.changes);
    let head = if doubts.is_empty() {
        format!(
            "The check agrees with the description. Its risk: {}, the description's: {first}.",
            o.risk.word()
        )
    } else {
        format!(
            "The check disputes {}. Its risk: {}, the description's: {first}.",
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
    // The advice is asked from what the ask tile offers (`suggestions`).
    let ask = if kind == DiveKind::Tests && tools::may_read_tests(app, u) {
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
        advice: app
            .mode
            .reviews()
            .then(|| Box::new(answer_tile(app, u, DiveKind::Recommend))),
        suggestions: app.suggestions_for(u),
        exchanges,
        draft: app.ask.draft.clone(),
        max: super::app::QUESTION_MAX,
        link: app.ask.link,
        web_may_ask: app.web_may_ask(),
        label: format!("agent ({})", app.cfg.model_label(Role::Deep)),
        tools: tools::TOOLS
            .iter()
            // The ones the mode has.
            .filter(|(_, a, ..)| app.allows(*a))
            .map(|(_, a, name, what)| hint(app, *a, format!("{name}: {what}")))
            .collect(),
    }
}

fn tile(app: &App, u: &Unit, p: Panel) -> Tile {
    let id = TileId::of(p);
    let (title, body) = match p {
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
        Panel::Fix => (
            super::fix::TILE.to_string(),
            TileBody::Note {
                text: app.fix_tile_text(u),
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
    Action::ToolTests,
    Action::ToolSecurity,
    Action::ToolProve,
    Action::ToolFix,
    Action::FixApply,
    Action::FixApplyAll,
    Action::SendSession,
    Action::GotoDefinition,
    Action::GotoCallers,
    Action::GotoEditor,
    Action::FindUnit,
    Action::ToggleSurface,
    Action::ToggleAutoAccepted,
    Action::ToggleZoom,
    Action::MoveNextPending,
    Action::MovePrevPending,
    Action::MoveNextHunk,
    Action::MovePrevHunk,
    Action::ToolFlagged,
    Action::ToolNotes,
    Action::ToolHistory,
    Action::ToolMode,
    Action::FinishOpen,
    Action::HelpOpen,
];

fn actions(app: &App) -> BTreeMap<&'static str, ActionView> {
    BUTTONS
        .iter()
        // What the mode leaves out has no button.
        .filter(|&&a| app.allows(a))
        .map(|&a| {
            let what = match app.screen {
                Screen::Flagged => help::flagged_describe(a),
                Screen::Queue if !app.mode.improves() => help::queue_describe(a),
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
        Action::ToggleSurface => Some(app.security.surface),
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
    let advice_open = app.unit.tiles.contains(&Panel::Ask);
    Some(UnitScreen {
        card: card(app, u, Notes::Full, syntax),
        second: (!advice_open && app.can_ask() && disputed(app, u))
            .then(|| second_model(app, u))
            .flatten(),
        tiles: app.unit.tiles.iter().map(|&p| tile(app, u, p)).collect(),
        focus: app.unit.panel.map(TileId::of),
        zoom: app.unit.zoom,
        typing: app.unit.typing,
        proposal: proposal_view(app, u, syntax),
    })
}

/// The change the agent proposes for `u`, or that one is on its way;
/// `None` with neither.
fn proposal_view(app: &App, u: &Unit, syntax: Option<&Cache>) -> Option<ProposalView> {
    use super::fix;
    let kind = app.shown_kind();
    let p = app.shown_proposal(u);
    let waiting = tools::pending(u, kind).then(|| tools::waiting(app, u, kind));
    if p.is_none() && waiting.is_none() {
        return None;
    }
    let reason = p
        .map(|p| p.reason.clone())
        .or_else(|| fix::target(u))
        .filter(|_| !app.mode.improves());
    let (answers, severity) = match reason.as_ref().map(fix::answers) {
        Some((a, s)) => (Some(a), s),
        None => (None, None),
    };
    let hunks = p.map_or_else(Vec::new, |p| {
        p.hunks
            .iter()
            .map(|h| {
                let rows = fix::hunk_rows(h, syntax);
                let (added, removed) = fix::counts(h);
                let first = rows
                    .iter()
                    .position(|r| matches!(r.kind, Kind::Added | Kind::Removed));
                HunkView {
                    place: fix::place(h),
                    why: sanitize(&h.why),
                    parses: h.parses,
                    state: h.state,
                    code: Code {
                        added,
                        removed,
                        rows: rows.iter().map(code_row).collect(),
                        notes: Vec::new(),
                        hunks: first.into_iter().collect(),
                    },
                }
            })
            .collect()
    });
    Some(ProposalView {
        answers,
        severity,
        label: tools::answer_label(app, u, kind),
        summary: p.map(|p| p.summary.clone()).unwrap_or_default(),
        second: p.and_then(|p| p.second.clone()),
        dropped: p.map_or(0, |p| p.dropped),
        current: (u.id == app.unit.id).then(|| app.current_fix()).flatten(),
        hunks,
        waiting,
        suggest: app.review.pr.is_some(),
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
        Overlay::Mode { sel } => OverlayView::Mode {
            sel: *sel,
            modes: crate::mode::Mode::EVERY
                .into_iter()
                .map(|m| ModeItem {
                    name: m.name(),
                    about: m.about(),
                    tools: m.tools(),
                    current: m == app.mode,
                })
                .collect(),
        },
        Overlay::ConfirmApply { id, hunks, kind } => OverlayView::ConfirmApply {
            question: app.apply_question(id, hunks, *kind),
        },
        Overlay::ConfirmCheck { question } => OverlayView::ConfirmCheck {
            question: question.clone(),
        },
        Overlay::SendTo {
            targets,
            sel,
            text,
            what,
        } => OverlayView::SendTo {
            what: what.clone(),
            targets: targets.clone(),
            sel: *sel,
            text: text.clone(),
            inside: targets
                .iter()
                .map(|t| {
                    t.cwd
                        .strip_prefix(&app.review.repo_root)
                        .map(|p| sanitize(&p.display().to_string()))
                        .unwrap_or_default()
                })
                .collect(),
            wrote: targets.iter().position(|t| app.wrote_change(t)),
            pi_install: app.send.pi_offer,
            confirm: app
                .send
                .confirm_pi
                .then(|| super::send::PI_QUESTION.to_string()),
        },
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
                    let plan = approve::plan(&app.review, &app.order, &app.finish_summary(), true);
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
    fn the_queue_is_the_tree_of_the_files_and_the_preview_the_cursors_file() {
        // u0..u2 in f0.rs, u3 and u4 in f1.rs.
        let mut a = app(5);
        a.review.units[3].decision = Decision::Investigate;
        a.review.units[1].prep = Prep::Pending;
        a.changed.insert("u4".into());
        let v = snapshot(&a, None);
        let rows = &v.queue.rows;
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["f0.rs", "f1.rs"]);
        assert!(rows.iter().all(|r| r.kind == RowKind::File && r.depth == 0));
        assert_eq!(
            (rows[0].id.as_str(), rows[0].path.as_str()),
            ("f0.rs", "f0.rs")
        );
        assert!(rows[0].selected && !rows[1].selected);
        assert_eq!(rows[0].level, Some(Level::Routine));
        let (f0, f1) = (rows[0].file.unwrap(), rows[1].file.unwrap());
        assert!(f0.preparing && !f0.flagged && !f0.done && !f0.changed);
        assert!(f1.flagged && f1.changed && !f1.preparing && !f1.done);
        assert_eq!(v.bar.pending, 4);
        // The preview: the file's units, in line order.
        let p = v.preview.expect("the file under the cursor");
        assert_eq!(p.path, "f0.rs");
        let ids: Vec<&str> = p.cards.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["u0", "u1", "u2"]);
        assert!(v.unit.is_none());
    }

    #[test]
    fn directories_are_rows_with_the_level_under_them_and_no_preview() {
        let mut a = app(4);
        for (u, file) in
            a.review
                .units
                .iter_mut()
                .zip(["src/tui/a.rs", "src/tui/a.rs", "src/b.rs", "c.rs"])
        {
            u.file = file.into();
        }
        a.review.skipped = vec![crate::git::SkippedFile {
            path: "src/logo.png".into(),
            status: crate::git::FileStatus::Added,
            why: "binary".into(),
        }];
        a.rebuild_order();
        a.levels[0].level = Level::Attention;
        let v = snapshot(&a, None);
        let rows: Vec<(RowKind, usize, &str, &str)> = v
            .queue
            .rows
            .iter()
            .map(|r| (r.kind, r.depth, r.name.as_str(), r.id.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                (RowKind::Dir, 0, "src", "src/"),
                (RowKind::Dir, 1, "tui", "src/tui/"),
                (RowKind::File, 2, "a.rs", "src/tui/a.rs"),
                (RowKind::File, 1, "b.rs", "src/b.rs"),
                (RowKind::Skipped, 1, "logo.png", "src/logo.png"),
                (RowKind::File, 0, "c.rs", "c.rs"),
            ]
        );
        let rows = &v.queue.rows;
        assert_eq!(rows[0].level, Some(Level::Attention), "the most under it");
        assert_eq!(rows[3].level, Some(Level::Routine));
        assert!(rows[0].file.is_none() && rows[4].file.is_none());
        assert_eq!(
            (rows[4].level, rows[4].why.as_deref()),
            (None, Some("binary"))
        );
        // A click on a directory selects and folds it: no preview.
        a.click_row("src/");
        let v = snapshot(&a, None);
        let names: Vec<&str> = v.queue.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["src", "c.rs"]);
        assert!(v.queue.rows[0].folded && v.queue.rows[0].selected);
        assert!(v.preview.is_none());
        // Visual mode marks its rows.
        press(&mut a, "Vj");
        let v = snapshot(&a, None);
        assert!(v.queue.rows.iter().all(|r| r.in_visual));
        assert_eq!(v.bar.visual, Some(2));
    }

    #[test]
    fn deciding_shows_in_the_rows_and_the_counts() {
        let mut a = app(4);
        a.sel = 0;
        press(&mut a, "a");
        let v = snapshot(&a, None);
        let rows = &v.queue.rows;
        assert!(rows[0].file.unwrap().done && !rows[0].selected);
        assert!(!rows[1].file.unwrap().done && rows[1].selected);
        assert_eq!((v.bar.accepted, v.bar.pending), (3, 1));
        assert_eq!(
            v.actions["decide.accept"].what,
            "accept the file's pending units"
        );
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
        assert_eq!(json["queue"]["rows"][0]["kind"], "File");
        assert_eq!(json["preview"]["path"], "f0.rs");
    }
}
