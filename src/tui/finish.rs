//! The `:w` sheet (v2 Phase 6): one default next step for the decisions
//! made, its alternatives, and the notes.

use crossterm::event::KeyEvent;
use ratatui::Frame;

use super::app::{App, Effect, Overlay};
use crate::keymap::Action;

/// The actions the sheet handles, for the help.
pub const ACTIONS: &[Action] = &[];

/// Draw the sheet over the screen.
pub fn draw(f: &mut Frame, app: &App) {
    let _ = (f, app);
}

impl App {
    /// Open the sheet (`:w`).
    pub fn open_finish(&mut self) -> Vec<Effect> {
        self.overlay = Overlay::Finish { sel: 0 };
        Vec::new()
    }

    /// A key while the sheet is open.
    pub(super) fn finish_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let _ = k;
        self.overlay = Overlay::None;
        Vec::new()
    }
}
