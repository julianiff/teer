//! The effects that in the terminal take over the screen: the editor
//! (`gf`), the agent CLI the notes go to, the editor for the commit
//! message and the review summary, git's commit, the settings. The window
//! carries them out its own way; what they come back with reaches the
//! review as [`Returned`].

use teer::session::Effect;

use crate::host::{Returned, Session};

/// Carry out `e` if it is the window's; else hand it back for the runtime.
pub fn take(s: &mut Session, e: Effect) -> Option<Effect> {
    match e {
        Effect::Edit { .. }
        | Effect::OpenAgent { .. }
        | Effect::EditComment { .. }
        | Effect::Commit { .. }
        | Effect::Settings => {
            s.app.status = "Teer.app can't do this yet: run teer in a terminal for it.".into();
            None
        }
        other => Some(other),
    }
}

/// What a handoff came back with, into the review.
pub fn returned(s: &mut Session, r: Returned) -> Vec<Effect> {
    match r {
        Returned::Agent { cli, ran } => {
            if s.rt.agent_exited(&mut s.app, cli, ran) {
                return vec![Effect::Quit];
            }
            Vec::new()
        }
        Returned::Edited(outcome) => {
            s.app.comment_edited(outcome);
            Vec::new()
        }
        Returned::Committed(outcome) => s.app.commit_finished(outcome),
        Returned::Settings(Some(cfg)) => {
            s.app.status = s.rt.settings_saved(&mut s.app, cfg);
            Vec::new()
        }
        Returned::Settings(None) => Vec::new(),
        Returned::Status(line) => {
            s.app.status = line;
            Vec::new()
        }
    }
}
