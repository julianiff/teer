//! Phase 5, the unit view (U1–U7 of `docs/v2-map.md` §11, and the
//! acceptance scenario "look closer"): any unit opens with why it has its
//! level, what changed, what to do about it and the second model; the
//! recommendation is asked for before the reviewer does; `K` asks in a panel
//! under the diff; go-to, the leader tools, and deciding without leaving the
//! view. Keys go in as Neovim notation; the agent's answers come in as the
//! `AppEvent`s the runtime would send, so nothing calls a model.
//!
//! Names the unit view's code may choose differently are kept in the
//! helpers right below, so they can be adapted in one place.

mod common;

use std::path::PathBuf;

use common::v2::{Driver, find, review, unit};
use common::{FixtureRepo, isolate_env};
use teer::config::Config;
use teer::review::ask::Progress;
use teer::review::attention::Level;
use teer::review::dive::DiveEvent;
use teer::review::model::{
    Answer, Change, Cite, Coverage, Decision, DiveKind, FoundTest, LineRef, Marker, Prep, Range,
    Reach, Related, Review, Risk, SecondOpinion, Side, TestSearch, Unit, Verdict, Visual,
};
use teer::review::{BuildOptions, build_review};
use teer::tui::app::{AppEvent, Effect, JumpKind, Overlay, Screen};
use teer::tui::unit::Panel;

use Decision::{Accepted as A, Investigate as F, Pending as P};

// ---- Guesses about names the unit view's code chooses -------------------------

/// The section headings, top to bottom (plan, Phase 5).
const WHY: &str = "Why this level";
const WHAT_CHANGED: &str = "What changed";
const WHAT_TO_DO: &str = "What to do about it";
const SECOND: &str = "Second model";

/// Units whose recommendation `effects` ask for (today's `Effect::Dive` of
/// kind Recommend; prefetching may use the same effect).
fn recommends(effects: &[Effect]) -> Vec<String> {
    asked(effects, DiveKind::Recommend)
}

/// Units whose `kind` answer `effects` ask for.
fn asked(effects: &[Effect], kind: DiveKind) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Dive {
                unit_id, kind: k, ..
            } if *k == kind => Some(unit_id.clone()),
            _ => None,
        })
        .collect()
}

/// The question `effects` send to the agent (`Effect::Ask`).
fn question_sent(effects: &[Effect]) -> Option<usize> {
    effects.iter().find_map(|e| match e {
        Effect::Ask { index } => Some(*index),
        _ => None,
    })
}

/// Is the `K` panel open under the diff?
fn ask_panel_open(d: &Driver) -> bool {
    d.app.unit.panel == Some(Panel::Ask)
}

/// Close a jump list `gd` opened by picking its first entry; a `gd` that
/// jumped at once leaves nothing to pick.
fn pick_jump(d: &mut Driver) {
    if matches!(d.overlay(), Overlay::Jump { .. }) {
        d.keys("<cr>");
    }
}

// ---- Fixtures ---------------------------------------------------------------------

/// Unit names: none is part of another, so the screen tells them apart.
const NAMES: [&str; 7] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf",
];

fn at(line: u32) -> Vec<LineRef> {
    vec![LineRef {
        side: Side::New,
        line,
    }]
}

/// Unit `u{i}` named `NAMES[i]` in `f{i/3}.rs`, its level set by `risk`: ▲
/// high, ◆ medium (described by the agent), ● when `None` (needs no
/// description).
fn leveled(i: usize, risk: Option<Risk>) -> Unit {
    let mut u = unit(i, &format!("f{}.rs", i / 3));
    u.symbol = NAMES[i].into();
    u.new_range = Some(Range::new(1 + 10 * (i as u32 % 3), 5 + 10 * (i as u32 % 3)));
    u.summary = Some(format!("{} now charges once.", NAMES[i]));
    if let Some(risk) = risk {
        u.prep = Prep::Ready;
        u.changes = vec![Change {
            marker: Marker::Changed,
            statement: format!("{} charges once.", NAMES[i]),
            evidence: at(2),
            risk,
        }];
    }
    u
}

/// `u` with a second model that disputes its first statement.
fn disputed(mut u: Unit) -> Unit {
    let statement = u.changes[0].statement.clone();
    u.checks.second = Some(SecondOpinion {
        risk: Risk::High,
        verdicts: vec![Verdict {
            statement,
            agree: false,
            why: "it still charges twice on retry".into(),
            evidence: at(2),
        }],
        missed: Vec::new(),
    });
    u
}

/// No web search: questions go out without the one-time confirm.
fn cfg() -> Config {
    Config {
        dive_web: false,
        ..Config::default()
    }
}

/// The unit the unit view shows: the first unit name on the screen below
/// the header (its title row), as an id.
fn on_show(d: &Driver) -> Option<String> {
    let screen = d.render(140, 40);
    let rows: Vec<&str> = screen.lines().collect();
    let body = &rows[1..rows.len() - 1];
    body.iter().find_map(|row| {
        d.app
            .review
            .units
            .iter()
            .find(|u| row.contains(&u.symbol))
            .map(|u| u.id.clone())
    })
}

/// Open the unit view on unit `id` from the queue.
fn open(d: &mut Driver, id: &str) -> Vec<Effect> {
    d.select_id(id);
    let effects = d.keys("l");
    assert_eq!(d.screen(), Screen::Unit, "{id}");
    assert_eq!(on_show(d).as_deref(), Some(id));
    effects
}

/// The agent's answer of `kind` about unit `id` arrives.
fn answer(d: &mut Driver, id: &str, kind: DiveKind, text: &str) -> Vec<Effect> {
    d.app.on_app_event(AppEvent::Dive(DiveEvent::Ready {
        id: id.into(),
        kind,
        text: text.into(),
        sources: Vec::new(),
        visual: None,
        behavior: None,
        coverage: None,
    }))
}

/// Pending units at ▲ or ◆, in queue order, by id.
fn closer_looks(d: &Driver) -> Vec<String> {
    d.app
        .order
        .iter()
        .filter(|&&i| {
            let u = &d.app.review.units[i];
            u.decision == P && matches!(d.app.shown_level(i), Level::Attention | Level::Look)
        })
        .map(|&i| d.app.review.units[i].id.clone())
        .collect()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// The headings of `want` that `screen` shows, each followed by something
/// before the next heading or the end.
fn sections_with_content(screen: &str, want: &[&str]) -> Vec<String> {
    let rows: Vec<&str> = screen.lines().collect();
    let mut shown = Vec::new();
    for h in want {
        let Some((_, y)) = find(screen, h) else {
            continue;
        };
        let next = rows[y as usize + 1..]
            .iter()
            .map(|r| r.trim_matches(|c: char| c.is_whitespace() || "│─┃".contains(c)))
            .find(|r| !r.is_empty());
        let filled = next.is_some_and(|r| !want.iter().any(|w| r.starts_with(w)));
        assert!(filled, "{h} is empty:\n{screen}");
        shown.push(h.to_string());
    }
    shown
}

// ---- U1 ---------------------------------------------------------------------------

/// `big` changes 30 lines at its top and one at its end, far apart.
fn two_hunks() -> (FixtureRepo, Review) {
    isolate_env();
    let body = |head: &dyn Fn(usize) -> String, tail: &str| {
        let mut s = String::from("def big(x):\n");
        for i in 0..30 {
            s.push_str(&format!("    {}\n", head(i)));
        }
        for i in 0..20 {
            s.push_str(&format!("    kept_{i} = {i}\n"));
        }
        s.push_str(&format!("    tail = '{tail}'\n    return x\n"));
        s
    };
    let repo = FixtureRepo::new();
    repo.write("big.py", &body(&|i| format!("old_line_{i} = {i}"), "old"))
        .commit("init");
    repo.write(
        "big.py",
        &body(&|i| format!("first_hunk_{i} = {i}"), "second_hunk"),
    );
    let mut r = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let u = r.units.iter_mut().find(|u| u.symbol == "big").unwrap();
    u.prep = Prep::Ready;
    u.changes = vec![Change {
        marker: Marker::Changed,
        statement: "Renames the counters.".into(),
        evidence: at(2),
        risk: Risk::Medium,
    }];
    (repo, r)
}

#[test]
fn sections_and_hunk_jumps() {
    let (_repo, r) = two_hunks();
    let id = r
        .units
        .iter()
        .find(|u| u.symbol == "big")
        .unwrap()
        .id
        .clone();
    let mut d = Driver::open_with_agent(r.clone(), cfg());
    open(&mut d, &id);

    // Title row: level, file, name, state.
    let screen = d.render(140, 200);
    let title = screen.lines().skip(1).find(|l| l.contains("big")).unwrap();
    assert!(title.contains("big.py"), "{title}");
    assert!(title.contains("pending"), "{title}");
    assert!(
        ['▲', '◆', '●'].iter().any(|g| title.contains(*g)),
        "{title}"
    );
    // The sections in order; the second model only when it disputes.
    let shown = sections_with_content(&screen, &[WHY, WHAT_CHANGED, WHAT_TO_DO, SECOND]);
    assert_eq!(shown, [WHY, WHAT_CHANGED, WHAT_TO_DO], "{screen}");
    let rows: Vec<u16> = shown.iter().map(|h| find(&screen, h).unwrap().1).collect();
    assert!(
        rows.windows(2).all(|w| w[0] < w[1]),
        "top to bottom: {rows:?}"
    );

    let mut r = r;
    let u = r.units.iter_mut().find(|u| u.symbol == "big").unwrap();
    *u = disputed(u.clone());
    let mut disputed_view = Driver::open_with_agent(r, cfg());
    open(&mut disputed_view, &id);
    let screen = disputed_view.render(140, 200);
    let shown = sections_with_content(&screen, &[WHY, WHAT_CHANGED, WHAT_TO_DO, SECOND]);
    assert_eq!(shown, [WHY, WHAT_CHANGED, WHAT_TO_DO, SECOND], "{screen}");

    // `]c` and `[c` jump between the hunks.
    let first = |s: &str| s.contains("old_line_0 ") || s.contains("first_hunk_0 ");
    let screen = d.render(140, 30);
    assert!(
        first(&screen) && !screen.contains("second_hunk"),
        "{screen}"
    );
    let mut presses = 0;
    while !d.render(140, 30).contains("second_hunk") && presses < 3 {
        d.keys("]c");
        presses += 1;
    }
    let screen = d.render(140, 30);
    assert!(
        screen.contains("second_hunk"),
        "after {presses} ]c:\n{screen}"
    );
    assert!(!first(&screen), "{screen}");
    let mut presses = 0;
    while !first(&d.render(140, 30)) && presses < 3 {
        d.keys("[c");
        presses += 1;
    }
    assert!(first(&d.render(140, 30)), "after {presses} [c");
    assert_eq!(d.screen(), Screen::Unit);
}

#[test]
fn l_and_enter_open_any_unit_and_h_goes_back_to_its_row() {
    let mut units: Vec<Unit> = (0..4).map(|i| leveled(i, None)).collect();
    units[1].decision = A;
    units[2].decision = F;
    let mut d = Driver::open(review(units), cfg());
    for (id, keys) in [("u0", "l"), ("u1", "<cr>"), ("u2", "l"), ("u3", "<cr>")] {
        d.select_id(id);
        let row = d.sel();
        d.keys(keys);
        assert_eq!(d.screen(), Screen::Unit, "{id}");
        assert_eq!(on_show(&d).as_deref(), Some(id));
        d.keys(if id == "u2" { "<esc>" } else { "h" });
        assert_eq!(d.screen(), Screen::Queue, "{id}");
        assert_eq!(d.sel(), row, "{id}: back on its row");
    }
}

// ---- U2 ---------------------------------------------------------------------------

/// alpha ▲, bravo ◆, charlie ● (f0); delta ▲, echo ◆ (f1); foxtrot ◆,
/// golf ● (f2).
fn mixed() -> Vec<Unit> {
    use Risk::{High, Medium};
    [
        Some(High),
        Some(Medium),
        None,
        Some(High),
        Some(Medium),
        Some(Medium),
        None,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, r)| leveled(i, r))
    .collect()
}

#[test]
fn prefetch_and_cache() {
    let mut d = Driver::open_with_agent(review(mixed()), cfg());
    let looks = closer_looks(&d);
    assert_eq!(looks.len(), 5, "{looks:?}");
    // As the queue opens: the first three pending ▲ and ◆.
    let first = looks[..3].to_vec();
    assert_eq!(sorted(recommends(&d.effects)), sorted(first.clone()));

    // Opening the third asks for nothing twice, and for the next pending unit.
    let third = first[2].clone();
    let pos = d.pos(&third);
    let next = d.id_at(d.app.next_pending_pos(pos).unwrap());
    assert!(!first.contains(&next));
    let effects = open(&mut d, &third);
    assert_eq!(recommends(&effects), vec![next.clone()], "{effects:?}");

    // The answer shows without a key; opened again, it is not asked again.
    answer(
        &mut d,
        &third,
        DiveKind::Recommend,
        "Add a test for the retry.",
    );
    assert!(d.render(140, 60).contains("Add a test for the retry."));
    d.keys("h");
    let effects = open(&mut d, &third);
    assert!(recommends(&effects).is_empty(), "cached: {effects:?}");
    assert!(d.render(140, 60).contains("Add a test for the retry."));
    d.keys("h");

    // Any unit, routine too, gets its recommendation as it opens.
    let effects = open(&mut d, "u6");
    assert!(
        recommends(&effects).contains(&"u6".to_string()),
        "{effects:?}"
    );
}

#[test]
fn a_recommendation_is_asked_again_once_the_code_changed() {
    let mut d = Driver::open_with_agent(review(mixed()), cfg());
    let id = closer_looks(&d)[0].clone();
    open(&mut d, &id);
    answer(&mut d, &id, DiveKind::Recommend, "Fine as it is.");
    d.keys("h");
    // The cache is per unit and content hash: a re-run on the same code
    // keeps the answer...
    d.app
        .on_app_event(AppEvent::Rerun(Ok(Box::new(review(mixed())))));
    let effects = open(&mut d, &id);
    assert!(!recommends(&effects).contains(&id), "{effects:?}");
    assert!(d.render(140, 60).contains("Fine as it is."));
    d.keys("h");
    // ...and one after the code changed asks again.
    let mut changed = review(mixed());
    changed.unit_mut(&id).unwrap().fingerprint = "changed".into();
    d.app.on_app_event(AppEvent::Rerun(Ok(Box::new(changed))));
    let effects = open(&mut d, &id);
    assert_eq!(recommends(&effects).first(), Some(&id), "{effects:?}");
}

// ---- U3 ---------------------------------------------------------------------------

#[test]
fn no_agent_hides_agent_sections() {
    let units = vec![disputed(leveled(0, Some(Risk::High))), leveled(1, None)];
    let mut d = Driver::open(review(units), cfg());
    assert!(d.effects.is_empty(), "no agent, no calls: {:?}", d.effects);
    for id in ["u0", "u1"] {
        let effects = open(&mut d, id);
        assert!(effects.is_empty(), "{id}: {effects:?}");
        let screen = d.render(140, 60);
        let shown = sections_with_content(&screen, &[WHY, WHAT_CHANGED, WHAT_TO_DO, SECOND]);
        assert!(shown.contains(&WHAT_CHANGED.to_string()), "{id}:\n{screen}");
        assert!(!screen.contains(WHAT_TO_DO), "{id}:\n{screen}");
        assert!(!screen.contains(SECOND), "{id}:\n{screen}");
        d.keys("h");
    }
}

// ---- U4 ---------------------------------------------------------------------------

#[test]
fn ask_panel_lifecycle() {
    let units: Vec<Unit> = (0..3).map(|i| leveled(i, Some(Risk::Medium))).collect();
    let mut d = Driver::open_with_agent(review(units), cfg());
    open(&mut d, "u0");

    // Focused: what is typed is the question, not keys.
    d.keys("K");
    assert!(ask_panel_open(&d));
    assert_eq!(d.screen(), Screen::Unit, "a panel in the unit view");
    let effects = d.keys("is alpha safe after a refund?<cr>");
    let first = question_sent(&effects).expect("asked");
    assert_eq!(d.decision_of("u0"), P, "typing decides nothing");
    let q = &d.app.review.questions[first];
    assert_eq!(q.text, "is alpha safe after a refund?");
    assert_eq!(q.unit.as_deref(), Some("u0"));

    // Steps, then the answer as it is written, then the answer.
    d.app.on_app_event(AppEvent::Asking {
        index: first,
        progress: Progress::Step("read f0.rs".into()),
    });
    assert!(d.render(140, 60).contains("read f0.rs"));
    d.app.on_app_event(AppEvent::Asking {
        index: first,
        progress: Progress::Text("It is safe".into()),
    });
    assert!(d.render(140, 60).contains("It is safe"));
    d.app.on_app_event(AppEvent::Answer {
        index: first,
        result: Ok(Answer {
            text: "It is safe: bravo refunds first.".into(),
            related: vec![Related {
                id: "u1".into(),
                why: "it refunds".into(),
            }],
            ..Default::default()
        }),
    });
    assert!(
        d.render(140, 60)
            .contains("It is safe: bravo refunds first.")
    );

    // A follow-up stays in the same panel, about the same unit.
    let effects = d.keys("and twice?<cr>");
    let second = question_sent(&effects).expect("asked again");
    assert_eq!(d.app.review.questions[second].unit.as_deref(), Some("u0"));
    assert!(ask_panel_open(&d));

    // Esc closes it; the conversation is kept with the unit.
    d.keys("<esc>");
    assert!(!ask_panel_open(&d));
    assert_eq!(d.screen(), Screen::Unit);
    assert_eq!(on_show(&d).as_deref(), Some("u0"));
    d.keys("K");
    let screen = d.render(140, 60);
    assert!(screen.contains("is alpha safe after a refund?"), "{screen}");
    assert!(screen.contains("and twice?"), "{screen}");
    d.keys("<esc>");
    d.keys("n");
    assert_eq!(on_show(&d).as_deref(), Some("u1"));
    d.keys("K");
    assert!(
        !d.render(140, 60).contains("is alpha safe after a refund?"),
        "another unit, another conversation"
    );
    d.keys("<esc>N");
    assert_eq!(on_show(&d).as_deref(), Some("u0"));

    // `tab` goes to the links, `enter` follows one.
    d.keys("K<tab><cr>");
    assert_eq!(d.screen(), Screen::Unit);
    assert_eq!(on_show(&d).as_deref(), Some("u1"), "to the related unit");
}

// ---- U5 ---------------------------------------------------------------------------

#[test]
fn goto_definition_callers_editor() {
    let mut units: Vec<Unit> = (0..7).map(|i| leveled(i, None)).collect();
    // alpha calls delta (another file); golf calls a function outside the review.
    units[0].callees = vec!["u3".into()];
    units[3].callers = vec!["u0".into()];
    units[6].callees = vec!["lib.rs::outside".into()];
    let mut d = Driver::open(review(units), cfg());

    open(&mut d, "u0");
    d.keys("gd");
    pick_jump(&mut d);
    assert_eq!(d.screen(), Screen::Unit);
    assert_eq!(on_show(&d).as_deref(), Some("u3"), "to the definition");

    d.keys("gr");
    assert!(
        matches!(d.overlay(), Overlay::Jump { kind: JumpKind::Callers { of }, .. } if of == "u3"),
        "{:?}",
        d.overlay()
    );
    let callers: Vec<String> = d
        .app
        .jump_items()
        .iter()
        .map(|&i| d.app.review.units[i].id.clone())
        .collect();
    assert_eq!(callers, ["u0"]);
    d.keys("<esc>");
    assert_eq!(d.screen(), Screen::Unit);

    // The editor at the unit's first changed line.
    let effects = d.keys("gf");
    let edit = effects.iter().find_map(|e| match e {
        Effect::Edit { file, line } => Some((file.clone(), *line)),
        _ => None,
    });
    let (file, line) = edit.expect("the editor opens");
    assert_eq!(file, PathBuf::from("f1.rs"));
    assert!((1..=5).contains(&line), "{line}");

    // Outside the review: shown read-only, nothing decided, still here.
    d.keys("h");
    open(&mut d, "u6");
    d.keys("gd");
    assert_eq!(d.screen(), Screen::Unit);
    assert!(
        *d.overlay() != Overlay::None || !d.status().is_empty(),
        "says or shows something"
    );
    assert!(d.decisions().iter().all(|x| *x == P));
}

// ---- U6 ---------------------------------------------------------------------------

#[test]
fn leader_panels_and_gx() {
    let units: Vec<Unit> = (0..3).map(|i| leveled(i, Some(Risk::High))).collect();
    let mut d = Driver::open_with_agent(review(units), cfg());
    open(&mut d, "u0");

    // `<space>` alone lists the tools.
    d.keys("<space>");
    let bar = d.bar(200, 40);
    for tool in [
        "tests that cover this unit",
        "run old and new",
        "diagram of the change",
    ] {
        assert!(bar.contains(tool), "{tool}: {bar}");
    }
    d.keys("<esc>");

    // Tests: teer's search, then the agent reads it on its own (Q6).
    let effects = d.keys("<space>t");
    assert_eq!(asked(&effects, DiveKind::Tests), ["u0"], "{effects:?}");
    let found = TestSearch {
        tests: vec![FoundTest {
            name: "test_alpha_charges_once".into(),
            cite: Cite {
                file: PathBuf::from("tests/test_f0.py"),
                start: 1,
                end: 4,
            },
            reach: Reach::Names,
        }],
        total: 1,
    };
    let effects = d.app.on_app_event(AppEvent::Dive(DiveEvent::Ready {
        id: "u0".into(),
        kind: DiveKind::Tests,
        text: String::new(),
        sources: Vec::new(),
        visual: None,
        behavior: None,
        coverage: Some(Box::new(Coverage::found(found))),
    }));
    assert!(
        effects.contains(&Effect::InterpretTests {
            unit_id: "u0".into()
        }),
        "{effects:?}"
    );
    assert!(d.render(140, 60).contains("test_alpha_charges_once"));

    // Run old and new: one panel at a time.
    let effects = d.keys("<space>r");
    assert_eq!(asked(&effects, DiveKind::Behavior), ["u0"], "{effects:?}");
    answer(
        &mut d,
        "u0",
        DiveKind::Behavior,
        "Old and new agree on every input.",
    );
    let screen = d.render(140, 60);
    assert!(
        screen.contains("Old and new agree on every input."),
        "{screen}"
    );
    assert!(!screen.contains("test_alpha_charges_once"), "{screen}");

    // The diagram, and `gx` opens it in the browser.
    let effects = d.keys("<space>v");
    assert_eq!(asked(&effects, DiveKind::Visualize), ["u0"], "{effects:?}");
    d.app.on_app_event(AppEvent::Dive(DiveEvent::Ready {
        id: "u0".into(),
        kind: DiveKind::Visualize,
        text: "Orders flow to the payment.".into(),
        sources: Vec::new(),
        visual: Some(Box::new(Visual {
            claim: "Orders flow to the payment.".into(),
            nodes: Vec::new(),
            edges: Vec::new(),
            path: PathBuf::from("/v2/u0.html"),
        })),
        behavior: None,
        coverage: None,
    }));
    let screen = d.render(140, 60);
    assert!(screen.contains("Orders flow to the payment."), "{screen}");
    assert!(!screen.contains("Old and new agree"), "{screen}");
    assert_eq!(
        d.keys("gx"),
        vec![Effect::OpenVisual {
            unit_id: "u0".into()
        }]
    );
    assert_eq!(d.screen(), Screen::Unit);
}

// ---- U7 ---------------------------------------------------------------------------

#[test]
fn decide_and_advance_in_unit_view() {
    let units: Vec<Unit> = (0..6).map(|i| leveled(i, None)).collect();
    let mut d = Driver::open(review(units), cfg());
    open(&mut d, "u0");

    // `n` and `N` move between pending units and decide nothing.
    d.keys("n");
    assert_eq!(on_show(&d).as_deref(), Some("u1"));
    d.keys("N");
    assert_eq!(on_show(&d).as_deref(), Some("u0"));
    assert!(d.decisions().iter().all(|x| *x == P));

    // `a`, `.` and `f` decide and go on to the next pending unit, here.
    d.keys("a");
    assert_eq!(d.decision_of("u0"), A);
    assert_eq!(
        (d.screen(), on_show(&d).as_deref()),
        (Screen::Unit, Some("u1"))
    );
    d.keys(".");
    assert_eq!(d.decision_of("u1"), A);
    assert_eq!(on_show(&d).as_deref(), Some("u2"));
    d.keys("f");
    d.keys("charges twice<cr>");
    assert_eq!(d.decision_of("u2"), F);
    assert_eq!(d.note("u2"), "charges twice");
    assert_eq!(
        (d.screen(), on_show(&d).as_deref()),
        (Screen::Unit, Some("u3"))
    );

    // `u` takes the flag back.
    d.keys("u");
    assert_eq!(d.decision_of("u2"), P);
    assert_eq!(d.screen(), Screen::Unit);

    // `A`: the file's pending units, at once.
    let shown = on_show(&d).unwrap();
    let file = d.app.review.unit(&shown).unwrap().file.clone();
    let before: Vec<(String, Decision)> = d
        .app
        .review
        .units
        .iter()
        .map(|u| (u.id.clone(), u.decision))
        .collect();
    d.keys("A");
    assert_eq!(*d.overlay(), Overlay::None, "no confirm");
    assert_eq!(d.screen(), Screen::Unit);
    for (id, was) in before {
        let u = d.app.review.unit(&id).unwrap();
        let want = if u.file == file && was == P { A } else { was };
        assert_eq!(u.decision, want, "{id}");
    }
}

#[test]
fn shift_f_in_the_unit_view_flags_the_file_with_one_note() {
    let units: Vec<Unit> = (0..6).map(|i| leveled(i, None)).collect();
    let mut d = Driver::open(review(units), cfg());
    d.app.review.unit_mut("u5").unwrap().decision = A;
    open(&mut d, "u4");
    d.keys("F");
    d.keys("same bug<cr>");
    assert_eq!(d.decisions(), vec![P, P, P, F, F, A]);
    assert_eq!(d.note("u3"), "same bug");
    assert_eq!(d.note("u4"), "same bug");
    assert_eq!(d.note("u5"), "");
    assert_eq!(d.screen(), Screen::Unit);
}

// ---- Acceptance: look closer ----------------------------------------------------

#[test]
fn look_closer() {
    let mut units = mixed();
    // alpha calls bravo.
    units[0].callees = vec!["u1".into()];
    units[1].callers = vec!["u0".into()];
    let mut d = Driver::open_with_agent(review(units), cfg());
    let first = closer_looks(&d)[0].clone();
    assert_eq!(
        d.selected(),
        Some(first.clone()),
        "the queue starts on the first ▲"
    );
    assert_eq!(first, "u0");
    let row = d.sel();

    // `l`: the recommendation is there without a key.
    let mut asked_for = recommends(&d.effects);
    asked_for.extend(recommends(&d.keys("l")));
    assert!(asked_for.contains(&first), "{asked_for:?}");
    answer(
        &mut d,
        &first,
        DiveKind::Recommend,
        "Add a test for the retry.",
    );
    assert!(d.render(140, 60).contains("Add a test for the retry."));

    // `K` streams an answer.
    let effects = d.keys("Kwhy does it retry?<cr>");
    let q = question_sent(&effects).expect("asked");
    d.app.on_app_event(AppEvent::Asking {
        index: q,
        progress: Progress::Text("Because the card".into()),
    });
    assert!(d.render(140, 60).contains("Because the card"));
    d.app.on_app_event(AppEvent::Answer {
        index: q,
        result: Ok(Answer {
            text: "Because the card may be declined.".into(),
            ..Default::default()
        }),
    });
    assert!(
        d.render(140, 60)
            .contains("Because the card may be declined.")
    );
    d.keys("<esc>");

    // `gd` jumps to a definition in the review; `h` back to the same row.
    d.keys("gd");
    pick_jump(&mut d);
    assert_eq!(on_show(&d).as_deref(), Some("u1"));
    d.keys("h");
    assert_eq!(d.screen(), Screen::Queue);
    assert_eq!(d.sel(), row);
}
