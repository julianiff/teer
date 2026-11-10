//! The queue's security order (`zs`): units by how exposed each is, the
//! most exposed first. Also what the finish takes of a unit's security:
//! its standing findings and its facts that need a close look.

use super::model::{Audit, Class, Fact, Finding, Weight};
use crate::review::model::{Cite, DiveKind, Risk, Side, Unit};
use crate::util::sanitize;

/// How exposed a unit is. Compared field by field, more is more exposed:
/// the most severe of its standing findings, then the heaviest of its
/// facts, then the most pressing class among both (see [`class_rank`]),
/// then how many there are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Exposure {
    pub severity: Option<Risk>,
    pub weight: Option<Weight>,
    pub class: u8,
    pub count: usize,
}

impl Exposure {
    /// It has a fact or a standing finding: the security order lists it
    /// before the units that have neither.
    pub fn is_exposed(&self) -> bool {
        self.count > 0
    }
}

/// How much a class presses, higher first: authorization, injection and
/// secrets most; crypto, which only the agent raises, least.
pub fn class_rank(c: Class) -> u8 {
    match c {
        Class::Authorization => 9,
        Class::Injection => 8,
        Class::Secret => 7,
        Class::Llm => 6,
        Class::Exposure => 5,
        Class::EntryPoint => 4,
        Class::Ci => 3,
        Class::Config => 2,
        Class::SupplyChain => 1,
        Class::Crypto => 0,
    }
}

/// The class in words, as a finding or a fact is labelled:
/// `authorization`, `entry point`, `supply chain`.
pub fn class_name(c: Class) -> String {
    c.id().replace('_', " ")
}

/// The audit of `u` (the Security tile's answer), when it has one.
pub fn audit(u: &Unit) -> Option<&Audit> {
    u.dive.results.get(&DiveKind::Security)?.audit.as_ref()
}

/// The findings of `u`'s audit that stand (a path the graph confirms, not
/// disputed), most severe first, then most confident.
pub fn standing(u: &Unit) -> Vec<&Finding> {
    let mut found: Vec<&Finding> = audit(u)
        .into_iter()
        .flat_map(|a| &a.findings)
        .filter(|f| f.stands())
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse((f.severity, f.confidence)));
    found
}

/// The facts of `u` at `least` weight or more, heaviest first, then by the
/// class that presses most.
pub fn facts_from(u: &Unit, least: Weight) -> Vec<&Fact> {
    let mut facts: Vec<&Fact> = u.security.iter().filter(|f| f.weight >= least).collect();
    facts.sort_by_key(|f| std::cmp::Reverse((f.weight, class_rank(f.class))));
    facts
}

/// How exposed `u` is: higher comes first in the security order.
pub fn exposure(u: &Unit) -> Exposure {
    let findings = standing(u);
    let classes = findings
        .iter()
        .map(|f| f.class)
        .chain(u.security.iter().map(|f| f.class));
    Exposure {
        severity: findings.iter().map(|f| f.severity).max(),
        weight: u.security.iter().map(|f| f.weight).max(),
        class: classes.map(class_rank).max().unwrap_or(0),
        count: findings.len() + u.security.len(),
    }
}

/// The severity a fact's weight stands for, beside findings': a fact that
/// needs a close look is high.
pub fn severity_of(w: Weight) -> Risk {
    match w {
        Weight::Attention => Risk::High,
        Weight::Look => Risk::Medium,
        Weight::Note => Risk::Low,
    }
}

/// `src/a.php:41`, `src/a.php:41-44`: cited lines on one line, safe to
/// print.
pub fn cite_text(c: &Cite) -> String {
    let file = sanitize(&c.file.to_string_lossy());
    if c.end > c.start {
        format!("{file}:{}-{}", c.start, c.end)
    } else {
        format!("{file}:{}", c.start)
    }
}

/// Where a fact of `u` is: `src/a.php:12-14`, with ` (old)` for lines of
/// the old code; the file alone when it names no lines.
pub fn fact_place(u: &Unit, f: &Fact) -> String {
    let file = sanitize(&u.file.to_string_lossy());
    let at = match f.lines {
        Some(r) if r.end > r.start => format!("{file}:{}-{}", r.start, r.end),
        Some(r) => format!("{file}:{}", r.start),
        None => file,
    };
    if f.side == Side::Old && f.lines.is_some() {
        format!("{at} (old)")
    } else {
        at
    }
}

/// `text` on one line: control characters and runs of blanks become one
/// space. For what the agent or another tool wrote.
pub fn one_line(text: &str) -> String {
    sanitize(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A finding's way from its source to its sink, cite by cite:
/// `a.php:41 → Repo::byId (b.php:88) → b.php:90`.
pub fn path_text(f: &Finding) -> String {
    let mut steps = vec![cite_text(&f.source)];
    for h in &f.path {
        steps.push(format!("{} ({})", one_line(&h.symbol), cite_text(&h.cite)));
    }
    steps.push(cite_text(&f.sink));
    steps.join(" → ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{DiveResult, Range};
    use crate::security::{Hop, SecondCheck};
    use std::path::PathBuf;

    fn unit() -> Unit {
        serde_json::from_value(serde_json::json!({
            "id": "u", "file": "src/a.php", "symbol": "show", "kind": "Function",
            "status": "Changed", "old_range": null, "new_range": null,
            "fingerprint": "f", "changes": [], "callers": [], "callees": [],
            "decision": "Pending"
        }))
        .unwrap()
    }

    fn fact(class: Class, weight: Weight) -> Fact {
        Fact {
            class,
            weight,
            text: format!("{} fact", class.id()),
            side: Side::New,
            lines: Some(Range::new(3, 4)),
            subject: String::new(),
            origin: Default::default(),
        }
    }

    fn cite(line: u32) -> Cite {
        Cite {
            file: PathBuf::from("src/a.php"),
            start: line,
            end: line,
        }
    }

    fn finding(severity: Risk, verified: bool) -> Finding {
        Finding {
            class: Class::Authorization,
            severity,
            confidence: Risk::Medium,
            title: "the id reaches the query".into(),
            source: cite(41),
            path: vec![Hop {
                symbol: "Repo::byId".into(),
                cite: cite(88),
            }],
            sink: cite(90),
            missing: "the owner check".into(),
            exploit: "tenant B reads tenant A's asset".into(),
            path_verified: verified,
            second: None,
            proof: None,
        }
    }

    fn with_findings(mut u: Unit, findings: Vec<Finding>) -> Unit {
        u.dive.results.insert(
            DiveKind::Security,
            DiveResult {
                audit: Some(Audit {
                    findings,
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        u
    }

    #[test]
    fn findings_outrank_facts_then_weight_then_class_then_count() {
        let bare = exposure(&unit());
        assert!(!bare.is_exposed());
        let mut note = unit();
        note.security = vec![fact(Class::SupplyChain, Weight::Note)];
        let mut look = unit();
        look.security = vec![fact(Class::Ci, Weight::Look)];
        let mut look_auth = unit();
        look_auth.security = vec![fact(Class::Authorization, Weight::Look)];
        let mut two = look_auth.clone();
        two.security.push(fact(Class::Ci, Weight::Note));
        let low_finding = with_findings(unit(), vec![finding(Risk::Low, true)]);
        let order = [
            bare,
            exposure(&note),
            exposure(&look),
            exposure(&look_auth),
            exposure(&two),
            exposure(&low_finding),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:#?}");
        assert!(order[1..].iter().all(Exposure::is_exposed));
    }

    #[test]
    fn only_standing_findings_count() {
        let mut disputed = finding(Risk::High, true);
        disputed.second = Some(SecondCheck {
            agree: false,
            why: "checked in the voter".into(),
            evidence: None,
        });
        let u = with_findings(
            unit(),
            vec![
                finding(Risk::High, false),
                disputed,
                finding(Risk::Medium, true),
            ],
        );
        let s = standing(&u);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].severity, Risk::Medium);
        assert_eq!(exposure(&u).severity, Some(Risk::Medium));
        assert_eq!(
            path_text(s[0]),
            "src/a.php:41 → Repo::byId (src/a.php:88) → src/a.php:90"
        );
    }

    #[test]
    fn a_fact_says_where_it_is_and_on_which_side() {
        let u = unit();
        let mut f = fact(Class::Authorization, Weight::Attention);
        assert_eq!(fact_place(&u, &f), "src/a.php:3-4");
        f.side = Side::Old;
        f.lines = Some(Range::new(7, 7));
        assert_eq!(fact_place(&u, &f), "src/a.php:7 (old)");
        f.lines = None;
        assert_eq!(fact_place(&u, &f), "src/a.php");
        assert_eq!(one_line(" a\n\tb  c\u{1b} "), "a b c");
    }
}
