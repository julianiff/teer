//! How much attention a unit needs. Facts from the code decide first
//! (removed but still called, signature changed under callers, size); the
//! agent's risk can only raise the level, never lower it. Every rule that
//! fired says where it came from, so the grouping can be checked.
//!
//! Three more signals are written out as sentences, the unit's `reasons`
//! (see [`settle`]): a change that is expensive to undo (a migration, or the
//! agent's word with the lines that show it), a second model that disputes
//! the description, and a change the commits or the PR description don't
//! mention. The first two make a unit worth a look at least; a dispute
//! lifts a unit with no behavior change to routine.
//!
//! [`settle`] also makes teer's own decisions: a unit with no behavior
//! change is accepted for the reviewer (`auto_accept`), unless the reviewer
//! has decided about it.

use std::collections::HashSet;

use serde::Serialize;

use super::model::{AcceptedBy, Decision, Doubt, Prep, Review, Risk, Unit, UnitKind, UnitStatus};
use crate::config::AutoAccept;
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

    /// The level a unit at this one is shown at. The units to review show
    /// three levels: one with no behavior change that waits for the
    /// reviewer (`auto_accept = "none"`, or taken back) is routine. Only one
    /// `auto_accepted` stays at `Same`.
    pub fn shown(self, auto_accepted: bool) -> Level {
        if auto_accepted {
            Level::Same
        } else {
            self.max(Level::Routine)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Assessment {
    pub level: Level,
    /// The rules that set the level: facts first, then the agent's view
    /// (prefixed "agent:").
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

/// Where the unit's second opinion disagrees with its description.
fn doubts_of(u: &Unit) -> Vec<String> {
    u.checks
        .second
        .as_ref()
        .map(|s| s.disagreements(&u.changes))
        .unwrap_or_default()
}

pub fn assess(review: &Review, u: &Unit) -> Assessment {
    if u.status == UnitStatus::NoBehaviorChange {
        let reason = u
            .nbc_reason
            .clone()
            .unwrap_or_else(|| "no behavior change".into());
        // The code says nothing changed; a second model that disagrees
        // makes it routine, so someone looks.
        let doubts = doubts_of(u);
        let level = if doubts.is_empty() {
            Level::Same
        } else {
            Level::Routine
        };
        return Assessment {
            level,
            reasons: vec![reason],
            cost: None,
            doubts,
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
    let doubts = doubts_of(u);
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

/// Every unit's assessment, by index into `review.units`.
pub fn assess_all(review: &Review) -> Vec<Assessment> {
    review.units.iter().map(|u| assess(review, u)).collect()
}

/// The written reasons of `u` assessed as `a`, one sentence each: expensive
/// to undo, each point a second model disputes (quoted), and a change the
/// commits or PR description don't mention.
pub fn written_reasons(u: &Unit, a: &Assessment) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(why) = &a.cost {
        out.push(format!("Expensive to undo: {}", sentence(why)));
    }
    if let Some(second) = &u.checks.second {
        for d in second.doubts(&u.changes) {
            out.push(match d {
                Doubt::Disputes { statement, why } => format!(
                    "The second model disputes \"{}\": \"{}\"",
                    crate::util::truncate(statement, 48),
                    why.trim()
                ),
                Doubt::Missed(statement) => format!(
                    "The second model found a change the description leaves out: \"{}\"",
                    statement.trim()
                ),
                Doubt::Risk { theirs, had } => format!(
                    "The second model rates it {} risk, not {}.",
                    theirs.word(),
                    had.word()
                ),
            });
        }
    }
    if let Some(gap) = &u.checks.unmentioned {
        out.push(format!(
            "Not mentioned in the commits or PR description: {}",
            sentence(&gap.text)
        ));
    }
    out
}

/// `text` trimmed, ending in a full stop unless it ends in punctuation.
fn sentence(text: &str) -> String {
    let t = text.trim();
    if t.ends_with(['.', '!', '?', '"', ')']) {
        t.to_string()
    } else {
        format!("{t}.")
    }
}

/// Is a unit at `level` accepted for the reviewer under `auto`?
pub fn accepted_for_you(level: Level, auto: AutoAccept) -> bool {
    auto == AutoAccept::NoBehaviorChange && level == Level::Same
}

/// Write down what the levels decide, `levels` being [`assess_all`] of
/// `review`: each unit's [`written_reasons`], and who accepted it.
///
/// teer's own decision holds for a unit the reviewer hasn't decided about
/// (no step in the undo history names it): with `auto_accept =
/// "no_behavior_change"`, a unit at [`Level::Same`] is accepted for the
/// reviewer (`AcceptedBy::Auto`). A unit accepted for the reviewer that is
/// no longer at that level (a second model disputes it), or with
/// `auto_accept = "none"`, goes back to pending. Any other acceptance is
/// the reviewer's.
pub fn settle(review: &mut Review, levels: &[Assessment], auto: AutoAccept) {
    let decided: HashSet<&str> = review
        .history
        .undo
        .iter()
        .flat_map(|s| s.changes.iter().map(|c| c.id.as_str()))
        .collect();
    for (u, a) in review.units.iter_mut().zip(levels) {
        u.reasons = written_reasons(u, a);
        if u.decision != Decision::Accepted {
            u.accepted_by = None;
        }
        let mine = accepted_for_you(a.level, auto);
        if u.auto_accepted() && !mine {
            u.decision = Decision::Pending;
            u.accepted_by = None;
        }
        if mine && !decided.contains(u.id.as_str()) && u.decision != Decision::Investigate {
            u.decision = Decision::Accepted;
            u.accepted_by = Some(AcceptedBy::Auto);
        }
        if u.decision == Decision::Accepted && u.accepted_by.is_none() {
            u.accepted_by = Some(AcceptedBy::Reviewer);
        }
    }
}

impl Review {
    /// Assess every unit and [`settle`] what that decides, for output
    /// without the TUI (`--json`).
    pub fn settle(&mut self, auto: AutoAccept) {
        let levels = assess_all(self);
        settle(self, &levels, auto);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::history::{Change as Decided, Step};
    use crate::review::model::{
        Change, Cost, LineRef, Marker, SecondOpinion, Side, Unmentioned, Verdict,
    };

    /// A pending unit `id` in `file`, as `status`, not sent to an agent.
    fn unit_at(id: &str, file: &str, status: &str) -> Unit {
        serde_json::from_value(serde_json::json!({
            "id": id, "file": file, "symbol": id, "kind": "Function",
            "status": status, "old_range": null, "new_range": null,
            "fingerprint": "x", "changes": [], "callers": [], "callees": [],
            "decision": "Pending", "prep": "Skipped"
        }))
        .unwrap()
    }

    fn review_of(units: Vec<Unit>) -> Review {
        let mut r: Review = serde_json::from_value(serde_json::json!({
            "id": "r", "repo_root": "/r", "base": "b", "head": "h", "head_commit": "h",
            "agent_session": null, "created_at": "2026-10-16T00:00:00Z", "files": [], "units": []
        }))
        .unwrap();
        r.units = units;
        r
    }

    fn at(line: u32) -> Vec<LineRef> {
        vec![LineRef {
            side: Side::New,
            line,
        }]
    }

    /// A second opinion that disputes the unit's one statement.
    fn disputing() -> SecondOpinion {
        SecondOpinion {
            risk: Risk::Low,
            verdicts: vec![Verdict {
                statement: "Only renames a variable.".into(),
                agree: false,
                why: "the retry wasn't removed, it moved to client.ts".into(),
                evidence: at(3),
            }],
            missed: Vec::new(),
        }
    }

    #[test]
    fn each_mark_becomes_a_written_reason() {
        let mut u = unit_at("f", "src/pay.py", "Changed");
        u.changes = vec![Change {
            marker: Marker::Changed,
            statement: "Retries the charge.".into(),
            evidence: at(2),
            risk: Risk::Low,
        }];
        u.checks.cost = Some(Cost {
            why: "Charges the card again on retry.".into(),
            evidence: at(2),
        });
        u.checks.second = Some(SecondOpinion {
            risk: Risk::High,
            verdicts: vec![Verdict {
                statement: "Retries the charge.".into(),
                agree: false,
                why: "it also stops sending the receipt".into(),
                evidence: at(4),
            }],
            missed: vec![Change {
                marker: Marker::Removed,
                statement: "Drops the audit log entry.".into(),
                evidence: at(5),
                risk: Risk::Medium,
            }],
        });
        u.checks.unmentioned = Some(Unmentioned {
            text: "Also stops sending the receipt email".into(),
            evidence: at(4),
        });
        let review = review_of(vec![u.clone()]);
        let a = assess(&review, &u);
        assert_eq!(
            written_reasons(&u, &a),
            [
                "Expensive to undo: Charges the card again on retry.",
                "The second model disputes \"Retries the charge.\": \"it also stops sending the receipt\"",
                "The second model found a change the description leaves out: \"Drops the audit log entry.\"",
                "The second model rates it high risk, not low.",
                "Not mentioned in the commits or PR description: Also stops sending the receipt email.",
            ]
        );
        // A migration is expensive to undo by its path.
        let m = unit_at("up", "db/migrations/1_add.py", "Changed");
        let a = assess(&review, &m);
        assert_eq!(
            written_reasons(&m, &a),
            ["Expensive to undo: a database migration."]
        );
        // Nothing found, nothing written.
        let plain = unit_at("g", "src/a.py", "Changed");
        assert!(written_reasons(&plain, &assess(&review, &plain)).is_empty());
    }

    #[test]
    fn a_disputed_unit_with_no_behavior_change_is_routine() {
        let mut u = unit_at("f", "src/a.py", "NoBehaviorChange");
        u.nbc_reason = Some("local names renamed".into());
        let review = review_of(vec![u.clone()]);
        assert_eq!(assess(&review, &u).level, Level::Same);
        u.checks.second = Some(disputing());
        let a = assess(&review, &u);
        assert_eq!(a.level, Level::Routine);
        assert_eq!(a.reasons, ["local names renamed"]);
        assert_eq!(
            written_reasons(&u, &a),
            [
                "The second model disputes \"Only renames a variable.\": \"the retry wasn't removed, it moved to client.ts\""
            ]
        );
    }

    #[test]
    fn units_with_no_behavior_change_are_accepted_for_you_unless_turned_off() {
        let units = vec![
            unit_at("same", "a.py", "NoBehaviorChange"),
            unit_at("changed", "a.py", "Changed"),
        ];
        let mut on = review_of(units.clone());
        on.settle(AutoAccept::NoBehaviorChange);
        assert_eq!(on.units[0].decision, Decision::Accepted);
        assert_eq!(on.units[0].accepted_by, Some(AcceptedBy::Auto));
        assert_eq!(
            (on.units[1].decision, on.units[1].accepted_by),
            (Decision::Pending, None)
        );
        let mut off = review_of(units);
        off.settle(AutoAccept::None);
        assert_eq!(
            (off.units[0].decision, off.units[0].accepted_by),
            (Decision::Pending, None)
        );
        // Turned off later, an acceptance for you is taken back.
        on.settle(AutoAccept::None);
        assert_eq!(
            (on.units[0].decision, on.units[0].accepted_by),
            (Decision::Pending, None)
        );
    }

    #[test]
    fn a_dispute_takes_an_acceptance_for_you_back() {
        let mut r = review_of(vec![unit_at("same", "a.py", "NoBehaviorChange")]);
        r.settle(AutoAccept::NoBehaviorChange);
        assert!(r.units[0].auto_accepted());
        r.units[0].checks.second = Some(disputing());
        r.settle(AutoAccept::NoBehaviorChange);
        assert_eq!(
            (r.units[0].decision, r.units[0].accepted_by),
            (Decision::Pending, None)
        );
        assert_eq!(r.units[0].reasons.len(), 1);
    }

    #[test]
    fn the_reviewers_decision_beats_teers_and_acceptances_are_theirs() {
        let mut r = review_of(vec![
            unit_at("same", "a.py", "NoBehaviorChange"),
            unit_at("changed", "a.py", "Changed"),
        ]);
        // The reviewer took the acceptance for you back: it stays pending.
        r.history.record(Step {
            changes: vec![Decided {
                id: "same".into(),
                before: Decision::Accepted,
                after: Decision::Pending,
            }],
            ..Default::default()
        });
        r.units[1].decision = Decision::Accepted;
        r.settle(AutoAccept::NoBehaviorChange);
        assert_eq!(
            (r.units[0].decision, r.units[0].accepted_by),
            (Decision::Pending, None)
        );
        assert_eq!(r.units[1].accepted_by, Some(AcceptedBy::Reviewer));
        // Once that step is undone, teer's decision holds again.
        r.history.undo();
        r.settle(AutoAccept::NoBehaviorChange);
        assert!(r.units[0].auto_accepted());
        // A flag is never overruled.
        r.units[0].decision = Decision::Investigate;
        r.settle(AutoAccept::NoBehaviorChange);
        assert_eq!(
            (r.units[0].decision, r.units[0].accepted_by),
            (Decision::Investigate, None)
        );
    }

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
