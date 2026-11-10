//! The queue's security order (`zs`): by how exposed each unit is, the
//! most exposed first, instead of by level.
//!
//! Phase 3 fills this in.

use super::app::App;

impl App {
    /// `zs`: the queue by level, or by how exposed each unit is.
    pub(crate) fn toggle_security_order(&mut self) {
        self.status = "The security order isn't built yet.".into();
    }
}
