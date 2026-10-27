//! Scrolling the detail areas (the list preview, the swipe card, the open
//! tab on the Flagged screen) with the keys and the wheel, and the layout
//! that gives the code change the room the list doesn't need.

mod common;

use std::collections::HashMap;

use common::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use teer::config::{Action, Config, KeyMap};
use teer::review::model::{Cite, Coverage, CoveringTest, Decision, DiveKind, DiveResult, Review};
use teer::review::{BuildOptions, build_review};
use teer::tui::app::{App, Overlay, Pane, Screen, Target, pane_key};
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

fn app_with(review: Review, keys: KeyMap) -> App {
    let mut app = App::new(review, Config::default(), keys);
    app.start(false, None, Vec::new(), None, false);
    app
}

/// The long and the short function, no agent; the cursor on `long_one`.
fn app() -> (App, FixtureRepo) {
    let (old, new) = sources();
    let (review, repo) = review_of(&old, &new);
    let app = app_with(review, KeyMap::default());
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

const ABOVE: &str = "lines above · K scroll up";
const MORE: &str = "more lines · J scroll · e editor";

#[test]
fn the_preview_scrolls_down_to_the_cut_rows_and_back_up() {
    let (mut app, _repo) = app();
    let (w, h) = (100, 30);
    let first = pane(&app, w, h);
    let text = first.join("\n");
    // Cut: the code's box ends with how much is left and how to see it;
    // nothing above.
    assert!(first[first.len() - 2].contains(MORE), "{text}");
    assert!(!has(&first, "above"), "{text}");
    assert!(has(&first, " long_one "), "{text}");
    assert!(!has(&first, "a15 = x + 15"), "{text}");

    key(&mut app, 'J');
    let after = pane(&app, w, h);
    let text = after.join("\n");
    // Only the code scrolls: the head stays and the box's first row says
    // what is above.
    assert!(has(&after, "│ … 3 lines above · K scroll up"), "{text}");
    assert!(has(&after, " long_one "), "{text}");
    assert!(after[after.len() - 2].contains(MORE), "{text}");
    assert!(has(&after, "a15 = x + 15"), "{text}");

    // At the end the last row of the change shows and nothing is left below.
    for _ in 0..40 {
        key(&mut app, 'J');
    }
    let end = pane(&app, w, h);
    let text = end.join("\n");
    assert!(has(&end, "return a29"), "{text}");
    assert!(!has(&end, "more lines"), "{text}");
    assert!(has(&end, ABOVE), "{text}");
    // Only whole rows: the box's bottom row is code, not blank or cut.
    assert!(end[end.len() - 2].contains("return a29"), "{text}");
    let max = app.detail_max_scroll.get();
    assert!(max > 0);
    key(&mut app, 'J');
    assert_eq!(app.detail_offset(), max, "J stops at the end");

    // Back up: K undoes it step by step.
    for _ in 0..40 {
        key(&mut app, 'K');
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
    let last = before[before.len() - 3].clone();
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
    key(&mut app, 'J');
    key(&mut app, 'J');
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
fn the_footer_offers_scrolling_only_while_something_is_cut_and_fits_at_80() {
    let (mut app, _repo) = app();
    let footer = |app: &App, w, h| screen(app, w, h).last().unwrap().clone();
    assert!(footer(&app, 100, 30).contains("J K scroll"));
    // At 80 columns it fits after the other buttons, which all stay.
    let f = footer(&app, 80, 24);
    assert!(f.contains("i investigate   / ask   J K scroll"), "{f}");
    // Clicking it scrolls.
    let r = app
        .targets
        .borrow()
        .iter()
        .find(|(_, t)| *t == Target::Action(Action::PreviewDown))
        .map(|(r, _)| *r)
        .unwrap();
    app.on_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Up(crossterm::event::MouseButton::Left),
        column: r.x + 1,
        row: r.y,
        modifiers: KeyModifiers::NONE,
    }));
    assert_eq!(app.detail_offset(), 3);
    // A unit whose change fits shows no scroll button.
    key(&mut app, 'j');
    assert!(!footer(&app, 100, 30).contains("scroll"));
}

#[test]
fn help_lists_the_scroll_keys() {
    let (mut app, _repo) = app();
    app.on_action(Action::Help);
    let s = screen(&app, 80, 24).join("\n");
    assert!(s.contains("J K  scroll the preview, card or answer"), "{s}");
    // Still fits at 60x20, legend and all.
    let s = screen(&app, 60, 20).join("\n");
    assert!(
        s.contains("✗ needs fix") && s.contains("scroll the preview"),
        "{s}"
    );
}

#[test]
fn the_scroll_keys_can_be_rebound() {
    let keys: HashMap<String, String> = [("preview_down", "w"), ("preview_up", "y")]
        .into_iter()
        .map(|(a, k)| (a.to_string(), k.to_string()))
        .collect();
    let toml = "[keys]\npreview_down = \"w\"\npreview_up = \"y\"\n";
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.keys, keys);
    let (old, new) = sources();
    let (review, _repo) = review_of(&old, &new);
    let mut app = app_with(review, cfg.keymap().unwrap());
    let (w, h) = (100, 30);
    let first = pane(&app, w, h);
    assert!(
        has(&first, "more lines · w scroll · e editor"),
        "{}",
        first.join("\n")
    );
    // J is nothing now; w scrolls and y goes back.
    key(&mut app, 'J');
    assert_eq!(app.detail_offset(), 0);
    key(&mut app, 'w');
    let rows = pane(&app, w, h);
    assert!(
        has(&rows, "lines above · y scroll up"),
        "{}",
        rows.join("\n")
    );
    key(&mut app, 'y');
    assert_eq!(app.detail_offset(), 0);
    app.on_action(Action::Help);
    let s = screen(&app, 80, 24).join("\n");
    assert!(s.contains("w y  scroll the preview"), "{s}");
}

// ---- Layout ---------------------------------------------------------------

/// One unit with a line too wide for 35% of a 120-column terminal.
fn one_unit() -> (App, FixtureRepo) {
    let old = "def only(x):\n    return x\n";
    let new = "def only(x):\n    return compute_total(x, discount=0.1, currency=\"EUR\")\n";
    let (review, repo) = review_of(old, new);
    (app_with(review, KeyMap::default()), repo)
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
    // The unit's row is its name, under its file's name; the pane says where.
    let file: String = lines[2].chars().take(28).collect();
    assert!(file.starts_with("│ calc.py"), "{file}");
    let row: String = lines[3].chars().take(28).collect();
    assert!(row.contains("◆ only") && !row.contains("calc.py"), "{row}");
    assert!(
        lines.iter().any(|l| l.contains("only  calc.py:1-2")),
        "{}",
        lines.join("\n")
    );
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
fn a_wide_list_is_capped_at_35_percent() {
    let (old, new) = sources();
    let long = "a_function_name_long_enough_to_want_most_of_a_wide_terminal_for_itself";
    let (review, _repo) = review_of(
        &old.replace("long_one", long),
        &new.replace("long_one", long),
    );
    let app = app_with(review, KeyMap::default());
    let (list, p) = layout(&app, 120, 40);
    assert_eq!((list.width, p.x, p.width), (42, 42, 78));
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

// ---- Swipe ----------------------------------------------------------------

#[test]
fn the_swipe_card_scrolls_and_starts_over_on_the_next_card() {
    let (mut app, _repo) = app();
    key(&mut app, 's');
    assert_eq!(app.screen, Screen::Swipe);
    let (w, h) = (90, 24);
    let card = screen(&app, w, h);
    let text = card.join("\n");
    assert!(has(&card, MORE), "{text}");
    assert!(!has(&card, "above"), "{text}");
    key(&mut app, 'J');
    let after = screen(&app, w, h);
    assert!(has(&after, ABOVE), "{}", after.join("\n"));
    // The wheel anywhere on the screen scrolls the card too.
    wheel(&mut app, true, 1, 1);
    assert_eq!(app.detail_offset(), 6);
    wheel(&mut app, false, 1, 1);
    assert_eq!(app.detail_offset(), 3);
    // Scrolling decides nothing.
    assert_eq!(app.swipe.cursor, 0);
    assert_eq!(app.review.units[0].decision, Decision::Pending);
    for _ in 0..40 {
        key(&mut app, 'J');
    }
    let end = screen(&app, w, h);
    assert!(
        has(&end, "return a29") && !has(&end, "more lines"),
        "{}",
        end.join("\n")
    );
    // The next card starts at the top, and so does the way back.
    key(&mut app, 'l');
    assert_eq!(app.swipe.cursor, 1);
    screen(&app, w, h);
    assert_eq!(app.detail_offset(), 0);
    key(&mut app, 'J');
    key(&mut app, 'h');
    assert_eq!(app.swipe.cursor, 0);
    let back = screen(&app, w, h);
    assert!(
        has(&back, "long_one") && !has(&back, "above"),
        "{}",
        back.join("\n")
    );
}

// ---- Flagged ----------------------------------------------------------------

/// Both units flagged, the Flagged screen open on `long_one`, with a
/// Recommend answer of 40 lines.
fn flagged() -> (App, FixtureRepo) {
    let (mut app, repo) = app();
    for u in &mut app.review.units {
        u.decision = Decision::Investigate;
    }
    let answer: Vec<String> = (1..=40)
        .map(|i| format!("step {i} of the answer"))
        .collect();
    app.review.units[0].dive.results.insert(
        DiveKind::Recommend,
        DiveResult {
            text: answer.join("\n"),
            ..Default::default()
        },
    );
    key(&mut app, 'd');
    assert_eq!(app.screen, Screen::Flagged);
    assert_eq!(app.flagged_unit().unwrap().symbol, "long_one");
    (app, repo)
}

const ANSWER_MORE: &str = "more lines · J scroll";

/// How far the selected Flagged unit's tile `pane` is scrolled.
fn tile_offset(app: &App, pane: Pane) -> usize {
    let id = app.flagged_unit_id().unwrap();
    app.pane_scroll
        .borrow()
        .get(&pane_key(&id, pane))
        .copied()
        .unwrap_or(0)
}

const RECOMMEND: Pane = Pane::Answer(DiveKind::Recommend);

#[test]
fn a_long_answer_on_the_flagged_screen_scrolls_to_its_end() {
    let (mut app, _repo) = flagged();
    let (w, h) = (100, 30);
    // The answer has the focus; zoomed, it is alone on the screen.
    assert_eq!(app.flagged.pane, RECOMMEND);
    key(&mut app, 'z');
    let first = screen(&app, w, h);
    let text = first.join("\n");
    assert!(has(&first, "step 1 of the answer"), "{text}");
    assert!(!has(&first, "step 40 of the answer"), "{text}");
    // An answer is not code: the hint doesn't offer the editor.
    let hint = first
        .iter()
        .find(|l| l.contains(ANSWER_MORE))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(!hint.contains("editor"), "{hint}");
    // The unit and the tile's title stay; the answer scrolls under them.
    key(&mut app, 'J');
    let after = screen(&app, w, h);
    let text = after.join("\n");
    assert!(has(&after, "… 3 lines above · K scroll up"), "{text}");
    assert!(
        !has(&after, "step 3 of the answer") && has(&after, "step 4 of"),
        "{text}"
    );
    assert!(
        has(&after, "╭ n Recommend ✓") && has(&after, " long_one "),
        "{text}"
    );
    for _ in 0..20 {
        key(&mut app, 'J');
    }
    let end = screen(&app, w, h);
    assert!(has(&end, "step 40 of the answer"), "{}", end.join("\n"));
    assert!(!has(&end, ANSWER_MORE), "{}", end.join("\n"));
}

#[test]
fn each_tile_keeps_its_own_scroll() {
    let (mut app, _repo) = flagged();
    let (w, h) = (100, 30);
    screen(&app, w, h);
    key(&mut app, 'J');
    assert_eq!(tile_offset(&app, RECOMMEND), 3);
    // The code is cut too; J scrolls the tile in focus only.
    press(&mut app, KeyCode::Left);
    assert_eq!(app.flagged.pane, Pane::Code);
    let code = screen(&app, w, h);
    assert!(
        code.iter()
            .any(|l| l.starts_with("│ … 43 more lines · J scroll")),
        "{}",
        code.join("\n")
    );
    key(&mut app, 'J');
    screen(&app, w, h);
    assert_eq!(tile_offset(&app, Pane::Code), 3);
    assert_eq!(tile_offset(&app, RECOMMEND), 3);
    // Another unit starts at the top; coming back, where it was left.
    key(&mut app, 'j');
    assert_eq!(app.flagged_unit().unwrap().symbol, "short_one");
    screen(&app, w, h);
    assert_eq!(tile_offset(&app, Pane::Code), 0);
    key(&mut app, 'k');
    assert_eq!(tile_offset(&app, RECOMMEND), 3);
    let back = screen(&app, w, h);
    assert!(!has(&back, "step 1 of the answer"), "{}", back.join("\n"));
}

#[test]
fn the_wheel_scrolls_the_tile_under_it_and_moves_over_the_header() {
    let (mut app, _repo) = flagged();
    let (w, h) = (100, 30);
    screen(&app, w, h);
    // Over the code tile, at the left: it gets the focus and scrolls.
    wheel(&mut app, true, 5, 20);
    assert_eq!(app.flagged.pane, Pane::Code);
    assert_eq!(tile_offset(&app, Pane::Code), 3);
    assert_eq!(tile_offset(&app, RECOMMEND), 0);
    assert_eq!(app.flagged_unit().unwrap().symbol, "long_one");
    // Over the unit's header row: the next flagged unit.
    screen(&app, w, h);
    wheel(&mut app, true, 5, 1);
    assert_eq!(app.flagged_unit().unwrap().symbol, "short_one");
}

#[test]
fn the_tests_tab_scrolls_too() {
    let (mut app, _repo) = flagged();
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
    let id = app.flagged_unit_id().unwrap();
    let u = app.review.unit_mut(&id).unwrap();
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
    key(&mut app, 'c');
    assert_eq!(app.flagged.pane, Pane::Answer(DiveKind::Tests));
    key(&mut app, 'z');
    let (w, h) = (100, 30);
    let first = screen(&app, w, h);
    assert!(has(&first, "25 tests cover it") && has(&first, ANSWER_MORE));
    assert!(!has(&first, "test_case_25"), "{}", first.join("\n"));
    for _ in 0..30 {
        key(&mut app, 'J');
    }
    let end = screen(&app, w, h);
    assert!(has(&end, "test_case_25"), "{}", end.join("\n"));
    assert!(!has(&end, ANSWER_MORE), "{}", end.join("\n"));
    assert_eq!(app.overlay, Overlay::None);
}
