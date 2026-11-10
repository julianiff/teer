//! `<space>p`: prove the unit's security findings, each attacker's case
//! run old against new (see `security::prove`).
//!
//! Phase 4 fills this in.

use super::app::{App, Effect};

impl App {
    /// `<space>p`: prove the security findings of the unit on show (or
    /// under the cursor).
    pub(crate) fn prove_findings(&mut self) -> Vec<Effect> {
        self.status = "Proving findings isn't built yet.".into();
        Vec::new()
    }
}
