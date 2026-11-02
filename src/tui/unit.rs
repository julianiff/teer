//! The unit view (v2 Phase 5): any unit, with why it has its level, what
//! changed, what to do about it and the second model, answered before the
//! reviewer asks.

use ratatui::Frame;
use ratatui::layout::Rect;

use super::app::{App, Effect, Screen};
use crate::keymap::Action;

/// The actions the unit view handles, for the help.
pub const ACTIONS: &[Action] = &[];

/// Draw the unit view into `area`.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let _ = (f, app, area);
}

impl App {
    /// Open the unit view on unit `id` (from the queue: `l`, `enter`).
    pub fn open_unit(&mut self, id: String) -> Vec<Effect> {
        let _ = id;
        self.screen = Screen::Unit;
        Vec::new()
    }
}
