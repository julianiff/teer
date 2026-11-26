//! The security lens on the queue (`zs`): the attack-surface card over
//! the tree, shown or hidden (see [`crate::security::surface`]).

use super::app::App;
use crate::security::SecurityConfig;

/// The security lens on the queue.
#[derive(Debug, Clone, Default)]
pub struct SecurityLens {
    /// The lens is on (`security.toml`'s `enabled`, read once at start):
    /// `zs` works and the security mode shows the card.
    pub enabled: bool,
    /// The attack-surface card is over the tree (`zs`, or the security
    /// mode).
    pub surface: bool,
}

impl SecurityLens {
    /// The lens as `security.toml` sets it, the card hidden.
    pub fn load() -> SecurityLens {
        SecurityLens {
            enabled: SecurityConfig::load().enabled,
            ..Default::default()
        }
    }
}

impl App {
    /// `zs`: show the attack-surface card over the tree, or hide it.
    pub(crate) fn toggle_surface(&mut self) {
        if !self.security.enabled {
            self.status = "The security lens is off (enabled = false in security.toml).".into();
            return;
        }
        self.security.surface = !self.security.surface;
    }
}
