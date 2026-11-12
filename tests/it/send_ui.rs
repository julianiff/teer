//! Sending a tile's result to an agent session: send_ui (see FEATURES.md).
//!
//! `s` on a tile in the unit view builds a plain-text message from what
//! the tile says (the advice, the findings that stand, the tests' gaps,
//! the cases that surprised, the last answer) and asks for the sessions
//! running in the repository; the picker chooses the session that wrote
//! the change, else the one chosen last, and sends on `enter`. No real
//! session here: the runtime's events are fed by hand.

use crate::common;

use std::path::PathBuf;

use common::v2::{Driver, review, unit};
use teer::keymap::Keymap;
use teer::review::model::{
    Answer, Behavior, Cite, Coverage, DiveKind, DiveResult, Expect, Gap, Outcome, PrInfo,
    ProbeCase, Question, Range, Risk, SessionRef, Shown, Side, Unit,
};
use teer::security::{Audit, Class, Fact, Finding, Hop, Origin, Proof, SecondCheck, Weight};
use teer::send::{Agent, PiState, Target};
use teer::session::app::{AppEvent, Effect, Overlay};
use teer::session::send::{CLOSING, MESSAGE_MAX, PI_ROW};
use teer::session::unit::Panel;

const HEAD: &str = "teer, reviewing your changes: charge (src/pay.rs:10-24)";

/// The unit every test sends from: `charge` in `src/pay.rs`, lines 10-24.
fn charge() -> Unit {
    let mut u = unit(0, "src/pay.rs");
    u.symbol = "charge".into();
    u.new_range = Some(Range::new(10, 24));
    u.old_range = Some(Range::new(10, 20));
    u
}

/// A review of `u` and one more unit, prepared by Claude Code session
/// `wrote-it`, with an agent: the unit view opens with the advice tile.
fn driver(u: Unit) -> Driver {
    let mut r = review(vec![u, unit(1, "src/z.rs")]);
    r.agent_session = Some(SessionRef {
        agent: "claude".into(),
        id: "wrote-it".into(),
        cwd: r.repo_root.clone(),
    });
    let mut d = Driver::with(r, Keymap::default());
    d.app.has_agent = true;
    d.select_id("u0");
    d
}

fn result(text: &str) -> DiveResult {
    DiveResult {
        text: text.into(),
        ..Default::default()
    }
}

fn cite(file: &str, start: u32, end: u32) -> Cite {
    Cite {
        file: file.into(),
        start,
        end,
    }
}

fn with(mut u: Unit, kind: DiveKind, r: DiveResult) -> Unit {
    u.dive.results.insert(kind, r);
    u
}

fn advised() -> Unit {
    let mut r = result("Retry the charge once.\nLog the \u{1b}[31mfailure\u{1b}[0m.");
    r.sources = vec!["https://docs.example/retry".into()];
    with(charge(), DiveKind::Recommend, r)
}

/// The message and what it is, from the `ListSessions` among `effects`.
fn listed(effects: &[Effect]) -> (String, String) {
    effects
        .iter()
        .find_map(|e| match e {
            Effect::ListSessions { text, what } => Some((text.clone(), what.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no ListSessions in {effects:?}"))
}

fn lists(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|e| matches!(e, Effect::ListSessions { .. }))
}

fn target(agent: Agent, id: &str, name: &str, active_at: u64) -> Target {
    Target {
        agent,
        id: id.into(),
        name: name.into(),
        cwd: PathBuf::from("/v2/repo"),
        status: "idle".into(),
        active_at,
        socket: None,
    }
}

/// Three sessions, the most recently active first: Codex, then Claude
/// Code's `macos-41` (another session), then the one that wrote the change.
fn targets() -> Vec<Target> {
    vec![
        target(Agent::Codex, "thread-9", "019a2b", 3),
        target(Agent::ClaudeCode, "other", "macos-41", 2),
        target(Agent::ClaudeCode, "wrote-it", "macos-7", 1),
    ]
}

fn found(d: &mut Driver, targets: Vec<Target>, pi: PiState) -> Vec<Effect> {
    let effects = d.app.on_app_event(AppEvent::Sessions {
        targets,
        pi,
        text: format!("{HEAD}\n\nThe advice:\nRetry the charge once.\n\n{CLOSING}"),
        what: "the advice".into(),
    });
    d.effects.extend(effects.iter().cloned());
    effects
}

fn sel(d: &Driver) -> usize {
    match d.overlay() {
        Overlay::SendTo { sel, .. } => *sel,
        o => panic!("no picker: {o:?}"),
    }
}

fn sent_to(effects: &[Effect]) -> Option<(Target, String)> {
    effects.iter().find_map(|e| match e {
        Effect::SendToSession { target, text } => Some(((**target).clone(), text.clone())),
        _ => None,
    })
}

// ---- The message, tile by tile ------------------------------------------------

#[test]
fn the_advice_goes_with_its_sources_between_the_head_and_what_to_do() {
    let mut d = driver(advised());
    d.keys("l<tab>");
    assert_eq!(d.app.unit.panel, Some(Panel::Recommend));
    let effects = d.keys("s");
    let (text, what) = listed(&effects);
    assert_eq!(what, "the advice");
    assert_eq!(
        text,
        format!(
            "{HEAD}\n\nThe advice:\nRetry the charge once.\nLog the [31mfailure[0m.\n\
             Sources:\n- https://docs.example/retry\n\n{CLOSING}"
        )
    );
    assert!(!text.contains('\u{1b}'), "no escape reaches the session");
    assert_eq!(d.status(), "Looking for agent sessions in this repository…");
    assert_eq!(
        d.overlay(),
        &Overlay::None,
        "the picker waits for the sessions"
    );
}

fn finding(title: &str, severity: Risk) -> Finding {
    Finding {
        class: Class::Injection,
        severity,
        confidence: Risk::High,
        title: title.into(),
        source: cite("src/routes.php", 4, 4),
        path: vec![Hop {
            symbol: "AssetRepository::byId".into(),
            cite: cite("src/repo.php", 3, 6),
        }],
        sink: cite("src/repo.php", 4, 4),
        missing: "the id is never cast to an int".into(),
        exploit: "GET /assets?id=1 OR 1=1 reads every asset".into(),
        path_verified: true,
        second: None,
        proof: None,
    }
}

fn fact(text: &str) -> Fact {
    Fact {
        class: Class::EntryPoint,
        weight: Weight::Look,
        text: text.into(),
        side: Side::New,
        lines: Some(Range::new(12, 12)),
        subject: "POST /charge".into(),
        origin: Origin::Teer,
    }
}

fn audited(findings: Vec<Finding>, facts: Vec<Fact>) -> Unit {
    let mut u = charge();
    u.security = facts;
    let r = DiveResult {
        text: "One way in.".into(),
        audit: Some(Audit {
            findings,
            ..Default::default()
        }),
        ..Default::default()
    };
    with(u, DiveKind::Security, r)
}

#[test]
fn security_sends_only_the_findings_that_stand_with_their_path() {
    let mut proven = finding("SQL injection through the asset id", Risk::High);
    proven.proof = Some(Proof::Proven {
        case: "id = 1 OR 1=1".into(),
        before: "1 row".into(),
        after: "12 rows".into(),
    });
    let mut unverified = finding("Path traversal in the export", Risk::High);
    unverified.path_verified = false;
    let mut disputed = finding("Open redirect after login", Risk::Medium);
    disputed.second = Some(SecondCheck {
        agree: false,
        why: "the target is checked".into(),
        evidence: None,
    });
    let low = finding("Verbose error page", Risk::Low);
    let u = audited(
        vec![low, unverified, disputed, proven],
        vec![fact("a new route")],
    );
    let mut d = driver(u);
    d.keys("l<space>x");
    assert_eq!(d.app.unit.panel, Some(Panel::Tool(DiveKind::Security)));
    let (text, what) = listed(&d.keys("s"));
    assert_eq!(what, "the security findings");
    assert!(
        text.starts_with(&format!("{HEAD}\n\nSecurity findings that stand")),
        "{text}"
    );
    assert!(text.ends_with(&format!("\n\n{CLOSING}")), "{text}");
    let body = "1. SQL injection through the asset id (high severity, injection)\n   \
                path: src/routes.php:4 → AssetRepository::byId (src/repo.php:3-6) → src/repo.php:4\n   \
                missing: the id is never cast to an int\n   \
                exploit: GET /assets?id=1 OR 1=1 reads every asset\n   \
                proven old against new: id = 1 OR 1=1 (before: 1 row, after: 12 rows)\n\
                2. Verbose error page (low severity, injection)";
    assert!(text.contains(body), "{text}");
    for gone in ["Path traversal", "Open redirect", "a new route"] {
        assert!(!text.contains(gone), "{gone} doesn't stand:\n{text}");
    }
}

#[test]
fn with_no_finding_standing_teers_facts_go_and_with_neither_nothing() {
    let mut disputed = finding("Open redirect after login", Risk::Medium);
    disputed.second = Some(SecondCheck {
        agree: false,
        why: "the target is checked".into(),
        evidence: None,
    });
    let mut d = driver(audited(vec![disputed], vec![fact("a new route")]));
    d.keys("l<space>x");
    let (text, what) = listed(&d.keys("s"));
    assert_eq!(what, "the security facts");
    assert!(
        text.contains(
            "Security facts teer found in the change:\n\
             - a new route (entry points, POST /charge, src/pay.rs:12)\n\n"
        ),
        "{text}"
    );

    let mut d = driver(audited(Vec::new(), Vec::new()));
    d.keys("l<space>x");
    let effects = d.keys("s");
    assert!(!lists(&effects), "{effects:?}");
    assert_eq!(
        d.status(),
        "Security has nothing to send: no finding stands, and teer found nothing."
    );
}

#[test]
fn tests_send_the_dangerous_cases_no_test_covers() {
    let gap = |case: &str, risk: Risk| Gap {
        case: case.into(),
        danger: "the customer is charged twice".into(),
        risk,
        evidence: vec![cite("src/pay.rs", 12, 14)],
    };
    let r = DiveResult {
        coverage: Some(Coverage {
            search: None,
            judged: true,
            covered: Vec::new(),
            gaps: vec![
                gap("the retry times out", Risk::Medium),
                gap("the card is declined twice", Risk::High),
            ],
        }),
        ..Default::default()
    };
    let mut d = driver(with(charge(), DiveKind::Tests, r));
    d.keys("l<space>t");
    assert_eq!(d.app.unit.panel, Some(Panel::Tool(DiveKind::Tests)));
    let (text, what) = listed(&d.keys("s"));
    assert_eq!(what, "the missing tests");
    assert_eq!(
        text,
        format!(
            "{HEAD}\n\nDangerous cases no test covers. Add tests that cover these:\n\
             - the card is declined twice (high risk): the customer is charged twice (src/pay.rs:12-14)\n\
             - the retry times out (medium risk): the customer is charged twice (src/pay.rs:12-14)\n\n\
             {CLOSING}"
        )
    );
}

#[test]
fn tests_found_but_not_read_by_the_agent_have_nothing_to_send_yet() {
    let r = DiveResult {
        coverage: Some(Coverage::found(Default::default())),
        ..Default::default()
    };
    let mut d = driver(with(charge(), DiveKind::Tests, r));
    d.keys("l<space>t");
    assert!(!lists(&d.keys("s")));
    assert_eq!(d.status(), "Tests has nothing to send yet.");
}

fn case(name: &str, expect: Expect, before: &str, after: &str, claim: usize) -> ProbeCase {
    ProbeCase {
        name: name.into(),
        input: format!("{name} input"),
        expect,
        why: "the retry is new".into(),
        before: Outcome::Value(before.into()),
        after: Outcome::Value(after.into()),
        claim,
    }
}

#[test]
fn behavior_sends_the_statements_the_runs_disprove_and_the_surprises() {
    let b = Behavior {
        cases: vec![
            case("same", Expect::Same, "1", "1", 0),
            case("expected", Expect::Different, "1", "2", 0),
            case("declined", Expect::Same, "\"ok\"", "\"retry\"", 1),
        ],
        command: vec!["node".into(), "probe.js".into()],
        dir: PathBuf::from("/tmp/probe"),
        claims: vec!["Charges once.".into()],
        tests: None,
        test_command: Vec::new(),
    };
    let r = DiveResult {
        behavior: Some(b),
        ..Default::default()
    };
    let mut d = driver(with(charge(), DiveKind::Behavior, r));
    d.keys("l<space>r");
    assert_eq!(d.app.unit.panel, Some(Panel::Tool(DiveKind::Behavior)));
    let (text, what) = listed(&d.keys("s"));
    assert_eq!(what, "the unexpected behavior");
    assert_eq!(
        text,
        format!(
            "{HEAD}\n\nThe old and the new code ran on the same inputs, in a sandbox:\n\
             Statements about the change the runs disprove:\n\
             - \"Charges once.\" (1 of 1 cases disagree)\n\
             Cases that came out other than predicted:\n\
             - declined: input declined input\n  \
             before: \"ok\"\n  \
             after: \"retry\"\n  \
             its statement implies no change: the retry is new\n\n\
             {CLOSING}"
        )
    );
}

#[test]
fn behavior_with_every_case_as_predicted_has_nothing_to_send() {
    let b = Behavior {
        cases: vec![case("same", Expect::Same, "1", "1", 0)],
        command: Vec::new(),
        dir: PathBuf::new(),
        claims: Vec::new(),
        tests: None,
        test_command: Vec::new(),
    };
    let r = DiveResult {
        behavior: Some(b),
        ..Default::default()
    };
    let mut d = driver(with(charge(), DiveKind::Behavior, r));
    d.keys("l<space>r");
    assert!(!lists(&d.keys("s")));
    assert_eq!(
        d.status(),
        "Behavior has nothing to send: every case came out as predicted."
    );
}

#[test]
fn behavior_of_a_new_unit_has_no_before_to_send() {
    let b = Behavior {
        cases: vec![case("declined", Expect::Same, "1", "2", 0)],
        command: Vec::new(),
        dir: PathBuf::new(),
        claims: Vec::new(),
        tests: None,
        test_command: Vec::new(),
    };
    let r = DiveResult {
        behavior: Some(b),
        ..Default::default()
    };
    let mut u = with(charge(), DiveKind::Behavior, r);
    u.status = teer::review::model::UnitStatus::Added;
    let mut d = driver(u);
    d.keys("l<space>r");
    assert!(!lists(&d.keys("s")));
    assert_eq!(
        d.status(),
        "Behavior has nothing to send: the unit is new, so nothing ran before."
    );
}

#[test]
fn ask_sends_the_last_answer_with_its_question() {
    let mut d = driver(charge());
    let q = |text: &str, answer: Option<&str>| Question {
        text: text.into(),
        unit: Some("u0".into()),
        answer: answer.map(|a| Answer {
            text: a.into(),
            shown: Vec::new(),
            related: Vec::new(),
        }),
        error: None,
    };
    let mut last = q(
        "Why retry only once?",
        Some("A second retry would double-charge."),
    );
    last.answer.as_mut().unwrap().shown = vec![Shown {
        cite: cite("src/pay.rs", 12, 14),
        why: "the retry".into(),
        code: vec!["retry()".into()],
        in_change: true,
        changed: Vec::new(),
    }];
    d.app.review.questions = vec![
        q("What does it charge?", Some("The order total.")),
        Question {
            unit: Some("u1".into()),
            ..q("Another unit's", Some("Not this one."))
        },
        last,
        q("Still on its way?", None),
    ];
    d.keys("lK<esc>");
    assert_eq!(d.app.unit.panel, Some(Panel::Ask));
    assert!(!d.app.unit.typing);
    let (text, what) = listed(&d.keys("s"));
    assert_eq!(what, "the answer");
    assert_eq!(
        text,
        format!(
            "{HEAD}\n\nA question about it, and the agent's answer:\n\
             Question: Why retry only once?\n\
             Answer:\nA second retry would double-charge.\n\
             Code it points at:\n- src/pay.rs:12-14 (the retry)\n\n{CLOSING}"
        )
    );
}

#[test]
fn typing_a_question_s_is_a_letter() {
    let mut d = driver(charge());
    d.keys("lK");
    assert!(d.app.unit.typing);
    assert!(!lists(&d.keys("s")));
    assert_eq!(d.app.ask.draft, "s");
}

#[test]
fn a_long_result_is_cut_short_and_still_says_what_to_do() {
    let long = "This is a line of advice that goes on.\n".repeat(400);
    let mut d = driver(with(charge(), DiveKind::Recommend, result(&long)));
    d.keys("l<tab>");
    let (text, _) = listed(&d.keys("s"));
    assert!(text.chars().count() <= MESSAGE_MAX, "{}", text.len());
    assert!(text.starts_with(HEAD));
    assert!(text.contains("… (cut short: the rest is in teer)\n\n"));
    assert!(text.ends_with(CLOSING));
}

// ---- Nothing to send, and where `s` says so ---------------------------------

#[test]
fn a_tile_without_a_result_or_still_waiting_has_nothing_to_send_yet() {
    // The advice, not asked yet.
    let mut d = driver(charge());
    d.keys("l<tab>");
    assert_eq!(d.app.unit.panel, Some(Panel::Recommend));
    assert!(!lists(&d.keys("s")));
    assert_eq!(d.status(), "Advice has nothing to send yet.");

    // On its way: the old answer doesn't go.
    let mut u = advised();
    u.dive_pending.insert(DiveKind::Recommend);
    let mut d = driver(u);
    d.keys("l<tab>");
    assert!(!lists(&d.keys("s")));
    assert_eq!(d.status(), "Advice has nothing to send yet.");

    // Visualize is a picture.
    let mut u = charge();
    u.dive
        .results
        .insert(DiveKind::Visualize, result("a claim"));
    let mut d = driver(u);
    d.keys("l<space>v");
    assert_eq!(d.app.unit.panel, Some(Panel::Tool(DiveKind::Visualize)));
    assert!(!lists(&d.keys("s")));
    assert_eq!(d.status(), "Visualize has nothing to send yet.");

    // Asked nothing yet.
    let mut d = driver(charge());
    d.keys("lK<esc>");
    assert!(!lists(&d.keys("s")));
    assert_eq!(d.status(), "Ask has nothing to send yet.");
}

#[test]
fn with_the_text_focused_s_asks_for_a_tile() {
    let mut d = driver(advised());
    d.keys("l");
    assert_eq!(d.app.unit.panel, None);
    assert!(!lists(&d.keys("s")));
    assert_eq!(d.status(), "Focus a tile (tab) to send its result.");
}

#[test]
fn in_the_queue_s_says_to_open_a_unit() {
    let mut d = driver(advised());
    assert!(!lists(&d.keys("s")));
    assert_eq!(
        d.status(),
        "Open a unit and focus a tile to send its result."
    );
}

#[test]
fn on_a_pull_request_nothing_is_sent() {
    let mut d = driver(advised());
    d.app.review.pr = Some(PrInfo::default());
    d.keys("l<tab>");
    assert!(!lists(&d.keys("s")));
    assert_eq!(
        d.status(),
        "On a pull request nothing is applied here: :w posts the review."
    );
    // Nor does its tile offer it.
    let screen = d.render(140, 40);
    assert!(!screen.contains("s send ·"), "{screen}");
}

// ---- The picker ---------------------------------------------------------------

#[test]
fn the_picker_chooses_the_session_that_wrote_the_change() {
    let mut d = driver(advised());
    found(&mut d, targets(), PiState::NotFound);
    assert!(matches!(d.overlay(), Overlay::SendTo { what, .. } if what == "the advice"));
    assert_eq!(
        sel(&d),
        2,
        "the session that wrote it, though not the latest"
    );
    assert!(d.app.wrote_change(&targets()[2]));
    assert!(!d.app.wrote_change(&targets()[1]));
}

#[test]
fn else_the_session_chosen_last_else_the_first() {
    let mut d = driver(advised());
    d.app.review.agent_session = None;
    found(&mut d, targets(), PiState::NotFound);
    assert_eq!(sel(&d), 0, "the first, most recently active");
    d.keys("j");
    let effects = d.keys("<cr>");
    let (t, _) = sent_to(&effects).expect("sent");
    assert_eq!(t.id, "other");
    // Next time it is chosen first, wherever it is listed now.
    let mut again = targets();
    again.swap(1, 2);
    found(&mut d, again, PiState::NotFound);
    assert_eq!(sel(&d), 2);
    // Gone: the first again.
    d.keys("<esc>");
    found(&mut d, targets()[..1].to_vec(), PiState::NotFound);
    assert_eq!(sel(&d), 0);
}

#[test]
fn j_and_k_move_enter_sends_and_esc_or_h_cancel() {
    let mut d = driver(advised());
    found(&mut d, targets(), PiState::NotFound);
    d.keys("k");
    assert_eq!(sel(&d), 1);
    d.keys("kk");
    assert_eq!(sel(&d), 0, "no further than the first");
    d.keys("<down>jj");
    assert_eq!(sel(&d), 2, "no further than the last");
    d.keys("<up>");
    let effects = d.keys("<cr>");
    let (t, text) = sent_to(&effects).expect("enter sends");
    assert_eq!(t, targets()[1]);
    assert!(text.starts_with(HEAD) && text.ends_with(CLOSING), "{text}");
    assert_eq!(d.overlay(), &Overlay::None);
    assert_eq!(d.status(), "Sending to Claude Code macos-41…");

    for cancel in ["<esc>", "h", "q"] {
        found(&mut d, targets(), PiState::NotFound);
        let effects = d.keys(cancel);
        assert!(effects.is_empty(), "{cancel}: {effects:?}");
        assert_eq!(d.overlay(), &Overlay::None, "{cancel} cancels");
    }
}

#[test]
fn a_click_selects_a_row_and_its_send_button_sends() {
    let mut d = driver(advised());
    found(&mut d, targets(), PiState::NotFound);
    d.app.pick(0);
    assert_eq!(sel(&d), 0);
    d.app.pick(9);
    assert_eq!(sel(&d), 2, "past the last row, the last");
    let effects = d.app.send_pick(1);
    let (t, _) = sent_to(&effects).expect("sent");
    assert_eq!(t.id, "other");
    assert_eq!(d.overlay(), &Overlay::None);
}

#[test]
fn no_session_says_to_start_one() {
    let mut d = driver(advised());
    let effects = found(&mut d, Vec::new(), PiState::NotFound);
    assert!(effects.is_empty());
    assert_eq!(d.overlay(), &Overlay::None);
    assert_eq!(
        d.status(),
        "No agent session runs in this repository: start one, then s again."
    );
    found(&mut d, Vec::new(), PiState::Ready);
    assert_eq!(d.overlay(), &Overlay::None);
    // Sessions that come while something else is open leave it open.
    d.app.overlay = Overlay::Help;
    found(&mut d, targets(), PiState::NotFound);
    assert_eq!(d.overlay(), &Overlay::Help);
    assert_eq!(
        d.status(),
        "The sessions came while something else was open: s again."
    );
}

#[test]
fn pi_without_teers_extension_offers_to_install_it_after_a_question() {
    let mut d = driver(advised());
    found(&mut d, Vec::new(), PiState::NoExtension);
    assert!(matches!(d.overlay(), Overlay::SendTo { targets, .. } if targets.is_empty()));
    assert_eq!(d.app.send_rows(), 1);
    let screen = d.render(120, 30);
    assert!(
        screen.contains("No agent session runs in this repository."),
        "{screen}"
    );
    assert!(screen.contains(PI_ROW), "{screen}");
    assert!(screen.contains("⏎ install · esc cancel"), "{screen}");

    // Enter asks first; anything but yes goes back to the list.
    assert!(d.keys("<cr>").is_empty());
    assert!(d.app.send.confirm_pi);
    let screen = d.render(120, 30);
    assert!(screen.contains("Install teer's pi extension?"), "{screen}");
    assert!(screen.contains("y/⏎ yes · esc no"), "{screen}");
    assert!(d.keys("n").is_empty());
    assert!(!d.app.send.confirm_pi);
    assert!(matches!(d.overlay(), Overlay::SendTo { .. }));

    d.keys("<cr>");
    let effects = d.keys("y");
    assert_eq!(effects, vec![Effect::InstallPiExtension]);
    assert_eq!(d.overlay(), &Overlay::None);
    assert_eq!(d.status(), "Installing teer's pi extension…");

    d.app.on_app_event(AppEvent::PiExtension(Ok(PathBuf::from(
        "/home/pi/extensions/teer-inbox.ts",
    ))));
    assert_eq!(
        d.status(),
        "Installed /home/pi/extensions/teer-inbox.ts: pi sessions started from now on can be reached."
    );
}

#[test]
fn with_sessions_the_pi_row_is_last() {
    let mut d = driver(advised());
    found(&mut d, targets(), PiState::NoExtension);
    assert_eq!(d.app.send_rows(), 4);
    assert_eq!(sel(&d), 2, "still the session that wrote the change");
    d.keys("G");
    assert_eq!(sel(&d), 3);
    d.keys("<cr>");
    assert!(d.app.send.confirm_pi);
    assert_eq!(d.keys("<cr>"), vec![Effect::InstallPiExtension]);
}

#[test]
fn sent_says_so_and_a_message_that_didnt_go_is_on_the_clipboard() {
    let mut d = driver(advised());
    let t = targets()[1].clone();
    let effects = d.app.on_app_event(AppEvent::Sent {
        target: Box::new(t.clone()),
        text: "the message".into(),
        result: Ok(()),
    });
    assert!(effects.is_empty());
    assert_eq!(d.status(), "Sent to Claude Code macos-41.");

    let effects = d.app.on_app_event(AppEvent::Sent {
        target: Box::new(t),
        text: "the message".into(),
        result: Err("its socket is gone".into()),
    });
    assert_eq!(
        effects,
        vec![Effect::Copy {
            text: "the message".into(),
            what: "the message".into(),
        }]
    );
    assert_eq!(
        d.status(),
        "Not sent to Claude Code macos-41: its socket is gone"
    );
}

#[test]
fn the_picker_draws_each_session_the_message_and_the_keys() {
    let mut d = driver(advised());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let mut ts = targets();
    ts[0].active_at = now - 2 * 60_000;
    ts[0].status = "busy".into();
    ts[1].cwd = PathBuf::from("/v2/repo/web");
    ts[1].active_at = 0;
    found(&mut d, ts, PiState::NotFound);
    let screen = d.render(120, 30);
    assert!(screen.contains("╭ Send the advice to "), "{screen}");
    let row = |name: &str| {
        screen
            .lines()
            .find(|l| l.contains(name))
            .unwrap_or_else(|| panic!("no row for {name}:\n{screen}"))
            .to_string()
    };
    let codex = row("019a2b");
    assert!(codex.contains("Codex"), "{codex}");
    assert!(codex.contains("busy · 2 min ago"), "{codex}");
    let other = row("macos-41");
    assert!(other.contains("Claude Code"), "{other}");
    assert!(other.contains("in web"), "{other}");
    let wrote = row("macos-7");
    assert!(wrote.contains("wrote this change"), "{wrote}");
    assert!(
        wrote.contains('▌'),
        "the selected row has the bar:\n{screen}"
    );
    assert!(!other.contains("wrote this change"));
    // The message's first lines under a rule, then the keys.
    assert!(screen.contains(HEAD), "{screen}");
    assert!(screen.contains("The advice:"), "{screen}");
    assert!(
        screen.contains("⏎ send · j k move · esc cancel"),
        "{screen}"
    );
}

#[test]
fn a_click_on_esc_cancel_closes_the_picker() {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let mut d = driver(advised());
    found(&mut d, targets(), PiState::NotFound);
    let screen = d.render(120, 30);
    let (y, line) = screen
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("esc cancel"))
        .expect("the keys");
    let x = line[..line.find("esc cancel").unwrap()].chars().count() as u16;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        d.app.on_mouse(MouseEvent {
            kind,
            column: x + 1,
            row: y as u16,
            modifiers: KeyModifiers::NONE,
        });
    }
    assert_eq!(d.overlay(), &Overlay::None);
}

// ---- The tile's keys ----------------------------------------------------------

#[test]
fn a_focused_tile_with_something_to_send_says_s_send_on_its_edge() {
    let mut d = driver(advised());
    d.keys("l<tab>");
    let screen = d.render(140, 40);
    assert!(screen.contains(" s send · tab next · h close "), "{screen}");
    // The keys tile lists it too.
    assert!(screen.contains("s send to agent"), "{screen}");

    // Nothing to send: no hint.
    let mut d = driver(charge());
    d.keys("l<tab>");
    let screen = d.render(140, 40);
    assert!(!screen.contains("s send ·"), "{screen}");
}

// ---- The window's view ----------------------------------------------------------

#[test]
fn the_window_gets_the_sessions_where_each_runs_and_which_wrote_the_change() {
    use teer::session::view::{OverlayView, snapshot};
    let mut d = driver(advised());
    let mut ts = targets();
    ts[1].cwd = PathBuf::from("/v2/repo/web");
    found(&mut d, ts, PiState::NoExtension);
    let OverlayView::SendTo {
        what,
        targets,
        sel,
        text,
        inside,
        wrote,
        pi_install,
        confirm,
    } = snapshot(&d.app, None).overlay
    else {
        panic!("no picker");
    };
    assert_eq!(what, "the advice");
    assert_eq!(targets.len(), 3);
    assert_eq!((sel, wrote), (2, Some(2)));
    assert!(text.starts_with(HEAD));
    assert_eq!(inside, vec!["", "web", ""]);
    assert!(pi_install);
    assert_eq!(confirm, None);
    // The pi row asks first.
    d.app.send_pick(3);
    let OverlayView::SendTo { confirm, .. } = snapshot(&d.app, None).overlay else {
        panic!("no picker");
    };
    assert_eq!(confirm.as_deref(), Some(teer::session::send::PI_QUESTION));
    // Serialized as the window reads it.
    let json = serde_json::to_value(snapshot(&d.app, None).overlay).unwrap();
    assert_eq!(json["kind"], "SendTo");
    assert_eq!(json["targets"][0]["agent"], "Codex");
    assert_eq!(json["targets"][2]["agent"], "ClaudeCode");
}
