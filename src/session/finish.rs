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

use super::app::{App, DraftState, Effect, Overlay, Screen, typed, units};
use crate::approve::{self, EditOutcome, Verdict};
use crate::commit::{self, Branch};
use crate::config::Provider;
use crate::keymap::{Action, Edit, Key, Sheet};
use crate::review::model::{Decision, PrInfo};
use crate::util::sanitize;

/// What the review is of, as the sheet tells its rows apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
    pub fn verb(self, api: bool) -> &'static str {
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

    /// What `step` does, in a sentence or two; `None` for the commit and
    /// the posts, whose details show what they will send instead.
    pub fn step_text(&self, step: Step) -> Option<String> {
        let k = |a: Action| self.keys.label(a);
        // What waiting for copied notes' changes does.
        let wait = || {
            format!(
                "Then teer waits, the queue on show: once the files have changed and stayed the \
same for {} s, it re-runs the review on what changed. {} stops waiting; {} re-runs later.",
                self.cfg.fix_loop.quiet_seconds,
                k(Action::ViewClose),
                k(Action::ReviewRerun)
            )
        };
        Some(match step {
            Step::KeepReviewing => format!(
                "Goes to the first pending unit. {} comes back here.",
                k(Action::FinishOpen)
            ),
            Step::SendNotes => match self.cfg.provider() {
                Provider::Api => format!(
                    "teer can't hand notes to the model API: this copies them, with what to do, \
for you to paste into your agent. {}",
                    wait()
                ),
                cli => {
                    let session = self
                        .review
                        .agent_session
                        .as_ref()
                        .filter(|s| !s.is_fresh() && cli == Provider::Claude)
                        .map(|s| {
                            let id: String = s.id.chars().take(8).collect();
                            format!(", in a fork of session {}", sanitize(&id))
                        })
                        .unwrap_or_default();
                    format!(
                        "Opens {} with the notes as its first message{session}. When you quit it, \
teer re-runs the review on what it changed.",
                        cli.title(),
                    )
                }
            },
            Step::Yank if self.waits_for_fixes() => format!(
                "Copies the notes as Markdown, each with its diff, for your agent. {}",
                wait()
            ),
            Step::Yank => "Copies the notes as Markdown, each with its diff.".into(),
            Step::Done => "Everything is accepted, and a branch has nothing to commit. Leaves \
teer; the review is saved."
                .into(),
            Step::Commit
            | Step::RequestChanges
            | Step::CommentOnly
            | Step::Approve
            | Step::ApproveWithComment => return None,
        })
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
    pub(crate) fn finish_key(&mut self, k: Key) -> Vec<Effect> {
        let Overlay::Finish { sel } = self.overlay else {
            return Vec::new();
        };
        let steps = self.finish_steps();
        let last = steps.len().saturating_sub(1);
        let sel = sel.min(last);
        let step = steps.get(sel).copied();
        if let Some(mut text) = self.finish.editing.take() {
            match self.keys.edit(k) {
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
            if self.keys.confirm(k) {
                return self.run_step(asked);
            }
            return Vec::new();
        }
        if self.keys.pending().is_none() && self.keys.command_line().is_none() {
            match self.keys.sheet(k) {
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
        let Some(a) = self.panel_action(k) else {
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
            Screen::Unit | Screen::Flagged => self.open_unit(id),
            Screen::Queue => self.move_sel(pos),
        }
    }

    /// `y`: the notes as Markdown on the clipboard, each with its diff.
    pub(crate) fn yank_notes(&mut self) -> Vec<Effect> {
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
            self.review.subject(),
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
    pub(crate) fn commit_drafted(&mut self, r: Result<String, String>) {
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
    pub(crate) fn posted(
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

/// "Checks PR #7 (owner/repo) is still at abc1234, then posts on GitHub."
pub fn head_check(pr: &PrInfo) -> String {
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
