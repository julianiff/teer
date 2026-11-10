//! Security lens, queue and finish (docs/security-plan.md, phase 3): the
//! attack-surface card over the queue, `zs`'s security order, severity in
//! the flagged list, findings in the notes for the fix agent, and the PR
//! post's Security section.

use crate::common;

use std::path::PathBuf;

use common::v2::{Driver, review, unit, units};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use teer::approve;
use teer::git::{ChangedFile, FileStatus};
use teer::review::model::{
    AcceptedBy, Cite, Decision, DiveKind, DiveResult, Range, Review, Risk, Side, Unit,
};
use teer::security::surface::{Surface, surface};
use teer::security::{Audit, Class, Fact, Finding, Hop, SecondCheck, Weight};
use teer::session::app::{Grouping, Screen};
use teer::session::view::snapshot;
use teer::tui::app::Target;

// ---- Building reviews ------------------------------------------------------

fn fact(class: Class, weight: Weight, subject: &str, text: &str) -> Fact {
    Fact {
        class,
        weight,
        text: text.into(),
        side: Side::New,
        lines: Some(Range::new(2, 3)),
        subject: subject.into(),
        origin: Default::default(),
    }
}

fn cite(file: &str, line: u32) -> Cite {
    Cite {
        file: PathBuf::from(file),
        start: line,
        end: line,
    }
}

/// A finding whose path the graph confirms.
fn finding(class: Class, severity: Risk, title: &str) -> Finding {
    Finding {
        class,
        severity,
        confidence: Risk::High,
        title: title.into(),
        source: cite("src/AssetController.php", 41),
        path: vec![Hop {
            symbol: "AssetRepository::byId".into(),
            cite: cite("src/AssetRepository.php", 88),
        }],
        sink: cite("src/AssetRepository.php", 90),
        missing: "isGranted('VIEW', $asset), which the 6 other actions call".into(),
        exploit: "tenant B sends tenant A's asset id and gets the asset".into(),
        path_verified: true,
        second: None,
        proof: None,
    }
}

/// `u` with an audit holding `findings`.
fn audited(mut u: Unit, findings: Vec<Finding>) -> Unit {
    u.dive.results.insert(
        DiveKind::Security,
        DiveResult {
            text: "1 finding".into(),
            audit: Some(Audit {
                findings,
                checked: vec![Class::Authorization],
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    u
}

/// The title of the finding in [`exposed`].
const IDOR: &str = "the asset id from the request reaches the query without the owner check";

/// Six units in `f0.rs`, `f1.rs`: u1 a new route (a note), u3 a raw SQL
/// sink worth a look, u4 a removed check and a standing finding.
fn exposed() -> Vec<Unit> {
    let mut us = units(6);
    us[1].security = vec![fact(
        Class::EntryPoint,
        Weight::Note,
        "POST /assets",
        "a new route",
    )];
    us[3].security = vec![fact(
        Class::Injection,
        Weight::Look,
        "raw SQL",
        "the request's sort reaches a raw query",
    )];
    let mut gone = fact(
        Class::Authorization,
        Weight::Attention,
        "denyAccessUnlessGranted",
        "denyAccessUnlessGranted is gone",
    );
    gone.side = Side::Old;
    us[4].security = vec![gone];
    us[4] = audited(
        us[4].clone(),
        vec![finding(Class::Authorization, Risk::High, IDOR)],
    );
    us
}

fn lines(screen: &str) -> Vec<&str> {
    screen.lines().collect()
}

/// The rows of the attack-surface card: from its top edge to its bottom.
fn card(screen: &str) -> Vec<String> {
    let ls = lines(screen);
    let top = ls
        .iter()
        .position(|l| l.contains("attack surface"))
        .unwrap_or_else(|| panic!("no card:\n{screen}"));
    let end = (top..ls.len()).find(|&i| ls[i].starts_with('╰')).unwrap();
    ls[top..=end].iter().map(|l| l.to_string()).collect()
}

// ---- The surface ------------------------------------------------------------

#[test]
fn the_surface_groups_by_class_and_lists_the_most_severe_first() {
    let mut us = exposed();
    // Another raw SQL sink, on a lighter unit: one item on two units.
    us[0].security = vec![fact(
        Class::Injection,
        Weight::Note,
        "raw SQL",
        "a constant query",
    )];
    // A finding the second model disputes, and one whose path isn't
    // verified: neither stands, neither is on the card.
    let mut disputed = finding(Class::Injection, Risk::High, "disputed");
    disputed.second = Some(SecondCheck {
        agree: false,
        why: "the value is an enum".into(),
        evidence: None,
    });
    let mut unverified = finding(Class::Injection, Risk::High, "unverified");
    unverified.path_verified = false;
    us[2] = audited(us[2].clone(), vec![disputed, unverified]);
    // A unit accepted for you is not to review: its facts aren't either.
    us[5].accepted_by = Some(AcceptedBy::Auto);
    us[5].security = vec![fact(Class::Secret, Weight::Attention, "", "an AWS key")];
    let s = surface(&review(us));
    let labels: Vec<&str> = s.rows.iter().map(|r| r.label).collect();
    assert_eq!(labels, ["entry points", "sinks", "checks"]);
    let texts =
        |i: usize| -> Vec<String> { s.rows[i].items.iter().map(|it| it.text.clone()).collect() };
    assert_eq!(texts(0), ["POST /assets"]);
    let sink = &s.rows[1].items[0];
    assert_eq!(
        (sink.text.as_str(), sink.units, sink.unit.as_str()),
        ("raw SQL", 2, "u3")
    );
    assert_eq!(s.rows[1].items.len(), 1);
    // The finding first, then the fact; the fact names its unit.
    let checks = &s.rows[2].items;
    assert!(checks[0].finding && checks[0].text == IDOR, "{checks:?}");
    assert_eq!(checks[0].weight, Weight::Attention);
    assert_eq!(checks[1].text, "denyAccessUnlessGranted gone from s4");
    assert!(checks[1].gone);
    assert!(surface(&review(units(3))).is_empty());
}

// ---- The card -----------------------------------------------------------------

#[test]
fn the_card_sits_over_the_boxes_with_its_rows_and_what_was_checked() {
    let d = Driver::of(exposed());
    let s = d.render(160, 40);
    let c = card(&s);
    assert!(c[0].starts_with("╭─ attack surface ─"), "{}", c[0]);
    assert!(c[1].starts_with("│ entry points  POST /assets"), "{}", c[1]);
    assert!(c[2].starts_with("│ sinks         raw SQL"), "{}", c[2]);
    assert!(
        c[3].starts_with(&format!(
            "│ checks        ! {IDOR} · ✗ denyAccessUnlessGranted"
        )),
        "{}",
        c[3]
    );
    assert!(
        c[4].contains(
            "checked: auth · entry points · injection · secrets · exposure · packages · ci · config"
        ),
        "{}",
        c[4]
    );
    assert!(c[4].contains("not: denial of service · business logic · crypto details"));
    assert_eq!(c.len(), 6);
    for l in &c {
        assert_eq!(l.chars().count(), 160, "{l}");
    }
    // Narrower, the second check gives way to how many more there are.
    let c = card(&d.render(120, 40));
    assert!(c[3].contains(&format!("{IDOR} · +1 more  ")), "{}", c[3]);
    // The boxes start under it.
    let ls = lines(&s);
    let card_end = ls.iter().position(|l| l.starts_with('╰')).unwrap();
    let first_box = ls
        .iter()
        .position(|l| l.contains("Needs a close look"))
        .unwrap();
    assert!(first_box > card_end, "{s}");
    assert!(!s.to_lowercase().contains("safe"), "{s}");
}

#[test]
fn with_nothing_found_the_card_is_one_muted_line_that_says_what_was_checked() {
    let d = Driver::new(3);
    let s = d.render(160, 30);
    let ls = lines(&s);
    assert!(
        ls[1].starts_with(
            " security: nothing found · checked: auth · entry points · injection · secrets"
        ),
        "{s}"
    );
    assert!(ls[1].contains("not: denial of service"), "{}", ls[1]);
    assert!(ls[2].starts_with("╭─ Routine"), "{s}");
    assert!(!s.contains("attack surface"));
    assert!(!s.to_lowercase().contains("safe"), "{s}");
}

#[test]
fn a_short_terminal_gets_the_card_on_one_line() {
    let d = Driver::of(exposed());
    let s = d.render(100, 16);
    let ls = lines(&s);
    assert!(
        ls[1].starts_with(" attack surface: entry points 1 · sinks 1 · checks 2   checked: auth"),
        "{s}"
    );
    assert!(!s.contains("╭─ attack surface"), "{s}");
    // Narrow, too.
    let s = d.render(36, 40);
    assert!(lines(&s)[1].starts_with(" attack surface: entry"), "{s}");
}

#[test]
fn many_classes_keep_the_card_to_eight_rows() {
    let mut us = units(9);
    let classes = [
        Class::EntryPoint,
        Class::Injection,
        Class::Authorization,
        Class::Secret,
        Class::Exposure,
        Class::SupplyChain,
        Class::Ci,
        Class::Config,
        Class::Crypto,
    ];
    for (u, c) in us.iter_mut().zip(classes) {
        // The secret weighs most: it keeps its row.
        let w = if c == Class::Secret {
            Weight::Attention
        } else {
            Weight::Note
        };
        u.security = vec![fact(c, w, "", &format!("{} thing", c.id()))];
    }
    let d = Driver::of(us);
    let c = card(&d.render(140, 60));
    assert!(c.len() <= 8, "{c:#?}");
    assert!(c.iter().any(|l| l.starts_with("│ secrets")), "{c:#?}");
    let more = c
        .iter()
        .find(|l| l.starts_with("│ more"))
        .expect("a row for the rest");
    assert!(more.contains("crypto 1"), "{more}");
    assert!(c.iter().any(|l| l.contains("checked: auth")), "{c:#?}");
}

#[test]
fn many_items_end_in_how_many_more() {
    let mut us = units(9);
    for (k, u) in us.iter_mut().enumerate() {
        u.security = vec![fact(
            Class::EntryPoint,
            Weight::Note,
            &format!("POST /a-rather-long-route-name-{k}"),
            "a new route",
        )];
    }
    let d = Driver::of(us);
    let c = card(&d.render(110, 40));
    let row = &c[1];
    assert!(row.contains("POST /a-rather-long-route-name-0"), "{row}");
    assert!(row.contains(" more"), "{row}");
    assert!(!row.contains("route-name-8"), "{row}");
    assert_eq!(row.chars().count(), 110);
}

#[test]
fn with_the_lens_off_there_is_no_card() {
    let mut d = Driver::of(exposed());
    d.app.security.enabled = false;
    let s = d.render(120, 40);
    assert!(
        !s.contains("attack surface") && !s.contains("security: nothing found"),
        "{s}"
    );
    assert!(snapshot(&d.app, None).queue.surface.is_none());
    d.keys("zs");
    assert!(!d.app.security.by_exposure);
    assert!(d.app.status.contains("lens is off"), "{}", d.app.status);
}

#[test]
fn a_click_on_an_item_selects_its_unit() {
    let mut d = Driver::of(exposed());
    d.app.sel = d.pos("u0");
    let s = d.render(120, 40);
    let c = card(&s);
    // The `raw SQL` item, on the card's third line.
    let top = lines(&s)
        .iter()
        .position(|l| l.contains("attack surface"))
        .unwrap();
    let col = c[2].chars().position(|ch| ch == 'r').unwrap() as u16;
    let row = (top + 2) as u16;
    let hit = d
        .app
        .targets
        .borrow()
        .iter()
        .rev()
        .find(|(r, _)| r.x <= col && col < r.x + r.width && r.y == row)
        .map(|(_, t)| *t);
    assert_eq!(hit, Some(Target::Unit(d.pos("u3"))));
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        d.app.on_mouse(MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }
    assert_eq!(d.selected().as_deref(), Some("u3"));
}

#[test]
fn the_desktop_gets_the_card_compact_and_whole() {
    let d = Driver::new(2);
    let json = serde_json::to_value(snapshot(&d.app, None).queue).unwrap();
    // Nothing found: what was checked, no rows.
    assert_eq!(
        json["surface"],
        serde_json::json!({
            "checked": "auth · entry points · injection · secrets · exposure · packages · ci · config",
            "not": "denial of service · business logic · crypto details",
        })
    );
    let d = Driver::of(exposed());
    let v = snapshot(&d.app, None).queue.surface.unwrap();
    let want: Surface = surface(&d.app.review);
    assert_eq!(v.rows, want.rows);
    let json = serde_json::to_value(&v).unwrap();
    assert_eq!(json["rows"][2]["class"], "authorization");
    assert_eq!(json["rows"][2]["items"][0]["weight"], "attention");
    assert_eq!(json["rows"][2]["items"][0]["finding"], true);
}

// ---- zs: the security order ------------------------------------------------------

#[test]
fn zs_lists_the_exposed_units_first_in_a_security_box_and_back() {
    let mut d = Driver::of(exposed());
    d.app.grouping = Grouping::Unit;
    d.app.rebuild_order();
    let by_level = d.app.order.clone();
    d.keys("zs");
    // Most exposed first: the standing finding, the fact worth a look,
    // then the note.
    let ids: Vec<String> = (0..3).map(|p| d.id_at(p)).collect();
    assert_eq!(ids, ["u4", "u3", "u1"]);
    assert_eq!(d.app.security.exposed, 3);
    let s = d.render(120, 40);
    let ls = lines(&s);
    let security = ls
        .iter()
        .position(|l| l.starts_with("╭─ Security · 3 units"))
        .unwrap_or_else(|| panic!("{s}"));
    let next = ls
        .iter()
        .position(|l| l.starts_with("╭─ Routine · 3 units"))
        .unwrap_or_else(|| panic!("{s}"));
    assert!(security < next, "{s}");
    // Each row keeps its level's glyph and its file.
    assert!(ls[security + 1].contains("▲ f1.rs  s4"), "{s}");
    assert!(ls[security + 2].contains("◆ f1.rs  s3"), "{s}");
    assert!(ls[security + 3].contains("● f0.rs  s1"), "{s}");
    // The desktop says the toggle is on, and has the box.
    let v = snapshot(&d.app, None);
    assert_eq!(v.actions["toggle.security"].on, Some(true));
    assert!(v.queue.boxes[0].security);
    assert_eq!(v.queue.boxes[0].title, "Security · 3 units");
    assert!(v.queue.boxes[1..].iter().all(|b| !b.security));
    d.keys("zs");
    assert_eq!(d.app.order, by_level);
    let s = d.render(120, 40);
    assert!(!s.contains("Security ·"), "{s}");
    assert_eq!(
        snapshot(&d.app, None).actions["toggle.security"].on,
        Some(false)
    );
}

#[test]
fn zs_by_file_keeps_each_files_exposed_units_together() {
    let mut us = units(9);
    // f2.rs's u6 is the most exposed; f0.rs has two exposed units.
    us[6] = audited(
        us[6].clone(),
        vec![finding(
            Class::Injection,
            Risk::High,
            "the sort reaches SQL",
        )],
    );
    us[0].security = vec![fact(Class::Ci, Weight::Note, "", "a workflow changed")];
    us[2].security = vec![fact(Class::SupplyChain, Weight::Look, "", "a new package")];
    let mut d = Driver::of(us);
    d.app.grouping = Grouping::File;
    d.app.rebuild_order();
    d.keys("zs");
    let ids: Vec<String> = (0..3).map(|p| d.id_at(p)).collect();
    assert_eq!(ids, ["u6", "u2", "u0"]);
    let s = d.render(120, 50);
    let ls = lines(&s);
    let at = |text: &str| {
        ls.iter()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("{text}:\n{s}"))
    };
    let security = at("╭─ Security · 2 files");
    assert!(security < at("f2.rs"), "{s}");
    assert!(at("f2.rs") < at("f0.rs"), "{s}");
    // Each file starts again below the security box.
    assert!(d.app.starts_file(3));
}

#[test]
fn zs_with_nothing_exposed_says_so() {
    let mut d = Driver::new(3);
    let before = d.app.order.clone();
    d.keys("zs");
    assert!(d.app.security.by_exposure);
    assert_eq!(d.app.order, before);
    assert!(
        d.app.status.contains("no unit has security facts"),
        "{}",
        d.app.status
    );
}

// ---- The flagged list --------------------------------------------------------------

#[test]
fn the_flagged_list_shows_each_units_security_with_its_severity() {
    let mut us = exposed();
    for u in &mut us[3..5] {
        u.decision = Decision::Investigate;
    }
    us[0].decision = Decision::Investigate;
    let mut d = Driver::of(us);
    d.keys("<space>f");
    assert_eq!(d.screen(), Screen::Flagged);
    let s = d.render(140, 80);
    assert!(
        s.contains(&format!(
            "▲ security: high · authorization: {IDOR} (+1 more)"
        )),
        "{s}"
    );
    assert!(
        s.contains("◆ security: medium · injection: the request's sort reaches a raw query"),
        "{s}"
    );
    // u0 has neither: no line (the reasons under "Why this level" have no severity).
    let lines = s.matches("security: high").count() + s.matches("security: medium").count();
    assert_eq!(lines, 2, "{s}");
    let line = teer::session::flagged_list::security_line(&d.app.review.units[4]).unwrap();
    assert_eq!(line.severity, Risk::High);
    assert!(line.finding);
}

#[test]
fn a_fact_that_needs_a_close_look_outranks_a_lesser_finding() {
    let mut u = audited(
        unit(0, "a.rs"),
        vec![finding(Class::Injection, Risk::Medium, "maybe")],
    );
    u.security = vec![fact(Class::Secret, Weight::Attention, "", "an AWS key")];
    let line = teer::session::flagged_list::security_line(&u).unwrap();
    assert_eq!(line.line(), "security: high · secret: an AWS key (+1 more)");
    assert!(!line.finding);
    u.security[0].weight = Weight::Note;
    let line = teer::session::flagged_list::security_line(&u).unwrap();
    assert_eq!(line.line(), "security: medium · injection: maybe");
}

// ---- Notes and the fix loop ----------------------------------------------------------

/// [`exposed`] with u4 flagged and noted, u3 flagged without a note, u1
/// noted but accepted.
fn noted() -> Review {
    let mut us = exposed();
    us[4].decision = Decision::Investigate;
    us[4].dive.note = "check the owner first".into();
    us[3].decision = Decision::Investigate;
    us[1].decision = Decision::Accepted;
    us[1].dive.note = "fine".into();
    us[0].security = vec![fact(
        Class::Secret,
        Weight::Attention,
        "",
        "a pending secret",
    )];
    review(us)
}

#[test]
fn the_notes_carry_a_flagged_units_findings_and_facts() {
    let r = noted();
    let md = teer::notes::markdown(&r);
    let at = md.find("## f1.rs · `s4`").expect(&md);
    let section = &md[at..];
    let want = [
        "> check the owner first",
        "",
        "**Security**",
        "",
        &format!("- **high · authorization**: {IDOR}"),
        "  - path: `src/AssetController.php:41 → AssetRepository::byId (src/AssetRepository.php:88) → src/AssetRepository.php:90`",
        "  - missing: isGranted('VIEW', $asset), which the 6 other actions call",
        "  - exploit: tenant B sends tenant A's asset id and gets the asset",
        "- **authorization** (found by teer, `f1.rs:2-3 (old)`): denyAccessUnlessGranted is gone",
        "",
    ]
    .join("\n");
    assert!(section.starts_with("## f1.rs · `s4`\n\n> check"), "{md}");
    assert!(section.contains(&want), "{md}");
    // Flagged without a note, a fact worth a look only: nothing to add.
    let s3 = md.find("## f1.rs · `s3`").expect(&md);
    assert!(!md[s3..at].contains("**Security**"), "{md}");
    // Accepted, or not listed (pending): their security stays out.
    assert!(
        !md.contains("POST /assets") && !md.contains("a pending secret"),
        "{md}"
    );
    let prompt = teer::notes::agent_prompt(&r);
    assert!(
        prompt.contains("a Security list says what teer's security lens found"),
        "{prompt}"
    );
    assert!(prompt.ends_with(&md), "{prompt}");
}

#[test]
fn notes_without_security_are_as_they_were() {
    let mut r = noted();
    // The same review with no audits and no facts that need a close look.
    let mut bare = r.clone();
    for u in &mut bare.units {
        u.security.clear();
        u.dive.results.clear();
    }
    for u in &mut r.units {
        u.dive.results.clear();
        for f in &mut u.security {
            f.weight = Weight::Look;
        }
    }
    assert_eq!(teer::notes::markdown(&r), teer::notes::markdown(&bare));
    assert_eq!(
        teer::notes::agent_prompt(&r),
        teer::notes::agent_prompt(&bare)
    );
    assert!(!teer::notes::agent_prompt(&r).contains("Security"));
}

// ---- The PR post --------------------------------------------------------------------

/// [`noted`] as a review whose files hold u4's change, so its note can go
/// inline.
fn posted() -> (Review, Vec<usize>) {
    let mut r = noted();
    let old = "fn a() {\n    one\n}\n";
    let new = "fn a() {\n    two\n}\n";
    r.files = ["f0.rs", "f1.rs"]
        .map(|f| ChangedFile {
            path: PathBuf::from(f),
            old_path: None,
            status: FileStatus::Modified,
            old_content: Some(old.into()),
            new_content: Some(new.into()),
        })
        .to_vec();
    for u in &mut r.units {
        u.old_range = Some(Range::new(1, 3));
        u.new_range = Some(Range::new(1, 3));
    }
    let d = Driver::with(r, Default::default());
    (d.app.review.clone(), d.app.order.clone())
}

#[test]
fn the_pr_post_has_a_security_section_and_findings_under_the_note() {
    let (r, order) = posted();
    let p = approve::plan(&r, &order, approve::DEFAULT_INTRO, true);
    assert!(p.body.starts_with(approve::DEFAULT_INTRO), "{}", p.body);
    let section = &p.body[p.body.find("### Security").expect(&p.body)..];
    assert_eq!(
        section,
        format!(
            "### Security\n\n\
- **high** · authorization: {IDOR} (`src/AssetRepository.php:90`)\n\
- **found by teer** · authorization: denyAccessUnlessGranted is gone (`f1.rs:2-3 (old)`)"
        )
    );
    // u4's note inline, its finding under it.
    let inline = p.inline.iter().find(|c| c.symbol == "s4").expect("inline");
    assert!(
        inline.body.starts_with(
            "check the owner first\n\n**Security** (teer)\n- **high · authorization**: "
        ),
        "{}",
        inline.body
    );
    assert!(
        inline.body.contains("Missing: isGranted"),
        "{}",
        inline.body
    );
    // The accepted unit's note has nothing added.
    let fine = p.inline.iter().find(|c| c.symbol == "s1").expect("inline");
    assert_eq!(fine.body, "fine");
    // Without inline comments, the section is still there.
    let flat = approve::plan(&r, &order, approve::DEFAULT_INTRO, false);
    assert!(flat.body.contains("### Security"), "{}", flat.body);
}

#[test]
fn the_pr_post_has_no_security_section_when_there_is_nothing() {
    let (mut r, order) = posted();
    for u in &mut r.units {
        u.security.clear();
        u.dive.results.clear();
    }
    assert!(approve::security_section(&r, &order).is_none());
    let notes = approve::collect_notes(&r, &order);
    let p = approve::plan(&r, &order, approve::DEFAULT_INTRO, true);
    assert_eq!(p, approve::build_plan(&notes, approve::DEFAULT_INTRO, true));
    assert!(!p.body.contains("Security"), "{}", p.body);
}
