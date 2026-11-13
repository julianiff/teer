//! Scrolling the detail areas (the queue's preview, the unit view's text
//! and the panel under it) with the keys and the wheel, and the layout that
//! gives the code change the room the queue doesn't need.

use crate::common;

use std::collections::HashMap;

use common::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use teer::config::Config;
use teer::keymap::{Action, KeySpec, Keymap};
use teer::review::model::{Cite, Coverage, CoveringTest, DiveKind, DiveResult, Review, SessionRef};
use teer::review::{BuildOptions, build_review};
use teer::session::app::{Grouping, Overlay, Screen};
use teer::session::unit::Panel;
use teer::tui::app::{App, Target};
use teer::tui::{preview, ui};

/// A function whose code change is far longer than any pane (every one of
/// its 30 lines changes), and a short one after it.
fn sources() -> (String, String) {
    let body = |op: &str| {
        (0..30)
            .map(|i| format!("    a{i} = x {op} {i}\n"))
            .collect::<String>()
    };
    let tail = "    return a29\n\n\ndef short_one(y):\n    return y + ";
    (
        format!("def long_one(x):\n{}{tail}1\n", body("+")),
        format!("def long_one(x):\n{}{tail}2\n", body("*")),
    )
}

fn review_of(old: &str, new: &str) -> (Review, FixtureRepo) {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("calc.py", old).commit("init");
    repo.write("calc.py", new);
    let review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    (review, repo)
}

/// The queue on `review`, listed by file (`zf`): the file's name above its
/// units.
fn app_with(review: Review, keys: Keymap) -> App {
    let mut app = App::new(review, Config::default(), keys);
    app.start(false, None, Vec::new(), None);
    app.grouping = Grouping::File;
    app.rebuild_order();
    // The list's geometry alone: the attack-surface card over it is
    // security_queue.rs's.
    app.security.enabled = false;
    app
}

/// The long and the short function, no agent; the cursor on `long_one`.
fn app() -> (App, FixtureRepo) {
    let (old, new) = sources();
    let (review, repo) = review_of(&old, &new);
    let app = app_with(review, Keymap::default());
    assert_eq!(app.selected().unwrap().symbol, "long_one");
    (app, repo)
}

fn screen(app: &App, w: u16, h: u16) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

/// The list and the pane for a `w` x `h` terminal (header and footer off).
fn layout(app: &App, w: u16, h: u16) -> (Rect, Rect) {
    let (list, pane) = preview::layout_of(app, Rect::new(0, 1, w, h - 2));
    (list, pane.unwrap().0)
}

/// The rows of the pane, drawn at `w` x `h`.
fn pane(app: &App, w: u16, h: u16) -> Vec<String> {
    let lines = screen(app, w, h);
    let (_, r) = layout(app, w, h);
    (r.y..r.y + r.height)
        .map(|y| {
            lines[y as usize]
                .chars()
                .skip(r.x as usize)
                .take(r.width as usize)
                .collect()
        })
        .collect()
}

fn key(app: &mut App, c: char) {
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )));
}

fn press(app: &mut App, code: KeyCode) {
    app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

/// Type `k` in Neovim notation (`<c-d>`, `zz`, `<space>t`).
fn keys(app: &mut App, k: &str) {
    common::v2::press(app, k);
}

/// Lines `<c-d>` scrolls: half the detail's rows as last drawn.
fn half(app: &App) -> usize {
    app.detail_rows.get() / 2
}

fn wheel(app: &mut App, down: bool, column: u16, row: u16) {
    let kind = if down {
        MouseEventKind::ScrollDown
    } else {
        MouseEventKind::ScrollUp
    };
    app.on_event(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }));
}

fn has(lines: &[String], text: &str) -> bool {
    lines.iter().any(|l| l.contains(text))
}

const ABOVE: &str = "lines above · ^u scroll up";
const MORE: &str = "more lines · ^d scroll · gf editor";

#[test]
fn the_preview_scrolls_down_to_the_cut_rows_and_back_up() {
    let (mut app, _repo) = app();
    let (w, h) = (100, 30);
    let first = pane(&app, w, h);
    let text = first.join("\n");
    // Cut: the code's box has no bottom edge; its last row says how much is
    // left and how to see it. Nothing above.
    assert!(first[first.len() - 1].contains(MORE), "{text}");
    assert!(!has(&first, "╰"), "{text}");
    assert!(!has(&first, "above"), "{text}");
    assert!(has(&first, " long_one "), "{text}");
    assert!(!has(&first, "a15 = x + 15"), "{text}");

    let step = half(&app);
    keys(&mut app, "<c-d>");
    assert_eq!(app.detail_offset(), step, "half the rows");
    let after = pane(&app, w, h);
    let text = after.join("\n");
    // Only the code scrolls: the head stays and the box's first row says
    // what is above.
    assert!(
        has(&after, &format!("│ … {step} lines above · ^u scroll up")),
        "{text}"
    );
    assert!(has(&after, " long_one "), "{text}");
    assert!(after[after.len() - 1].contains(MORE), "{text}");
    assert!(has(&after, "a15 = x + 15"), "{text}");

    // At the end the last row of the change shows and nothing is left below.
    for _ in 0..40 {
        keys(&mut app, "<c-d>");
    }
    let end = pane(&app, w, h);
    let text = end.join("\n");
    assert!(has(&end, "return a29"), "{text}");
    assert!(!has(&end, "more lines"), "{text}");
    assert!(has(&end, ABOVE), "{text}");
    // Only whole rows: the box's bottom row is code, not blank or cut, and
    // the box closes under it.
    assert!(end[end.len() - 2].contains("return a29"), "{text}");
    assert!(end[end.len() - 1].contains("╰──"), "{text}");
    let max = app.detail_max_scroll.get();
    assert!(max > 0);
    keys(&mut app, "<c-d>");
    assert_eq!(app.detail_offset(), max, "^d stops at the end");

    // Back up: ^u undoes it step by step.
    for _ in 0..40 {
        keys(&mut app, "<c-u>");
    }
    assert_eq!(pane(&app, w, h), first);
    assert_eq!(app.detail_offset(), 0);
}

#[test]
fn page_down_and_page_up_scroll_a_page() {
    let (mut app, _repo) = app();
    let (w, h) = (100, 30);
    let before = pane(&app, w, h);
    press(&mut app, KeyCode::PageDown);
    // The box's rows, less the two hint rows.
    assert_eq!(app.detail_offset(), app.detail_rows.get() - 2);
    let after = pane(&app, w, h);
    // The last code row shown before is near the top of the box now.
    let last = before[before.len() - 2].clone();
    let at = after
        .iter()
        .position(|l| *l == last)
        .unwrap_or_else(|| panic!("{}", after.join("\n")));
    let top = after
        .iter()
        .position(|l| l.contains("╭ code change"))
        .unwrap();
    assert!(at <= top + 3, "{}", after.join("\n"));
    press(&mut app, KeyCode::PageUp);
    assert_eq!(app.detail_offset(), 0);
    // The list's selection never moved.
    assert_eq!(app.selected().unwrap().symbol, "long_one");
}

#[test]
fn the_scroll_starts_over_when_the_selection_moves() {
    let (mut app, _repo) = app();
    let (w, h) = (100, 30);
    pane(&app, w, h);
    keys(&mut app, "<c-d>");
    keys(&mut app, "<c-d>");
    assert!(has(&pane(&app, w, h), ABOVE));
    key(&mut app, 'j');
    assert_eq!(app.selected().unwrap().symbol, "short_one");
    let other = pane(&app, w, h);
    assert!(has(&other, " short_one "), "{}", other.join("\n"));
    assert!(!has(&other, "above"), "{}", other.join("\n"));
    // Back on the first unit it starts at the top again.
    key(&mut app, 'k');
    let back = pane(&app, w, h);
    assert!(has(&back, " long_one ") && !has(&back, "above"));
    assert_eq!(app.detail_offset(), 0);
}

#[test]
fn the_wheel_scrolls_the_pane_under_it_and_moves_the_selection_over_the_list() {
    for (w, h) in [(100u16, 30u16), (120, 40)] {
        let (mut app, _repo) = app();
        pane(&app, w, h);
        let (list, p) = layout(&app, w, h);
        wheel(&mut app, true, p.x + p.width / 2, p.y + p.height / 2);
        let rows = pane(&app, w, h);
        assert!(has(&rows, ABOVE), "{w}x{h}\n{}", rows.join("\n"));
        assert_eq!(app.selected().unwrap().symbol, "long_one", "{w}x{h}");
        wheel(&mut app, false, p.x + 2, p.y + 2);
        assert_eq!(app.detail_offset(), 0, "{w}x{h}");
        // Over a row of the list the wheel moves the selection.
        let row = app
            .targets
            .borrow()
            .iter()
            .find(|(_, t)| *t == Target::Unit(0))
            .map(|(r, _)| *r)
            .unwrap();
        assert!(list.y <= row.y && row.y < list.y + list.height);
        wheel(&mut app, true, row.x + 3, row.y);
        assert_eq!(app.selected().unwrap().symbol, "short_one", "{w}x{h}");
        wheel(&mut app, false, row.x + 3, row.y);
        assert_eq!(app.selected().unwrap().symbol, "long_one", "{w}x{h}");
    }
}

#[test]
fn help_lists_the_scroll_keys() {
    let (mut app, _repo) = app();
    app.on_action(Action::HelpOpen);
    for (w, h) in [(80, 24), (60, 20)] {
        let s = screen(&app, w, h).join("\n");
        assert!(s.contains("^d ^u  scroll half a page"), "{w}x{h}\n{s}");
        assert!(s.contains("pgdn pgup  scroll a page"), "{w}x{h}\n{s}");
    }
}

#[test]
fn the_scroll_keys_can_be_rebound() {
    let want: HashMap<String, KeySpec> = [("scroll.half_down", "w"), ("scroll.half_up", "b")]
        .into_iter()
        .map(|(a, k)| (a.to_string(), KeySpec::One(k.to_string())))
        .collect();
    let toml = "[keys]\n\"scroll.half_down\" = \"w\"\n\"scroll.half_up\" = \"b\"\n";
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.keys, want);
    let (old, new) = sources();
    let (review, _repo) = review_of(&old, &new);
    let (keymap, warnings) = cfg.keymap().unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let mut app = app_with(review, keymap);
    let (w, h) = (100, 30);
    let first = pane(&app, w, h);
    assert!(
        has(&first, "more lines · w scroll · gf editor"),
        "{}",
        first.join("\n")
    );
    // ^d is nothing now; w scrolls and b goes back.
    keys(&mut app, "<c-d>");
    assert_eq!(app.detail_offset(), 0);
    key(&mut app, 'w');
    let rows = pane(&app, w, h);
    assert!(
        has(&rows, "lines above · b scroll up"),
        "{}",
        rows.join("\n")
    );
    key(&mut app, 'b');
    assert_eq!(app.detail_offset(), 0);
    app.on_action(Action::HelpOpen);
    let s = screen(&app, 80, 24).join("\n");
    assert!(s.contains("w b  scroll half a page"), "{s}");
}

// ---- Layout ---------------------------------------------------------------

/// One unit with a line too wide for 45% of a 120-column terminal.
fn one_unit() -> (App, FixtureRepo) {
    let old = "def only(x):\n    return x\n";
    let new = "def only(x):\n    return compute_total(x, discount=0.1, currency=\"EUR\")\n";
    let (review, repo) = review_of(old, new);
    (app_with(review, Keymap::default()), repo)
}

#[test]
fn beside_a_one_unit_list_the_code_change_gets_the_width() {
    let (app, _repo) = one_unit();
    let (w, h) = (120, 40);
    let lines = screen(&app, w, h);
    let (list, p) = layout(&app, w, h);
    // The list is as narrow as its box's title allows, the pane takes the rest.
    assert_eq!(list.width, 28, "{}", lines.join("\n"));
    assert_eq!((p.x, p.width), (28, 92));
    // Only the unit's row is a target: the file's name is a label.
    let units: Vec<(Rect, Target)> = app
        .targets
        .borrow()
        .iter()
        .filter(|(_, t)| matches!(t, Target::Unit(_)))
        .cloned()
        .collect();
    assert_eq!(units, [(Rect::new(0, 3, 28, 1), Target::Unit(0))]);
    // The unit's row is its name, under its file's name.
    let file: String = lines[2].chars().take(28).collect();
    assert!(file.starts_with("│ calc.py"), "{file}");
    let row: String = lines[3].chars().take(28).collect();
    assert!(row.contains("◆ only") && !row.contains("calc.py"), "{row}");
    // The long line fits on one row instead of wrapping.
    let pane_rows = pane(&app, w, h);
    assert!(
        pane_rows
            .iter()
            .any(|l| l.contains("2 +     return compute_total(x, discount=0.1, currency=\"EUR\")")),
        "{}",
        pane_rows.join("\n")
    );
}

#[test]
fn a_wide_list_is_capped_at_45_percent() {
    let (old, new) = sources();
    let long = "a_function_name_long_enough_to_want_most_of_a_wide_terminal_for_itself";
    let (review, _repo) = review_of(
        &old.replace("long_one", long),
        &new.replace("long_one", long),
    );
    let app = app_with(review, Keymap::default());
    let (list, p) = layout(&app, 120, 40);
    assert_eq!((list.width, p.x, p.width), (54, 54, 66));
    // Up to the cap, the list is just as wide as its rows need.
    let (list, _) = layout(&app, 240, 40);
    let lines = screen(&app, 240, 40);
    assert!(list.width < 110, "{}", list.width);
    assert!(
        lines[3].contains(long)
            && !lines[3]
                .chars()
                .take(list.width as usize)
                .collect::<String>()
                .contains("calc.py"),
        "{}",
        lines[3]
    );
}

#[test]
fn below_a_short_list_the_pane_takes_the_free_rows() {
    let (app, _repo) = app();
    let (w, h) = (100, 30);
    let lines = screen(&app, w, h);
    let (list, p) = layout(&app, w, h);
    // The box: its title, the file's name and its two units, its bottom
    // edge; then the pane down to the footer.
    assert_eq!((list.y, list.height), (1, 5), "{}", lines.join("\n"));
    assert_eq!((p.y, p.height), (6, 23));
    assert!(
        lines[1].starts_with("╭─ Worth a look · 1 file"),
        "{}",
        lines.join("\n")
    );
    assert!(lines[2].starts_with("│ calc.py"), "{}", lines.join("\n"));
    assert!(lines[4].contains("◆ short_one"), "{}", lines.join("\n"));
    assert_eq!(lines[6], "─".repeat(w as usize));
    // A one-unit list: its box takes four rows, the rest is the change.
    let (app, _repo) = one_unit();
    let (list, p) = layout(&app, w, h);
    assert_eq!((list.height, p.y, p.height), (4, 5, 24));
    // Small terminals keep their rules: 80x24 still shows the list and a pane.
    let (app, _repo) = self::app();
    let (list, p) = layout(&app, 80, 24);
    assert!(list.height >= 3 && p.height >= 5, "{list:?} {p:?}");
}

// ---- Unit view --------------------------------------------------------------

/// `long_one` open in the unit view with an agent; its advice is an
/// answer of 40 lines and its Tests answer 25 covering tests.
fn unit_view() -> (App, FixtureRepo) {
    let (mut app, repo) = app();
    app.has_agent = true;
    app.review.agent_session = Some(SessionRef::default());
    let answer: Vec<String> = (1..=40)
        .map(|i| format!("step {i} of the answer"))
        .collect();
    let covered = (1..=25)
        .map(|i| CoveringTest {
            name: format!("test_case_{i}"),
            cite: Cite {
                file: "tests/test_calc.py".into(),
                start: i,
                end: i,
            },
            checks: format!("checks result {i}"),
        })
        .collect();
    let id = app.selected_id().unwrap();
    let u = app.review.unit_mut(&id).unwrap();
    u.dive.results.insert(
        DiveKind::Recommend,
        DiveResult {
            text: answer.join("\n"),
            ..Default::default()
        },
    );
    u.dive.results.insert(
        DiveKind::Tests,
        DiveResult {
            coverage: Some(Coverage {
                search: None,
                judged: true,
                covered,
                gaps: Vec::new(),
            }),
            ..Default::default()
        },
    );
    key(&mut app, 'l');
    assert_eq!(app.screen, Screen::Unit);
    assert_eq!(app.unit.id, id);
    (app, repo)
}

/// Where target `t` was last drawn.
fn target_rect(app: &App, t: Target) -> Rect {
    app.targets
        .borrow()
        .iter()
        .find(|(_, x)| *x == t)
        .map(|(r, _)| *r)
        .unwrap()
}

#[test]
fn a_long_recommendation_in_its_tile_scrolls_to_its_end() {
    let (mut app, _repo) = unit_view();
    let (w, h) = (100, 30);
    let first = screen(&app, w, h);
    let text = first.join("\n");
    assert!(has(&first, " long_one "), "{text}");
    assert!(has(&first, "Advice"), "{text}");
    assert!(has(&first, "step 1 of the answer"), "{text}");
    assert!(!has(&first, "step 40 of the answer"), "{text}");
    // Its tile in focus, `^d` scrolls it half its rows; the text stays.
    keys(&mut app, "<tab>");
    assert_eq!(app.unit.panel, Some(Panel::Ask));
    screen(&app, w, h);
    // The ask tile scrolls as its conversation does.
    let at = |app: &App| app.ask.chat_scroll.get().unwrap_or(0);
    let step = half(&app);
    keys(&mut app, "<c-d>");
    assert_eq!(at(&app), step);
    assert_eq!(app.detail_offset(), 0);
    for _ in 0..40 {
        keys(&mut app, "<c-d>");
        screen(&app, w, h);
    }
    let end = screen(&app, w, h);
    let text = end.join("\n");
    assert!(has(&end, "step 40 of the answer"), "{text}");
    let max = at(&app);
    assert!(max > 0);
    keys(&mut app, "<c-d>");
    assert_eq!(at(&app), max, "^d stops at the end");
    // Back up to the top.
    for _ in 0..40 {
        keys(&mut app, "<c-u>");
    }
    assert_eq!(at(&app), 0);
}

#[test]
fn the_text_and_the_tile_beside_it_scroll_apart() {
    let (mut app, _repo) = unit_view();
    let (w, h) = (100, 30);
    screen(&app, w, h);
    let text_step = half(&app);
    keys(&mut app, "<c-d>");
    assert_eq!(app.detail_offset(), text_step);
    // With a tile open (in focus), ^d scrolls the tile and the text stays.
    keys(&mut app, "<space>t");
    assert_eq!(app.unit.panel, Some(Panel::Tool(DiveKind::Tests)));
    screen(&app, w, h);
    let panel_step = half(&app);
    keys(&mut app, "<c-d>");
    screen(&app, w, h);
    let tests = Panel::Tool(DiveKind::Tests);
    assert_eq!(app.unit.tile_offset(tests), panel_step);
    assert_eq!(app.detail_offset(), text_step);
    // Closing the tile gives the keys back to the text.
    keys(&mut app, "<space>t");
    assert_eq!(app.unit.panel, None);
    screen(&app, w, h);
    keys(&mut app, "<c-u>");
    assert_eq!(app.detail_offset(), 0);
    // Another unit starts at the top.
    keys(&mut app, "<c-d>");
    keys(&mut app, "n");
    assert_eq!(app.unit.id, "calc.py::short_one");
    screen(&app, w, h);
    assert_eq!(app.detail_offset(), 0);
}

#[test]
fn the_wheel_scrolls_the_text_or_the_tile_beside_it() {
    let (mut app, _repo) = unit_view();
    let (w, h) = (100, 30);
    keys(&mut app, "<space>t");
    screen(&app, w, h);
    let tests = Panel::Tool(DiveKind::Tests);
    let panel = target_rect(&app, Target::Panel(tests));
    let text = target_rect(&app, Target::Detail);
    // Over the tile: the tile, three lines.
    wheel(&mut app, true, panel.x + 5, panel.y + panel.height / 2);
    assert_eq!(app.unit.tile_offset(tests), 3);
    assert_eq!(app.detail_offset(), 0);
    // Over the text beside it: the text.
    screen(&app, w, h);
    wheel(&mut app, true, text.x + 5, text.y + 1);
    assert_eq!(app.detail_offset(), 3);
    assert_eq!(app.unit.tile_offset(tests), 3);
    wheel(&mut app, false, text.x + 5, text.y + 1);
    assert_eq!(app.detail_offset(), 0);
    // The wheel moves to no other unit.
    assert_eq!(app.unit.id, "calc.py::long_one");
}

#[test]
fn the_tests_panel_scrolls_to_its_end() {
    let (mut app, _repo) = unit_view();
    keys(&mut app, "<space>t");
    assert_eq!(app.unit.panel, Some(Panel::Tool(DiveKind::Tests)));
    let (w, h) = (100, 40);
    let first = screen(&app, w, h);
    assert!(has(&first, "25 tests cover it"), "{}", first.join("\n"));
    assert!(!has(&first, "test_case_25"), "{}", first.join("\n"));
    // Half a tile at a time, however tall the keys tile leaves it.
    for _ in 0..80 {
        keys(&mut app, "<c-d>");
    }
    let end = screen(&app, w, h);
    assert!(has(&end, "test_case_25"), "{}", end.join("\n"));
    assert_eq!(app.overlay, Overlay::None);
}
