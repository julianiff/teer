//! How much attention a unit needs. Facts from the code decide first
//! (removed but still called, signature changed under callers, size); the
//! agent's risk can only raise the level, never lower it. Every reason says
//! where it came from, so the grouping can be checked.
//!
//! Two more signals are kept apart from the reasons, each on a line of its
//! own: a change that is expensive to undo (a migration, or the agent's
//! word with the lines that show it), and a second model that disagrees with
//! the description. Either makes a unit worth a look at least.

use serde::Serialize;

use super::model::{Prep, Review, Risk, Unit, UnitKind, UnitStatus};
use crate::units::whole;
use crate::util::{DiffOp, diff_lines};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Level {
    /// Formatting, comments, renames: found from the code.
    Same,
    /// Low risk, nothing in the code says otherwise.
    Routine,
    Look,
    Attention,
}

impl Level {
    pub fn title(self) -> &'static str {
        match self {
            Level::Same => "No behavior change",
            Level::Routine => "Routine",
            Level::Look => "Worth a look",
            Level::Attention => "Needs a close look",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Level::Same => "○",
            Level::Routine => "●",
            Level::Look => "◆",
            Level::Attention => "▲",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Assessment {
    pub level: Level,
    /// Facts first, then the agent's view (prefixed "agent:").
    pub reasons: Vec<String>,
    /// Why undoing it after it ships would be costly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<String>,
    /// Where the second opinion disagrees with the description, most important first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub doubts: Vec<String>,
}

/// A database migration, found from the path: a folder named `migrations`,
/// `migration` or `migrate`.
fn migration(u: &Unit) -> bool {
    u.file.parent().is_some_and(|dir| {
        dir.components().any(|c| {
            let c = c.as_os_str().to_string_lossy().to_ascii_lowercase();
            matches!(c.as_str(), "migrations" | "migration" | "migrate")
        })
    })
}

/// Changes this many lines or more always get a look.
const BIG_CHANGE: usize = 40;

fn lines_of(content: Option<&str>, r: Option<super::model::Range>) -> Vec<&str> {
    match (content, r) {
        (Some(c), Some(r)) => c
            .lines()
            .skip(r.start as usize - 1)
            .take(r.len() as usize)
            .collect(),
        _ => Vec::new(),
    }
}

/// Lines added plus lines removed within the unit.
fn changed_lines(review: &Review, u: &Unit) -> usize {
    let Some(f) = review.file_of(u) else { return 0 };
    let old = lines_of(f.old_content.as_deref(), u.old_range);
    let new = lines_of(f.new_content.as_deref(), u.new_range);
    diff_lines(&old, &new)
        .iter()
        .filter(|d| d.0 != DiffOp::Equal)
        .count()
}

/// The declaration up to its body, whitespace-insensitive.
fn signature(lines: &[&str]) -> String {
    let mut sig = String::new();
    for l in lines.iter().take(8) {
        sig.push_str(l.trim());
        if l.contains('{') || l.trim_end().ends_with(':') || l.contains("=>") {
            break;
        }
    }
    let sig = sig.split('{').next().unwrap_or("").to_string();
    sig.chars().filter(|c| !c.is_whitespace()).collect()
}

fn signature_changed(review: &Review, u: &Unit) -> bool {
    if !matches!(u.kind, UnitKind::Function | UnitKind::Method) || u.status != UnitStatus::Changed {
        return false;
    }
    let Some(f) = review.file_of(u) else {
        return false;
    };
    let old = lines_of(f.old_content.as_deref(), u.old_range);
    let new = lines_of(f.new_content.as_deref(), u.new_range);
    signature(&old) != signature(&new)
}

fn callers(n: usize) -> String {
    format!("{n} caller{}", if n == 1 { "" } else { "s" })
}

pub fn assess(review: &Review, u: &Unit) -> Assessment {
    if u.status == UnitStatus::NoBehaviorChange {
        let reason = u
            .nbc_reason
            .clone()
            .unwrap_or_else(|| "no behavior change".into());
        return Assessment {
            level: Level::Same,
            reasons: vec![reason],
            cost: None,
            doubts: Vec::new(),
        };
    }
    let mut level = Level::Routine;
    let mut reasons = Vec::new();
    let n_callers = u.callers.len();

    // Facts from the code.
    if u.status == UnitStatus::Removed {
        if u.kind == UnitKind::File {
            reasons.push("file deleted".into());
        } else if n_callers > 0 {
            level = Level::Attention;
            let verb = if n_callers == 1 { "calls" } else { "call" };
            reasons.push(format!("removed, {} still {verb} it", callers(n_callers)));
        } else {
            reasons.push("removed, no callers found".into());
        }
    }
    if signature_changed(review, u) && n_callers > 0 {
        level = level.max(Level::Look);
        reasons.push(format!("signature changed, {}", callers(n_callers)));
    }
    let size = changed_lines(review, u);
    // A lock file or a test snapshot is routine whatever its size.
    let generated = (u.kind == UnitKind::File)
        .then(|| whole::generated(&u.file))
        .flatten();
    if let Some(what) = generated {
        reasons.push(what.into());
    } else if size >= BIG_CHANGE {
        level = level.max(Level::Look);
        reasons.push(format!("{size} lines changed"));
    }
    // A whole file this large isn't sent to the agent: someone has to read it.
    let too_large = u.kind == UnitKind::File
        && generated.is_none()
        && review.file_of(u).is_some_and(whole::too_large);
    if too_large {
        level = level.max(Level::Look);
        reasons.push("too large to describe; read the diff".into());
    }

    // The agent's view.
    match u.prep {
        Prep::Ready => match u.highest_risk() {
            Some(Risk::High) => {
                level = Level::Attention;
                reasons.push("agent: high risk".into());
            }
            Some(Risk::Medium) => {
                level = level.max(Level::Look);
                reasons.push("agent: medium risk".into());
            }
            _ => {}
        },
        // Without a description, don't call it routine: the code has to be read.
        Prep::Unavailable | Prep::NotRequested
            if u.status == UnitStatus::Changed || n_callers > 0 =>
        {
            level = level.max(Level::Look);
            reasons.push("not described, read the code".into());
        }
        _ => {}
    }
    // Expensive to undo, and a second opinion that disagrees.
    let cost = match &u.checks.cost {
        Some(c) => Some(c.why.clone()),
        None => migration(u).then(|| "a database migration".to_string()),
    };
    let doubts = u
        .checks
        .second
        .as_ref()
        .map(|s| s.disagreements(&u.changes))
        .unwrap_or_default();
    if cost.is_some() || !doubts.is_empty() {
        level = level.max(Level::Look);
    }
    if level >= Level::Look && n_callers > 0 && !reasons.iter().any(|r| r.contains("caller")) {
        reasons.insert(0, callers(n_callers));
    }
    Assessment {
        level,
        reasons,
        cost,
        doubts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_migration_folder_makes_a_change_expensive_to_undo() {
        let unit = |file: &str| -> Unit {
            serde_json::from_value(serde_json::json!({
                "id": "x", "file": file, "symbol": "up", "kind": "Function",
                "status": "Changed", "old_range": null, "new_range": null,
                "fingerprint": "x", "changes": [], "callers": [], "callees": [],
                "decision": "Pending", "prep": "Skipped"
            }))
            .unwrap()
        };
        let review: Review = serde_json::from_value(serde_json::json!({
            "id": "r", "repo_root": "/r", "base": "b", "head": "h", "head_commit": "h",
            "agent_session": null, "created_at": "2026-10-16T00:00:00Z", "files": [], "units": []
        }))
        .unwrap();
        for (file, expensive) in [
            ("database/migrations/2026_10_16_add_flag.php", true),
            ("db/migrate/20261016_add_flag.rb.py", true),
            ("src/Migrations/Version1.php", true),
            ("cli/Command/MigrateToFfyAutomations.php", false),
            ("src/billing.py", false),
        ] {
            let a = assess(&review, &unit(file));
            assert_eq!(a.cost.is_some(), expensive, "{file}");
            assert_eq!(a.level >= Level::Look, expensive, "{file}");
        }
    }

    #[test]
    fn signature_ignores_whitespace_and_body() {
        assert_eq!(
            signature(&["def f(a, b):", "    return a"]),
            signature(&["def f(a,  b):", "  pass"])
        );
        assert_ne!(
            signature(&["fn f(a: u32) {"]),
            signature(&["fn f(a: u64) {"])
        );
        assert_eq!(
            signature(&["function f(", "  $a,", ") {"]),
            "functionf($a,)"
        );
    }
}
