//! `s`: send the result of the tile in focus (the advice, the security
//! findings, the tests' gaps, old against new, an answer) to an agent
//! session running in this repository, which applies it. Proposed hunks
//! aren't sent: `p`/`P` put them in. See [`crate::send`].
//!
//! The message is plain text: who sends it and about what unit, a line
//! naming what follows, the tile's result, and what to do with it. The
//! picker ([`Overlay::SendTo`]) lists the sessions, the one that wrote the
//! change first chosen; with pi installed but without teer's extension,
//! its last row offers to install it.

use super::app::{App, Effect, Overlay, Screen};
use super::names;
use super::tools;
use super::unit::Panel;
use crate::keymap::{Action, Edit, Key};
use crate::review::model::{
    Cite, ClaimCheck, DiveKind, DiveResult, Expect, Outcome, ProbeCase, Unit, UnitKind, UnitStatus,
};
use crate::security::{Finding, Proof};
use crate::send::{Agent, PiState, Target};
use crate::util::sanitize;

/// Most characters a message holds: past it, the result is cut short.
pub const MESSAGE_MAX: usize = 8000;

/// The message's last line: what the session is to do with it.
pub const CLOSING: &str =
    "Change the code to address this. Keep it small and say what you changed, or why not.";

/// The picker's last row with pi installed but without teer's extension.
pub const PI_ROW: &str = "pi: install teer's extension so pi sessions can be reached";

/// What the picker asks before installing pi's extension.
pub const PI_QUESTION: &str =
    "Install teer's pi extension? pi sessions started from then on can be reached.";

/// What [`App`] keeps for `s` besides the picker itself: the session
/// chosen last (this run only, never saved), whether the picker offers to
/// install pi's extension, and whether it is asking before it does.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SendState {
    /// The session a result went to last, by agent and id.
    pub last: Option<(Agent, String)>,
    /// The picker's last row installs teer's pi extension.
    pub pi_offer: bool,
    /// The picker asks before installing it.
    pub confirm_pi: bool,
}

/// A tile's result as a message: its text, and what it is ("the advice").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub what: String,
}

/// The tile's name, as its border says it.
pub fn tile_name(p: Panel) -> &'static str {
    match p {
        Panel::Recommend => "Advice",
        Panel::Ask => "Ask",
        Panel::Outside => "Outside the review",
        Panel::Tool(kind) => match tools::light(kind).0 {
            "" => "This tile",
            name => name,
        },
    }
}

/// How long ago `active_at` (ms since the epoch) was at `now` (the same):
/// "just now", "2 min ago", "3 h ago", "4 d ago"; empty when unknown.
pub fn ago(active_at: u64, now: u64) -> String {
    if active_at == 0 {
        return String::new();
    }
    let s = now.saturating_sub(active_at) / 1000;
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}

/// Now, in ms since the epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Text from the review for the message: control characters (escapes
/// among them) go, newlines and tabs stay.
fn plain(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\r' { '\n' } else { c })
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

/// One line of text from the review: control characters and line breaks
/// become spaces.
fn one_line(s: &str) -> String {
    sanitize(s).trim().to_string()
}

/// `src/x.php:41`, `src/x.php:3-9`.
fn cite(c: &Cite) -> String {
    let file = one_line(&c.file.display().to_string());
    if c.end > c.start {
        format!("{file}:{}-{}", c.start, c.end)
    } else {
        format!("{file}:{}", c.start)
    }
}

/// Where `u` is, for the message's first line: `src/x.rs:10-24`, or the
/// file alone (a whole file, or no lines known).
fn unit_place(u: &Unit) -> String {
    let file = one_line(&u.file.display().to_string());
    let range = u.new_range.or(u.old_range);
    match range {
        _ if u.kind == UnitKind::File => file,
        Some(r) if r.end > r.start => format!("{file}:{}-{}", r.start, r.end),
        Some(r) => format!("{file}:{}", r.start),
        None => file,
    }
}

/// `text` cut to at most `max` characters at a line's end, saying so.
fn cut(text: String, max: usize) -> String {
    const CUT: &str = "\n… (cut short: the rest is in teer)";
    if text.chars().count() <= max {
        return text;
    }
    let keep = max.saturating_sub(CUT.chars().count());
    let head: String = text.chars().take(keep).collect();
    let head = match head.rfind('\n') {
        Some(i) if i > 0 => head[..i].to_string(),
        _ => head,
    };
    format!("{}{CUT}", head.trim_end())
}

/// What `kind`'s tile has to send, or that it has nothing (yet).
enum Body {
    /// What it is, the line before it, and the result.
    Some {
        what: &'static str,
        intro: &'static str,
        body: String,
    },
    /// The status line saying there is nothing to send.
    None(String),
}

fn nothing_yet(p: Panel) -> Body {
    Body::None(format!("{} has nothing to send yet.", tile_name(p)))
}

impl App {
    /// `s`: what the tile in focus says, as a message for an agent; then
    /// the sessions to send it to.
    pub(crate) fn send_to_session(&mut self) -> Vec<Effect> {
        if self.review.pr.is_some() {
            self.status = format!(
                "On a pull request nothing is applied here: {} posts the review.",
                self.keys.label(Action::FinishOpen)
            );
            return Vec::new();
        }
        if self.screen != Screen::Unit {
            self.status = "Open a unit and focus a tile to send its result.".into();
            return Vec::new();
        }
        let Some(p) = self.unit.panel else {
            self.status = format!(
                "Focus a tile ({}) to send its result.",
                self.keys.label(Action::MoveNextTile)
            );
            return Vec::new();
        };
        let Some(u) = self.unit_on_show() else {
            return Vec::new();
        };
        match self.tile_message(u, p) {
            Ok(Message { text, what }) => {
                self.status = "Looking for agent sessions in this repository…".into();
                vec![Effect::ListSessions { text, what }]
            }
            Err(why) => {
                self.status = why;
                Vec::new()
            }
        }
    }

    /// Whether `s` on tile `p` of `u` sends something: there is a result,
    /// and this isn't a pull request.
    pub fn can_send(&self, u: &Unit, p: Panel) -> bool {
        self.review.pr.is_none() && self.tile_message(u, p).is_ok()
    }

    /// The message tile `p` of `u` makes, or the status line saying why
    /// there is none.
    pub fn tile_message(&self, u: &Unit, p: Panel) -> Result<Message, String> {
        let body = match p {
            Panel::Recommend => advice_body(u, p),
            Panel::Ask => self.ask_body(u, p),
            Panel::Tool(DiveKind::Security) => self.security_body(u, p),
            Panel::Tool(DiveKind::Tests) => tests_body(u, p),
            Panel::Tool(DiveKind::Behavior) => behavior_body(u, p),
            Panel::Tool(_) | Panel::Outside => nothing_yet(p),
        };
        let (what, intro, body) = match body {
            Body::Some { what, intro, body } => (what, intro, body),
            Body::None(why) => return Err(why),
        };
        let head = format!(
            "teer, reviewing {}: {} ({})",
            one_line(&self.review.subject()),
            names::row_name(u),
            unit_place(u)
        );
        // The head, the intro and the closing always go in; the body is cut.
        let fixed = head.chars().count() + intro.chars().count() + CLOSING.chars().count() + 5;
        let body = cut(body, MESSAGE_MAX.saturating_sub(fixed));
        Ok(Message {
            text: format!("{head}\n\n{intro}\n{body}\n\n{CLOSING}"),
            what: what.into(),
        })
    }

    /// The Ask tile: the last answer in the unit's conversation, with its
    /// question and the code it points at.
    fn ask_body(&self, u: &Unit, p: Panel) -> Body {
        let last = self
            .chat_of(&u.id)
            .into_iter()
            .rev()
            .find_map(|n| {
                let q = &self.review.questions[n];
                q.answer.as_ref().map(|a| (q, a))
            })
            .filter(|(_, a)| !a.text.trim().is_empty());
        let Some((q, a)) = last else {
            return nothing_yet(p);
        };
        let mut body = format!(
            "Question: {}\nAnswer:\n{}",
            one_line(&q.text),
            plain(&a.text)
        );
        if !a.shown.is_empty() {
            body.push_str("\nCode it points at:");
            for s in &a.shown {
                body.push_str(&format!("\n- {} ({})", cite(&s.cite), one_line(&s.why)));
            }
        }
        Body::Some {
            what: "the answer",
            intro: "A question about it, and the agent's answer:",
            body,
        }
    }

    /// The Security tile: the findings that stand, most severe first;
    /// with none, the facts teer found.
    fn security_body(&self, u: &Unit, p: Panel) -> Body {
        let kind = DiveKind::Security;
        let audit = u.dive.results.get(&kind).and_then(|r| r.audit.as_ref());
        // The second model may still dispute one.
        if tools::pending(u, kind) || audit.is_some_and(|a| a.checking) {
            return nothing_yet(p);
        }
        let mut standing: Vec<&Finding> = audit
            .map(|a| a.findings.iter().filter(|f| f.stands()).collect())
            .unwrap_or_default();
        standing.sort_by_key(|f| std::cmp::Reverse(f.severity));
        if !standing.is_empty() {
            let body = standing
                .iter()
                .enumerate()
                .map(|(i, f)| self.finding_text(i + 1, f))
                .collect::<Vec<_>>()
                .join("\n");
            return Body::Some {
                what: "the security findings",
                intro: "Security findings that stand (their path verified on teer's call graph):",
                body,
            };
        }
        if !u.security.is_empty() {
            let body = u
                .security
                .iter()
                .map(|f| {
                    let mut at = vec![f.class.word().to_string()];
                    if !f.subject.is_empty() {
                        at.push(one_line(&f.subject));
                    }
                    if let Some(r) = f.lines {
                        let old = if f.side == crate::review::model::Side::Old {
                            " (old)"
                        } else {
                            ""
                        };
                        at.push(format!(
                            "{}:{}{old}",
                            one_line(&u.file.display().to_string()),
                            r.start
                        ));
                    }
                    format!("- {} ({})", one_line(&f.text), at.join(", "))
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Body::Some {
                what: "the security facts",
                intro: "Security facts teer found in the change:",
                body,
            };
        }
        match audit {
            Some(_) => Body::None(
                "Security has nothing to send: no finding stands, and teer found nothing.".into(),
            ),
            None => nothing_yet(p),
        }
    }

    /// Finding `f`, numbered `n`: its title, severity and class; its path
    /// from where the input enters to where it does harm; what's missing;
    /// the exploit; what proving it showed.
    fn finding_text(&self, n: usize, f: &Finding) -> String {
        let g = &self.review.graph;
        let named = |c: &Cite| match crate::security::audit::symbol_at(g, c) {
            Some(s) => format!("{} ({})", cite(c), one_line(s)),
            None => cite(c),
        };
        let mut path = vec![named(&f.source)];
        path.extend(
            f.path
                .iter()
                .map(|h| format!("{} ({})", one_line(&h.symbol), cite(&h.cite))),
        );
        path.push(named(&f.sink));
        let mut out = format!(
            "{n}. {} ({} severity, {})\n   path: {}",
            one_line(&f.title),
            f.severity.word(),
            f.class.word(),
            path.join(" → ")
        );
        if !f.missing.trim().is_empty() {
            out.push_str(&format!("\n   missing: {}", one_line(&f.missing)));
        }
        if !f.exploit.trim().is_empty() {
            out.push_str(&format!("\n   exploit: {}", one_line(&f.exploit)));
        }
        if let Some(Proof::Proven {
            case,
            before,
            after,
        }) = &f.proof
        {
            out.push_str(&format!(
                "\n   proven old against new: {} (before: {}, after: {})",
                one_line(case),
                one_line(before),
                one_line(after)
            ));
        }
        out
    }

    /// The session the picker chooses first: the one that wrote the
    /// change, else the one chosen last, else the first.
    fn default_target(&self, targets: &[Target]) -> usize {
        targets
            .iter()
            .position(|t| self.wrote_change(t))
            .or_else(|| {
                let (agent, id) = self.send.last.as_ref()?;
                targets
                    .iter()
                    .position(|t| t.agent == *agent && t.id == *id)
            })
            .unwrap_or(0)
    }

    /// Is `t` the session that wrote the change: the one the review was
    /// prepared from.
    pub fn wrote_change(&self, t: &Target) -> bool {
        let Some(s) = &self.review.agent_session else {
            return false;
        };
        let agent = match t.agent {
            Agent::ClaudeCode => "claude",
            Agent::Codex => "codex",
            Agent::Pi => "pi",
        };
        !s.id.is_empty() && s.id == t.id && s.agent == agent
    }

    /// The sessions found for `text`: the picker, or why there is none.
    /// `pi` says whether pi sessions could be among them: installed
    /// without teer's extension, the picker's last row installs it.
    pub(crate) fn sessions_found(
        &mut self,
        targets: Vec<Target>,
        pi: PiState,
        text: String,
        what: String,
    ) -> Vec<Effect> {
        let offer = pi == PiState::NoExtension;
        if targets.is_empty() && !offer {
            self.status = format!(
                "No agent session runs in this repository: start one, then {} again.",
                self.keys.label(Action::SendSession)
            );
            return Vec::new();
        }
        // Something else opened meanwhile: it stays.
        if self.overlay != Overlay::None {
            self.status = format!(
                "The sessions came while something else was open: {} again.",
                self.keys.label(Action::SendSession)
            );
            return Vec::new();
        }
        self.status.clear();
        self.send.pi_offer = offer;
        self.send.confirm_pi = false;
        let sel = self.default_target(&targets);
        self.overlay = Overlay::SendTo {
            targets,
            sel,
            text,
            what,
        };
        Vec::new()
    }

    /// The picker's rows: its sessions, then the pi row when it offers
    /// one.
    pub fn send_rows(&self) -> usize {
        match &self.overlay {
            Overlay::SendTo { targets, .. } => targets.len() + usize::from(self.send.pi_offer),
            _ => 0,
        }
    }

    /// Keys of the session picker: `j`/`k` (and the list's other moves)
    /// choose a session, `enter` (or `view.open`) sends to it, or on the
    /// pi row asks before installing pi's extension; `esc`, `h` or `q`
    /// cancel. Asking, `y`/`enter` installs, any other key goes back.
    pub(crate) fn send_key(&mut self, k: Key) -> Vec<Effect> {
        if !matches!(self.overlay, Overlay::SendTo { .. }) {
            return Vec::new();
        }
        if self.send.confirm_pi {
            self.send.confirm_pi = false;
            if self.keys.confirm(k) {
                self.close_picker();
                self.status = "Installing teer's pi extension…".into();
                return vec![Effect::InstallPiExtension];
            }
            return Vec::new();
        }
        let action = match self.keys.edit(k) {
            Edit::Submit => {
                self.keys.reset();
                Some(Action::ViewOpen)
            }
            Edit::Cancel => {
                self.keys.reset();
                Some(Action::ViewBack)
            }
            _ => self.panel_action(k),
        };
        let last = self.send_rows().saturating_sub(1);
        let Overlay::SendTo { sel, .. } = &mut self.overlay else {
            return Vec::new();
        };
        match action {
            Some(Action::MoveDown) => *sel = (*sel + 1).min(last),
            Some(Action::MoveUp) => *sel = sel.saturating_sub(1),
            Some(Action::MoveFirst) => *sel = 0,
            Some(Action::MoveLast) => *sel = last,
            Some(Action::ViewOpen) => return self.send_chosen(),
            Some(Action::ViewBack | Action::ViewClose) => self.close_picker(),
            _ => {}
        }
        Vec::new()
    }

    /// Choose row `i` of the picker (a click).
    pub fn send_select(&mut self, i: usize) {
        let last = self.send_rows().saturating_sub(1);
        if let Overlay::SendTo { sel, .. } = &mut self.overlay {
            *sel = i.min(last);
            self.send.confirm_pi = false;
        }
    }

    /// Choose row `i` of the picker and send to it, as `enter` would (a
    /// click on its send button): the pi row asks first.
    pub fn send_pick(&mut self, i: usize) -> Vec<Effect> {
        if !matches!(self.overlay, Overlay::SendTo { .. }) {
            return Vec::new();
        }
        self.send_select(i);
        self.send_chosen()
    }

    /// Send to the session chosen, remembering it; on the pi row, ask
    /// before installing pi's extension.
    fn send_chosen(&mut self) -> Vec<Effect> {
        let Overlay::SendTo {
            targets, sel, text, ..
        } = &self.overlay
        else {
            return Vec::new();
        };
        let Some(t) = targets.get(*sel).cloned() else {
            if self.send.pi_offer {
                self.send.confirm_pi = true;
            }
            return Vec::new();
        };
        let text = text.clone();
        self.send.last = Some((t.agent, t.id.clone()));
        self.close_picker();
        self.status = format!("Sending to {} {}…", t.agent.title(), t.name);
        vec![Effect::SendToSession {
            target: Box::new(t),
            text,
        }]
    }

    fn close_picker(&mut self) {
        self.overlay = Overlay::None;
        self.send.pi_offer = false;
        self.send.confirm_pi = false;
    }

    /// teer's pi extension was written (or not): say so.
    pub(crate) fn pi_installed(
        &mut self,
        result: Result<std::path::PathBuf, String>,
    ) -> Vec<Effect> {
        self.status = match result {
            Ok(p) => format!(
                "Installed {}: pi sessions started from now on can be reached.",
                p.display()
            ),
            Err(e) => format!("teer's pi extension isn't installed: {e}"),
        };
        Vec::new()
    }

    /// `text` went to `target`, or didn't: say so; when it didn't, put it
    /// on the clipboard.
    pub(crate) fn sent(
        &mut self,
        target: &Target,
        text: String,
        result: Result<(), String>,
    ) -> Vec<Effect> {
        match result {
            Ok(()) => {
                self.status = format!("Sent to {} {}.", target.agent.title(), target.name);
                Vec::new()
            }
            Err(e) => {
                self.status = format!("Not sent to {} {}: {e}", target.agent.title(), target.name);
                vec![Effect::Copy {
                    text,
                    what: "the message".into(),
                }]
            }
        }
    }
}

/// The advice tile: the advice and its sources.
fn advice_body(u: &Unit, p: Panel) -> Body {
    let kind = DiveKind::Recommend;
    let Some(r) = answered(u, kind) else {
        return nothing_yet(p);
    };
    let text = plain(&r.text);
    if text.is_empty() {
        return nothing_yet(p);
    }
    let mut body = text;
    if !r.sources.is_empty() {
        body.push_str("\nSources:");
        for s in &r.sources {
            body.push_str(&format!("\n- {}", one_line(s)));
        }
    }
    Body::Some {
        what: "the advice",
        intro: "The advice:",
        body,
    }
}

/// `kind`'s answer on `u`, when it came and nothing newer is on its way.
fn answered(u: &Unit, kind: DiveKind) -> Option<&DiveResult> {
    if tools::pending(u, kind) {
        return None;
    }
    u.dive.results.get(&kind)
}

/// The Tests tile: the dangerous cases no test covers, once the agent has
/// read the tests.
fn tests_body(u: &Unit, p: Panel) -> Body {
    let Some(c) = answered(u, DiveKind::Tests).and_then(|r| r.coverage.as_ref()) else {
        return nothing_yet(p);
    };
    if !c.judged {
        return nothing_yet(p);
    }
    if c.gaps.is_empty() {
        return Body::None("Tests has nothing to send: no dangerous case the tests miss.".into());
    }
    let mut gaps: Vec<_> = c.gaps.iter().collect();
    gaps.sort_by_key(|g| std::cmp::Reverse(g.risk));
    let body = gaps
        .iter()
        .map(|g| {
            let mut line = format!(
                "- {} ({} risk): {}",
                one_line(&g.case),
                g.risk.word(),
                one_line(&g.danger)
            );
            if !g.evidence.is_empty() {
                let at: Vec<String> = g.evidence.iter().map(cite).collect();
                line.push_str(&format!(" ({})", at.join(", ")));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n");
    Body::Some {
        what: "the missing tests",
        intro: "Dangerous cases no test covers. Add tests that cover these:",
        body,
    }
}

fn outcome(o: &Outcome) -> String {
    match o {
        Outcome::Value(v) => one_line(v),
        Outcome::Error(e) => format!("error: {}", one_line(e)),
        Outcome::Missing => "nothing printed".into(),
    }
}

/// The Behavior tile: the statements the runs disprove, and each case that
/// came out other than predicted, with its input, before and after.
fn behavior_body(u: &Unit, p: Panel) -> Body {
    let Some(b) = answered(u, DiveKind::Behavior).and_then(|r| r.behavior.as_ref()) else {
        return nothing_yet(p);
    };
    // A new unit has no before to be surprised by.
    if u.status == UnitStatus::Added {
        return Body::None(
            "Behavior has nothing to send: the unit is new, so nothing ran before.".into(),
        );
    }
    let surprises: Vec<&ProbeCase> = b.cases.iter().filter(|c| c.surprise()).collect();
    if surprises.is_empty() {
        return Body::None(
            "Behavior has nothing to send: every case came out as predicted.".into(),
        );
    }
    let mut out = Vec::new();
    let wrong: Vec<String> = b
        .claim_checks()
        .into_iter()
        .filter_map(|(text, check)| match check {
            ClaimCheck::Fails { wrong, of } => Some(format!(
                "- \"{}\" ({wrong} of {of} cases disagree)",
                one_line(&text)
            )),
            _ => None,
        })
        .collect();
    if !wrong.is_empty() {
        out.push("Statements about the change the runs disprove:".to_string());
        out.extend(wrong);
    }
    out.push("Cases that came out other than predicted:".to_string());
    for c in surprises {
        let predicted = match (c.claim > 0, c.expect) {
            (true, Expect::Same) => "its statement implies no change",
            (true, Expect::Different) => "its statement implies a change",
            (false, Expect::Same) => "the agent predicted no change",
            (false, Expect::Different) => "the agent predicted a change",
        };
        out.push(format!(
            "- {}: input {}",
            one_line(&c.name),
            one_line(&c.input)
        ));
        out.push(format!("  before: {}", outcome(&c.before)));
        out.push(format!("  after: {}", outcome(&c.after)));
        let why = one_line(&c.why);
        if why.is_empty() {
            out.push(format!("  {predicted}"));
        } else {
            out.push(format!("  {predicted}: {why}"));
        }
    }
    Body::Some {
        what: "the unexpected behavior",
        intro: "The old and the new code ran on the same inputs, in a sandbox:",
        body: out.join("\n"),
    }
}
