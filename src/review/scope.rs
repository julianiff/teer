//! What an answer about a unit may cite, and names for the folders teer
//! keeps per review and unit.

use std::collections::HashMap;

use super::model::{Review, Unit};
use super::validate::{Region, unit_regions};
use crate::util::{hash_hex, sanitize};

/// The code an answer about `unit` may cite: the unit's own lines, then
/// every caller's and callee's lines from the call graph.
pub fn scope(review: &Review, unit: &Unit) -> Vec<Region> {
    let mut out = review
        .file_of(unit)
        .map(|f| unit_regions(unit, f))
        .unwrap_or_default();
    let nodes: HashMap<&str, _> = review
        .graph
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect();
    for (ids, role) in [(&unit.callers, "caller"), (&unit.callees, "callee")] {
        for n in ids.iter().filter_map(|id| nodes.get(id.as_str())) {
            let r = Region {
                file: n.file.clone(),
                range: n.range,
                role: format!("{role} {}", sanitize(&n.symbol)),
            };
            if !out.contains(&r) {
                out.push(r);
            }
        }
    }
    out
}

/// `[A-Za-z0-9._-]` only, runs of anything else as one `_`, no leading dot.
pub(crate) fn safe_name(s: &str, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
            c
        } else {
            '_'
        };
        if !(c == '_' && out.ends_with('_')) {
            out.push(c);
        }
        if out.len() >= max {
            break;
        }
    }
    let out = out.trim_start_matches('.');
    if out.is_empty() {
        "unit".into()
    } else {
        out.into()
    }
}

/// A folder name for a unit. The hash keeps ids that clean up to the same
/// name (`a/b::c`, `a_b::c`) apart.
pub(crate) fn unit_name(unit_id: &str) -> String {
    format!(
        "{}-{}",
        safe_name(unit_id, 80),
        &hash_hex(&[unit_id.as_bytes()])[..8]
    )
}
