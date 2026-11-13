//! What is written into a unit's code change, under the rows it cites: a
//! [`Comment`] each. The mode picks the question the comments answer:
//!
//! - **review**: what does it do, and is it right? Each statement under its risk,
//!   with the second model's dispute; then what the description left out,
//!   what is expensive to undo, what the author didn't mention, and the
//!   cases no test covers.
//! - **security**: how can it be abused? The security facts, the possible
//!   risks the description's model saw in each hunk, and each
//!   finding at its source, the calls on its path and its sink. Nothing
//!   else: the statements are left out.
//! - **all**: the comments of review, then those of security.
//!
//! Only data the review already holds: no agent is asked.

use super::model::{Change, Cite, DiveKind, LineRef, Marker, Risk, Side, Unit};
use crate::mode::Mode;
use crate::security::{Fact, Finding, Origin, Proof, Suspect, Weight};

/// A cite longer than this is anchored by its first and last line only,
/// so the code change doesn't unfold for it.
const CITE_LINES: u32 = 6;

/// One card in the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// On the card's top border: `medium · disputed`, `sink · injection ·
    /// high`. Empty for a statement as it is.
    pub title: String,
    /// How much it weighs, when it says so: the title's colour.
    pub risk: Option<Risk>,
    /// A statement's `+ ~ −`; nothing for the rest.
    pub marker: Option<Marker>,
    /// The card's text, a paragraph each. Never empty.
    pub text: Vec<String>,
    /// The lines it is about: it is written under the last one shown.
    pub evidence: Vec<LineRef>,
}

/// The comments of `u` as `mode` has them, in the order they are written
/// in where several sit under one row.
pub fn comments(u: &Unit, mode: Mode) -> Vec<Comment> {
    let mut out = Vec::new();
    match mode {
        Mode::Review => judged(u, &mut out),
        Mode::Security => abuse(u, &mut out),
        Mode::All => {
            judged(u, &mut out);
            abuse(u, &mut out);
        }
    }
    out
}

/// The agent's statements, once it has made them, each under its risk
/// and with the second model's dispute.
fn statements(u: &Unit, out: &mut Vec<Comment>) {
    if u.prep != super::model::Prep::Ready {
        return;
    }
    out.extend(u.changes.iter().map(|c| statement(u, c)));
}

fn statement(u: &Unit, c: &Change) -> Comment {
    let mut title = c.risk.word().to_string();
    let mut text = vec![c.statement.clone()];
    let disputes = u.checks.second.iter().flat_map(|s| &s.verdicts);
    for v in disputes.filter(|v| !v.agree && v.statement == c.statement) {
        title.push_str(" · disputed");
        text.push(format!("The second model disputes it: {}", v.why));
    }
    Comment {
        title,
        risk: Some(c.risk),
        marker: Some(c.marker),
        text,
        evidence: c.evidence.clone(),
    }
}

/// Review: the statements under their risk, then everything else there is
/// to judge about the unit that has lines.
fn judged(u: &Unit, out: &mut Vec<Comment>) {
    statements(u, out);
    if let Some(second) = &u.checks.second {
        for m in second.missed.iter().filter(|m| m.risk >= Risk::Medium) {
            out.push(Comment {
                title: format!("missed · {}", m.risk.word()),
                risk: Some(m.risk),
                marker: None,
                text: vec![
                    m.statement.clone(),
                    "A second model found it; the description leaves it out.".into(),
                ],
                evidence: m.evidence.clone(),
            });
        }
    }
    if let Some(cost) = &u.checks.cost {
        out.push(Comment {
            title: "expensive to undo".into(),
            risk: Some(Risk::High),
            marker: None,
            text: vec![cost.why.clone()],
            evidence: cost.evidence.clone(),
        });
    }
    if let Some(un) = &u.checks.unmentioned {
        out.push(Comment {
            title: "not mentioned".into(),
            risk: Some(Risk::Medium),
            marker: None,
            text: vec![
                un.text.clone(),
                "The commits and the description don't say so.".into(),
            ],
            evidence: un.evidence.clone(),
        });
    }
    let coverage = u.dive.results.get(&DiveKind::Tests);
    let gaps = coverage
        .and_then(|r| r.coverage.as_ref())
        .filter(|c| c.judged)
        .map_or(&[][..], |c| &c.gaps[..]);
    for g in gaps {
        let evidence: Vec<LineRef> = g.evidence.iter().flat_map(|c| lines_in(u, c)).collect();
        // A gap shown by other code has no row here: the Tests tile has it.
        if evidence.is_empty() {
            continue;
        }
        out.push(Comment {
            title: format!("untested · {}", g.risk.word()),
            risk: Some(g.risk),
            marker: None,
            text: vec![format!("No test covers: {}", g.case), g.danger.clone()],
            evidence,
        });
    }
}

/// Security: the facts teer found, and each finding where it touches the
/// unit.
fn abuse(u: &Unit, out: &mut Vec<Comment>) {
    out.extend(u.security.iter().filter_map(fact));
    out.extend(u.checks.suspects.iter().map(suspect));
    let audit = u.dive.results.get(&DiveKind::Security);
    for f in audit
        .and_then(|r| r.audit.as_ref())
        .iter()
        .flat_map(|a| &a.findings)
    {
        finding(u, f, out);
    }
}

/// A fact with lines; one without has no row to sit under ("why this
/// level" lists it).
fn fact(f: &Fact) -> Option<Comment> {
    let lines = f.lines?;
    let mut title = f.class.word().to_string();
    if let Origin::Import { tool, .. } = &f.origin {
        title = format!("{tool} · {title}");
    }
    Some(Comment {
        title,
        risk: match f.weight {
            Weight::Note => None,
            Weight::Look => Some(Risk::Medium),
            Weight::Attention => Some(Risk::High),
        },
        marker: None,
        text: vec![f.text.clone()],
        evidence: span(f.side, lines.start, lines.end),
    })
}

/// A possible risk the description's model saw in the unit's code: said as
/// possible, since nothing confirmed it.
fn suspect(s: &Suspect) -> Comment {
    Comment {
        title: format!("possible · {} · {}", s.class.word(), s.severity.word()),
        risk: Some(s.severity),
        marker: None,
        text: vec![s.text.clone()],
        evidence: s.evidence.clone(),
    }
}

/// A finding as a card at each place of its path inside `u`: where the
/// untrusted value enters, the calls it goes through, where it does harm.
/// The card at the sink says the whole of it.
fn finding(u: &Unit, f: &Finding, out: &mut Vec<Comment>) {
    let what = format!("{} · {}", f.class.word(), f.severity.word());
    let card = |title: String, text: Vec<String>, evidence: Vec<LineRef>| Comment {
        title,
        risk: Some(f.severity),
        marker: None,
        text,
        evidence,
    };
    let sink = lines_in(u, &f.sink);
    let source = lines_in(u, &f.source);
    let same = f.source == f.sink;
    if !source.is_empty() && !same {
        let to = if sink.is_empty() {
            format!(" It reaches {}.", place(&f.sink))
        } else {
            String::new()
        };
        out.push(card(
            format!("source · {what}"),
            vec![
                format!("An untrusted value enters here.{to}"),
                f.title.clone(),
            ],
            source,
        ));
    }
    for (k, h) in f.path.iter().enumerate() {
        let at = lines_in(u, &h.cite);
        if at.is_empty() || h.cite == f.sink || h.cite == f.source {
            continue;
        }
        out.push(card(
            format!("hop {}/{} · {what}", k + 1, f.path.len()),
            vec![format!(
                "{} passes the value on, towards {}.",
                h.symbol,
                place(&f.sink)
            )],
            at,
        ));
    }
    if sink.is_empty() {
        return;
    }
    let role = if same { "source and sink" } else { "sink" };
    let mut text = vec![f.title.clone(), format!("Missing: {}", f.missing)];
    let hops = match f.path.len() {
        0 => "no calls between".to_string(),
        1 => "1 hop".to_string(),
        n => format!("{n} hops"),
    };
    let verified = if f.path_verified {
        "verified"
    } else {
        "not verified"
    };
    text.push(format!("Path: {hops}, {verified}."));
    if let Some(s) = f.second.as_ref().filter(|s| !s.agree) {
        text.push(format!("The second model disputes it: {}", s.why));
    }
    match &f.proof {
        Some(Proof::Proven { case, .. }) => text.push(format!("Proven: {case}")),
        Some(Proof::NotShown { case, .. }) => text.push(format!("Not shown by: {case}")),
        Some(Proof::Unproven { .. }) | None => {}
    }
    out.push(card(format!("{role} · {what}"), text, sink));
}

/// The lines of `c` that are in `u`'s new code; none when it cites another
/// file or other code of this one.
fn lines_in(u: &Unit, c: &Cite) -> Vec<LineRef> {
    let inside = u
        .new_range
        .is_some_and(|r| c.start <= r.end && c.end >= r.start);
    if c.file != u.file || !inside {
        return Vec::new();
    }
    span(Side::New, c.start, c.end)
}

/// Lines `start..=end` on `side`; a long run by its ends.
fn span(side: Side, start: u32, end: u32) -> Vec<LineRef> {
    let end = end.max(start);
    let lines: Vec<u32> = if end - start < CITE_LINES {
        (start..=end).collect()
    } else {
        vec![start, end]
    };
    lines
        .into_iter()
        .map(|line| LineRef { side, line })
        .collect()
}

/// Where a cite is, short: `AssetController.php:41`.
fn place(c: &Cite) -> String {
    let name = c.file.file_name().map_or_else(
        || c.file.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    format!("{name}:{}", c.start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{
        Checks, Cost, Coverage, DiveResult, Gap, Prep, Range, SecondOpinion, Verdict,
    };
    use crate::security::{Audit, Class, Hop};

    fn unit() -> Unit {
        let mut u: Unit = serde_json::from_value(serde_json::json!({
            "id": "a.rs::f", "file": "a.rs", "symbol": "f", "kind": "Function",
            "status": "Changed", "old_range": {"start": 10, "end": 30},
            "new_range": {"start": 10, "end": 30},
            "fingerprint": "x", "changes": [], "callers": [], "callees": [],
            "decision": "Pending"
        }))
        .unwrap();
        u.prep = Prep::Ready;
        u.changes = vec![Change {
            marker: Marker::Changed,
            statement: "Loads the asset before the permission test".into(),
            evidence: vec![LineRef {
                side: Side::New,
                line: 12,
            }],
            risk: Risk::Medium,
        }];
        u
    }

    fn cite(file: &str, start: u32, end: u32) -> Cite {
        Cite {
            file: file.into(),
            start,
            end,
        }
    }

    fn finding() -> Finding {
        Finding {
            class: Class::Injection,
            severity: Risk::High,
            confidence: Risk::High,
            title: "The id reaches the query unchecked".into(),
            source: cite("a.rs", 11, 11),
            path: vec![
                Hop {
                    symbol: "g".into(),
                    cite: cite("a.rs", 14, 14),
                },
                Hop {
                    symbol: "h".into(),
                    cite: cite("b.rs", 3, 3),
                },
            ],
            sink: cite("a.rs", 20, 21),
            missing: "an owner check".into(),
            exploit: String::new(),
            path_verified: true,
            second: None,
            proof: None,
        }
    }

    fn titles(u: &Unit, mode: Mode) -> Vec<String> {
        comments(u, mode).into_iter().map(|c| c.title).collect()
    }

    #[test]
    fn review_has_the_statements_with_their_marker() {
        let got = comments(&unit(), Mode::Review);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, ["Loads the asset before the permission test"]);
        assert_eq!(got[0].marker, Some(Marker::Changed));
    }

    #[test]
    fn nothing_is_said_before_the_agent_has() {
        let mut u = unit();
        u.prep = Prep::Pending;
        assert!(comments(&u, Mode::Review).is_empty());
    }

    #[test]
    fn review_titles_a_statement_with_its_risk_and_the_dispute() {
        let mut u = unit();
        assert_eq!(titles(&u, Mode::Review), ["medium"]);
        u.checks = Checks {
            second: Some(SecondOpinion {
                risk: Risk::High,
                verdicts: vec![Verdict {
                    statement: u.changes[0].statement.clone(),
                    agree: false,
                    why: "the test still runs first".into(),
                    evidence: Vec::new(),
                }],
                missed: vec![Change {
                    marker: Marker::Removed,
                    statement: "Drops the audit log entry".into(),
                    evidence: vec![LineRef {
                        side: Side::Old,
                        line: 18,
                    }],
                    risk: Risk::High,
                }],
            }),
            cost: Some(Cost {
                why: "Rollback deletes the workflows.".into(),
                evidence: vec![LineRef {
                    side: Side::New,
                    line: 25,
                }],
            }),
            ..Checks::default()
        };
        let got = comments(&u, Mode::Review);
        assert_eq!(
            got.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(),
            ["medium · disputed", "missed · high", "expensive to undo"]
        );
        assert_eq!(
            got[0].text[1],
            "The second model disputes it: the test still runs first"
        );
        assert_eq!(got[1].marker, None);
    }

    #[test]
    fn review_shows_a_gap_only_where_its_lines_are_the_units() {
        let mut u = unit();
        let gap = |file: &str| Gap {
            case: "a caller without access".into(),
            danger: "It reads the asset.".into(),
            risk: Risk::High,
            evidence: vec![cite(file, 12, 13)],
        };
        let mut coverage = Coverage::found(Default::default());
        coverage.gaps = vec![gap("a.rs"), gap("other.rs")];
        u.dive.results.insert(
            DiveKind::Tests,
            DiveResult {
                coverage: Some(coverage.clone()),
                ..DiveResult::default()
            },
        );
        // Found, and not read by the agent yet: the gaps aren't its word.
        assert_eq!(titles(&u, Mode::Review), ["medium"]);
        coverage.judged = true;
        u.dive.results.get_mut(&DiveKind::Tests).unwrap().coverage = Some(coverage);
        let got = comments(&u, Mode::Review);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].title, "untested · high");
        assert_eq!(
            got[1].text,
            [
                "No test covers: a caller without access",
                "It reads the asset."
            ]
        );
    }

    #[test]
    fn security_has_the_facts_and_the_path_and_no_statement() {
        let mut u = unit();
        u.security = vec![
            Fact {
                class: Class::Authorization,
                weight: Weight::Attention,
                text: "denyAccessUnlessGranted is gone".into(),
                side: Side::Old,
                lines: Some(Range::new(13, 13)),
                subject: String::new(),
                origin: Origin::Teer,
            },
            Fact {
                class: Class::SupplyChain,
                weight: Weight::Note,
                text: "no lines".into(),
                side: Side::New,
                lines: None,
                subject: String::new(),
                origin: Origin::Teer,
            },
        ];
        u.dive.results.insert(
            DiveKind::Security,
            DiveResult {
                audit: Some(Audit {
                    findings: vec![finding()],
                    ..Audit::default()
                }),
                ..DiveResult::default()
            },
        );
        u.checks.suspects = vec![Suspect {
            class: Class::Authorization,
            severity: Risk::Medium,
            text: "A caller can pass any asset id.".into(),
            evidence: vec![LineRef {
                side: Side::New,
                line: 12,
            }],
            checks: Vec::new(),
        }];
        // Not what review writes in.
        assert_eq!(titles(&u, Mode::Review), ["medium"]);
        let got = comments(&u, Mode::Security);
        assert_eq!(
            got.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(),
            [
                "auth",
                "possible · auth · medium",
                "source · injection · high",
                "hop 1/2 · injection · high",
                "sink · injection · high",
            ]
        );
        assert_eq!(got[0].evidence[0].side, Side::Old);
        assert_eq!(got[1].text, ["A caller can pass any asset id."]);
        assert_eq!(
            got[4].text,
            [
                "The id reaches the query unchecked",
                "Missing: an owner check",
                "Path: 2 hops, verified.",
            ]
        );
        // All: what review says, then what security says.
        assert_eq!(titles(&u, Mode::All).len(), 6);
        assert_eq!(titles(&u, Mode::All)[0], "medium");
    }

    #[test]
    fn a_finding_that_enters_and_harms_at_one_place_is_one_card() {
        let mut f = finding();
        f.source = f.sink.clone();
        f.path.clear();
        f.path_verified = false;
        let mut out = Vec::new();
        super::finding(&unit(), &f, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "source and sink · injection · high");
        assert_eq!(out[0].text[2], "Path: no calls between, not verified.");
    }

    #[test]
    fn a_long_cite_is_anchored_by_its_ends() {
        assert_eq!(span(Side::New, 3, 5).len(), 3);
        let long = span(Side::New, 3, 40);
        assert_eq!(long.iter().map(|l| l.line).collect::<Vec<_>>(), [3, 40]);
    }
}
