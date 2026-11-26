//! What the ask tile offers to ask next about the unit on show: the
//! questions worth asking, each one key away. Both modes work this way;
//! what is offered is the mode's.
//!
//! - **review**: what to do about the unit (the advice), until it is there.
//! - **security**: the checks that follow up each risk the description's
//!   model saw in the unit, then the full check of every path into it.
//! - in both, the questions the newest answer says are worth asking next,
//!   so one answer leads to the next check.
//!
//! `enter` on an empty question asks the first, a number and `enter` that
//! one; anything else typed is the reviewer's own question.

use super::app::{App, Effect};
use super::tools;
use super::unit::Panel;
use crate::keymap::Action;
use crate::review::model::{DiveKind, Signature, Unit};

/// As many are offered at once.
pub const MAX_OFFERED: usize = 6;

/// What the advice is asked with.
pub const ADVICE: &str = "What should I do about this change, step by step?";
/// What the full security check is asked with.
pub const AUDIT: &str = "Check every path into this unit, from its entry points to its sinks";

/// Asks if the long signature of `name` is a design weakness.
pub fn signature_design(name: &str, sig: &Signature) -> String {
    format!(
        "Is the signature of {name} a design weakness? It has {}. Read its callers and say what to do in this change.",
        sig.words()
    )
}

/// Asks if a caller of `name` can mix up its arguments unnoticed.
pub fn signature_order(name: &str, sig: &Signature) -> String {
    format!(
        "Do the callers of {name} pass its {} arguments by position, and can two change places with no error?",
        sig.params
    )
}

/// What taking a suggestion does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Offer {
    /// The agent's advice on the unit ([`DiveKind::Recommend`]).
    Advice,
    /// A question to the agent, asked like one the reviewer typed.
    Question,
    /// The full security check ([`DiveKind::Security`]), in its own tile.
    Audit,
}

/// A question the ask tile offers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Suggestion {
    pub text: String,
    pub offer: Offer,
    /// What it follows up, in a few words: `auth · medium`, `the last
    /// answer`. Empty for the advice and the full check.
    pub about: String,
}

impl App {
    /// What the ask tile offers for the unit on show, the first one what
    /// `enter` on an empty question asks. Nothing without an agent.
    pub fn suggestions(&self) -> Vec<Suggestion> {
        let Some(u) = self.review.unit(&self.unit.id) else {
            return Vec::new();
        };
        self.suggestions_for(u)
    }

    /// [`App::suggestions`] for `u`.
    pub fn suggestions_for(&self, u: &Unit) -> Vec<Suggestion> {
        if !self.can_ask() {
            return Vec::new();
        }
        let chat = self.chat_of(&u.id);
        let asked = |text: &str| {
            chat.iter()
                .any(|&n| self.review.questions[n].text.eq_ignore_ascii_case(text))
        };
        let mut out: Vec<Suggestion> = Vec::new();
        let offer = |out: &mut Vec<Suggestion>, text: &str, about: String| {
            let text = text.trim();
            if !text.is_empty() && !asked(text) && !out.iter().any(|s| s.text == text) {
                out.push(Suggestion {
                    text: text.to_string(),
                    offer: Offer::Question,
                    about,
                });
            }
        };
        if self.mode.reviews() && tools::may_advise(self, u) {
            out.push(Suggestion {
                text: ADVICE.into(),
                offer: Offer::Advice,
                about: String::new(),
            });
        }
        // What the newest answer leads to comes before what was offered
        // all along: it is where the reviewer is.
        let newest = chat
            .iter()
            .rev()
            .find_map(|&n| self.review.questions[n].answer.as_ref());
        for q in newest.iter().flat_map(|a| &a.next) {
            offer(&mut out, q, "the last answer".into());
        }
        // A parameter list the change made long: teer counted it, the
        // agent reads the callers and judges it.
        if let Some(sig) = u.signature.as_ref().filter(|s| s.grew_long())
            && self.mode.reviews()
        {
            let name = u.symbol.rsplit("::").next().unwrap_or(&u.symbol);
            let about = format!("signature · {} parameters", sig.params);
            offer(&mut out, &signature_design(name, sig), about.clone());
            offer(&mut out, &signature_order(name, sig), about);
        }
        let secures = self.mode.secures() && self.security.enabled;
        if secures {
            for s in &u.checks.suspects {
                let about = format!("{} · {}", s.class.word(), s.severity.word());
                for q in &s.checks {
                    offer(&mut out, q, about.clone());
                }
            }
        }
        out.truncate(MAX_OFFERED - 1);
        let kind = DiveKind::Security;
        if secures
            && self.allows(Action::ToolSecurity)
            && !u.dive.results.contains_key(&kind)
            && !tools::pending(u, kind)
        {
            out.push(Suggestion {
                text: AUDIT.into(),
                offer: Offer::Audit,
                about: String::new(),
            });
        }
        out
    }

    /// Does `enter` on the ask tile, with nothing typed, ask the first
    /// suggestion: there is one, and it isn't the full check, which is
    /// not started that lightly.
    pub fn offers_at_once(&self) -> bool {
        self.suggestions()
            .first()
            .is_some_and(|s| s.offer != Offer::Audit)
    }

    /// The suggestion a question typed as `draft` means: the first for
    /// nothing typed (unless it is the full check, which takes its
    /// number), the one with that number for a number alone.
    pub(crate) fn suggestion_typed(&self, draft: &str) -> Option<usize> {
        let n = self.suggestions().len();
        let draft = draft.trim();
        if draft.is_empty() {
            return self.offers_at_once().then_some(0);
        }
        let i: usize = draft.parse().ok()?;
        (1..=n).contains(&i).then(|| i - 1)
    }

    /// Ask suggestion `i` of the unit on show (`enter`, its number, or a
    /// click on it).
    pub fn take_suggestion(&mut self, i: usize) -> Vec<Effect> {
        let Some(s) = self.suggestions().into_iter().nth(i) else {
            return Vec::new();
        };
        match s.offer {
            Offer::Advice => self.ask_advice(),
            Offer::Audit => self.open_audit(),
            Offer::Question => {
                if !self.unit.tiles.contains(&Panel::Ask) {
                    self.open_ask_panel();
                }
                self.unit.typing = false;
                self.ask_about_unit(s.text)
            }
        }
    }
}
