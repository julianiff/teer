//! `<space>p`: prove the unit's security findings, each attacker's case
//! run old against new (see `security::prove`).

use super::app::{App, Effect, Screen};
use super::unit::Panel;
use crate::keymap::Action;
use crate::review::model::DiveKind;

impl App {
    /// `<space>p`: prove the security findings of the unit on show (or
    /// under the cursor) that have no proof yet. It takes an agent and an
    /// audit with such findings, and one proof at a time per unit; the
    /// status line says why not. The Security tile, when it is open, gets
    /// the focus.
    pub(crate) fn prove_findings(&mut self) -> Vec<Effect> {
        let Some(id) = self.current_unit().map(|u| u.id.clone()) else {
            return Vec::new();
        };
        if !self.can_ask() {
            self.status = "No agent: proving needs one.".into();
            return Vec::new();
        }
        let ask = self.keys.label(Action::ToolSecurity);
        let Some(audit) = self.review.audit_mut(&id) else {
            self.status = format!("No findings to prove: {ask} asks for them first.");
            return Vec::new();
        };
        if audit.proving {
            self.status = "Already proving: the findings are running old against new.".into();
            return Vec::new();
        }
        let todo = audit.findings.iter().filter(|f| f.proof.is_none()).count();
        if audit.findings.is_empty() {
            self.status = format!("No findings to prove: the agent found none ({ask}).");
            return Vec::new();
        }
        if todo == 0 {
            self.status = "Nothing left to prove: every finding ran old against new.".into();
            return Vec::new();
        }
        audit.proving = true;
        let tile = Panel::Tool(DiveKind::Security);
        if self.screen == Screen::Unit && self.unit.id == id && self.unit.tiles.contains(&tile) {
            self.unit.panel = Some(tile);
            self.unit.typing = false;
            self.ask.link = None;
        }
        let s = if todo == 1 { "" } else { "s" };
        self.status = format!("Proving {todo} finding{s} old against new…");
        vec![Effect::ProveFindings { unit_id: id }]
    }
}
