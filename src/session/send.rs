//! `s`: send the result of the tile in focus (the advice, the security
//! findings, the tests' gaps, old against new, an answer) to an agent
//! session running in this repository, which applies it. Proposed hunks
//! aren't sent: `p`/`P` put them in. See [`crate::send`].

use super::app::{App, Effect, Overlay};
use crate::keymap::Key;
use crate::send::Target;

impl App {
    /// `s`: what the tile in focus says, as a message for an agent; then
    /// the sessions to send it to.
    pub(crate) fn send_to_session(&mut self) -> Vec<Effect> {
        self.status = "Sending to a session isn't built yet.".into();
        Vec::new()
    }

    /// The sessions found for `text`: the picker, or why there is none.
    pub(crate) fn sessions_found(
        &mut self,
        targets: Vec<Target>,
        text: String,
        what: String,
    ) -> Vec<Effect> {
        if targets.is_empty() {
            self.status = "No agent session runs in this repository.".into();
            return Vec::new();
        }
        self.overlay = Overlay::SendTo {
            targets,
            sel: 0,
            text,
            what,
        };
        Vec::new()
    }

    /// Keys of the session picker.
    pub(crate) fn send_key(&mut self, k: Key) -> Vec<Effect> {
        let _ = k;
        self.overlay = Overlay::None;
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
