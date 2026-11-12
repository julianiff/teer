//! The session picker (`s`): the agent sessions running in this
//! repository, the one the result goes to selected.
//!
//! Filled in by the UI slice.

use ratatui::Frame;

use super::app::App;
use crate::send::Target;

/// Draw the picker over the screen.
pub fn draw(f: &mut Frame, app: &App, targets: &[Target], sel: usize, what: &str) {
    let _ = (f, app, targets, sel, what);
}
