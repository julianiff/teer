//! What a review is for. A mode picks the tools that fit one purpose and
//! leaves the rest out: their keys do nothing but say so, and the help,
//! the keys tile and the prefix hints don't list them.
//!
//! - **review**: what the change does and whether it is right: yours, your
//!   agent's or someone else's. The default.
//! - **security**: how the change can be abused, and nothing else: the
//!   risks written into the code, and the checks that follow each up.
//! - **improve**: what to add so the change is better. The agent writes
//!   inserts for each unit, and the queue lists them instead of the units
//!   (see [`crate::session::improve`]). They are asked for in this mode
//!   only.
//! - **all**: every tool of review and security.
//!
//! Review and security work the same way: comments in the code, and one
//! tile where the agent is asked, which offers the questions worth asking
//! next. What the comments say and what the tile offers is the mode's.
//!
//! The mode is named at start (`teer review`, `teer security`, `teer
//! improve`), kept with the review, and changed inside it with `<space>m`.

use serde::{Deserialize, Serialize};

use crate::keymap::Action;
use crate::review::model::Review;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Also what a review saved as `check` (a mode since joined with this
    /// one) opens in.
    #[serde(alias = "check")]
    Review,
    Security,
    Improve,
    /// Every tool. What an [`App`](crate::session::App) has until a front
    /// end names a mode.
    #[default]
    All,
}

impl Mode {
    /// The modes as the picker lists them.
    pub const EVERY: [Mode; 4] = [Mode::Review, Mode::Security, Mode::Improve, Mode::All];

    /// Its name: what the command line, the config and the header say.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Review => "review",
            Mode::Security => "security",
            Mode::Improve => "improve",
            Mode::All => "all",
        }
    }

    /// Its icon: what the header shows in place of the name. Both are two
    /// cells wide in every terminal. None for every tool at once.
    pub fn icon(self) -> &'static str {
        match self {
            Mode::Review => "🔍",
            Mode::Security => "🔒",
            Mode::Improve => "✨",
            Mode::All => "",
        }
    }

    pub fn from_name(name: &str) -> Option<Mode> {
        Mode::EVERY.into_iter().find(|m| m.name() == name)
    }

    /// What the mode is for, in one line.
    pub fn about(self) -> &'static str {
        match self {
            Mode::Review => "what the change does, and whether it is right",
            Mode::Security => "how the change can be abused, and nothing else",
            Mode::Improve => "what to add so the change is better",
            Mode::All => "every tool at once",
        }
    }

    /// The tools the mode has beside the ones every mode has (deciding,
    /// notes, asking, finishing), as the picker says them.
    pub fn tools(self) -> &'static str {
        match self {
            Mode::Review => {
                "what changed · ask · tests · proposed changes · commits · send to your agent"
            }
            Mode::Security => {
                "risks in the code · checks that follow them up · full check · prove · proposed \
                 changes · send to your agent"
            }
            Mode::Improve => {
                "inserts the agent writes, listed by what each does · put in · leave out · flag \
                 for your agent · send to your agent"
            }
            Mode::All => "the tools of review and security",
        }
    }

    /// The mode a review opens in when nothing names one.
    pub fn for_review(_review: &Review) -> Mode {
        Mode::Review
    }

    /// Does the mode have `a`? Moving, deciding, notes, asking, the lists
    /// and finishing are in every mode; the tools are not.
    pub fn allows(self, a: Action) -> bool {
        use Action::*;
        match self {
            Mode::All => true,
            Mode::Review => !matches!(
                a,
                ToolSecurity | ToolProve | ToggleSecurityOrder | InsertLeave
            ),
            // The security order is the mode's own: nothing to toggle.
            Mode::Security => !matches!(
                a,
                ToolTests | ToggleSecurityOrder | ToggleByFile | ToolHistory | InsertLeave
            ),
            // The list is the inserts: nothing to list by file, by exposure
            // or with the units accepted for you; a decision is on one
            // insert, so nothing decides a file or a selection.
            Mode::Improve => !matches!(
                a,
                ToolSecurity
                    | ToolProve
                    | ToggleSecurityOrder
                    | ToolTests
                    | ToolHistory
                    | ToggleByFile
                    | ToggleAutoAccepted
                    | ToolFix
                    | DecideAcceptFile
                    | DecideFlagFile
                    | VisualStart
            ),
        }
    }

    /// Does the mode write what the change does into the code, and offer
    /// the advice on it?
    pub fn reviews(self) -> bool {
        matches!(self, Mode::Review | Mode::Improve | Mode::All)
    }

    /// Does the mode write the security risks into the code, and offer the
    /// checks that follow them up?
    pub fn secures(self) -> bool {
        matches!(self, Mode::Security | Mode::All)
    }

    /// Is the queue in the security order (the security box and the
    /// attack-surface card on top) whatever `zs` says?
    pub fn security_order(self) -> bool {
        self == Mode::Security
    }

    /// Does the queue list the inserts the agent writes, and are they
    /// asked for (see [`crate::session::improve`])?
    pub fn improves(self) -> bool {
        self == Mode::Improve
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_all_has_every_action() {
        for m in Mode::EVERY {
            assert_eq!(Mode::from_name(m.name()), Some(m));
        }
        assert_eq!(Mode::from_name("pr"), None);
        assert!(Action::all().all(|a| Mode::All.allows(a)));
    }

    #[test]
    fn every_mode_decides_notes_asks_finishes_and_changes_mode() {
        let always = [
            Action::MoveDown,
            Action::DecideAccept,
            Action::DecideFlag,
            Action::NoteEdit,
            Action::HistoryUndo,
            Action::ViewOpen,
            Action::ViewBack,
            Action::AskOpen,
            Action::GotoEditor,
            Action::ToolNotes,
            Action::ToolFlagged,
            Action::ToolSettings,
            Action::ToolMode,
            Action::FinishOpen,
            Action::ReviewRerun,
            Action::HelpOpen,
            Action::ViewClose,
        ];
        for m in Mode::EVERY {
            for a in always {
                assert!(m.allows(a), "{} in {}", a.id(), m.name());
            }
        }
    }

    #[test]
    fn each_mode_keeps_its_own_tools_and_drops_the_others() {
        use Action::*;
        assert!(Mode::Review.allows(ToolTests) && Mode::Review.allows(ToolHistory));
        assert!(Mode::Review.allows(SendSession) && Mode::Review.allows(ToolFix));
        assert!(!Mode::Review.allows(ToolProve) && !Mode::Review.allows(ToolSecurity));
        assert!(Mode::Security.allows(ToolSecurity) && Mode::Security.allows(ToolProve));
        assert!(!Mode::Security.allows(ToolTests) && !Mode::Security.allows(ToolHistory));
        assert!(Mode::Improve.allows(FixApply) && Mode::Improve.allows(InsertLeave));
        assert!(Mode::Improve.allows(SendSession) && Mode::Improve.allows(AskOpen));
        assert!(!Mode::Improve.allows(ToolFix) && !Mode::Improve.allows(ToolSecurity));
        assert!(!Mode::Improve.allows(ToggleByFile) && !Mode::Improve.allows(VisualStart));
        assert!(!Mode::Review.allows(InsertLeave) && !Mode::Security.allows(InsertLeave));
        assert!(Mode::Review.reviews() && !Mode::Review.secures());
        assert!(Mode::Security.secures() && !Mode::Security.reviews());
        assert!(Mode::Improve.reviews() && !Mode::Improve.secures());
        assert!(Mode::Improve.improves() && !Mode::All.improves());
        assert!(Mode::All.reviews() && Mode::All.secures());
        // A review saved in the mode that is now part of review.
        assert_eq!(
            serde_json::from_str::<Mode>("\"check\"").unwrap(),
            Mode::Review
        );
    }
}
