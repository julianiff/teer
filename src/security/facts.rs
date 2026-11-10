//! What teer finds in a change without an agent: an authorization check
//! removed or missing where its siblings have it, a new entry point,
//! untrusted input reaching a sink, a secret, a package change, a CI
//! workflow or security config loosened. Each becomes a [`Fact`] on its
//! unit, and facts raise levels (`attention::assess`).
//!
//! Phase 1 fills this in.

use super::config::SecurityConfig;
#[allow(unused_imports)]
use super::model::Fact;
use crate::review::model::Review;

/// Write each unit's security facts ([`crate::review::model::Unit::security`])
/// from the change and the call graph. With the lens off, none.
pub fn annotate(review: &mut Review, cfg: &SecurityConfig) {
    for u in &mut review.units {
        u.security.clear();
    }
    if cfg.enabled {
        // Phase 1.
    }
}
