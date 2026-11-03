//! The `:w` sheet (v2 Phase 6): one default next step for the decisions
//! made, its alternatives, and the notes. It replaces the all-reviewed
//! popup, the commit and post confirmations, and the notes view's ways out
//! of teer, on every screen.
//!
//! | Reviewing | State | Default step (`⏎`) | Alternatives (`j`/`k`) |
//! | --- | --- | --- | --- |
//! | your changes | something flagged | send the notes to your agent | yank the notes, commit anyway |
//! | your changes | all accepted | commit (the agent drafts the message) | yank the notes |
//! | a branch | something flagged | send the notes to your agent | yank the notes |
//! | a branch | all accepted | done (leave teer) | yank the notes |
//! | a pull request | something flagged | request changes (notes inline, a summary) | comment only, yank the notes |
//! | a pull request | all accepted | approve | approve with a comment, yank the notes |
//!
//! With units still pending, the sheet says how many first, "keep
//! reviewing" is the default step, and the others ask before they run.
//! A commit or a post that went through ends teer with one line saying so.

use std::cell::Cell;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};

use super::app::{App, DraftState, Effect, Overlay, Screen, typed};
use super::ui::{self, fixed};
use crate::approve::{self, EditOutcome, Verdict};
use crate::commit::{self, Branch};
use crate::config::Provider;
use crate::keymap::{Action, Edit, Sheet};
use crate::review::model::{Decision, PrInfo, Side, Unit};
use crate::util::{sanitize, truncate, wrap};

/// The actions the sheet handles, for the help. `⏎` (the step) and `esc`
/// (close) are the sheet's own keys, as on every sheet.
pub const ACTIONS: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::MoveNextPending,
    Action::NoteEdit,
    Action::FinishYank,
    Action::ScrollHalfDown,
    Action::ScrollHalfUp,
    Action::ScrollPageDown,
    Action::ScrollPageUp,
    Action::ViewBack,
    Action::ViewClose,
    Action::HelpOpen,
];

/// Most file rows listed for a commit before "and N more".
const FILES_SHOWN: usize = 8;

/// What the review is of, as the sheet tells its rows apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reviewing {
    /// Uncommitted changes (plain `teer` on a dirty tree): they can be
    /// committed.
    Changes,
    /// Commits on a branch, or since `--base`: nothing to commit.
    Branch,
    /// A pull request: the review is posted on GitHub.
    Pr,
}

/// A next step the sheet offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Close the sheet on the first pending unit.
    KeepReviewing,
    /// Hand the notes to Claude Code or Codex; with the model API, copy
    /// them for the reviewer to paste into their agent.
    SendNotes,
    /// Copy the notes as Markdown, each with its diff.
    Yank,
    /// Commit the reviewed files ("commit anyway" with units flagged).
    Commit,
    /// Leave teer: a branch reviewed, everything accepted.
    Done,
    /// Post the notes as inline comments and a summary, requesting changes.
    RequestChanges,
    /// The same as a plain comment, without a verdict.
    CommentOnly,
    Approve,
    /// Approve with a comment the reviewer writes first.
    ApproveWithComment,
}

impl Step {
    /// What a step posts on the PR, for the steps that post.
    pub fn verdict(self) -> Option<Verdict> {
        match self {
            Step::RequestChanges => Some(Verdict::RequestChanges),
            Step::CommentOnly => Some(Verdict::Comment),
            Step::Approve | Step::ApproveWithComment => Some(Verdict::Approve),
            _ => None,
        }
    }

    /// The verb on the `⏎` hint.
    fn verb(self, api: bool) -> &'static str {
        match self {
            Step::KeepReviewing => "keep reviewing",
            Step::SendNotes if api => "copy",
            Step::SendNotes => "send",
            Step::Yank => "yank",
            Step::Commit => "commit",
            Step::Done => "done",
            Step::RequestChanges => "request changes",
            Step::CommentOnly => "comment",
            Step::Approve | Step::ApproveWithComment => "approve",
        }
    }
}

/// What the sheet keeps while teer runs, open or not: the commit message
/// and the review summary as written so far, a question it is asking, and
/// the line teer leaves with.
#[derive(Debug, Default)]
pub struct FinishState {
    /// The commit message: the template, the agent's draft or the user's.
    pub message: String,
    /// Where `message` came from; `None` until a commit was offered.
    pub draft: Option<DraftState>,
    /// The review summary the user wrote for the PR; `None` posts teer's
    /// first line ([`approve::intro_for`]).
    pub summary: Option<String>,
    /// The one-line editor for the commit subject or the summary, when the
    /// configured editor can't start.
    pub editing: Option<String>,
    /// A step waiting for a yes, since units are still pending.
    pub confirm: Option<Step>,
    /// First line shown of the sheet's body: the step's details, the notes.
    pub scroll: usize,
    /// The last line that can be first, as last drawn.
    pub max_scroll: Cell<usize>,
    /// The branch HEAD is on, read each time the commit is offered.
    pub branch: Option<Branch>,
    /// Changed paths outside the review, read with `branch`.
    pub others: usize,
    /// What the step did, printed once teer has given the terminal back.
    pub exit_line: Option<String>,
}

/// "1 unit", "3 units".
fn units(n: usize) -> String {
    if n == 1 {
        "1 unit".into()
    } else {
        format!("{n} units")
    }
}

impl App {
    /// What the review is of.
    pub fn reviewing(&self) -> Reviewing {
        if self.review.pr.is_some() {
            Reviewing::Pr
        } else if self.review.head == crate::git::WORKTREE {
            Reviewing::Changes
        } else {
            Reviewing::Branch
        }
    }

    /// The reviewed changes can be committed from here: uncommitted
    /// changes (not a branch, not a PR), not committed yet.
    pub fn can_commit(&self) -> bool {
        self.reviewing() == Reviewing::Changes && self.committed.is_none()
    }

    /// A review of the PR can be posted now: nothing is being posted.
    pub fn can_post(&self) -> bool {
        self.review.pr.is_some() && !self.approving
    }

    /// The steps the sheet offers for the decisions made, the default first.
    pub fn finish_steps(&self) -> Vec<Step> {
        let (_, flagged, pending) = self.decision_counts();
        let mut steps = Vec::new();
        if pending > 0 {
            steps.push(Step::KeepReviewing);
        }
        let flagged = flagged > 0;
        steps.extend(match self.reviewing() {
            Reviewing::Pr if flagged => {
                vec![Step::RequestChanges, Step::CommentOnly, Step::Yank]
            }
            Reviewing::Pr => vec![Step::Approve, Step::ApproveWithComment, Step::Yank],
            Reviewing::Changes if flagged && self.can_commit() => {
                vec![Step::SendNotes, Step::Yank, Step::Commit]
            }
            Reviewing::Changes if self.can_commit() => vec![Step::Commit, Step::Yank],
            _ if flagged => vec![Step::SendNotes, Step::Yank],
            _ => vec![Step::Done, Step::Yank],
        });
        steps
    }

    /// The step under the sheet's cursor, while the sheet is open.
    pub fn finish_step(&self) -> Option<Step> {
        let Overlay::Finish { sel } = self.overlay else {
            return None;
        };
        let steps = self.finish_steps();
        steps.get(sel.min(steps.len().saturating_sub(1))).copied()
    }

    /// The step as the sheet lists it.
    pub fn step_label(&self, s: Step) -> String {
        let n = self.review.pr.as_ref().map_or(0, |p| p.number);
        match s {
            Step::KeepReviewing => "keep reviewing".into(),
            Step::SendNotes => match self.cfg.provider() {
                Provider::Api => "copy the notes for your agent and wait for the changes".into(),
                cli => format!("send the notes to {}", cli.title()),
            },
            Step::Yank if self.waits_for_fixes() => {
                "yank the notes and wait for the changes".into()
            }
            Step::Yank => "yank the notes".into(),
            Step::Commit if self.decision_counts().1 > 0 => "commit anyway".into(),
            Step::Commit => "commit".into(),
            Step::Done => "done".into(),
            Step::RequestChanges => format!("request changes on PR #{n}"),
            Step::CommentOnly => "comment only".into(),
            Step::Approve => format!("approve PR #{n}"),
            Step::ApproveWithComment => "approve with a comment".into(),
        }
    }

    /// Open the sheet (`:w`) on its default step. A commit on offer gets its
    /// message: the template, and the agent asked for a better one.
    pub fn open_finish(&mut self) -> Vec<Effect> {
        self.finish.editing = None;
        self.finish.confirm = None;
        self.finish.scroll = 0;
        self.overlay = Overlay::Finish { sel: 0 };
        self.on_step()
    }

    /// The configured editor is about to edit the commit message, not the
    /// review summary.
    pub fn edits_commit_message(&self) -> bool {
        self.finish_step() == Some(Step::Commit)
    }

    /// A key while the sheet is open. The one-line editor and the pending
    /// question take every key; then `⏎` runs the step and `esc` closes
    /// (the sheet's own keys, [`crate::keymap::Keymap::sheet`]), and the
    /// rest are actions ([`App::panel_action`]): `j`/`k` pick a step, `n`
    /// goes to the first pending unit, `i` edits the message or summary,
    /// `y` copies the notes, the scroll keys scroll, `h`/`q` close.
    pub(super) fn finish_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let Overlay::Finish { sel } = self.overlay else {
            return Vec::new();
        };
        let steps = self.finish_steps();
        let last = steps.len().saturating_sub(1);
        let sel = sel.min(last);
        let step = steps.get(sel).copied();
        if let Some(mut text) = self.finish.editing.take() {
            match self.keys.edit(&k) {
                Edit::Cancel => {}
                Edit::Submit => self.line_edited(&text),
                e => {
                    let max = match step {
                        Some(Step::Commit) => commit::SUBJECT_MAX * 2,
                        _ => approve::BODY_MAX_CHARS,
                    };
                    typed(&mut text, e, max);
                    self.finish.editing = Some(text);
                }
            }
            return Vec::new();
        }
        if let Some(asked) = self.finish.confirm.take() {
            if self.keys.confirm(&k) {
                return self.run_step(asked);
            }
            return Vec::new();
        }
        if self.keys.pending().is_none() && self.keys.command_line().is_none() {
            match self.keys.sheet(&k) {
                Some(Sheet::Confirm) => {
                    return step.map(|s| self.ask_step(s)).unwrap_or_default();
                }
                Some(Sheet::Cancel) => {
                    self.overlay = Overlay::None;
                    return Vec::new();
                }
                _ => {}
            }
        }
        let Some(a) = self.panel_action(&k) else {
            return Vec::new();
        };
        let to = match a {
            Action::MoveDown => (sel + 1).min(last),
            Action::MoveUp => sel.saturating_sub(1),
            Action::MoveFirst => 0,
            Action::MoveLast => last,
            Action::MoveNextPending => return self.keep_reviewing(),
            Action::NoteEdit => return self.edit_finish_text(step),
            Action::FinishYank => return self.yank_and_wait(),
            Action::ScrollHalfDown | Action::ScrollPageDown => {
                self.scroll_finish(8);
                sel
            }
            Action::ScrollHalfUp | Action::ScrollPageUp => {
                self.scroll_finish(-8);
                sel
            }
            Action::ViewBack | Action::ViewClose => {
                self.overlay = Overlay::None;
                return Vec::new();
            }
            Action::HelpOpen => {
                self.open_help();
                return Vec::new();
            }
            _ => sel,
        };
        if to == sel {
            return Vec::new();
        }
        self.overlay = Overlay::Finish { sel: to };
        self.finish.scroll = 0;
        self.on_step()
    }

    /// Scroll the sheet's body `by` lines, no further than it last drew.
    fn scroll_finish(&mut self, by: isize) {
        let max = self.finish.max_scroll.get();
        self.finish.scroll = self.finish.scroll.saturating_add_signed(by).min(max);
    }

    /// The step under the cursor changed: a commit on offer gets its message.
    fn on_step(&mut self) -> Vec<Effect> {
        if self.finish_step() == Some(Step::Commit) {
            self.offer_commit()
        } else {
            Vec::new()
        }
    }

    /// The commit's message and where it goes, as git says now: the
    /// template from the accepted units, and once per session the agent's
    /// draft. A message the agent drafted or the user wrote stays.
    fn offer_commit(&mut self) -> Vec<Effect> {
        let repo = &self.review.repo_root;
        self.finish.branch = Some(commit::current_branch(repo));
        self.finish.others = commit::reviewed_paths(&self.review)
            .map(|p| commit::other_changed(repo, &p))
            .unwrap_or(0);
        match self.finish.draft {
            Some(DraftState::Pending | DraftState::Drafted | DraftState::Edited) => Vec::new(),
            // The units accepted since are in it.
            Some(DraftState::Template) => {
                self.finish.message = self.template_message();
                Vec::new()
            }
            None => {
                self.finish.message = self.template_message();
                if self.can_ask() {
                    self.finish.draft = Some(DraftState::Pending);
                    vec![Effect::DraftCommitMessage(Box::new(commit::draft_input(
                        &self.review,
                        &self.order,
                    )))]
                } else {
                    self.finish.draft = Some(DraftState::Template);
                    self.status = format!(
                        "No agent to draft a message; using the template. {} edits it.",
                        self.keys.label(Action::NoteEdit)
                    );
                    Vec::new()
                }
            }
        }
    }

    fn template_message(&self) -> String {
        commit::template_message(
            &commit::file_names(&self.review),
            &commit::accepted_summaries(&self.review, &self.order),
        )
    }

    /// `⏎` on `step`: with units still pending, ask first (keeping on
    /// reviewing doesn't).
    fn ask_step(&mut self, step: Step) -> Vec<Effect> {
        if step != Step::KeepReviewing && self.decision_counts().2 > 0 {
            self.finish.confirm = Some(step);
            return Vec::new();
        }
        self.run_step(step)
    }

    fn run_step(&mut self, step: Step) -> Vec<Effect> {
        match step {
            Step::KeepReviewing => self.keep_reviewing(),
            Step::SendNotes => self.send_notes(),
            Step::Yank => {
                let effects = self.yank_and_wait();
                if !effects.is_empty() {
                    self.overlay = Overlay::None;
                }
                effects
            }
            Step::Commit => self.commit_now(),
            Step::Done => self.finish_done(),
            Step::RequestChanges | Step::CommentOnly | Step::Approve | Step::ApproveWithComment => {
                self.post(step)
            }
        }
    }

    /// Close the sheet on the first pending unit, in list order: in the
    /// unit view when that is open, else in the list.
    fn keep_reviewing(&mut self) -> Vec<Effect> {
        let Some(pos) = self
            .order
            .iter()
            .position(|&i| self.review.units[i].decision == Decision::Pending)
        else {
            self.status = "Nothing pending.".into();
            return Vec::new();
        };
        self.overlay = Overlay::None;
        let id = self.review.units[self.order[pos]].id.clone();
        match self.screen {
            Screen::Unit => self.open_unit(id),
            Screen::Queue => self.move_sel(pos),
        }
    }

    /// `y`: the notes as Markdown on the clipboard, each with its diff.
    pub(super) fn yank_notes(&mut self) -> Vec<Effect> {
        let listed = crate::notes::listed(&self.review);
        if listed.is_empty() {
            self.status = "No notes to copy yet.".into();
            return Vec::new();
        }
        let what = format!("{} as Markdown", crate::notes::count(&listed));
        vec![Effect::Copy {
            text: crate::notes::markdown(&self.review),
            what,
        }]
    }

    /// Copying the notes from the sheet hands them to an agent teer doesn't
    /// run, to fix what they say: your changes or your branch with something
    /// flagged (Phase 7). A pull request's fixes come as a push (`:e`
    /// fetches them).
    pub fn waits_for_fixes(&self) -> bool {
        self.reviewing() != Reviewing::Pr && self.decision_counts().1 > 0
    }

    /// `y` on the sheet, or its "yank" step: copy the notes; when they are
    /// for fixing ([`App::waits_for_fixes`]), close the sheet and wait for
    /// the changes on the queue ([`App::start_waiting`]). teer stays open.
    fn yank_and_wait(&mut self) -> Vec<Effect> {
        let effects = self.yank_notes();
        if !effects.is_empty() && self.waits_for_fixes() {
            self.start_waiting(crate::notes::listed(&self.review).len());
        }
        effects
    }

    /// Send the notes to the agent CLI; the model API has no session to take
    /// them, so they go on the clipboard with what to do with them, and teer
    /// waits for the changes (Q11).
    fn send_notes(&mut self) -> Vec<Effect> {
        let listed = crate::notes::listed(&self.review);
        if listed.is_empty() {
            self.status = format!(
                "No notes to send yet: {} on a unit writes one.",
                self.keys.label(Action::NoteEdit)
            );
            return Vec::new();
        }
        if self.cfg.provider().program().is_none() {
            self.overlay = Overlay::None;
            let n = listed.len();
            let copy = Effect::Copy {
                text: crate::notes::agent_prompt(&self.review),
                what: format!(
                    "{} for your agent (teer can't hand them to the model API)",
                    crate::notes::count(&listed)
                ),
            };
            if self.waits_for_fixes() {
                self.start_waiting(n);
            }
            return vec![copy];
        }
        self.notes_to_agent()
    }

    /// Hand the notes to the agent CLI in the repository, as the first
    /// message of a session: a fork of the review's Claude Code session when
    /// there is one. Never for a pull request: its code is untrusted, and
    /// the session could change files and run commands. When the agent
    /// exits, the review re-runs on its own ([`App::agent_returned`]).
    fn notes_to_agent(&mut self) -> Vec<Effect> {
        if self.review.pr.is_some() {
            return Vec::new();
        }
        let cli = self.cfg.provider();
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

    /// "Done": a branch review needs nothing more; leave teer.
    fn finish_done(&mut self) -> Vec<Effect> {
        self.overlay = Overlay::None;
        let (accepted, _, pending) = self.decision_counts();
        let mut line = format!(
            "Reviewed {}: {} accepted",
            crate::cli::subject(&self.review),
            units(accepted)
        );
        if pending > 0 {
            line.push_str(&format!(", {pending} pending"));
        }
        line.push('.');
        self.finish.exit_line = Some(line);
        vec![Effect::Save, Effect::Quit]
    }

    // ---- Committing -------------------------------------------------------

    /// `⏎` on the commit: check that nothing changed since the review, then
    /// commit. With units flagged or pending (the reviewer chose "anyway")
    /// only the files are checked. A refusal closes the sheet (the reason is
    /// in the status line) and runs no git command that changes anything.
    fn commit_now(&mut self) -> Vec<Effect> {
        if !self.can_commit() {
            self.status = "Nothing to commit from here.".into();
            return Vec::new();
        }
        if self.finish.draft.is_none() {
            self.finish.message = self.template_message();
        }
        // What the user is about to commit is final: a late draft must not replace it.
        self.finish.draft = Some(DraftState::Edited);
        let message = commit::normalize_message(&self.finish.message);
        if message.is_empty() {
            self.status = format!(
                "The commit message is empty. Press {} to write one.",
                self.keys.label(Action::NoteEdit)
            );
            return Vec::new();
        }
        let anyway = self
            .review
            .units
            .iter()
            .any(|u| u.decision != Decision::Accepted);
        let checked = if anyway {
            commit::check_anyway(&self.review)
        } else {
            commit::check(&self.review)
        };
        match checked {
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

    /// git finished (see `Effect::Commit`): a commit ends teer with one line;
    /// a failure stays on the sheet to retry or edit.
    pub fn commit_finished(&mut self, outcome: commit::Outcome) -> Vec<Effect> {
        match outcome {
            commit::Outcome::Committed { short } => {
                let line = format!(
                    "Committed {short}: {}",
                    commit::subject_of(&self.finish.message)
                );
                self.status = line.clone();
                self.finish.exit_line = Some(line);
                self.committed = Some(short);
                self.overlay = Overlay::None;
                vec![Effect::Save, Effect::Quit]
            }
            commit::Outcome::Failed { code, output } => {
                self.status = commit::failure_status(code, &output);
                Vec::new()
            }
            commit::Outcome::Refused(why) => {
                self.status = approve::tidy_output(&why);
                Vec::new()
            }
        }
    }

    /// The agent's draft arrived. It never replaces text the user has touched.
    pub(super) fn commit_drafted(&mut self, r: Result<String, String>) {
        if self.finish.draft != Some(DraftState::Pending) {
            return;
        }
        let open = matches!(self.overlay, Overlay::Finish { .. });
        let say = match r {
            Ok(m) if !commit::normalize_message(&m).is_empty() => {
                self.finish.message = commit::normalize_message(&m);
                self.finish.draft = Some(DraftState::Drafted);
                format!(
                    "Drafted a commit message. {} edits it.",
                    self.keys.label(Action::NoteEdit)
                )
            }
            Ok(_) => {
                self.finish.draft = Some(DraftState::Template);
                "The agent's draft was empty; kept the template message.".into()
            }
            Err(why) => {
                self.finish.draft = Some(DraftState::Template);
                format!(
                    "Couldn't draft a message ({}); kept the template.",
                    approve::tidy_output(&why)
                )
            }
        };
        if open {
            self.status = say;
        }
    }

    // ---- Posting ----------------------------------------------------------

    /// The summary comment as it would be posted: the user's, or teer's
    /// first line.
    pub fn finish_summary(&self) -> String {
        self.finish
            .summary
            .clone()
            .unwrap_or_else(|| approve::intro_for(&self.review))
    }

    /// `⏎` on a post: the notes as inline comments and the summary, with
    /// the step's verdict. "Approve with a comment" opens the editor on the
    /// comment until there is one. The PR head is checked again before
    /// anything is sent (`approve::post`).
    fn post(&mut self, step: Step) -> Vec<Effect> {
        let Some(verdict) = step.verdict() else {
            return Vec::new();
        };
        let Some(pr) = self.review.pr.clone() else {
            return Vec::new();
        };
        if !self.can_post() {
            self.status = format!("{} on GitHub…", verdict.doing(pr.number));
            return Vec::new();
        }
        if pr.remote.is_none() || pr.head_sha.is_none() || pr.repo_dir.is_none() {
            self.status = format!(
                "Can't post to PR #{}: teer doesn't know its remote or head.",
                pr.number
            );
            return Vec::new();
        }
        if verdict == Verdict::Approve && pr.is_approved() {
            self.status = format!("You approved PR #{} at this head already.", pr.number);
            return Vec::new();
        }
        if step == Step::ApproveWithComment && self.finish.summary.is_none() {
            self.status = "Write the comment, then ⏎ approves with it.".into();
            return self.edit_finish_text(Some(step));
        }
        let notes = approve::collect_notes(&self.review, &self.order);
        let intro = self.finish_summary();
        let plan = approve::build_plan(&notes, &intro, true);
        let flat_body = approve::build_plan(&notes, &intro, false).body;
        self.approving = true;
        self.overlay = Overlay::None;
        self.status = format!("{} on GitHub…", verdict.doing(pr.number));
        vec![Effect::PostReview {
            pr: Box::new(pr),
            verdict,
            body: plan.body,
            inline: plan.inline,
            flat_body,
        }]
    }

    /// Posting finished: a post that went through ends teer with one line;
    /// a failure (the PR page was opened where that helps) says why.
    pub(super) fn posted(
        &mut self,
        verdict: Verdict,
        result: Result<String, String>,
    ) -> Vec<Effect> {
        self.approving = false;
        match result {
            Ok(line) => {
                if verdict == Verdict::Approve
                    && let Some(p) = self.review.pr.as_mut()
                {
                    p.approved_head = p.head_sha.clone();
                }
                self.status = line.clone();
                self.finish.exit_line = Some(line);
                vec![Effect::Save, Effect::Quit]
            }
            Err(why) => {
                self.status = why;
                Vec::new()
            }
        }
    }

    // ---- Writing ----------------------------------------------------------

    /// `i`: the commit message or the review summary in the configured
    /// editor (the runtime hands the result to [`App::comment_edited`]).
    fn edit_finish_text(&mut self, step: Option<Step>) -> Vec<Effect> {
        match step {
            Some(Step::Commit) => {
                if self.finish.draft.is_none() {
                    self.finish.message = self.template_message();
                }
                // A late draft must not replace what is being written.
                self.finish.draft = Some(DraftState::Edited);
                vec![Effect::EditComment {
                    text: self.finish.message.clone(),
                }]
            }
            Some(s) if s.verdict().is_some() => vec![Effect::EditComment {
                text: self.finish_summary(),
            }],
            _ => {
                self.status = format!(
                    "Nothing to write for this step: {} edits the commit message or the review summary.",
                    self.keys.label(Action::NoteEdit)
                );
                Vec::new()
            }
        }
    }

    /// The configured editor finished, or could not start: then the one-line
    /// editor takes over.
    pub fn comment_edited(&mut self, outcome: EditOutcome) {
        let step = self.finish_step();
        let commit = step == Some(Step::Commit);
        if !commit && !step.is_some_and(|s| s.verdict().is_some()) {
            return;
        }
        match outcome {
            EditOutcome::Edited(t) if commit => {
                let t = commit::normalize_message(&t);
                if !t.is_empty() {
                    self.finish.message = t;
                }
            }
            EditOutcome::Edited(t) => {
                if !t.trim().is_empty() {
                    self.finish.summary = Some(t);
                }
            }
            EditOutcome::Cancelled => {}
            EditOutcome::Unavailable(why) => {
                let (line, what) = if commit {
                    (
                        commit::subject_of(&self.finish.message).to_string(),
                        "the subject",
                    )
                } else {
                    (self.finish_summary().replace('\n', " "), "the summary")
                };
                self.finish.editing = Some(line);
                self.status = format!(
                    "Couldn't start the editor ({}); editing {what} on one line.",
                    approve::tidy_output(&why)
                );
            }
        }
    }

    /// `⏎` in the one-line editor: the commit's new subject (its body
    /// kept), or the new summary. Blank changes nothing.
    fn line_edited(&mut self, text: &str) {
        if self.finish_step() == Some(Step::Commit) {
            let line: String = text.chars().filter(|c| !c.is_control()).collect();
            let line = line.trim();
            if !line.is_empty() {
                let new = match self.finish.message.split_once("\n\n") {
                    Some((_, body)) => format!("{line}\n\n{body}"),
                    None => line.to_string(),
                };
                self.finish.message = commit::normalize_message(&new);
            }
            self.finish.draft = Some(DraftState::Edited);
            return;
        }
        let t = approve::clamp_text(&text.replace('\n', " "), approve::BODY_MAX_CHARS);
        if !t.trim().is_empty() {
            self.finish.summary = Some(t.trim().to_string());
        }
    }
}

// ---- Drawing ----------------------------------------------------------------

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// The sheet over the screen: what is left and the steps on top, the
/// selected step's details and the notes below them (scrolling), the keys
/// or the pending question at the bottom.
pub fn draw(f: &mut Frame, app: &App) {
    let Overlay::Finish { sel } = app.overlay else {
        return;
    };
    let t = &app.theme;
    let area = f.area();
    let box_w = area.width.saturating_sub(2).min(100);
    // Borders and padding take 4 columns.
    let w = box_w.saturating_sub(4) as usize;
    if box_w < 16 || area.height < 6 {
        f.render_widget(Clear, centered(area, box_w, area.height));
        return;
    }
    let steps = app.finish_steps();
    let sel = sel.min(steps.len().saturating_sub(1));
    let step = steps.get(sel).copied();
    let (accepted, flagged, pending) = app.decision_counts();

    let mut head: Vec<Line> = Vec::new();
    let what = if pending > 0 {
        let warn = Style::new().fg(t.risk_medium).add_modifier(Modifier::BOLD);
        Line::styled(
            format!(
                "{} pending · {} goes to the first",
                units(pending),
                app.keys.label(Action::MoveNextPending)
            ),
            warn,
        )
    } else if flagged > 0 {
        Line::styled(
            format!("{flagged} flagged · {accepted} accepted"),
            t.muted(),
        )
    } else {
        Line::styled(format!("All {} accepted.", units(accepted)), t.ok())
    };
    head.push(what);
    // Never claim more than was seen: the files teer couldn't show.
    let name = |f: &crate::git::SkippedFile| {
        f.path
            .file_name()
            .map(|n| sanitize(&n.to_string_lossy()))
            .unwrap_or_default()
    };
    if let Some(note) = app.review.not_shown(name) {
        for l in wrap(&format!("{note}."), w).into_iter().take(2) {
            head.push(Line::styled(l, Style::new().fg(t.risk_medium)));
        }
    }
    head.push(Line::default());
    for (i, s) in steps.iter().enumerate() {
        let label = truncate(&app.step_label(*s), w.saturating_sub(2));
        head.push(if i == sel {
            Line::from(vec![
                Span::styled("▌ ", t.accent()),
                Span::styled(label, t.accent().add_modifier(Modifier::BOLD)),
            ])
        } else {
            Line::from(vec![Span::raw("  "), Span::styled(label, t.text())])
        });
    }

    let mut body: Vec<Line> = vec![Line::default()];
    body.extend(details(app, step, w));
    body.push(Line::default());
    body.extend(notes(app, w));

    let api = app.cfg.provider() == Provider::Api;
    let (foot, plain) = if let Some(asked) = app.finish.confirm {
        let label = capitalized(&app.step_label(asked));
        let anyway = if label.ends_with("anyway") {
            ""
        } else {
            " anyway"
        };
        let q = format!("{} still pending. {label}{anyway}?", units(pending));
        let (keys, plain) = ui::hint_line(
            app,
            &[
                ui::hint(format!("{}/{}", fixed::YES, fixed::ENTER), "yes"),
                ui::hint(fixed::ESC, "no"),
            ],
        );
        let mut lines: Vec<Line> = wrap(&q, w)
            .into_iter()
            .map(|l| Line::styled(l, t.bold()))
            .collect();
        lines.push(keys);
        (lines, plain)
    } else if app.finish.editing.is_some() {
        let (keys, plain) = ui::hint_line(
            app,
            &[ui::hint(fixed::ENTER, "save"), ui::hint(fixed::ESC, "back")],
        );
        (vec![keys], plain)
    } else {
        let k = |a: Action| app.keys.label(a);
        let mut keys = Vec::new();
        if let Some(s) = step {
            keys.push(ui::hint(fixed::ENTER, s.verb(api)));
        }
        if steps.len() > 1 {
            keys.push(ui::hint(
                format!("{} {}", k(Action::MoveDown), k(Action::MoveUp)),
                "pick",
            ));
        }
        match step {
            Some(Step::Commit) => keys.push(ui::hint(k(Action::NoteEdit), "edit message")),
            Some(s) if s.verdict().is_some() => {
                keys.push(ui::hint(k(Action::NoteEdit), "edit summary"))
            }
            _ => {}
        }
        let copy = if app.waits_for_fixes() {
            "copy notes, wait"
        } else {
            "copy notes"
        };
        keys.push(ui::hint(k(Action::FinishYank), copy));
        if pending > 0 {
            keys.push(ui::hint(k(Action::MoveNextPending), "first pending"));
        }
        keys.push(ui::hint(fixed::ESC, "close"));
        let (line, plain) = ui::hint_line(app, &keys);
        (vec![line], plain)
    };
    let foot_h = foot
        .iter()
        .map(|l| wrap(&line_text(l), w).len().max(1))
        .sum::<usize>()
        .clamp(1, 4) as u16;

    let head_h = head.len() as u16;
    let want = (head_h as usize + body.len() + foot_h as usize + 2).clamp(6, 40) as u16;
    let r = centered(area, box_w, want.min(area.height.saturating_sub(2).max(6)));
    f.render_widget(Clear, r);
    let title = format!(
        " Finish · {} ",
        truncate(&crate::cli::subject(&app.review), w.saturating_sub(12))
    );
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.accent())
        .title(Span::styled(title, t.accent().add_modifier(Modifier::BOLD)))
        .padding(Padding::new(1, 1, 0, 0));
    let inner = block.inner(r);
    f.render_widget(block, r);
    // The steps first, the keys last; the body gets the rest and scrolls.
    let head_h = head_h.min(inner.height.saturating_sub(foot_h));
    let rows = inner.height.saturating_sub(head_h + foot_h) as usize;
    let max = body.len().saturating_sub(rows);
    app.finish.max_scroll.set(max);
    let start = app.finish.scroll.min(max);
    let shown: Vec<Line> = body.into_iter().skip(start).take(rows).collect();
    let mut y = inner.y;
    f.render_widget(
        Paragraph::new(head),
        Rect::new(inner.x, y, inner.width, head_h),
    );
    y += head_h;
    f.render_widget(
        Paragraph::new(shown),
        Rect::new(inner.x, y, inner.width, rows as u16),
    );
    y += rows as u16;
    let foot_area = Rect::new(inner.x, y, inner.width, foot_h.min(inner.height));
    f.render_widget(Paragraph::new(foot).wrap(Wrap { trim: true }), foot_area);
    ui::text_area(app, inner, 0);
    ui::click_hints(f, app, foot_area, &plain);
}

/// A line's text without its styles.
fn line_text(l: &Line) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// `s` with its first letter upper case.
fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// What the selected step does, in a few lines: the commit's files and
/// message, the post's summary and inline comments, where the notes go.
fn details(app: &App, step: Option<Step>, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let k = |a: Action| app.keys.label(a);
    let say = |text: String| -> Vec<Line<'static>> {
        wrap(&text, w)
            .into_iter()
            .map(|l| Line::styled(l, t.muted()))
            .collect()
    };
    // What waiting for copied notes' changes does.
    let wait = || {
        format!(
            "Then teer waits, the queue on show: once the files have changed and stayed the \
same for {} s, it re-runs the review on what changed. {} stops waiting; {} re-runs later.",
            app.cfg.fix_loop.quiet_seconds,
            k(Action::ViewClose),
            k(Action::ReviewRerun)
        )
    };
    match step {
        None => Vec::new(),
        Some(Step::KeepReviewing) => say(format!(
            "Goes to the first pending unit. {} comes back here.",
            k(Action::FinishOpen)
        )),
        Some(Step::SendNotes) => match app.cfg.provider() {
            Provider::Api => say(format!(
                "teer can't hand notes to the model API: this copies them, with what to do, \
for you to paste into your agent. {}",
                wait()
            )),
            cli => {
                let session = app
                    .review
                    .agent_session
                    .as_ref()
                    .filter(|s| !s.is_fresh() && cli == Provider::Claude)
                    .map(|s| {
                        let id: String = s.id.chars().take(8).collect();
                        format!(", in a fork of session {}", sanitize(&id))
                    })
                    .unwrap_or_default();
                say(format!(
                    "Opens {} with the notes as its first message{session}. When you quit it, \
teer re-runs the review on what it changed.",
                    cli.title(),
                ))
            }
        },
        Some(Step::Yank) if app.waits_for_fixes() => say(format!(
            "Copies the notes as Markdown, each with its diff, for your agent. {}",
            wait()
        )),
        Some(Step::Yank) => say("Copies the notes as Markdown, each with its diff.".into()),
        Some(Step::Done) => say(
            "Everything is accepted, and a branch has nothing to commit. Leaves teer; the \
review is saved."
                .into(),
        ),
        Some(Step::Commit) => commit_details(app, w),
        Some(s) => post_details(app, s, w),
    }
}

/// The one-line editor: `label`, then the text wrapped with the cursor.
fn editor_lines(app: &App, label: &str, text: &str, w: usize) -> Vec<Line<'static>> {
    let mut out = vec![Line::styled(label.to_string(), app.theme.bold())];
    let mut lines = wrap(&sanitize(text), w.saturating_sub(1));
    if lines.is_empty() {
        lines.push(String::new());
    }
    let last = lines.len() - 1;
    for (i, l) in lines.into_iter().enumerate() {
        let mut sp = vec![Span::raw(l)];
        if i == last {
            sp.push(Span::styled(
                " ",
                Style::new().add_modifier(Modifier::REVERSED),
            ));
        }
        out.push(Line::from(sp));
    }
    out
}

/// The commit: the files and the branch, the changes left out, the message.
fn commit_details(app: &App, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let rows = commit::file_rows(&app.review);
    let n = rows.len();
    let plural = if n == 1 { "" } else { "s" };
    let what = match &app.finish.branch {
        Some(Branch::Named(b)) => format!("Commits {n} file{plural} on {}:", sanitize(b)),
        Some(Branch::Detached) => format!("Commits {n} file{plural} on a detached HEAD:"),
        _ => format!("Commits {n} file{plural}:"),
    };
    let mut out: Vec<Line> = wrap(&what, w)
        .into_iter()
        .map(|l| Line::styled(l, t.accent()))
        .collect();
    for (sym, path) in rows.iter().take(FILES_SHOWN) {
        let color = match sym {
            '+' => t.added,
            '−' => t.removed,
            _ => t.changed,
        };
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(sym.to_string(), Style::new().fg(color)),
            Span::raw(format!(" {}", truncate(path, w.saturating_sub(4)))),
        ]));
    }
    if n > FILES_SHOWN {
        out.push(Line::styled(
            format!("  … and {} more", n - FILES_SHOWN),
            t.muted(),
        ));
    }
    let others = app.finish.others;
    if others > 0 {
        let text = if others == 1 {
            "1 other changed file is not part of this review and will not be included.".to_string()
        } else {
            format!(
                "{others} other changed files are not part of this review and will not be included."
            )
        };
        for l in wrap(&text, w) {
            out.push(Line::styled(l, t.muted()));
        }
    }
    out.push(Line::default());
    if let Some(text) = &app.finish.editing {
        out.extend(editor_lines(app, "Edit the subject (one line):", text, w));
        return out;
    }
    let mut label = vec![Span::styled("Message", t.accent())];
    match app.finish.draft {
        Some(DraftState::Pending) => label.push(Span::styled(
            format!("  {} drafting…", app.spinner()),
            t.muted(),
        )),
        Some(DraftState::Drafted) => label.push(Span::styled("  drafted by the agent", t.muted())),
        Some(DraftState::Template) => label.push(Span::styled("  template", t.muted())),
        Some(DraftState::Edited) | None => {}
    }
    out.push(Line::from(label));
    for l in wrap(&app.finish.message, w.saturating_sub(2)) {
        out.push(Line::from(format!("  {l}")));
    }
    out
}

/// The post: that the head is checked, the summary comment, the inline
/// comments.
fn post_details(app: &App, step: Step, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let Some(pr) = app.review.pr.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<Line> = wrap(&head_check(pr), w)
        .into_iter()
        .map(|l| Line::styled(l, t.muted()))
        .collect();
    out.push(Line::default());
    if let Some(text) = &app.finish.editing {
        out.extend(editor_lines(app, "Edit the summary (one line):", text, w));
        return out;
    }
    if step == Step::ApproveWithComment && app.finish.summary.is_none() {
        out.push(Line::styled(
            format!(
                "Write the comment first: {} or ⏎ opens the editor.",
                app.keys.label(Action::NoteEdit)
            ),
            t.accent(),
        ));
        return out;
    }
    let notes = approve::collect_notes(&app.review, &app.order);
    let plan = approve::build_plan(&notes, &app.finish_summary(), true);
    out.push(Line::styled("Summary comment", t.accent()));
    for l in wrap(&plan.body, w.saturating_sub(2)) {
        out.push(Line::from(format!("  {l}")));
    }
    out.push(Line::default());
    if plan.inline.is_empty() {
        out.push(Line::styled("No inline comments.", t.muted()));
        return out;
    }
    out.push(Line::styled(
        format!(
            "{} inline comment{}",
            plan.inline.len(),
            if plan.inline.len() == 1 { "" } else { "s" }
        ),
        t.accent(),
    ));
    for c in &plan.inline {
        let side = if c.side == Side::Old { " (old)" } else { "" };
        let text = format!("{}:{}{side}  {}", sanitize(&c.path), c.line, c.body);
        for (i, l) in wrap(&text, w.saturating_sub(4)).into_iter().enumerate() {
            out.push(Line::from(format!(
                "{}{l}",
                if i == 0 { "  " } else { "    " }
            )));
        }
    }
    out
}

/// "Checks PR #7 (owner/repo) is still at abc1234, then posts on GitHub."
fn head_check(pr: &PrInfo) -> String {
    let head: String = pr
        .head_sha
        .as_deref()
        .unwrap_or_default()
        .chars()
        .take(7)
        .collect();
    format!(
        "Checks PR #{} ({}/{}) is still at {}, the commit you reviewed, then posts on GitHub.",
        pr.number,
        sanitize(&pr.owner),
        sanitize(&pr.repo),
        sanitize(&head)
    )
}

/// The notes overview: every flagged or noted unit, its level and place,
/// and its note.
fn notes(app: &App, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let listed = crate::notes::listed(&app.review);
    if listed.is_empty() {
        return vec![Line::styled("No notes.", t.muted())];
    }
    let mut out = vec![Line::styled(
        format!("Notes · {}", crate::notes::count(&listed)),
        t.accent(),
    )];
    for u in listed {
        out.extend(note_lines(app, u, w));
    }
    out
}

/// One unit of the notes overview.
fn note_lines(app: &App, u: &Unit, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let level = app
        .assessment(&u.id)
        .map(|a| a.level.shown(u.auto_accepted()))
        .unwrap_or(crate::review::attention::Level::Routine);
    let place = crate::notes::place(u);
    let state = match u.decision {
        Decision::Investigate => "flagged",
        Decision::Accepted => "accepted",
        Decision::Pending => "pending",
    };
    let name_w = w.saturating_sub(4 + place.chars().count().min(w / 2) + state.len() + 4);
    let mut out = vec![Line::from(vec![
        Span::styled(
            format!("{} ", level.icon()),
            super::slim::level_style(app, level),
        ),
        Span::styled(truncate(&sanitize(&u.symbol), name_w), t.bold()),
        Span::styled(format!("  {}", truncate(&place, w / 2)), t.muted()),
        Span::styled(format!(" · {state}"), t.muted()),
    ])];
    let note = u.dive.note.trim();
    if note.is_empty() {
        out.push(Line::styled("    no note", t.muted()));
    } else {
        for l in wrap(&sanitize(note), w.saturating_sub(4)) {
            out.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(l, t.text()),
            ]));
        }
    }
    if u.note_is_stale() {
        out.push(Line::styled(
            "    the code changed since this note",
            t.muted(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::config::Config;
    use crate::keymap::Keymap;
    use crate::review::model::{Prep, Review, SessionRef, UnitKind, UnitStatus};

    use Decision::{Accepted as Acc, Investigate as Flag, Pending};

    fn unit(i: usize, d: Decision) -> Unit {
        Unit {
            id: format!("u{i}"),
            file: PathBuf::from(format!("f{i}.rs")),
            symbol: format!("s{i}"),
            kind: UnitKind::Function,
            status: UnitStatus::Changed,
            old_range: None,
            new_range: None,
            renamed_from: None,
            fingerprint: format!("fp{i}"),
            changes: Vec::new(),
            prep: Prep::Skipped,
            callers: Vec::new(),
            callees: Vec::new(),
            decision: d,
            summary: None,
            nbc_reason: None,
            dive: Default::default(),
            accepted_by: None,
            reasons: Vec::new(),
            dive_pending: Default::default(),
            checks: Default::default(),
        }
    }

    const NOWHERE: &str = "/nowhere/teer-finish";

    fn pr() -> PrInfo {
        PrInfo {
            host: "github.com".into(),
            port: None,
            base_assumed: None,
            owner: "o".into(),
            repo: "r".into(),
            number: 7,
            remote: Some("origin".into()),
            head_sha: Some("a".repeat(40)),
            approved_head: None,
            repo_dir: Some(PathBuf::from(NOWHERE)),
        }
    }

    /// A review whose units are `d`: of the uncommitted changes when
    /// `head` is the working tree, else of a branch, or of `pr`; in a
    /// repository that doesn't exist, so nothing is read from one.
    fn app(d: &[Decision], head: &str, pr: Option<PrInfo>) -> App {
        let review = Review {
            id: "t".into(),
            repo_root: PathBuf::from(NOWHERE),
            base: "base".into(),
            head: head.into(),
            head_commit: "c".into(),
            agent_session: None,
            pr,
            intent: Default::default(),
            created_at: chrono::Utc::now(),
            files: Vec::new(),
            units: d.iter().enumerate().map(|(i, d)| unit(i, *d)).collect(),
            skipped: Vec::new(),
            graph: Default::default(),
            questions: Vec::new(),
            version: 0,
            history: Default::default(),
            branch: None,
            cursor: None,
        };
        App::new(review, Config::default(), Keymap::default())
    }

    fn changes(d: &[Decision]) -> App {
        app(d, crate::git::WORKTREE, None)
    }

    fn branch(d: &[Decision]) -> App {
        app(d, "feature", None)
    }

    fn pull(d: &[Decision]) -> App {
        app(d, "feature", Some(pr()))
    }

    fn press(a: &mut App, keys: &str) -> Vec<Effect> {
        crate::keymap::parse_keys(keys)
            .unwrap()
            .into_iter()
            .flat_map(|k| a.on_key(k.event()))
            .collect()
    }

    fn open(a: &mut App) -> Vec<Effect> {
        press(a, ":w<cr>")
    }

    fn render(a: &App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| super::super::ui::draw(f, a)).unwrap();
        let buf = t.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn each_case_has_one_default_and_its_alternatives() {
        use Step::*;
        let cases = [
            (changes(&[Acc, Flag]), vec![SendNotes, Yank, Commit]),
            (changes(&[Acc, Acc]), vec![Commit, Yank]),
            (branch(&[Acc, Flag]), vec![SendNotes, Yank]),
            (branch(&[Acc, Acc]), vec![Done, Yank]),
            (pull(&[Acc, Flag]), vec![RequestChanges, CommentOnly, Yank]),
            (pull(&[Acc, Acc]), vec![Approve, ApproveWithComment, Yank]),
        ];
        for (mut a, want) in cases {
            assert_eq!(a.finish_steps(), want);
            // Something still pending: keeping on reviewing comes first.
            a.review.units.push(unit(9, Pending));
            let mut with = vec![KeepReviewing];
            with.extend(want);
            assert_eq!(a.finish_steps(), with);
        }
    }

    #[test]
    fn commit_anyway_and_the_api_say_what_they_do() {
        let mut a = changes(&[Acc, Flag]);
        assert_eq!(a.step_label(Step::Commit), "commit anyway");
        assert_eq!(
            a.step_label(Step::SendNotes),
            "send the notes to Claude Code"
        );
        a.cfg.provider = Some(Provider::Api);
        assert_eq!(
            a.step_label(Step::SendNotes),
            "copy the notes for your agent and wait for the changes"
        );
        assert_eq!(
            a.step_label(Step::Yank),
            "yank the notes and wait for the changes"
        );
        assert_eq!(changes(&[Acc]).step_label(Step::Commit), "commit");
        // Nothing flagged: the notes aren't for fixing, nothing waits.
        assert_eq!(changes(&[Acc]).step_label(Step::Yank), "yank the notes");
    }

    #[test]
    fn j_and_k_pick_a_step_and_esc_or_q_close_without_doing_anything() {
        let mut a = changes(&[Acc, Flag]);
        assert!(open(&mut a).is_empty());
        assert_eq!(a.finish_step(), Some(Step::SendNotes));
        press(&mut a, "j");
        assert_eq!(a.finish_step(), Some(Step::Yank));
        press(&mut a, "jj");
        assert_eq!(a.finish_step(), Some(Step::Commit));
        press(&mut a, "k");
        assert_eq!(a.finish_step(), Some(Step::Yank));
        for close in ["<esc>", "q", "h", ":q<cr>"] {
            open(&mut a);
            assert!(press(&mut a, close).is_empty(), "{close}");
            assert_eq!(a.overlay, Overlay::None, "{close}");
        }
    }

    #[test]
    fn pending_units_come_first_and_n_goes_to_the_first() {
        let mut a = changes(&[Acc, Pending, Pending]);
        a.sel = 0;
        open(&mut a);
        let screen = render(&a, 90, 30);
        assert!(screen.contains("2 units pending"), "{screen}");
        // Only the cursor moved: only it is saved.
        assert_eq!(press(&mut a, "n"), vec![Effect::SaveCursor]);
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.selected_id().as_deref(), Some("u1"));
        // ⏎ on the default does the same.
        a.sel = 0;
        open(&mut a);
        press(&mut a, "<cr>");
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.selected_id().as_deref(), Some("u1"));
    }

    #[test]
    fn other_steps_ask_first_while_units_are_pending() {
        let mut a = changes(&[Flag, Pending]);
        a.review.units[0].dive.note = "why?".into();
        open(&mut a);
        press(&mut a, "jj");
        assert_eq!(a.finish_step(), Some(Step::Yank));
        // No: nothing happens, the sheet stays.
        assert!(press(&mut a, "<cr>").is_empty());
        assert_eq!(a.finish.confirm, Some(Step::Yank));
        let screen = render(&a, 90, 30);
        assert!(
            screen
                .contains("1 unit still pending. Yank the notes and wait for the changes anyway?"),
            "{screen}"
        );
        assert!(press(&mut a, "x").is_empty());
        assert_eq!(a.finish.confirm, None);
        assert!(matches!(a.overlay, Overlay::Finish { .. }));
        // Yes: it runs.
        press(&mut a, "<cr>");
        let e = press(&mut a, "y");
        assert!(matches!(&e[..], [Effect::Copy { .. }]), "{e:?}");
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn y_copies_every_note_as_markdown() {
        let mut a = changes(&[Flag, Acc, Flag]);
        a.review.units[0].dive.note = "why 2?".into();
        a.review.units[1].dive.note = "fine, but rename".into();
        open(&mut a);
        let e = press(&mut a, "y");
        let [Effect::Copy { text, what }] = &e[..] else {
            panic!("{e:?}");
        };
        assert_eq!(*text, crate::notes::markdown(&a.review));
        assert_eq!(what, "2 notes and 1 flagged unit without one as Markdown");
        // Notes to fix: the sheet closes and teer waits for the changes.
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.waiting.as_ref().map(|w| w.notes), Some(3));
        // Nothing flagged: `y` copies and leaves the sheet open.
        let mut c = changes(&[Acc]);
        c.review.units[0].dive.note = "nice".into();
        open(&mut c);
        assert!(matches!(&press(&mut c, "y")[..], [Effect::Copy { .. }]));
        assert!(matches!(c.overlay, Overlay::Finish { .. }));
        assert!(c.waiting.is_none());
        // Nothing to copy says so.
        let mut b = changes(&[Acc]);
        open(&mut b);
        assert!(press(&mut b, "y").is_empty());
        assert_eq!(b.status, "No notes to copy yet.");
    }

    #[test]
    fn sending_hands_the_notes_to_the_cli_or_copies_them_for_the_api() {
        let mut a = changes(&[Flag, Acc]);
        a.review.units[0].dive.note = "check the bound".into();
        open(&mut a);
        let e = press(&mut a, "<cr>");
        let [Effect::OpenAgent { cli, prompt, .. }] = &e[..] else {
            panic!("{e:?}");
        };
        assert_eq!(*cli, Provider::Claude);
        assert_eq!(*prompt, crate::notes::agent_prompt(&a.review));
        assert_eq!(a.overlay, Overlay::None);

        a.cfg.provider = Some(Provider::Api);
        open(&mut a);
        let e = press(&mut a, "<cr>");
        let [Effect::Copy { text, what }] = &e[..] else {
            panic!("{e:?}");
        };
        assert_eq!(*text, crate::notes::agent_prompt(&a.review));
        assert!(what.contains("can't hand them to the model API"), "{what}");
    }

    #[test]
    fn a_branch_with_everything_accepted_is_done() {
        let mut a = branch(&[Acc, Acc]);
        open(&mut a);
        assert_eq!(press(&mut a, "<cr>"), vec![Effect::Save, Effect::Quit]);
        let line = a.finish.exit_line.clone().unwrap();
        assert_eq!(line, "Reviewed since base: 2 units accepted.");
    }

    #[test]
    fn i_edits_the_commit_message_in_the_editor_or_on_one_line() {
        let mut a = changes(&[Acc]);
        open(&mut a);
        assert_eq!(a.finish.draft, Some(DraftState::Template));
        let template = a.finish.message.clone();
        assert!(a.edits_commit_message());
        assert_eq!(
            press(&mut a, "i"),
            vec![Effect::EditComment { text: template }]
        );
        a.comment_edited(EditOutcome::Edited("Fix the bound\n\n- why".into()));
        assert_eq!(a.finish.message, "Fix the bound\n\n- why");
        assert_eq!(a.finish.draft, Some(DraftState::Edited));
        // No editor: the subject on one line, the body kept.
        press(&mut a, "i");
        a.comment_edited(EditOutcome::Unavailable("no editor".into()));
        assert_eq!(a.finish.editing.as_deref(), Some("Fix the bound"));
        press(&mut a, "<C-u>Better<cr>");
        assert_eq!(a.finish.message, "Better\n\n- why");
        assert_eq!(a.finish.editing, None);
    }

    #[test]
    fn i_edits_the_review_summary_when_posting() {
        let mut a = pull(&[Acc, Flag]);
        open(&mut a);
        assert!(!a.edits_commit_message());
        assert_eq!(
            press(&mut a, "i"),
            vec![Effect::EditComment {
                text: approve::DEFAULT_INTRO.into()
            }]
        );
        a.comment_edited(EditOutcome::Edited("Two things.".into()));
        assert_eq!(a.finish_summary(), "Two things.");
        a.comment_edited(EditOutcome::Unavailable("no editor".into()));
        assert_eq!(a.finish.editing.as_deref(), Some("Two things."));
        press(&mut a, "<C-u>One thing.<cr>");
        assert_eq!(a.finish_summary(), "One thing.");
        // Steps with nothing to write say so.
        let mut b = changes(&[Flag]);
        open(&mut b);
        assert!(press(&mut b, "i").is_empty());
        assert!(b.status.starts_with("Nothing to write"), "{}", b.status);
    }

    #[test]
    fn a_late_draft_never_replaces_an_edited_message() {
        let mut a = changes(&[Acc]);
        a.has_agent = true;
        a.review.agent_session = Some(SessionRef::fresh("claude", PathBuf::from(NOWHERE)));
        let e = open(&mut a);
        assert!(matches!(&e[..], [Effect::DraftCommitMessage(_)]), "{e:?}");
        assert_eq!(a.finish.draft, Some(DraftState::Pending));
        a.commit_drafted(Ok("Drafted subject\n\nbody".into()));
        assert_eq!(a.finish.message, "Drafted subject\n\nbody");
        assert_eq!(a.finish.draft, Some(DraftState::Drafted));
        // Asked once per session.
        press(&mut a, "<esc>");
        assert!(open(&mut a).is_empty());
        // Being written: a draft that comes late is dropped.
        a.finish.draft = Some(DraftState::Pending);
        press(&mut a, "i");
        a.commit_drafted(Ok("Late".into()));
        assert_eq!(a.finish.message, "Drafted subject\n\nbody");
    }

    #[test]
    fn posting_takes_enter_and_its_verdict() {
        let mut a = pull(&[Acc, Flag]);
        a.review.units[1].dive.note = "off by one".into();
        open(&mut a);
        press(&mut a, "j");
        let e = press(&mut a, "<cr>");
        let [Effect::PostReview { verdict, body, .. }] = &e[..] else {
            panic!("{e:?}");
        };
        assert_eq!(*verdict, Verdict::Comment);
        assert!(body.contains("off by one"), "{body}");
        assert!(a.approving);
        // Nothing twice while it is on its way.
        open(&mut a);
        assert!(press(&mut a, "<cr>").is_empty());
    }

    #[test]
    fn approving_with_a_comment_asks_for_the_comment_first() {
        let mut a = pull(&[Acc]);
        open(&mut a);
        let e = press(&mut a, "<cr>");
        assert!(
            matches!(
                &e[..],
                [Effect::PostReview {
                    verdict: Verdict::Approve,
                    ..
                }]
            ),
            "{e:?}"
        );
        a.approving = false;
        open(&mut a);
        press(&mut a, "j");
        let e = press(&mut a, "<cr>");
        assert!(matches!(&e[..], [Effect::EditComment { .. }]), "{e:?}");
        a.comment_edited(EditOutcome::Edited("LGTM, nice tests.".into()));
        let e = press(&mut a, "<cr>");
        let [Effect::PostReview { verdict, body, .. }] = &e[..] else {
            panic!("{e:?}");
        };
        assert_eq!(*verdict, Verdict::Approve);
        assert!(body.starts_with("LGTM, nice tests."), "{body}");
    }

    #[test]
    fn a_post_or_a_commit_that_went_through_ends_teer_with_one_line() {
        let mut a = pull(&[Acc]);
        a.approving = true;
        let e = a.posted(Verdict::Approve, Ok("PR #7 approved on GitHub.".into()));
        assert_eq!(e, vec![Effect::Save, Effect::Quit]);
        assert_eq!(
            a.finish.exit_line.as_deref(),
            Some("PR #7 approved on GitHub.")
        );
        assert!(a.review.pr.as_ref().unwrap().is_approved());
        // Failed: it says why, and teer stays.
        let mut b = pull(&[Acc]);
        let e = b.posted(Verdict::Approve, Err("gh failed: boom".into()));
        assert!(e.is_empty());
        assert_eq!(b.status, "gh failed: boom");
        assert!(!b.approving && b.finish.exit_line.is_none());

        let mut c = changes(&[Acc]);
        open(&mut c);
        c.finish.message = "Fix it\n\n- body".into();
        let e = c.commit_finished(commit::Outcome::Committed {
            short: "abc1234".into(),
        });
        assert_eq!(e, vec![Effect::Save, Effect::Quit]);
        assert_eq!(
            c.finish.exit_line.as_deref(),
            Some("Committed abc1234: Fix it")
        );
        let mut d = changes(&[Acc]);
        open(&mut d);
        let e = d.commit_finished(commit::Outcome::Failed {
            code: Some(1),
            output: "hook said no".into(),
        });
        assert!(e.is_empty());
        assert!(d.status.contains("hook said no"), "{}", d.status);
        assert!(matches!(d.overlay, Overlay::Finish { .. }));
    }

    #[test]
    fn a_pr_teer_cant_reach_or_approved_already_is_not_posted() {
        let mut p = pr();
        p.remote = None;
        let mut a = app(&[Acc], "feature", Some(p));
        open(&mut a);
        assert!(press(&mut a, "<cr>").is_empty());
        assert!(a.status.starts_with("Can't post to PR #7"), "{}", a.status);
        let mut p = pr();
        p.approved_head = p.head_sha.clone();
        let mut b = app(&[Acc], "feature", Some(p));
        open(&mut b);
        assert!(press(&mut b, "<cr>").is_empty());
        assert!(b.status.contains("already"), "{}", b.status);
    }

    #[test]
    fn the_sheet_lists_the_notes_and_fits_any_screen() {
        let mut a = changes(&[Flag, Pending, Acc]);
        a.review.units[0].dive.note = "a long note ".repeat(30);
        a.review.units[2].dive.note = "rename it".into();
        open(&mut a);
        for (w, h) in [(120, 40), (80, 24), (40, 12), (20, 6), (10, 4)] {
            render(&a, w, h);
        }
        let screen = render(&a, 100, 50);
        for want in [
            "Finish · your changes",
            "keep reviewing",
            "send the notes to Claude Code",
            "commit anyway",
            "Notes · 2 notes",
            "s2  f2.rs · accepted",
            "rename it",
        ] {
            assert!(screen.contains(want), "{want}:\n{screen}");
        }
    }
}
