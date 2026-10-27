//! Render tests for the Flagged screen and its overlays.

mod common;

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::config::{Config, KeyMap};
use teer::review::model::{
    Behavior, Decision, DiveKind, DiveResult, Expect, Outcome, Prep, ProbeCase, Range, Resolution,
    Review, TestCheck, Unit, UnitKind, UnitStatus,
};
use teer::tui::app::{App, Overlay, Pane, Screen};
use teer::tui::ui;

fn unit(i: usize, range: Option<Range>) -> Unit {
    Unit {
        id: format!("src/f{i}.rs::func_{i}"),
        file: PathBuf::from(format!("src/f{i}.rs")),
        symbol: format!("func_{i}"),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: None,
        new_range: range,
        renamed_from: None,
        fingerprint: String::new(),
        changes: Vec::new(),
        prep: Prep::Unavailable,
        callers: vec!["caller_fn".into()],
        callees: vec!["callee_fn".into()],
        decision: Decision::Investigate,
        summary: Some("Changes how the thing is computed".into()),
        nbc_reason: None,
        dive: Default::default(),
        dive_pending: Default::default(),
        checks: Default::default(),
    }
}

fn app() -> App {
    common::isolate_env();
    let units = vec![
        unit(0, Some(Range::new(4, 4))),
        unit(1, Some(Range::new(10, 20))),
        unit(2, None),
    ];
    let review = Review {
        id: "x".into(),
        repo_root: PathBuf::from("/r"),
        base: "b".into(),
        head: "h".into(),
        head_commit: "h".into(),
        agent_session: None,
        pr: None,
        intent: Default::default(),
        created_at: chrono::Utc::now(),
        files: Vec::new(),
        units,
        skipped: Vec::new(),
        graph: Default::default(),
        questions: Vec::new(),
    };
    let mut a = App::new(review, Config::default(), KeyMap::default());
    a.screen = Screen::Flagged;
    a
}

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const SIZES: [(u16, u16); 3] = [(60, 20), (80, 24), (120, 40)];

#[test]
fn nothing_asked_yet_offers_every_question_with_its_key() {
    let a = app();
    for (w, h) in SIZES {
        let out = render(&a, w, h);
        assert!(out.contains("func_0"), "{w}x{h}:\n{out}");
        assert!(
            out.contains("Not sure yet? Bring more light into it:"),
            "{w}x{h}:\n{out}"
        );
        // Each question once in the tile, after its key.
        for q in [
            "n  Recommend",
            "v  Visualize",
            "c  Tests",
            "t  Behavior",
            "x  Disagreement",
        ] {
            assert_eq!(out.matches(q).count(), 1, "{q} at {w}x{h}:\n{out}");
        }
        // The keys that ask are on top of the tile, named where they fit.
        let top = out
            .lines()
            .find(|l| l.contains("╭ +"))
            .unwrap_or_else(|| panic!("{w}x{h}:\n{out}"));
        assert!(
            top.contains("+ n Recommend · v Visualize · c Tests · t Behavior · x Disagreement")
                || top.contains("+ n v c t x"),
            "{w}x{h}: {top}"
        );
    }
}

#[test]
fn the_unit_is_a_header_row_then_its_explanation_then_the_tiles() {
    let a = app();
    let out = render(&a, 120, 40);
    let rows: Vec<&str> = out.lines().collect();
    let at = |s: &str| {
        rows.iter()
            .position(|r| r.contains(s))
            .unwrap_or_else(|| panic!("{s}:\n{out}"))
    };
    // The list is folded into the header: name, place, which of the three.
    let head = at("func_0  src/f0.rs:4");
    assert_eq!(head, 1, "{out}");
    assert!(rows[head].contains("1/3 · j k other units"), "{out}");
    assert!(
        !rows[head].contains("investigate"),
        "no decision badge: {}",
        rows[head]
    );
    assert!(!out.contains("func_1"), "one unit at a time:\n{out}");
    assert!(rows[head + 1].trim().is_empty(), "{out}");
    assert!(rows[head + 2].contains("caller"), "the explanation:\n{out}");
    // What the agent said, behind its bar, above the tiles; no callers or
    // callees lists (the reasons count the callers).
    let tiles = at("╭");
    assert!(at("┃ agent (haiku)") < tiles, "{out}");
    assert!(rows[tiles - 1].trim().is_empty(), "{out}");
    assert!(!out.contains("callees:"), "{out}");
}

#[test]
fn result_sources_and_tab_checkmark() {
    let mut a = app();
    a.review.units[0].dive.results.insert(
        DiveKind::Recommend,
        DiveResult {
            text: "It rounds the total before tax.".into(),
            sources: vec!["https://example.com/rounding".into()],
            visual: None,
            behavior: None,
            coverage: None,
        },
    );
    for (w, h) in SIZES {
        let out = render(&a, w, h);
        assert!(out.contains("rounds the total"), "{w}x{h}:\n{out}");
        assert!(out.contains("Sources"), "{w}x{h}:\n{out}");
        assert!(out.contains("example.com"), "{w}x{h}:\n{out}");
        assert!(out.contains("✓"), "{w}x{h}:\n{out}");
    }
    // The answer says who wrote it, under it on the tile's border: Flagged
    // questions run on Claude Code's own model.
    let out = render(&a, 120, 40);
    let at = |s: &str| out.find(s).unwrap_or_else(|| panic!("{s} missing:\n{out}"));
    assert!(at("agent (Claude Code's default model)") > at("It rounds the total"));
    assert!(out.contains("╭ n Recommend ✓"), "{out}");
}

fn case(name: &str, expect: Expect, before: Outcome, after: Outcome) -> ProbeCase {
    ProbeCase {
        name: name.into(),
        input: format!("{name} input"),
        expect,
        why: format!("{name} reason"),
        before,
        after,
        claim: 0,
    }
}

#[test]
fn behavior_tab_lists_surprises_first_with_both_sides() {
    let mut a = app();
    let v = |s: &str| Outcome::Value(s.into());
    let b = Behavior {
        cases: vec![
            case("ordinary", Expect::Same, v("12.5"), v("12.5")),
            case("edge", Expect::Different, v("100"), v("90.0")),
            case(
                "negative",
                Expect::Same,
                v("-5"),
                Outcome::Error("ValueError: negative amount".into()),
            ),
        ],
        command: vec!["python3".into(), "teer_probe.py".into()],
        dir: PathBuf::from("/data/probes/x/u"),
        claims: Vec::new(),
        tests: Some(TestCheck::Unguarded),
        test_command: vec!["python3".into(), "-m".into(), "pytest".into()],
    };
    a.review.units[0].dive.results.insert(
        DiveKind::Behavior,
        DiveResult {
            text: b.summary(),
            sources: Vec::new(),
            visual: None,
            behavior: Some(b),
            coverage: None,
        },
    );
    // Zoomed, the tile has the width of the screen.
    a.flagged.pane = Pane::Answer(DiveKind::Behavior);
    a.flagged.zoom = true;
    let out = render(&a, 120, 40);
    assert!(
        out.contains("3 cases ran before and after the change: 2 behave differently"),
        "{out}"
    );
    let at = |s: &str| out.find(s).unwrap_or_else(|| panic!("{s} missing:\n{out}"));
    assert!(at("! negative") < at("≠ edge") && at("≠ edge") < at("= ordinary"));
    assert!(out.contains("error: ValueError: negative amount"), "{out}");
    assert!(
        out.contains("the agent predicted no change: negative reason"),
        "{out}"
    );
    assert!(out.contains("before  100"), "{out}");
    assert!(out.contains("after   90.0"), "{out}");
    assert!(out.contains("ordinary input  12.5"), "{out}");
    assert!(
        out.contains("ran `python3 teer_probe.py` in a sandbox"),
        "{out}"
    );
    assert!(out.contains("no test notices it"), "{out}");
    assert!(out.contains("`python3 -m pytest`"), "{out}");
    assert!(
        out.contains("probe by the agent (Claude Code's default model), run by teer"),
        "{out}"
    );
    // Narrow and short screens still draw without a panic.
    for (w, h) in SIZES {
        render(&a, w, h);
    }
}

#[test]
fn behavior_pending_says_what_happens() {
    let mut a = app();
    a.review.units[0].dive_pending.insert(DiveKind::Behavior);
    a.flagged.pane = Pane::Answer(DiveKind::Behavior);
    let out = render(&a, 120, 40);
    assert!(out.contains("the agent writes a probe"), "{out}");
}

#[test]
fn pending_shows_spinner_text() {
    let mut a = app();
    a.review.units[0].dive_pending.insert(DiveKind::Recommend);
    for (w, h) in SIZES {
        let out = render(&a, w, h);
        assert!(out.contains("asking the agent"), "{w}x{h}:\n{out}");
    }
}

#[test]
fn resolved_glyphs_and_ranges() {
    let mut a = app();
    a.review.units[0].dive.resolution = Some(Resolution::Ok);
    a.review.units[1].dive.resolution = Some(Resolution::NeedsFix);
    // The header row: how the unit stands, its name and where it is, a
    // single line without a range.
    let head = |a: &App| render(a, 120, 40).lines().nth(1).unwrap().to_string();
    assert!(
        head(&a).starts_with(" ● func_0  src/f0.rs:4 "),
        "{}",
        head(&a)
    );
    assert!(!head(&a).contains("f0.rs:4-4"), "{}", head(&a));
    a.flagged.sel = 1;
    assert!(head(&a).starts_with(" ✗ func_1"), "{}", head(&a));
    a.flagged.sel = 2;
    assert!(head(&a).starts_with(" ⚑ func_2"), "{}", head(&a));
}

#[test]
fn narrow_stacks_without_list_column() {
    let a = app();
    let out = render(&a, 60, 20);
    assert!(out.contains("1/3"), "{out}");
    for line in out.lines() {
        assert!(unicode_width::UnicodeWidthStr::width(line) <= 60);
    }
}

#[test]
fn untrusted_text_is_sanitized() {
    let mut a = app();
    a.review.units[0].dive.results.insert(
        DiveKind::Recommend,
        DiveResult {
            text: "bad\x1b[31m text\x07".into(),
            sources: vec!["src\x1b]0;x".into()],
            visual: None,
            behavior: None,
            coverage: None,
        },
    );
    a.review.units[0].dive.note = "n\x1b[2Jote".into();
    let out = render(&a, 100, 30);
    assert!(!out.contains('\x1b'));
    assert!(!out.contains('\x07'));
}

#[test]
fn tiny_sizes_never_panic() {
    let mut a = app();
    a.review.units[0].dive.results.insert(
        DiveKind::Recommend,
        DiveResult {
            text: "word ".repeat(100),
            sources: vec!["s".repeat(200)],
            visual: None,
            behavior: None,
            coverage: None,
        },
    );
    a.review.units[1].dive_pending.insert(DiveKind::Visualize);
    for (w, h) in [
        (1, 1),
        (2, 2),
        (5, 3),
        (10, 4),
        (20, 6),
        (30, 8),
        (89, 10),
        (90, 10),
        (40, 3),
    ] {
        render(&a, w, h);
        a.overlay = Overlay::Note {
            id: String::new(),
            text: "hello".into(),
            back: None,
        };
        render(&a, w, h);
        a.overlay = Overlay::ConfirmWeb;
        render(&a, w, h);
        a.overlay = Overlay::Help;
        render(&a, w, h);
        a.overlay = Overlay::None;
    }
    let mut empty = app();
    empty.review.units.clear();
    for (w, h) in [(1, 1), (60, 20)] {
        render(&empty, w, h);
    }
}

#[test]
fn overlays_render() {
    let mut a = app();
    a.overlay = Overlay::Note {
        id: String::new(),
        text: "remember to check rounding".into(),
        back: None,
    };
    let out = render(&a, 80, 24);
    assert!(out.contains("Note"), "{out}");
    assert!(out.contains("remember to check rounding"), "{out}");
    assert!(out.contains("enter save"), "{out}");
    assert!(out.contains("esc cancel"), "{out}");
    a.overlay = Overlay::Note {
        id: String::new(),
        text: "x".repeat(300),
        back: None,
    };
    let out = render(&a, 40, 12);
    assert!(out.contains("enter save"), "{out}");
    a.overlay = Overlay::ConfirmWeb;
    let out = render(&a, 80, 24);
    assert!(out.contains("never your code"), "{out}");
    assert!(out.contains("(y/n)"), "{out}");
}

#[test]
fn footer_and_help_mention_flagged_keys() {
    let mut a = app();
    let out = render(&a, 120, 30);
    let last = out.lines().last().unwrap();
    // The tabs name their keys; the footer doesn't repeat them.
    assert!(
        !last.contains("recommend") && !last.contains("visualize"),
        "{last}"
    );
    assert!(
        last.contains("ask again") && last.contains("browser"),
        "{last}"
    );
    assert!(!last.contains("reset"), "{last}");
    a.overlay = Overlay::Help;
    let out = render(&a, 120, 40);
    assert!(out.contains("Flagged"), "{out}");
    assert!(!out.contains("Deep dive"), "{out}");
}

#[test]
fn header_says_flagged_with_the_count_on_the_screen() {
    let a = app();
    let out = render(&a, 120, 30);
    assert!(out.lines().next().unwrap().contains("Flagged (3)"), "{out}");
}

#[test]
fn list_footer_is_editor_swipe_help_then_flagged_then_investigate() {
    let mut a = app();
    a.screen = Screen::Slim;
    let out = render(&a, 140, 30);
    let last = out.lines().last().unwrap();
    let at = |w: &str| {
        last.find(w)
            .unwrap_or_else(|| panic!("no {w:?} in {last:?}"))
    };
    assert!(
        at("open in editor") < at("swipe") && at("swipe") < at("help"),
        "{last}"
    );
    assert!(
        at("help") < at("flagged") && at("flagged") < at("investigate"),
        "{last}"
    );
    for gone in ["accept", "reset", "preview", "dive"] {
        assert!(!last.contains(gone), "{gone}: {last}");
    }
    assert!(out.contains("3 flagged"), "{out}");
    // Narrow footers drop from the end: investigate first, then flagged.
    let out = render(&a, 55, 30);
    let last = out.lines().last().unwrap();
    assert!(
        last.contains("flagged") && !last.contains("investigate"),
        "{last}"
    );
    let out = render(&a, 40, 30);
    let last = out.lines().last().unwrap();
    assert!(last.contains("help") && !last.contains("flagged"), "{last}");
    // Nothing flagged: no flagged button.
    for u in &mut a.review.units {
        u.decision = Decision::Pending;
    }
    let out = render(&a, 140, 30);
    let last = out.lines().last().unwrap();
    assert!(
        !last.contains("flagged") && last.contains("investigate"),
        "{last}"
    );
}
