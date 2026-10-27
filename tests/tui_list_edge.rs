//! Stress and edge tests for the slim review list. None of these may panic,
//! and the selected row must stay visible when it fits.

mod common;

use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::config::{Action, Config, KeyMap};
use teer::review::model::{
    Change, Decision, Marker, Prep, Range, Review, Risk, Unit, UnitKind, UnitStatus,
};
use teer::tui::app::{App, Overlay};
use teer::tui::{slim, ui};

fn unit(i: usize) -> Unit {
    Unit {
        id: format!("file{i:04}.rs::sym{i}"),
        file: PathBuf::from(format!("src/file{i:04}.rs")),
        symbol: format!("sym{i}"),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: None,
        new_range: Some(Range::new(10 + i as u32, 20 + i as u32)),
        renamed_from: None,
        fingerprint: String::new(),
        changes: Vec::new(),
        prep: Prep::Skipped,
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Pending,
        summary: Some("Does a thing".into()),
        nbc_reason: None,
        dive: Default::default(),
        dive_pending: Default::default(),
        checks: Default::default(),
    }
}

fn with_change(mut u: Unit, statement: &str) -> Unit {
    u.prep = Prep::Ready;
    u.changes = vec![Change {
        marker: Marker::Changed,
        statement: statement.into(),
        evidence: Vec::new(),
        risk: Risk::Low,
    }];
    u
}

fn app_of(units: Vec<Unit>) -> App {
    common::isolate_env();
    let review = Review {
        id: "t".into(),
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
    App::new(review, Config::default(), KeyMap::default())
}

fn app(n: usize) -> App {
    app_of((0..n).map(unit).collect())
}

fn render(app: &App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    t
}

fn text(t: &Terminal<TestBackend>) -> String {
    format!("{}", t.backend())
}

fn key(app: &mut App, c: char) {
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )));
}

const SIZES: &[(u16, u16)] = &[
    (0, 0),
    (1, 1),
    (0, 10),
    (10, 0),
    (2, 2),
    (10, 3),
    (39, 10),
    (40, 10),
    (40, 1),
    (60, 5),
    (80, 24),
    (109, 29),
    (110, 30),
    (200, 60),
    (500, 40),
    (500, 1),
];

fn render_all_sizes(app: &App) {
    for &(w, h) in SIZES {
        render(app, w, h);
        // The list builder itself, at every width including degenerate ones.
        let rows = slim::build(app, w);
        assert_eq!(rows.spans.len(), app.order.len());
        assert_eq!(rows.lines.len(), rows.owner.len());
    }
}

#[test]
fn tiny_and_huge_terminals_never_panic() {
    for n in [0, 1, 2, 7] {
        let a = app(n);
        render_all_sizes(&a);
    }
}

#[test]
fn every_selection_at_every_size_does_not_panic() {
    let mut a = app_of(
        (0..6)
            .map(|i| with_change(unit(i), "A statement that is long enough to need wrapping."))
            .collect(),
    );
    for sel in 0..a.order.len() {
        a.sel = sel;
        render_all_sizes(&a);
    }
}

#[test]
fn empty_review_renders_and_handles_every_key() {
    let mut a = app(0);
    assert!(a.order.is_empty());
    render_all_sizes(&a);
    for c in "jkgGxXamMvtuseqRrc?\n \t".chars() {
        key(&mut a, c);
        render(&a, 80, 24);
        a.overlay = Overlay::None;
    }
    for code in [
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Esc,
        KeyCode::Tab,
        KeyCode::Enter,
    ] {
        a.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        a.overlay = Overlay::None;
        render(&a, 80, 24);
    }
    assert_eq!(a.sel, 0);
}

#[test]
fn single_unit() {
    let mut a = app(1);
    render_all_sizes(&a);
    a.on_action(Action::Down);
    a.on_action(Action::Up);
    a.on_action(Action::NextPending);
    a.on_action(Action::PrevGroup);
    a.on_action(Action::NextGroup);
    assert_eq!(a.sel, 0);
    render_all_sizes(&a);
    a.on_action(Action::Accept);
    render_all_sizes(&a);
    a.on_action(Action::Undo);
    assert_eq!(a.review.units[0].decision, Decision::Pending);
}

#[test]
fn all_units_decided() {
    for d in [Decision::Accepted, Decision::Investigate] {
        let mut a = app(12);
        for u in a.review.units.iter_mut() {
            u.decision = d;
        }
        render_all_sizes(&a);
        a.on_action(Action::NextPending);
        a.on_action(Action::PrevPending);
        assert!(a.status.contains("No more pending"));
        a.on_action(Action::Bottom);
        a.on_action(Action::Accept);
        render(&a, 80, 24);
    }
}

#[test]
fn a_thousand_units_half_of_them_decided() {
    let mut a = app(1000);
    for i in (0..1000).step_by(2) {
        a.review.units[i].decision = Decision::Accepted;
    }
    for sel in [0, 1, 499, 500, 998, 999] {
        a.sel = sel;
        render(&a, 80, 24);
        render(&a, 39, 10);
        render(&a, 500, 60);
    }
    a.on_action(Action::Bottom);
    assert_eq!(a.sel, 999);
    a.on_action(Action::Down);
    assert_eq!(a.sel, 999);
    // Deciding the last unit asks nothing and undoes in one step.
    a.on_action(Action::Accept);
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(a.review.units[999].decision, Decision::Accepted);
    render(&a, 80, 24);
    render(&a, 10, 3);
    a.on_action(Action::Undo);
    assert_eq!(a.review.units[999].decision, Decision::Pending);
    assert_eq!(
        a.review
            .units
            .iter()
            .filter(|u| u.decision == Decision::Accepted)
            .count(),
        500
    );
}

const LONG: &str = "an_extremely_long_symbol_name_that_goes_on_and_on_and_on_and_never_stops_until_well_past_any_column";

#[test]
fn very_long_and_wide_unicode_names() {
    let names = [
        LONG.to_string(),
        "日本語のとても長いシンボル名前日本語のとても長いシンボル名前".to_string(),
        "😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀".to_string(),
        "e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}".to_string(),
        "\u{301}\u{301}\u{301}".to_string(),
        "👨\u{200d}👩\u{200d}👧\u{200d}👦👨\u{200d}👩\u{200d}👧\u{200d}👦👨\u{200d}👩\u{200d}👧\u{200d}👦".to_string(),
        "".to_string(),
        " ".to_string(),
    ];
    let statements = [
        LONG.repeat(5),
        "日本語の文。".repeat(40),
        "😀 ".repeat(100),
        "e\u{301} ".repeat(100),
        "".to_string(),
    ];
    let mut units = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let mut u = with_change(unit(i), &statements[i % statements.len()]);
        u.symbol = n.clone();
        u.id = format!("{i}::{n}");
        u.file = PathBuf::from(format!("{}/{}.rs", LONG, names[(i + 1) % names.len()]));
        u.summary = Some(statements[(i + 1) % statements.len()].clone());
        units.push(u);
    }
    // Module-level and renamed units too.
    let mut m = unit(50);
    m.kind = UnitKind::Module;
    m.symbol = "日本語".into();
    units.push(m);
    let mut r = unit(51);
    r.status = UnitStatus::Removed;
    r.symbol = "😀".repeat(20);
    units.push(r);
    let mut a = app_of(units);
    for sel in 0..a.order.len() {
        a.sel = sel;
        render_all_sizes(&a);
        for w in [40u16, 41, 45, 60, 71, 80] {
            // Every line, file rows and blank lines included, is exactly the width.
            let rows = slim::build(&a, w);
            for l in &rows.lines {
                assert_eq!(l.width(), w as usize, "width {w}");
            }
            for &(s, e) in &rows.spans {
                for l in &rows.lines[s..e] {
                    assert_eq!(
                        l.width(),
                        w as usize,
                        "row width at {w}: {:?}",
                        l.spans
                            .iter()
                            .map(|s| s.content.to_string())
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
    }
}

#[test]
fn unicode_statements_in_the_preview_pane() {
    let mut a = app_of(
        (0..3)
            .map(|i| with_change(unit(i), &"日本語😀e\u{301} ".repeat(60)))
            .collect(),
    );
    for sel in 0..3 {
        a.sel = sel;
        for &(w, h) in &[
            (110u16, 30u16),
            (120, 40),
            (80, 30),
            (60, 31),
            (500, 100),
            (111, 3),
        ] {
            render(&a, w, h);
        }
    }
}

#[test]
fn every_unit_is_one_line_at_any_width() {
    let mut a = app(5);
    a.sel = 2;
    for w in [39u16, 40, 80, 500] {
        let rows = slim::build(&a, w);
        for &(s, e) in &rows.spans {
            assert_eq!(e - s, 1, "width {w}");
        }
    }
}

#[test]
fn selection_at_first_and_last_row_is_visible() {
    let mut a = app(30);
    for sel in [0usize, 29] {
        a.sel = sel;
        for h in [6u16, 8, 12, 24] {
            let t = render(&a, 60, h);
            let s = text(&t);
            let needle = format!("▌  ● sym{sel} ");
            assert_eq!(
                s.lines().filter(|l| l.contains(&needle)).count(),
                1,
                "sel {sel} h {h}: row not visible\n{s}"
            );
            assert!(s.contains('▌'), "selected gutter missing at h {h}\n{s}");
        }
    }
}

/// Scroll down and up by single steps over the units; after every move, the
/// selected row must be on screen.
#[test]
fn scrolling_keeps_the_selected_row_visible() {
    for (w, h) in [(60u16, 8u16), (60, 12), (80, 24), (45, 6), (60, 5), (60, 4)] {
        let mut a = app(40);
        let check = |a: &App, dir: &str| {
            let t = render(a, w, h);
            let s = text(&t);
            let name = format!("● {}", a.selected().unwrap().symbol);
            let rows = s
                .lines()
                .filter(|l| l.contains('▌') && l.contains(&name))
                .count();
            assert_eq!(rows, 1, "{dir} {} at {w}x{h}: row missing\n{s}", a.sel);
        };
        check(&a, "initial");
        let steps = a.order.len() - 1;
        for _ in 0..steps {
            key(&mut a, 'j');
            check(&a, "down");
        }
        for _ in 0..steps {
            key(&mut a, 'k');
            check(&a, "up");
        }
        a.on_action(Action::Bottom);
        check(&a, "bottom");
        a.on_action(Action::Top);
        check(&a, "top");
    }
}

#[test]
fn jumping_far_keeps_the_selection_visible() {
    let mut a = app(500);
    for sel in [0usize, 250, 499, 3, 400, 0, 499] {
        a.sel = sel;
        let s = text(&render(&a, 60, 15));
        let row = format!("▌  ● sym{sel} ");
        assert_eq!(
            s.lines().filter(|l| l.contains(&row)).count(),
            1,
            "sel {sel}\n{s}"
        );
    }
}

#[test]
fn resizing_between_frames_with_a_stale_scroll_offset() {
    let mut a = app(200);
    let mut t = Terminal::new(TestBackend::new(80, 30)).unwrap();
    let sequence: &[(u16, u16)] = &[
        (80, 30),
        (500, 80),
        (10, 3),
        (0, 0),
        (1, 1),
        (39, 10),
        (120, 50),
        (40, 2),
        (80, 24),
        (3, 200),
        (200, 3),
    ];
    for (n, &(w, h)) in sequence.iter().enumerate() {
        // Move to somewhere deep, so the scroll offset is large and then stale.
        a.sel = (n * 37) % 200;
        t.backend_mut().resize(w, h);
        t.draw(|f| ui::draw(f, &a)).unwrap();
        a.sel = (a.sel * 7 + 3) % 200;
        t.backend_mut().resize(h.max(1), w.max(1));
        t.draw(|f| ui::draw(f, &a)).unwrap();
    }
    // A stale offset beyond the end of a shrunk list.
    a.slim_scroll.set(10_000);
    a.sel = 0;
    render(&a, 80, 24);
    // And after the review shrinks under a large offset.
    a.slim_scroll.set(10_000);
    a.review.units.truncate(2);
    a.rebuild_order();
    render(&a, 80, 24);
}

#[test]
fn the_selected_row_is_visible_after_a_resize() {
    let mut a = app(60);
    a.sel = 45;
    let mut t = Terminal::new(TestBackend::new(60, 30)).unwrap();
    t.draw(|f| ui::draw(f, &a)).unwrap();
    for (w, h) in [(60u16, 10u16), (60, 6), (60, 30), (50, 8)] {
        t.backend_mut().resize(w, h);
        t.draw(|f| ui::draw(f, &a)).unwrap();
        let s = text(&t);
        assert_eq!(
            s.lines().filter(|l| l.contains("▌  ● sym45 ")).count(),
            1,
            "{w}x{h}\n{s}"
        );
    }
}

#[test]
fn p_does_nothing_and_the_reset_confirm_still_works() {
    let mut a = app(20);
    // `p` is no longer bound: the preview is always there and nothing moves.
    let sel = a.sel;
    key(&mut a, 'p');
    assert_eq!(a.sel, sel);
    assert_eq!(a.overlay, Overlay::None);
    a.on_action(Action::Reset);
    assert_eq!(a.overlay, Overlay::ConfirmReset);
    for &(w, h) in SIZES {
        render(&a, w, h);
    }
    // Key path: any key other than enter/y cancels, and the key is swallowed.
    a.review.units[0].decision = Decision::Accepted;
    key(&mut a, 'p');
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(a.review.units[0].decision, Decision::Accepted, "not reset");
    a.on_action(Action::Investigate);
    render(&a, 120, 40);
    assert_eq!(
        a.review
            .units
            .iter()
            .filter(|u| u.decision == Decision::Investigate)
            .count(),
        1
    );
}

#[test]
fn rebuild_order_when_units_leave_the_list() {
    let mut a = app(30);
    a.sel = 9;

    // Plain rebuild keeps the selection.
    a.rebuild_order();
    assert_eq!(a.sel, 9);

    // Units drop out of the list (become "no behavior change"): the
    // selection follows its unit by id.
    let selected = a.review.units[a.order[a.sel]].id.clone();
    for i in [2usize, 4, 5, 20] {
        a.review.units[i].status = UnitStatus::NoBehaviorChange;
    }
    a.rebuild_order();
    assert_eq!(a.order.len(), 26);
    assert_eq!(a.review.units[a.order[a.sel]].id, selected);
    render_all_sizes(&a);

    // The selected unit leaves: the selection clamps or moves, no panic.
    a.sel = a.order.len() - 1;
    let last = a.order[a.sel];
    a.review.units[last].status = UnitStatus::NoBehaviorChange;
    a.rebuild_order();
    assert!(a.sel < a.order.len());

    // The whole list empties.
    for u in a.review.units.iter_mut() {
        u.status = UnitStatus::NoBehaviorChange;
    }
    a.rebuild_order();
    assert!(a.order.is_empty());
    assert_eq!(a.sel, 0);
    render_all_sizes(&a);
    a.on_action(Action::Accept);
    a.on_action(Action::Undo);
    a.on_action(Action::NextGroup);
    a.on_action(Action::PrevGroup);
    render(&a, 80, 24);
}

#[test]
fn rebuild_order_with_a_stale_selection_past_the_end() {
    let mut a = app(5);
    a.sel = 99;
    a.rebuild_order();
    assert_eq!(a.sel, 4);
    render(&a, 80, 24);
    // Deciding after the clamp touches the last listed unit.
    a.on_action(Action::Accept);
    assert_eq!(a.review.units[a.order[4]].decision, Decision::Accepted);
}

#[test]
fn decisions_while_rendering_tiny() {
    let mut a = app(12);
    for i in 0..8 {
        a.review.units[i].decision = Decision::Accepted;
    }
    for c in ['j', 'a', 'y', 'u', 'x', 'p', 'J', 'K', 'n', 'N'] {
        key(&mut a, c);
        for &(w, h) in &[(0u16, 0u16), (1, 1), (10, 3), (39, 10)] {
            render(&a, w, h);
        }
        a.overlay = Overlay::None;
    }
}

/// Agent text with newlines or control characters must not break the one-line
/// rows: every list row has to render as exactly one line of exactly the width.
#[test]
fn control_characters_and_newlines_keep_rows_on_one_line() {
    let mut units = Vec::new();
    for (i, bad) in [
        "line one\nline two\n\n\nline five",
        "a\tb\u{0}c\u{7f}",
        "x\r\ny",
    ]
    .iter()
    .enumerate()
    {
        let mut u = with_change(unit(i), bad);
        u.symbol = bad.to_string();
        u.summary = Some(bad.to_string());
        units.push(u);
    }
    let mut a = app_of(units);
    for sel in 0..a.order.len() {
        a.sel = sel;
        for w in [40u16, 60, 80, 120] {
            let rows = slim::build(&a, w);
            for &(s, e) in &rows.spans {
                for l in &rows.lines[s..e] {
                    let joined: String = l.spans.iter().map(|x| x.content.to_string()).collect();
                    assert!(
                        !joined.chars().any(|c| c.is_control()),
                        "control char in row at width {w}: {joined:?}"
                    );
                    assert_eq!(l.width(), w as usize, "row width at {w}: {joined:?}");
                }
            }
        }
        render_all_sizes(&a);
    }
}

#[test]
fn investigate_button_and_help_show_i_and_k_is_up() {
    let mut a = app(3);
    let footer = |a: &App| {
        let t = render(a, 100, 10);
        text(&t)
            .lines()
            .rev()
            .find(|l| l.contains("investigate"))
            .unwrap()
            .to_string()
    };
    assert!(footer(&a).contains("i investigate"), "{}", footer(&a));
    a.overlay = Overlay::Help;
    let help = text(&render(&a, 100, 40));
    assert!(help.contains("l  i") || help.contains("l i"), "{help}");
    assert!(help.contains("k") && !help.contains("later"), "{help}");
}

#[test]
fn the_last_unit_shows_its_boxs_bottom_edge_and_a_blank_row() {
    let mut a = app_of((0..40).map(|i| with_change(unit(i), "Changes")).collect());
    a.on_action(Action::Bottom);
    let s = text(&render(&a, 140, 24));
    let rows: Vec<&str> = s.lines().collect();
    let n = rows.len();
    // The selected row, the box's bottom edge, a blank row, the footer.
    assert!(rows[n - 4].contains("▌  ● sym39 "), "{s}");
    assert!(rows[n - 3].contains('╰'), "{s}");
    assert!(!rows[n - 2].contains(['│', '╰']), "{s}");
}
