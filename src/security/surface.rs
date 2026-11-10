//! The attack-surface card at the top of the queue: the entry points,
//! sinks, checks, secrets, packages, CI and config the change touches,
//! from the facts of the units to review and their standing findings, and
//! what was checked and what wasn't. "Nothing found" never reads as safe:
//! the card always says what nobody checked.

use serde::Serialize;

use super::model::{Class, Weight};
use super::order::{class_rank, one_line, standing};
use crate::review::model::{Review, Risk, Side, Unit};
use crate::session::names;

/// What teer's facts look for, in the card's words.
pub const CHECKED: [&str; 8] = [
    "auth",
    "entry points",
    "injection",
    "secrets",
    "exposure",
    "packages",
    "ci",
    "config",
];

/// What nobody checks: neither teer's facts nor, by default, the agent.
pub const NOT_CHECKED: [&str; 3] = ["denial of service", "business logic", "crypto details"];

/// The card's rows, in the order it lists them: one per class with
/// something to show.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Surface {
    pub rows: Vec<Row>,
}

/// One class's row: `sinks  raw SQL · outgoing HTTP`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub class: Class,
    /// `entry points`, `sinks`, `checks`.
    pub label: &'static str,
    /// Most severe first.
    pub items: Vec<Item>,
}

/// One thing the change touches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    /// The unit it is on, by id: the most severe one when several units
    /// share the text.
    pub unit: String,
    /// Plain text, one line: a sink's kind, a route, a sentence.
    pub text: String,
    /// What it weighs; a finding's severity as a weight (high is
    /// [`Weight::Attention`]).
    pub weight: Weight,
    /// The agent's finding, standing (not a fact teer found).
    pub finding: bool,
    /// A check that is gone: the fact is on the old side.
    pub gone: bool,
    /// How many units it is on.
    pub units: usize,
}

impl Item {
    /// Compared most severe first: weight, then a finding before a fact.
    fn rank(&self) -> (Weight, bool) {
        (self.weight, self.finding)
    }
}

/// The card's order of classes, each with the row's label.
const ROWS: [(Class, &str); 10] = [
    (Class::EntryPoint, "entry points"),
    (Class::Injection, "sinks"),
    (Class::Authorization, "checks"),
    (Class::Secret, "secrets"),
    (Class::Exposure, "data exposure"),
    (Class::SupplyChain, "packages"),
    (Class::Ci, "ci"),
    (Class::Config, "config"),
    (Class::Crypto, "crypto"),
    (Class::Llm, "llm"),
];

fn weight_of(r: Risk) -> Weight {
    match r {
        Risk::High => Weight::Attention,
        Risk::Medium => Weight::Look,
        Risk::Low => Weight::Note,
    }
}

/// What the change exposes: the facts and standing findings of the units
/// to review (not those accepted for you), by class, most severe first.
/// Items with the same text are one, on the most severe unit.
pub fn surface(review: &Review) -> Surface {
    let mut items: Vec<(Class, Item)> = Vec::new();
    for u in review.units.iter().filter(|u| !u.auto_accepted()) {
        for f in &u.security {
            let text = match f.class {
                // A route, a sink's kind: its name says it.
                Class::EntryPoint | Class::Injection if !f.subject.trim().is_empty() => {
                    one_line(&f.subject)
                }
                // Which unit lost or lacks the check matters most.
                Class::Authorization => {
                    format!("{} ({})", one_line(&f.text), names::row_name(u))
                }
                _ => one_line(&f.text),
            };
            items.push((
                f.class,
                item(
                    u,
                    text,
                    f.weight,
                    false,
                    f.class == Class::Authorization && f.side == Side::Old,
                ),
            ));
        }
        for f in standing(u) {
            items.push((
                f.class,
                item(u, one_line(&f.title), weight_of(f.severity), true, false),
            ));
        }
    }
    let rows = ROWS
        .iter()
        .filter_map(|&(class, label)| {
            let mut row: Vec<Item> = Vec::new();
            for (_, it) in items.iter().filter(|(c, _)| *c == class) {
                match row.iter_mut().find(|r| r.text == it.text) {
                    Some(same) => {
                        same.units += 1;
                        if it.rank() > same.rank() {
                            let units = same.units;
                            *same = it.clone();
                            same.units = units;
                        }
                    }
                    None => row.push(it.clone()),
                }
            }
            // Most severe first; the review's order among equals.
            row.sort_by_key(|it| std::cmp::Reverse(it.rank()));
            (!row.is_empty()).then_some(Row {
                class,
                label,
                items: row,
            })
        })
        .collect();
    Surface { rows }
}

fn item(u: &Unit, text: String, weight: Weight, finding: bool, gone: bool) -> Item {
    Item {
        unit: u.id.clone(),
        text,
        weight,
        finding,
        gone,
        units: 1,
    }
}

impl Surface {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

impl Row {
    /// How severe the row's worst item is, and how much its class presses:
    /// which rows a short card keeps.
    pub fn rank(&self) -> (Weight, bool, u8) {
        let top = self.items.first().map(Item::rank).unwrap_or_default();
        (top.0, top.1, class_rank(self.class))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::Fact;

    fn unit(id: &str) -> Unit {
        serde_json::from_value(serde_json::json!({
            "id": id, "file": "src/a.php", "symbol": id, "kind": "Function",
            "status": "Changed", "old_range": null, "new_range": null,
            "fingerprint": "f", "changes": [], "callers": [], "callees": [],
            "decision": "Pending"
        }))
        .unwrap()
    }

    fn fact(class: Class, weight: Weight, subject: &str, text: &str) -> Fact {
        Fact {
            class,
            weight,
            text: text.into(),
            side: Side::New,
            lines: None,
            subject: subject.into(),
            origin: Default::default(),
        }
    }

    fn review(units: Vec<Unit>) -> Review {
        let mut r: Review = serde_json::from_value(serde_json::json!({
            "id": "r", "repo_root": "/tmp/shop", "base": "HEAD", "head": "WORKTREE",
            "head_commit": "abc", "agent_session": null,
            "created_at": "2026-11-10T00:00:00Z", "files": [], "units": []
        }))
        .unwrap();
        r.units = units;
        r
    }

    #[test]
    fn a_change_without_facts_has_an_empty_surface() {
        assert!(surface(&review(vec![unit("a")])).is_empty());
    }

    #[test]
    fn sinks_are_named_by_kind_once_and_checks_by_their_unit() {
        let mut a = unit("a");
        a.security = vec![
            fact(
                Class::Injection,
                Weight::Look,
                "raw SQL",
                "user input reaches a raw query",
            ),
            fact(
                Class::EntryPoint,
                Weight::Note,
                "POST /automations",
                "a new route",
            ),
        ];
        let mut b = unit("b");
        let mut gone = fact(
            Class::Authorization,
            Weight::Attention,
            "isGranted",
            "isGranted is gone",
        );
        gone.side = Side::Old;
        b.security = vec![
            fact(
                Class::Injection,
                Weight::Attention,
                "raw SQL",
                "another raw query",
            ),
            gone,
        ];
        let s = surface(&review(vec![a, b]));
        let labels: Vec<&str> = s.rows.iter().map(|r| r.label).collect();
        assert_eq!(labels, ["entry points", "sinks", "checks"]);
        let sinks = &s.rows[1].items;
        assert_eq!(sinks.len(), 1);
        assert_eq!((sinks[0].text.as_str(), sinks[0].units), ("raw SQL", 2));
        // The most severe unit's.
        assert_eq!(
            (sinks[0].unit.as_str(), sinks[0].weight),
            ("b", Weight::Attention)
        );
        let check = &s.rows[2].items[0];
        assert_eq!(check.text, "isGranted is gone (b)");
        assert!(check.gone);
    }
}
