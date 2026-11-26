//! The queue's tree on screen: the directory and its files by name, each
//! file behind its level's icon and followed only by what there is to act
//! on; no boxes, no titles, no counts; an always-present pane with the
//! file's changes, and scrolling and clicks that line up with the rows.

use crate::common;

use std::path::PathBuf;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use teer::config::Config;
use teer::keymap::Keymap;
use teer::review::model::{Decision, Prep, Range, Review, Unit, UnitKind, UnitStatus};
use teer::tui::app::{App, Target};
use teer::tui::{preview, ui};

/// Unit `i`, alone in its file, gets a level by `i % 4`: routine, worth a
/// look (not described), close look (removed, still called), no behavior
/// change.
fn unit(i: usize) -> Unit {
    let mut u = Unit {
        id: format!("file{i:03}.rs::sym{i:03}"),
        file: PathBuf::from(format!("src/file{i:03}.rs")),
        symbol: format!("sym{i:03}"),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: None,
        new_range: Some(Range::new(10 + i as u32, 20 + i as u32)),
        nested: Vec::new(),
        renamed_from: None,
        fingerprint: String::new(),
        changes: Vec::new(),
        prep: Prep::Skipped,
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Pending,
        summary: Some("A summary that must stay out of the list rows".into()),
        nbc_reason: None,
        dive: Default::default(),
        accepted_by: None,
        reasons: Vec::new(),
        dive_pending: Default::default(),
        checks: Default::default(),
        security: Vec::new(),
        signature: None,
    };
    match i % 4 {
        1 => u.prep = Prep::Unavailable,
        2 => {
            u.status = UnitStatus::Removed;
            u.callers = vec!["somebody".into()];
        }
        3 => {
            u.status = UnitStatus::NoBehaviorChange;
            u.nbc_reason = Some("formatting only".into());
        }
        _ => {}
    }
    u
}

fn app(n: usize) -> App {
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
        units: (0..n).map(unit).collect(),
        skipped: Vec::new(),
        graph: Default::default(),
        questions: Vec::new(),
        version: 0,
        history: Default::default(),
        branch: None,
        cursor: None,
        mode: None,
    };
    let mut app = App::new(review, Config::default(), Keymap::default());

    app.rebuild_order();
    // The tree's geometry alone: the attack-surface card over it is
    // security_queue.rs's.
    app.security.enabled = false;
    app
}

fn draw(app: &App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    t.backend().buffer().clone()
}

fn row(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect()
}

/// The tree's rect on a `buf`-sized screen (header and footer excluded).
fn list_rect(app: &App, buf: &Buffer) -> Rect {
    let (w, h) = (buf.area.width, buf.area.height);
    preview::layout_of(app, Rect::new(0, 1, w, h.saturating_sub(2))).0
}

/// The screen lines of the tree, cut to its width (the pane may sit beside
/// or below it) and without the blanks at their end, by screen row.
fn list_lines(app: &App, buf: &Buffer) -> Vec<(u16, String)> {
    let r = list_rect(app, buf);
    (r.y..r.y + r.height)
        .map(|y| {
            let line: String = row(buf, y).chars().take(r.width as usize).collect();
            (y, line.trim_end().to_string())
        })
        .collect()
}

/// The level icon of a file's row under `src` is at this cell: after the
/// cursor's bar and the indent.
const ICON_X: u16 = 3;
/// Its name starts at this cell.
const NAME_X: u16 = 5;

/// Where each row of the tree was drawn, with its index.
fn row_rects(app: &App) -> Vec<(Rect, usize)> {
    app.targets
        .borrow()
        .iter()
        .filter_map(|(r, t)| match t {
            Target::Row(i) => Some((*r, *i)),
            _ => None,
        })
        .collect()
}

/// The tree's row of unit `i`'s file, by its index.
fn row_of(app: &App, i: usize) -> usize {
    let file = &app.review.units[i].file;
    app.rows().iter().position(|r| &r.path == file).unwrap()
}

/// Put the cursor on the file of unit `i`.
fn on_file(app: &mut App, i: usize) {
    let row = row_of(app, i);
    app.select_row(row);
}

fn key(app: &mut App, c: char) {
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )));
}

fn click(app: &mut App, x: u16, y: u16) {
    app.last_click = None;
    app.on_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }));
}

/// The pane rect for a terminal of `w` x `h` (header and footer excluded).
fn pane(app: &App, w: u16, h: u16) -> Option<(Rect, preview::Pane)> {
    preview::layout_of(app, Rect::new(0, 1, w, h.saturating_sub(2))).1
}

#[test]
fn the_tree_is_the_directory_and_its_files_by_name_each_behind_its_levels_icon() {
    for w in [60u16, 80, 120] {
        let mut a = app(12);
        let buf = draw(&a, w, 70);
        let lines: Vec<String> = list_lines(&a, &buf).into_iter().map(|(_, l)| l).collect();
        // The directory, then its files by name: routine (0, 4, 8), worth a
        // look (1, 5, 9), close look (2, 6, 10). The ones with no behavior
        // change (3, 7, 11) are accepted for you: one line says so.
        assert_eq!(
            lines[..12],
            [
                " ▾ src",
                "   ┃ file000.rs",
                "   ┃ file001.rs",
                // The cursor starts on the first close look.
                "▌  ┃ file002.rs",
                "   ┃ file004.rs",
                "   ┃ file005.rs",
                "   ┃ file006.rs",
                "   ┃ file008.rs",
                "   ┃ file009.rs",
                "   ┃ file010.rs",
                "",
                "   za shows 3 accepted for you",
            ],
            "{w}"
        );
        // No box, no title, no count.
        let all = lines.join("\n");
        for gone in ["╭", "│", "Needs a close look", "Routine", "0/1", "sym0"] {
            assert!(!all.contains(gone), "{w}: {gone}\n{all}");
        }
        // `za` lists them where the tree has them, behind their own icon.
        common::v2::press(&mut a, "za");
        let buf = draw(&a, w, 70);
        let lines: Vec<String> = list_lines(&a, &buf).into_iter().map(|(_, l)| l).collect();
        assert_eq!(lines[4], "   ○ file003.rs", "{w}");
        assert_eq!(lines[12], "   ○ file011.rs", "{w}");
        assert_eq!(lines[14], "   za hides 3 accepted for you", "{w}");
    }
}

#[test]
fn rows_are_exactly_the_trees_width_with_no_edge() {
    for (w, h) in [(60u16, 30u16), (80, 24), (120, 40), (160, 50)] {
        let a = app(12);
        let buf = draw(&a, w, h);
        let list = list_rect(&a, &buf);
        let rects = row_rects(&a);
        assert!(rects.len() >= 3, "{w}x{h}");
        for (r, i) in rects {
            assert_eq!((r.x, r.width, r.height), (list.x, list.width, 1), "{w}x{h}");
            let c: Vec<char> = row(&buf, r.y).chars().collect();
            assert!([' ', '▌'].contains(&c[r.x as usize]), "{w}x{h}");
            if i > 0 {
                assert!(
                    ['┃', '┃', '┃'].contains(&c[(r.x + ICON_X) as usize]),
                    "{w}x{h}"
                );
            }
            // Beside the pane, the rule that ends the tree comes right
            // after the row.
            if r.x + r.width < w {
                assert_eq!(c[(r.x + r.width - 1) as usize], ' ', "{w}x{h}");
                assert_eq!(c[(r.x + r.width) as usize], '│', "{w}x{h}");
            }
        }
    }
}

#[test]
fn a_row_is_the_icon_the_name_and_only_what_there_is_to_act_on() {
    for (w, h) in [(60u16, 30u16), (80, 24), (120, 40)] {
        let mut a = app(12);
        a.review.units[0].decision = Decision::Accepted;
        a.review.units[4].decision = Decision::Investigate;
        a.review.units[8].decision = Decision::Investigate;
        on_file(&mut a, 5);
        let buf = draw(&a, w, h);
        let tree = a.rows();
        for (r, i) in row_rects(&a).into_iter().filter(|(_, i)| *i > 0) {
            let line: String = row(&buf, r.y)
                .chars()
                .skip(r.x as usize)
                .take(r.width as usize)
                .collect();
            let u = a
                .review
                .units
                .iter()
                .find(|u| u.file == tree[i].path)
                .unwrap();
            // The file's name directly after the icon; where it is, the
            // tree says, and what is in it, the pane.
            let after: String = line.chars().skip(NAME_X as usize).collect();
            let name = &tree[i].name;
            let want = match u.decision {
                Decision::Investigate => format!("{name} ⚑"),
                _ => name.clone(),
            };
            assert_eq!(after.trim_end(), want, "{w}x{h}: {line:?}");
            for gone in ["sym", "src/", "summary", "caller", "agent", "✓", ":1"] {
                assert!(!line.contains(gone), "{w}x{h}: {gone} in {line:?}");
            }
            for state in ["pending", "accepted", "flagged"] {
                assert!(!line.contains(state), "{w}x{h}: {state} in {line:?}");
            }
        }
    }
}

#[test]
fn a_flagged_file_has_its_flag_and_one_with_nothing_left_to_do_is_grey() {
    let mut a = app(12);
    a.review.units[0].decision = Decision::Accepted;
    a.review.units[4].decision = Decision::Investigate;
    on_file(&mut a, 8);
    let buf = draw(&a, 100, 40);
    let rects = row_rects(&a);
    let cell = |unit: usize, x: u16| {
        let i = row_of(&a, unit);
        let r = rects.iter().find(|(_, p)| *p == i).unwrap().0;
        buf[(r.x + x, r.y)].style()
    };
    let muted = a.theme.muted().fg;
    let (pending, flagged, accepted) = (cell(1, NAME_X), cell(4, NAME_X), cell(0, NAME_X));
    assert_ne!(pending.fg, muted);
    // Flagged: the name as it was, then the flag in its own colour.
    assert_eq!(flagged.fg, pending.fg);
    let flag = NAME_X + "file004.rs ".len() as u16;
    assert_eq!(cell(4, flag).fg, a.theme.badge_investigate().fg);
    // Accepted: its icon steps back with its name.
    assert_eq!(accepted.fg, muted);
    assert_eq!(cell(0, ICON_X).fg, muted);
    // A close look that waits keeps its colour, a routine file has none.
    assert_eq!(cell(2, ICON_X).fg, Some(a.theme.risk_high));
    assert_eq!(cell(1, ICON_X).fg, Some(a.theme.risk_medium));
    assert_eq!(cell(8, ICON_X).fg, muted);
}

#[test]
fn a_removed_units_name_is_struck_through_in_the_pane() {
    let mut a = app(12);
    on_file(&mut a, 2);
    let buf = draw(&a, 100, 40);
    let (y, line) = (0..40)
        .map(|y| (y, row(&buf, y)))
        .find(|(_, l)| l.contains("┃ sym002"))
        .unwrap();
    let x = line.chars().position(|c| c == 's').unwrap() as u16;
    assert!(
        buf[(x, y)]
            .modifier
            .contains(ratatui::style::Modifier::CROSSED_OUT)
    );
    // The file is still there: its row is not.
    let r = row_rects(&a)[row_of(&a, 2)].0;
    assert!(
        !buf[(r.x + NAME_X, r.y)]
            .modifier
            .contains(ratatui::style::Modifier::CROSSED_OUT)
    );
}

#[test]
fn the_cursor_stays_visible_on_every_row_it_moves_to() {
    for (w, h) in [(60u16, 14u16), (80, 24), (100, 12), (60, 9)] {
        let mut a = app(40);
        let n = a.rows().len();
        let check = |a: &mut App, dir: &str| {
            let buf = draw(a, w, h);
            let lines = list_lines(a, &buf);
            let marked: Vec<&String> = lines
                .iter()
                .map(|(_, l)| l)
                .filter(|l| l.contains('▌'))
                .collect();
            let at = a.cursor_row().unwrap();
            assert_eq!(marked.len(), 1, "{dir} {at} at {w}x{h}\n{lines:?}");
            let name = a.rows()[at].name.clone();
            assert!(marked[0].ends_with(&name), "{dir} {name} at {w}x{h}");
        };
        check(&mut a, "start");
        for _ in 0..n {
            key(&mut a, 'j');
            check(&mut a, "down");
        }
        assert_eq!(a.cursor_row(), Some(n - 1));
        for _ in 0..n {
            key(&mut a, 'k');
            check(&mut a, "up");
        }
        // At the top: the directory.
        assert_eq!(a.cursor_row(), Some(0));
        assert!(a.selected().is_none());
    }
}

#[test]
fn clicks_select_the_row_under_the_pointer_and_nothing_else_does() {
    for (w, h) in [(60u16, 24u16), (80, 24), (120, 40)] {
        let mut a = app(40);
        // Scroll somewhere in the middle first.
        let middle = a.rows().len() / 2;
        a.select_row(middle);
        let buf = draw(&a, w, h);
        let rects = row_rects(&a);
        assert!(rects.len() >= 3, "{w}x{h}");
        let tree = a.rows();
        for (r, i) in &rects {
            assert!(row(&buf, r.y).contains(&tree[*i].name), "{w}x{h} row {i}");
        }
        // Click the name, then the right-most cell, of every file's row on
        // show, from a cursor that scrolls the tree.
        for (r, i) in rects.into_iter().filter(|(_, i)| *i > 0) {
            for dx in [NAME_X, r.width - 1] {
                a.select_row(middle);
                draw(&a, w, h);
                click(&mut a, r.x + dx, r.y);
                assert_eq!(a.cursor_row(), Some(i), "{w}x{h} dx {dx}");
                assert_eq!(a.selected().unwrap().file, tree[i].path);
            }
        }
    }
    // Under the tree's last row nothing is a target: the blank line, the
    // line about the units accepted for you, the rows left over.
    let mut a = app(12);
    let buf = draw(&a, 120, 40);
    let rows: Vec<u16> = row_rects(&a).iter().map(|(r, _)| r.y).collect();
    let list = list_rect(&a, &buf);
    let before = a.cursor_row();
    for (y, l) in list_lines(&a, &buf) {
        if !rows.contains(&y) {
            click(&mut a, list.x + NAME_X, y);
            assert_eq!(a.cursor_row(), before, "click on {l:?}");
            draw(&a, 120, 40);
        }
    }
    // A click on the directory puts the cursor on it and folds it; a
    // second one opens it again.
    let dir = row_rects(&a)[0].0;
    click(&mut a, dir.x + 4, dir.y);
    assert_eq!(a.cursor_row(), Some(0));
    assert_eq!(a.rows().len(), 1);
    draw(&a, 120, 40);
    click(&mut a, dir.x + 4, dir.y);
    assert_eq!(a.rows().len(), 10);
}

#[test]
fn the_pane_is_always_there_for_the_file_under_the_cursor() {
    for (w, h) in [
        (60u16, 20u16),
        (80, 24),
        (109, 30),
        (110, 30),
        (120, 40),
        (160, 50),
    ] {
        let mut a = app(12);
        on_file(&mut a, 1);
        let (rect, kind) = pane(&a, w, h).unwrap_or_else(|| panic!("no pane at {w}x{h}"));
        assert_eq!(kind == preview::Pane::Side, w >= 110, "{w}x{h}");
        let buf = draw(&a, w, h);
        let text: String = (rect.y..rect.y + rect.height)
            .map(|y| {
                (rect.x..rect.x + rect.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        // The unit's head line, then what the agent says it does. Why it
        // has its level is in its view.
        let lines: Vec<&str> = text.lines().collect();
        let head = lines.iter().position(|l| l.contains(" ┃ sym001")).unwrap();
        assert!(
            lines[head + 1].contains("   A summary that must stay out of the list rows"),
            "{w}x{h}\n{text}"
        );
        for gone in ["review", "not described", "file001.rs", "Nothing selected"] {
            assert!(!text.contains(gone), "{w}x{h}: {gone}\n{text}");
        }
        // The tree still has room for several rows.
        assert!(row_rects(&a).len() >= 3, "{w}x{h}");
        // On the directory the pane says nothing.
        a.select_row(0);
        let buf = draw(&a, w, h);
        for y in rect.y + 1..rect.y + rect.height {
            let line: String = (rect.x + 1..rect.x + rect.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            assert_eq!(line.trim(), "", "{w}x{h} row {y}");
        }
    }
}

#[test]
fn p_is_not_bound_and_tiny_terminals_never_panic() {
    let mut a = app(12);
    let before = a.sel;
    key(&mut a, 'p');
    assert_eq!(a.sel, before);
    for (w, h) in [
        (0u16, 0u16),
        (1, 1),
        (10, 3),
        (10, 4),
        (10, 5),
        (3, 40),
        (40, 2),
        (60, 6),
        (60, 7),
        (111, 3),
        (200, 1),
    ] {
        for row in [0, 5, 8] {
            a.select_row(row);
            draw(&a, w, h);
        }
    }
}
