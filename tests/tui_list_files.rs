//! The list is a box per level, each holding the files whose most critical
//! unit has that level: the file's name as a label, its units under it,
//! most critical first. The cursor only ever moves over units. `ctrl+d`
//! `ctrl+u` jump file to file, `A` accepts a file's pending units (one undo
//! step), and a large review asks the agent about a file when the cursor
//! gets to it.

mod common;

use std::path::PathBuf;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::config::{Action, Config, KeyMap};
use teer::review::model::{Decision, Prep, Range, Review, SessionRef, Unit, UnitKind, UnitStatus};
use teer::tui::app::{App, Effect, Overlay, Screen, Target};
use teer::tui::{slim, ui};

#[derive(Clone, Copy)]
enum Kind {
    /// Removed while still called: needs a close look.
    Close,
    /// Not described: worth a look.
    Look,
    Routine,
    /// Formatting only: no behavior change.
    Same,
}

fn unit(id: &str, file: &str, line: u32, kind: Kind) -> Unit {
    let mut u = Unit {
        id: format!("{file}::{id}"),
        file: PathBuf::from(file),
        symbol: id.into(),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: None,
        new_range: Some(Range::new(line, line + 5)),
        renamed_from: None,
        fingerprint: String::new(),
        changes: Vec::new(),
        prep: Prep::Skipped,
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Pending,
        summary: Some("summary".into()),
        nbc_reason: None,
        dive: Default::default(),
        dive_pending: Default::default(),
        checks: Default::default(),
    };
    match kind {
        Kind::Close => {
            u.status = UnitStatus::Removed;
            u.callers = vec!["somebody".into()];
        }
        Kind::Look => u.prep = Prep::Unavailable,
        Kind::Routine => {}
        Kind::Same => {
            u.status = UnitStatus::NoBehaviorChange;
            u.nbc_reason = Some("formatting only".into());
        }
    }
    u
}

/// Two close looks in `src/c.rs`, a routine unit above them and a
/// formatting change after them; three worth-a-look units in two files; 20
/// routine units in five files of four; and `src/q.rs` with only a
/// formatting change.
/// Positions: `src/c.rs` 0-2 (close_a, close_b, c_helper), `src/l0.rs` 3-4,
/// `src/l1.rs` 5, `src/r0.rs` 6-9, `src/r1.rs` 10-13, ... `src/r4.rs` 22-25.
fn units() -> Vec<Unit> {
    let mut v = vec![
        unit("c_helper", "src/c.rs", 5, Kind::Routine),
        unit("close_a", "src/c.rs", 10, Kind::Close),
        unit("close_b", "src/c.rs", 30, Kind::Close),
        unit("tidy", "src/c.rs", 50, Kind::Same),
        unit("look_a", "src/l0.rs", 10, Kind::Look),
        unit("look_b", "src/l0.rs", 30, Kind::Look),
        unit("look_c", "src/l1.rs", 10, Kind::Look),
        unit("quiet", "src/q.rs", 10, Kind::Same),
    ];
    for f in 0..5 {
        for k in 0..4 {
            v.push(unit(
                &format!("r{f}_{k}"),
                &format!("src/r{f}.rs"),
                10 + 20 * k,
                Kind::Routine,
            ));
        }
    }
    v
}

fn app_with(units: Vec<Unit>, cfg: Config) -> App {
    app_keys(units, cfg, KeyMap::default())
}

fn app_keys(units: Vec<Unit>, cfg: Config, keys: KeyMap) -> App {
    common::isolate_env();
    let mut paths: Vec<PathBuf> = units.iter().map(|u| u.file.clone()).collect();
    paths.dedup();
    let files = paths
        .into_iter()
        .map(|path| teer::git::ChangedFile {
            path,
            old_path: None,
            status: teer::git::FileStatus::Modified,
            old_content: None,
            new_content: None,
        })
        .collect();
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
        files,
        units,
        skipped: vec![teer::git::SkippedFile {
            path: PathBuf::from("assets/logo.png"),
            status: teer::git::FileStatus::Added,
            why: "binary".into(),
        }],
        graph: Default::default(),
        questions: Vec::new(),
    };
    App::new(review, cfg, keys)
}

fn app() -> App {
    app_with(units(), Config::default())
}

fn render(app: &App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    t
}

/// The screen as lines of plain text.
fn lines(app: &App, w: u16, h: u16) -> Vec<String> {
    let t = render(app, w, h);
    let buf = t.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

fn text(app: &App, w: u16, h: u16) -> String {
    lines(app, w, h).join("\n")
}

fn press(a: &mut App, code: KeyCode, mods: KeyModifiers) -> Vec<Effect> {
    a.on_event(Event::Key(KeyEvent::new(code, mods)))
}

fn key(a: &mut App, c: char) -> Vec<Effect> {
    press(a, KeyCode::Char(c), KeyModifiers::NONE)
}

fn code(a: &mut App, c: KeyCode) -> Vec<Effect> {
    press(a, c, KeyModifiers::NONE)
}

fn ctrl(a: &mut App, c: char) -> Vec<Effect> {
    press(a, KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn shift_a(a: &mut App) -> Vec<Effect> {
    press(a, KeyCode::Char('A'), KeyModifiers::SHIFT)
}

fn symbol(a: &App) -> &str {
    &a.selected().unwrap().symbol
}

fn decisions(a: &App) -> Vec<Decision> {
    a.review.units.iter().map(|u| u.decision).collect()
}

fn accepted(a: &App) -> Vec<&str> {
    a.review
        .units
        .iter()
        .filter(|u| u.decision == Decision::Accepted)
        .map(|u| u.symbol.as_str())
        .collect()
}

fn prepared(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .flat_map(|e| match e {
            Effect::Prepare(ids) => ids.clone(),
            _ => vec![],
        })
        .collect()
}

/// Plain text of a built row.
fn row_text(l: &ratatui::text::Line) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

// ---- Rows -----------------------------------------------------------------

#[test]
fn each_file_sits_in_the_box_of_its_most_critical_unit_with_its_units_under_its_name() {
    let a = app();
    let s = lines(&a, 120, 60);
    let at = |t: &str| {
        s.iter()
            .position(|l| l.contains(t))
            .unwrap_or_else(|| panic!("no {t}\n{}", s.join("\n")))
    };
    // The boxes in attention order, each file in the box of its most
    // critical unit, files by path inside a box.
    let order = [
        "╭─ Needs a close look · 1 file ",
        "│ src/c.rs",
        "╭─ Worth a look · 2 files ",
        "│ src/l0.rs",
        "│ src/l1.rs",
        "╭─ Routine · 5 files ",
        "│ src/r0.rs",
        "│ src/r4.rs",
        "╭─ No behavior change · 1 file ",
        "│   src/q.rs",
        "╭─ Not shown · 1 file ",
        "│   assets/logo.png",
    ];
    for pair in order.windows(2) {
        assert!(at(pair[0]) < at(pair[1]), "{pair:?}");
    }
    // The file's name is a label with its count; no open or closed marker.
    let label = &s[at("│ src/c.rs")];
    assert!(
        label.starts_with("│ src/c.rs") && label.contains("0/3 │"),
        "{label}"
    );
    assert!(!s.iter().any(|l| l.contains('▾') || l.contains('▸')));
    // Units right under it, most critical first, behind their icons.
    assert_eq!(at("▲ close_a"), at("│ src/c.rs") + 1);
    assert_eq!(at("▲ close_b"), at("│ src/c.rs") + 2);
    assert_eq!(at("● c_helper"), at("│ src/c.rs") + 3);
    assert_eq!(at("○ tidy"), at("│ src/c.rs") + 4);
    assert_eq!(at("◆ look_a"), at("│ src/l0.rs") + 1);
    assert_eq!(at("● r0_0"), at("│ src/r0.rs") + 1);
    // Units are indented under the file's name.
    let col = |l: &str, c: char| l.chars().position(|x| x == c).unwrap();
    assert_eq!(col(&s[at("▲ close_a")], '▲'), col(label, 's') + 2);
    insta::assert_snapshot!("files", render(&a, 120, 60).backend());
}

#[test]
fn only_units_are_selectable() {
    let a = app();
    let rows = slim::build(&a, 80);
    let owned: Vec<String> = (0..rows.lines.len())
        .filter(|&y| rows.owner[y].is_some())
        .map(|y| row_text(&rows.lines[y]))
        .collect();
    assert_eq!(owned.len(), a.order.len());
    for (pos, &(s, e)) in rows.spans.iter().enumerate() {
        assert_eq!(e - s, 1);
        assert_eq!(rows.owner[s], Some(pos));
        assert!(rows.heads[pos] < s && rows.titles[pos] < rows.heads[pos]);
        assert_eq!(rows.owner[rows.heads[pos]], None);
        assert!(row_text(&rows.lines[rows.titles[pos]]).starts_with('╭'));
    }
    for t in [
        "src/c.rs",
        "○ tidy",
        "src/q.rs",
        "○ quiet",
        "assets/logo.png",
    ] {
        let y = rows
            .lines
            .iter()
            .position(|l| row_text(l).contains(t))
            .unwrap();
        assert_eq!(rows.owner[y], None, "{t}");
    }
    let logo = rows
        .lines
        .iter()
        .find(|l| row_text(l).contains("assets/logo.png"))
        .unwrap();
    assert!(row_text(logo).contains("binary"));
    // Nothing to decide at all: the muted boxes still say what changed.
    let units: Vec<Unit> = units()
        .into_iter()
        .filter(|u| u.status == UnitStatus::NoBehaviorChange)
        .collect();
    let b = app_with(units, Config::default());
    let s = text(&b, 80, 20);
    assert!(s.contains("○ tidy") && s.contains("assets/logo.png"), "{s}");
}

#[test]
fn every_row_fits_its_width_and_nothing_is_cut_at_the_fitted_width() {
    let a = app();
    for w in [0u16, 1, 3, 4, 10, 30, 60, 80, 120] {
        for l in &slim::build(&a, w).lines {
            assert!(l.width() <= w as usize, "{w}: {l:?}");
        }
    }
    for w in [30u16, 60, 80, 120] {
        for l in &slim::build(&a, w).lines {
            assert_eq!(l.width(), w as usize, "{w}: {l:?}");
        }
    }
    let w = slim::fit_width(&a) as u16;
    let s: Vec<String> = slim::build(&a, w).lines.iter().map(row_text).collect();
    for u in &a.review.units {
        assert!(
            s.iter().any(|l| l.contains(&u.symbol)),
            "{} at {w}",
            u.symbol
        );
    }
    assert!(
        s.iter()
            .any(|l| l.contains("src/r4.rs") && l.contains("0/4"))
    );
    assert!(s.iter().any(|l| l.contains("Needs a close look · 1 file")));
}

#[test]
fn blank_lines_separate_boxes_and_files_when_the_list_has_room() {
    let a = app();
    let rows = slim::build(&a, 80);
    let text: Vec<String> = rows.lines.iter().map(row_text).collect();
    let at = |t: &str| text.iter().position(|l| l.contains(t)).unwrap();
    // Between boxes: a bare blank line.
    let worth = at("Worth a look");
    assert!(text[worth - 1].trim().is_empty());
    assert!(text[worth - 2].starts_with('╰'));
    // Between files in a box: a blank line inside it.
    let l1 = at("src/l1.rs");
    assert_eq!(text[l1 - 1].trim_matches(|c| c == '│' || c == ' '), "");
    assert!(text[l1 - 1].starts_with('│'));
    for y in (0..text.len()).filter(|&y| text[y].trim_matches(|c| c == '│' || c == ' ').is_empty())
    {
        assert_eq!(rows.owner[y], None);
    }
    // Without padding: none of them.
    let flat: Vec<String> = slim::build_with(&a, 80, false)
        .lines
        .iter()
        .map(row_text)
        .collect();
    let l1 = flat.iter().position(|l| l.contains("src/l1.rs")).unwrap();
    assert!(flat[l1 - 1].contains("look_b"), "{}", flat.join("\n"));
    let worth = flat
        .iter()
        .position(|l| l.contains("Worth a look"))
        .unwrap();
    assert!(flat[worth - 1].starts_with('╰'));
    // A short list drops them.
    let s = lines(&a, 80, 20);
    let c = s.iter().position(|l| l.contains("Worth a look")).unwrap();
    assert!(s[c - 1].starts_with('╰'), "{}", s.join("\n"));
}

#[test]
fn clicks_select_units_and_do_nothing_anywhere_else_in_the_list() {
    for w in [60u16, 80, 120] {
        let mut a = app();
        let s = lines(&a, w, 60);
        let units: Vec<(u16, usize)> = a
            .targets
            .borrow()
            .iter()
            .filter_map(|(r, t)| match t {
                Target::Unit(p) => Some((r.y, *p)),
                _ => None,
            })
            .collect();
        let click = |a: &mut App, y: u16| {
            a.on_event(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 4,
                row: y,
                modifiers: KeyModifiers::NONE,
            }));
        };
        let list_w = slim::fit_width(&a).min(w as usize);
        for (y, l) in s.iter().enumerate().take(40).skip(1) {
            let row: String = l.chars().take(list_w).collect();
            if units.iter().any(|&(uy, _)| uy == y as u16) || row.contains("──") && y > 38 {
                continue;
            }
            let before = a.sel;
            click(&mut a, y as u16);
            assert_eq!(a.sel, before, "{w}: click on line {y}: {row}");
        }
        for &(y, pos) in units.iter().rev() {
            click(&mut a, y);
            assert_eq!(a.sel, pos, "{w}: line {y}");
        }
    }
}

// ---- Moving -----------------------------------------------------------------

#[test]
fn j_and_k_move_over_units_only_and_enter_and_esc_do_nothing() {
    let mut a = app();
    assert_eq!(a.sel, 0);
    assert_eq!(symbol(&a), "close_a");
    key(&mut a, 'j');
    key(&mut a, 'j');
    assert_eq!(symbol(&a), "c_helper");
    // Past a file's name and a box edge, straight to the next unit.
    key(&mut a, 'j');
    assert_eq!(symbol(&a), "look_a");
    key(&mut a, 'k');
    assert_eq!(symbol(&a), "c_helper");
    key(&mut a, 'G');
    assert_eq!(symbol(&a), "r4_3");
    key(&mut a, 'j');
    assert_eq!(symbol(&a), "r4_3");
    key(&mut a, 'g');
    assert_eq!(a.sel, 0);
    key(&mut a, 'k');
    assert_eq!(a.sel, 0);
    for k in [KeyCode::Enter, KeyCode::Esc, KeyCode::Tab] {
        let before = text(&a, 120, 30);
        assert!(code(&mut a, k).is_empty());
        assert_eq!(text(&a, 120, 30), before, "{k:?}");
    }
    // Deciding moves on to the next pending unit, across files.
    a.sel = 2;
    key(&mut a, 'l');
    assert_eq!(symbol(&a), "look_a");
}

#[test]
fn ctrl_d_and_ctrl_u_go_to_the_first_unit_of_the_next_or_previous_file() {
    let mut a = app();
    ctrl(&mut a, 'd');
    assert_eq!(symbol(&a), "look_a");
    a.sel = 4;
    ctrl(&mut a, 'd');
    assert_eq!(symbol(&a), "look_c");
    // From the middle of a file: the file above, not the start of this one.
    a.sel = 8;
    ctrl(&mut a, 'u');
    assert_eq!(symbol(&a), "look_c");
    a.sel = 1;
    ctrl(&mut a, 'u');
    assert_eq!(a.sel, 1);
    assert_eq!(a.status, "first file");
    key(&mut a, 'j');
    assert!(a.status.is_empty());
    a.sel = 23;
    ctrl(&mut a, 'd');
    assert_eq!(a.sel, 23);
    assert_eq!(a.status, "last file");
    // The actions do the same.
    a.sel = 0;
    a.on_action(Action::NextGroup);
    assert_eq!(symbol(&a), "look_a");
    a.on_action(Action::PrevGroup);
    assert_eq!(symbol(&a), "close_a");
}

#[test]
fn ctrl_d_is_list_only_and_plain_d_still_opens_flagged() {
    let mut a = app();
    a.review.units[1].decision = Decision::Investigate;
    ctrl(&mut a, 'd');
    assert_eq!((a.screen, symbol(&a)), (Screen::Slim, "look_a"));
    key(&mut a, 'd');
    assert_eq!(a.screen, Screen::Flagged);
    let sel = a.flagged.sel;
    ctrl(&mut a, 'd');
    assert_eq!((a.screen, a.flagged.sel), (Screen::Flagged, sel));
    let mut b = app();
    key(&mut b, 's');
    let cursor = b.swipe.cursor;
    ctrl(&mut b, 'd');
    assert_eq!((b.screen, b.swipe.cursor), (Screen::Swipe, cursor));
    let mut c = app();
    key(&mut c, 'd');
    assert_eq!(c.sel, 0);
}

#[test]
fn the_selected_unit_stays_selected_through_a_rebuild() {
    let mut a = app();
    a.sel = 7;
    let id = a.selected().unwrap().id.clone();
    a.review.units.swap(0, 10);
    a.rebuild_order();
    assert_eq!(a.selected().unwrap().id, id);
}

// ---- A: accept the file ------------------------------------------------------

#[test]
fn a_asks_first_then_accepts_the_files_pending_units_in_one_undo_step() {
    for confirm in [KeyCode::Char('y'), KeyCode::Enter] {
        let mut a = app();
        a.sel = 15; // r2_1
        a.review.units[a.order[16]].decision = Decision::Investigate;
        let before = decisions(&a);
        assert!(shift_a(&mut a).is_empty());
        match &a.overlay {
            Overlay::ConfirmAcceptFile { file, ids } => {
                assert_eq!(ids.len(), 3);
                assert_eq!(file, &PathBuf::from("src/r2.rs"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(decisions(&a), before);
        let s = text(&a, 80, 30);
        assert!(
            s.contains("Accept 3 pending units in \"src/r2.rs\"?"),
            "{s}"
        );
        let effects = code(&mut a, confirm);
        assert_eq!(effects, vec![Effect::Save]);
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(accepted(&a), ["r2_0", "r2_1", "r2_3"]);
        assert_eq!(a.sel, 15);
        assert_eq!(a.swipe.history.len(), 1);
        key(&mut a, 'u');
        assert_eq!(decisions(&a), before);
    }
    let mut b = app();
    b.review.units[1].decision = Decision::Accepted;
    b.review.units[2].decision = Decision::Accepted;
    shift_a(&mut b);
    let s = text(&b, 100, 24);
    assert!(
        s.contains("Accept 1 pending unit in \"src/c.rs\"? (y/n)"),
        "{s}"
    );
}

#[test]
fn other_keys_cancel_the_file_accept_and_nothing_pending_asks_nothing() {
    for k in [KeyCode::Char('n'), KeyCode::Esc, KeyCode::Char('x')] {
        let mut a = app();
        let before = decisions(&a);
        shift_a(&mut a);
        assert!(code(&mut a, k).is_empty());
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(decisions(&a), before);
    }
    let mut a = app();
    a.review.units[0].decision = Decision::Accepted;
    a.review.units[1].decision = Decision::Accepted;
    a.review.units[2].decision = Decision::Investigate;
    shift_a(&mut a);
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(a.status, "nothing pending in this file");
    let mut b = app();
    key(&mut b, 's');
    shift_a(&mut b);
    assert_eq!(b.overlay, Overlay::None);
    let mut c = app();
    key(&mut c, 'R');
    assert_eq!(c.overlay, Overlay::ConfirmReset);
}

#[test]
fn both_keys_can_be_rebound_in_the_config() {
    let cfg: Config =
        toml::from_str("[keys]\nnext_group = \"n\"\naccept_file = \"ctrl+a\"\n").unwrap();
    let keys = cfg.keymap().unwrap();
    let mut a = app_keys(units(), cfg, keys);
    ctrl(&mut a, 'd');
    assert_eq!(a.sel, 0);
    key(&mut a, 'n');
    assert_eq!(symbol(&a), "look_a");
    shift_a(&mut a);
    assert_eq!(a.overlay, Overlay::None);
    ctrl(&mut a, 'a');
    assert!(matches!(a.overlay, Overlay::ConfirmAcceptFile { .. }));
}

// ---- Help, paths -------------------------------------------------------------

#[test]
fn the_keys_are_in_the_help_overlay_and_the_legend_still_fits() {
    for (w, h) in [(60u16, 20u16), (80, 24), (100, 40)] {
        let mut a = app();
        a.overlay = Overlay::Help;
        let out = text(&a, w, h);
        assert!(
            out.contains("^d ^u  next / previous file"),
            "{w}x{h}\n{out}"
        );
        assert!(
            out.contains("accept all pending in the file"),
            "{w}x{h}\n{out}"
        );
        assert!(
            !out.contains("sidebar") && !out.contains("open / close"),
            "{w}x{h}\n{out}"
        );
        assert!(
            out.contains("legend") && out.contains("▲ close look") && out.contains("needs fix"),
            "{w}x{h}\n{out}"
        );
    }
    // The Flagged keys need the extra rows: shown at 26, not at 25.
    let mut a = app();
    a.overlay = Overlay::Help;
    let at26 = text(&a, 80, 26);
    assert!(at26.contains("recommend / visualize / tests"), "{at26}");
    assert!(at26.contains("again, ok / needs fix, browser"), "{at26}");
    assert!(!text(&a, 80, 25).contains("Flagged"));
    // The footer has no file button.
    a.overlay = Overlay::None;
    let out = text(&a, 120, 30);
    let footer = out.lines().last().unwrap();
    assert!(!footer.contains("file"), "{footer}");
}

#[test]
fn a_long_path_is_cut_at_the_front_so_the_file_name_stays() {
    let dir = "application/module/MediaDistribution/Contract/Data";
    let units: Vec<Unit> = (0..4)
        .map(|k| {
            unit(
                &format!("r{k}"),
                &format!("{dir}/Membership0Data.php"),
                10 + 20 * k,
                Kind::Routine,
            )
        })
        .collect();
    let a = app_with(units, Config::default());
    let s = text(&a, 60, 34);
    let label = s.lines().find(|l| l.contains("Data.php")).unwrap();
    assert!(
        label.contains("│ …") && label.contains("/Membership0Data.php"),
        "{label}"
    );
}

// ---- Large reviews, swipe, sizes -------------------------------------------------

#[test]
fn a_large_review_asks_about_a_file_when_the_cursor_gets_to_it() {
    let mut units = units();
    for u in &mut units {
        u.prep = Prep::NotRequested;
    }
    let cfg = Config {
        large_review: 3,
        ..Config::default()
    };
    let mut a = app_with(units, cfg);
    let effects = a.start(true, None, Vec::new(), Some(SessionRef::default()), false);
    // The close looks and the rest of the selected unit's file.
    let mut first = prepared(&effects);
    first.sort();
    assert_eq!(
        first,
        [
            "src/c.rs::c_helper",
            "src/c.rs::close_a",
            "src/c.rs::close_b"
        ]
    );
    // Moving inside the file asks nothing more; into the next one asks
    // about that file.
    assert!(prepared(&key(&mut a, 'j')).is_empty());
    assert!(prepared(&key(&mut a, 'j')).is_empty());
    let mut ids = prepared(&key(&mut a, 'j'));
    ids.sort();
    assert_eq!(ids, ["src/l0.rs::look_a", "src/l0.rs::look_b"]);
}

#[test]
fn swipe_still_shows_the_most_attention_first() {
    let mut units = units();
    // A close look in a routine file.
    units[11].status = UnitStatus::Removed;
    units[11].callers = vec!["somebody".into()];
    let mut a = app_with(units, Config::default());
    key(&mut a, 's');
    let first: Vec<&str> = a.swipe.cards[..3]
        .iter()
        .map(|id| a.review.unit(id).unwrap().symbol.as_str())
        .collect();
    assert_eq!(first, ["close_a", "close_b", "r0_3"]);
}

/// A thousand units in a hundred files: no move panics, and the selected
/// unit is always on screen exactly once.
#[test]
fn a_thousand_units_in_a_hundred_files() {
    let units: Vec<Unit> = (0..1000)
        .map(|i| {
            unit(
                &format!("u{i:04}"),
                &format!("src/f{:02}.rs", i / 10),
                10 + (i % 10) as u32 * 10,
                if i % 7 == 0 {
                    Kind::Look
                } else {
                    Kind::Routine
                },
            )
        })
        .collect();
    let mut a = app_with(units, Config::default());
    let steps: &[&dyn Fn(&mut App)] = &[
        &|a| {
            key(a, 'j');
        },
        &|a| {
            key(a, 'l');
        },
        &|a| {
            ctrl(a, 'd');
        },
        &|a| {
            key(a, 'G');
        },
        &|a| {
            key(a, 'k');
        },
        &|a| {
            ctrl(a, 'u');
        },
        &|a| {
            key(a, 'g');
        },
        &|a| {
            key(a, 'u');
        },
    ];
    for (n, step) in steps.iter().cycle().take(60).enumerate() {
        step(&mut a);
        let name = symbol(&a).to_string();
        for (w, h) in [(80u16, 24u16), (40, 8), (120, 40)] {
            let s = text(&a, w, h);
            let marked: Vec<&str> = s.lines().filter(|l| l.contains('▌')).collect();
            assert_eq!(marked.len(), 1, "step {n} at {w}x{h}\n{s}");
            if w >= 80 {
                assert!(marked[0].contains(&name), "step {n} at {w}x{h}\n{s}");
            }
        }
    }
    render(&a, 3, 3);
    render(&a, 0, 0);
}

#[test]
fn the_selection_stays_visible_with_its_box_title_or_file_name_when_they_fit() {
    for (w, h) in [(60u16, 24u16), (80, 24), (120, 30), (80, 40)] {
        let mut a = app();
        for _ in 0..a.order.len() {
            let s = lines(&a, w, h);
            let name = symbol(&a).to_string();
            let marked: Vec<&String> = s.iter().filter(|l| l.contains('▌')).collect();
            assert_eq!(marked.len(), 1, "{w}x{h} {}\n{}", a.sel, s.join("\n"));
            assert!(marked[0].contains(&name), "{w}x{h} {name}");
            // Stepping into a file shows its name.
            if a.starts_file(a.sel) {
                let path = a.selected().unwrap().file.display().to_string();
                assert!(s.iter().any(|l| l.contains(&path)), "{w}x{h} {path}");
            }
            key(&mut a, 'j');
        }
    }
}

#[test]
fn the_overlay_and_padding_never_panic_at_tiny_sizes() {
    let mut a = app();
    shift_a(&mut a);
    assert!(matches!(a.overlay, Overlay::ConfirmAcceptFile { .. }));
    for (w, h) in [
        (0u16, 0u16),
        (1, 1),
        (3, 3),
        (4, 4),
        (10, 4),
        (20, 6),
        (40, 8),
        (60, 12),
        (200, 3),
    ] {
        for sel in [0, 4, 8] {
            a.sel = sel;
            render(&a, w, h);
        }
    }
}
