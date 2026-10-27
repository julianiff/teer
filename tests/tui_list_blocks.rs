//! The Slim list as a titled box per level, each holding its files: a
//! file's name, then its units, each unit row its level's icon and its
//! name; an always-present preview pane, and scrolling and clicks that
//! still line up with the units.

mod common;

use std::path::PathBuf;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use teer::config::{Config, KeyMap};
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
        dive_pending: Default::default(),
        checks: Default::default(),
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
    };
    App::new(review, Config::default(), KeyMap::default())
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

/// The list's rect on a `buf`-sized screen (header and footer excluded).
fn list_rect(app: &App, buf: &Buffer) -> Rect {
    let (w, h) = (buf.area.width, buf.area.height);
    preview::layout_of(app, Rect::new(0, 1, w, h.saturating_sub(2))).0
}

/// The screen lines of the list, cut to its width (the pane may sit beside
/// or below it), by screen row.
fn list_lines(app: &App, buf: &Buffer) -> Vec<(u16, String)> {
    let r = list_rect(app, buf);
    (r.y..r.y + r.height)
        .map(|y| (y, row(buf, y).chars().take(r.width as usize).collect()))
        .collect()
}

fn rows(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height).map(|y| row(buf, y)).collect()
}

/// The level icon of a unit row starts at this cell of the row: after the
/// box's border, the gutter and two blanks.
const ICON_X: u16 = 4;
/// The name of a unit row starts at this cell.
const NAME_X: u16 = 6;

fn unit_rects(app: &App) -> Vec<(Rect, usize)> {
    app.targets
        .borrow()
        .iter()
        .filter_map(|(r, t)| match t {
            Target::Unit(p) => Some((*r, *p)),
            _ => None,
        })
        .collect()
}

fn key(app: &mut App, c: char) {
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )));
}

fn click(app: &mut App, x: u16, y: u16) {
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
fn every_level_is_a_titled_box_of_its_files_with_their_units() {
    for w in [60u16, 80, 120] {
        let a = app(12);
        let buf = draw(&a, w, 70);
        let lines = list_lines(&a, &buf);
        let at = |t: &str| {
            lines
                .iter()
                .position(|(_, l)| l.contains(t))
                .unwrap_or_else(|| panic!("{w}: no {t:?}"))
        };
        // Boxes by attention, then the muted one for files with no
        // behavior change.
        let titles = [
            "Needs a close look · 3 files",
            "Worth a look · 3 files",
            "Routine · 3 files",
            "No behavior change · 3 files",
        ];
        for pair in titles.windows(2) {
            assert!(at(pair[0]) < at(pair[1]), "{w}: {pair:?}");
        }
        for t in titles {
            let (_, l) = &lines[at(t)];
            assert!(l.starts_with("╭─ ") && l.ends_with('╮'), "{w}: {l:?}");
        }
        // Each box holds its files: close looks (2, 6, 10), worth a look
        // (1, 5, 9), routine (0, 4, 8), no behavior change (3, 7, 11).
        let files = [2, 6, 10, 1, 5, 9, 0, 4, 8, 3, 7, 11];
        for pair in files.windows(2) {
            let (f0, f1) = (
                format!("file{:03}.rs", pair[0]),
                format!("file{:03}.rs", pair[1]),
            );
            assert!(at(&f0) < at(&f1), "{w}: {f0} before {f1}");
        }
        for (k, &f) in files.iter().enumerate() {
            let file = at(&format!("file{f:03}.rs"));
            assert!(at(titles[k / 3]) < file, "{w}: file{f:03} in its box");
            if k % 3 == 2 {
                let next = lines[file + 2..]
                    .iter()
                    .position(|(_, l)| l.starts_with('╰'))
                    .map(|n| file + 2 + n);
                assert!(next.is_some(), "{w}: box of file{f:03} closes");
            }
        }
        for (i, icon) in [(2, '▲'), (1, '◆'), (0, '●')] {
            let file = at(&format!("file{i:03}.rs"));
            let (_, name) = &lines[file];
            // The file's name, a label inside the box.
            assert!(name.starts_with("│ src/"), "{w}: {name:?}");
            assert!(name.ends_with("0/1 │"), "{w}: {name:?}");
            // The unit right under it, behind its level's icon.
            let unit: Vec<char> = lines[file + 1].1.chars().collect();
            assert_eq!(unit[ICON_X as usize], icon, "{w}: {:?}", lines[file + 1].1);
            let name: String = unit[NAME_X as usize..].iter().collect();
            assert!(name.starts_with(&format!("sym{i:03}")), "{w}: {name:?}");
        }
        // A unit with no behavior change is muted, under its file, with why
        // (cut in the narrow list beside the pane).
        let quiet = &lines[at("file003.rs") + 1].1;
        assert!(quiet.contains("○ sym003"), "{w}: {quiet:?}");
        assert!(
            w >= 110 || quiet.contains("formatting only"),
            "{w}: {quiet:?}"
        );
    }
}

#[test]
fn unit_rows_are_exactly_the_list_width_and_inside_their_box() {
    for (w, h) in [(60u16, 30u16), (80, 24), (120, 40), (160, 50)] {
        let a = app(12);
        let buf = draw(&a, w, h);
        let list = list_rect(&a, &buf);
        let rects = unit_rects(&a);
        assert!(rects.len() >= 3, "{w}x{h}");
        for (r, _) in rects {
            assert_eq!((r.x, r.width), (list.x, list.width), "{w}x{h}");
            let c: Vec<char> = row(&buf, r.y).chars().collect();
            assert_eq!(c[r.x as usize], '│', "{w}x{h}");
            assert_eq!(c[(r.x + r.width - 1) as usize], '│', "{w}x{h}");
            assert!(
                ['▲', '◆', '●'].contains(&c[(r.x + ICON_X) as usize]),
                "{w}x{h}"
            );
            // Beside the pane, the cell after the row is blank.
            if r.x + r.width < w {
                assert_eq!(c[(r.x + r.width) as usize], ' ', "{w}x{h}");
            }
        }
    }
}

#[test]
fn rows_are_just_the_icon_and_the_name() {
    for (w, h) in [(60u16, 30u16), (80, 24), (120, 40)] {
        let mut a = app(12);
        a.review.units[0].decision = Decision::Accepted;
        a.review.units[4].decision = Decision::Investigate;
        a.review.units[8].decision = Decision::Investigate;
        a.sel = 5;
        let buf = draw(&a, w, h);
        for (r, pos) in unit_rects(&a) {
            let line: String = row(&buf, r.y)
                .chars()
                .skip(r.x as usize)
                .take(r.width as usize)
                .collect();
            let u = &a.review.units[a.order[pos]];
            // The name, nothing else: where it is, the preview says.
            assert!(line.contains(&u.symbol), "{w}x{h}: {line:?}");
            assert!(!line.contains("file"), "{w}x{h}: {line:?}");
            assert!(
                !line.contains(&format!(":{}-", 10 + a.order[pos])),
                "{w}x{h}: {line:?}"
            );
            assert!(!line.contains("summary"), "{w}x{h}: {line:?}");
            assert!(!line.contains("caller"), "{w}x{h}: {line:?}");
            assert!(!line.contains("agent"), "{w}x{h}: {line:?}");
            for g in ["✓", "⚑", "+", "~", "−"] {
                assert!(!line.contains(g), "{w}x{h}: {g} in {line:?}");
            }
            // After the border, the gutter and the icon, the name starts
            // directly.
            let after: String = line.chars().skip(NAME_X as usize).collect();
            assert!(after.starts_with("sym"), "{w}x{h}: {line:?}");
        }
    }
}

#[test]
fn flagged_rows_are_styled_distinctly_and_accepted_ones_grey() {
    let mut a = app(12);
    a.review.units[0].decision = Decision::Accepted;
    a.review.units[4].decision = Decision::Investigate;
    a.sel = a.order.iter().position(|&i| i == 8).unwrap();
    let buf = draw(&a, 100, 40);
    let rects = unit_rects(&a);
    let cell = |unit: usize, x: u16| {
        let pos = a.order.iter().position(|&i| i == unit).unwrap();
        let r = rects.iter().find(|(_, p)| *p == pos).unwrap().0;
        buf[(r.x + x, r.y)].style()
    };
    let pending = cell(1, NAME_X);
    let flagged = cell(4, NAME_X);
    let accepted = cell(0, NAME_X);
    assert_ne!(flagged, pending);
    assert_ne!(accepted, pending);
    assert_ne!(flagged, accepted);
    assert_eq!(flagged.fg, a.theme.flagged_row().fg);
    assert_eq!(accepted.fg, a.theme.accepted_row().fg);
    // An accepted unit's icon steps back with its name.
    assert_eq!(cell(0, ICON_X).fg, a.theme.accepted_row().fg);
    // The row is not a glyph: nothing extra in it.
    let pos = a.order.iter().position(|&i| i == 4).unwrap();
    let r = rects.iter().find(|(_, p)| *p == pos).unwrap().0;
    assert!(!row(&buf, r.y).contains('⚑'));
}

#[test]
fn removed_names_are_struck_through() {
    let a = app(12);
    let buf = draw(&a, 100, 40);
    let pos = a.order.iter().position(|&i| i == 2).unwrap();
    let r = unit_rects(&a).iter().find(|(_, p)| *p == pos).unwrap().0;
    assert!(
        buf[(r.x + NAME_X, r.y)]
            .modifier
            .contains(ratatui::style::Modifier::CROSSED_OUT)
    );
}

#[test]
fn the_selection_stays_visible_with_its_file_and_box_title() {
    for (w, h) in [(60u16, 14u16), (80, 24), (100, 12), (60, 9)] {
        let mut a = app(40);
        let n = a.order.len();
        let check = |a: &mut App, dir: &str| {
            let buf = draw(a, w, h);
            let lines = list_lines(a, &buf);
            let marked: Vec<&String> = lines
                .iter()
                .map(|(_, l)| l)
                .filter(|l| l.contains('▌'))
                .collect();
            assert_eq!(
                marked.len(),
                1,
                "{dir} {} at {w}x{h}\n{}",
                a.sel,
                rows(&buf).join("\n")
            );
            let sym = a.selected().unwrap().symbol.clone();
            assert!(marked[0].contains(&sym), "{dir} {sym} at {w}x{h}");
        };
        check(&mut a, "start");
        for _ in 0..n {
            key(&mut a, 'j');
            check(&mut a, "down");
        }
        assert_eq!(a.sel, n - 1);
        for _ in 0..n {
            key(&mut a, 'k');
            check(&mut a, "up");
        }
        assert_eq!(a.sel, 0);
        // Every unit brings its file's name; the first of a box its title.
        for pos in 0..n {
            a.sel = pos;
            let buf = draw(&a, w, h);
            let lines = list_lines(&a, &buf);
            let u = &a.review.units[a.order[pos]];
            let name = u.file.file_name().unwrap().to_string_lossy().to_string();
            assert!(
                lines.iter().any(|(_, l)| l.contains(&name)),
                "file name {name} missing for pos {pos} at {w}x{h}\n{}",
                rows(&buf).join("\n")
            );
            let level = a.level_of(a.order[pos]).level;
            let first = pos == 0 || a.level_of(a.order[pos - 1]).level != level;
            if first {
                assert!(
                    lines.iter().any(|(_, l)| l.contains(level.title())),
                    "title {} missing for pos {pos} at {w}x{h}\n{}",
                    level.title(),
                    rows(&buf).join("\n")
                );
            }
        }
    }
}

#[test]
fn clicks_map_to_the_unit_under_the_pointer_and_nothing_else_selects() {
    for (w, h) in [(60u16, 24u16), (80, 24), (120, 40)] {
        let mut a = app(40);
        // Scroll somewhere in the middle first.
        let middle = a.order.len() / 2;
        a.sel = middle;
        let buf = draw(&a, w, h);
        let rects = unit_rects(&a);
        assert!(rects.len() >= 3, "{w}x{h}");
        for (r, pos) in &rects {
            let sym = a.review.units[a.order[*pos]].symbol.clone();
            assert!(row(&buf, r.y).contains(&sym), "{w}x{h} pos {pos}");
        }
        // Click the name, then the right-most cell, of every row on show,
        // from a selection that scrolls the list.
        for (r, pos) in rects {
            for dx in [NAME_X, r.width - 1] {
                a.sel = middle;
                draw(&a, w, h);
                click(&mut a, r.x + dx, r.y);
                assert_eq!(a.sel, pos, "{w}x{h} dx {dx}");
            }
        }
        // File names, box edges and blank lines are not targets: a click
        // there leaves the selection.
        a.sel = middle;
        let buf = draw(&a, w, h);
        let units: Vec<u16> = unit_rects(&a).iter().map(|(r, _)| r.y).collect();
        let list = list_rect(&a, &buf);
        let others: Vec<(u16, String)> = list_lines(&a, &buf)
            .into_iter()
            .filter(|(y, _)| !units.contains(y))
            .collect();
        assert!(
            others.iter().any(|(_, l)| l.contains("src/file")),
            "{w}x{h}"
        );
        for (y, l) in others {
            click(&mut a, list.x + NAME_X, y);
            assert_eq!(a.sel, middle, "{w}x{h} click on {l:?}");
            draw(&a, w, h);
        }
    }
}

#[test]
fn the_preview_is_always_there_for_the_selected_unit() {
    for (w, h) in [
        (60u16, 20u16),
        (80, 24),
        (109, 30),
        (110, 30),
        (120, 40),
        (160, 50),
    ] {
        let mut a = app(12);
        a.sel = 1;
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
        let sym = &a.selected().unwrap().symbol;
        assert!(text.contains(&format!(" {sym} ")), "{w}x{h}\n{text}");
        // Reasons come first, right under the head.
        let lines: Vec<&str> = text.lines().collect();
        let head = lines.iter().position(|l| l.contains(sym)).unwrap();
        assert!(
            lines[head + 1].contains("not described") || lines[head + 1].contains("removed"),
            "{w}x{h}: {:?}",
            lines[head + 1]
        );
        // The list still has room for several rows.
        assert!(unit_rects(&a).len() >= 3, "{w}x{h}");
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
        for sel in [0, 5, 8] {
            a.sel = sel;
            draw(&a, w, h);
        }
    }
}
