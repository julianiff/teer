//! The queue's security order (`zs`): units by how exposed they are, the
//! most exposed first.
//!
//! Phase 3 fills this in.

use crate::review::model::Unit;

/// How exposed `u` is: higher comes first in the security order. Today
/// the heaviest of its facts, then how many it has.
pub fn exposure(u: &Unit) -> (u8, usize) {
    let heaviest = u
        .security
        .iter()
        .map(|f| f.weight as u8 + 1)
        .max()
        .unwrap_or(0);
    (heaviest, u.security.len())
}
