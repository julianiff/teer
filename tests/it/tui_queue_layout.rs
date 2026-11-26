//! Full-frame render tests for the queue at several terminal sizes: where
//! the pane with the file's changes sits and what it starts with, the
//! overlays over it, and the mouse on the pane.

use crate::common;

use std::path::PathBuf;
use std::sync::Arc;

use common::*;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use serde_json::{Value, json};
use teer::agent::mock::MockAdapter;
use teer::config::Config;
use teer::keymap::{Action, Keymap};
use teer::review::model::{Decision, Review, SessionRef};
use teer::review::{BuildOptions, build_review};
use teer::session::app::{Effect, Overlay, Screen};
use teer::tui::app::{App, Target};
use teer::tui::preview::Pane;
use teer::tui::ui;

const API: &str = "from checkout import charge_order\n\n\ndef handle_checkout(request):\n    return charge_order(request.order)\n\n\ndef main():\n    handle_checkout(parse())\n";

const SIZES: [(u16, u16); 4] = [(60, 20), (80, 24), (120, 40), (160, 50)];

fn responder(prompt: &str, _schema: &Value) -> anyhow::Result<Value> {
    let mut out = Vec::new();
    let mut id = String::new();
    for line in prompt.lines() {
        if let Some(rest) = line.strip_prefix("### Unit ") {
            id = rest.to_string();
        }
        if line.starts_with("new lines: ") {
            let symbol = id.rsplit("::").next().unwrap();
            out.push(json!({
                "id": id,
                "summary": format!("Summary of {symbol} that is long enough to need truncating on narrow screens."),
                "changes": [],
            }));
        }
    }
    Ok(json!({ "units": out }))
}

fn session() -> SessionRef {
    SessionRef {
        agent: "mock".into(),
        id: "s1".into(),
        cwd: PathBuf::from("/work/shop"),
    }
}

async fn app() -> (App, FixtureRepo) {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD)
        .write("api.py", API)
        .commit("init");
    repo.write("checkout.py", PY_NEW);
    let mut review: Review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    teer::start::prepare_all(
        &mut review,
        Arc::new(MockAdapter::new(responder)),
        session(),
        2,
    )
    .await;
    review.repo_root = PathBuf::from("/work/shop");
    review.base = "a1b2c3d4e5f6".into();
    review.agent_session = Some(session());
    let mut app = App::new(review, Config::default(), Keymap::default());
    app.start(true, None, Vec::new(), Some(session()));
    assert_eq!(app.screen, Screen::Queue);

    app.rebuild_order();
    // The queue's geometry alone: the attack-surface card over it is
    // security_queue.rs's.
    app.security.enabled = false;
    (app, repo)
}

/// Rendered buffer as a grid of cell symbols.
fn grid(app: &App, w: u16, h: u16) -> Vec<Vec<String>> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
        .collect()
}

fn text(g: &[Vec<String>]) -> String {
    g.iter().map(|r| r.concat()).collect::<Vec<_>>().join("\n")
}

/// The tree's rect (x and width) from the click targets of its rows.
fn list_x_w(app: &App) -> (u16, u16) {
    let rows = app.targets.borrow();
    let (r, _) = rows
        .iter()
        .find(|(_, t)| matches!(t, Target::Row(_)))
        .expect("a row");
    (r.x, r.width)
}

/// The pane rect the layout gives a `w` x `h` terminal (header and footer
/// excluded; no banner).
fn pane_rect(app: &App, w: u16, h: u16) -> Option<(Rect, Pane)> {
    teer::tui::preview::layout_of(app, Rect::new(0, 1, w, h - 2)).1
}

/// Where the pane is, read from the rendered frame: a rule down the
/// screen right after the tree, or one across the full width.
fn pane_at(app: &App, g: &[Vec<String>]) -> Option<Pane> {
    let h = g.len();
    let (lx, lw) = list_x_w(app);
    let col = (lx + lw) as usize;
    if col + 1 < g[0].len() && (1..h - 1).all(|y| g[y][col] == "│") {
        return Some(Pane::Side);
    }
    if g.iter().any(|r| r.iter().all(|c| c == "─")) {
        return Some(Pane::Bottom);
    }
    None
}

/// The text of the pane, from the rendered frame.
fn pane_text(app: &App, g: &[Vec<String>], w: u16, h: u16) -> String {
    let (r, _) = pane_rect(app, w, h).unwrap_or_else(|| panic!("no pane at {w}x{h}"));
    (r.y..r.y + r.height)
        .map(|y| g[y as usize][r.x as usize..(r.x + r.width) as usize].concat())
        .collect::<Vec<_>>()
        .join("\n")
}

fn mouse(app: &mut App, col: u16, row: u16) -> Vec<Effect> {
    app.on_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }))
}

fn key(app: &mut App, c: char) -> Vec<Effect> {
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )))
}

/// Jump to unit `symbol`, as the find list does.
fn jump(app: &mut App, symbol: &str) {
    let units = &app.review.units;
    let pos = (app.order.iter().position(|&i| units[i].symbol == symbol)).unwrap();
    app.move_sel(pos);
}

#[tokio::test]
async fn the_pane_is_beside_the_tree_from_110_columns_and_below_it_under_that() {
    for (w, h, want) in [
        (60, 20, Pane::Bottom),
        (80, 24, Pane::Bottom),
        (109, 30, Pane::Bottom),
        (109, 31, Pane::Bottom),
        (80, 32, Pane::Bottom),
        (110, 20, Pane::Side),
        (110, 30, Pane::Side),
        (120, 40, Pane::Side),
        (160, 50, Pane::Side),
    ] {
        let (mut app, _r) = app().await;
        let g = grid(&app, w, h);
        assert_eq!(pane_at(&app, &g), Some(want), "{w}x{h}\n{}", text(&g));
        // The pane sits where the layout says.
        let (r, kind) = pane_rect(&app, w, h).unwrap();
        assert_eq!(kind, want, "{w}x{h}");
        if want == Pane::Bottom {
            assert_eq!(g[r.y as usize].concat(), "─".repeat(w as usize), "{w}x{h}");
            // About 60% of the height, and what the short tree leaves free.
            assert!(r.height >= 5 && r.height > h / 2, "{w}x{h}: {r:?}");
        }
        // No key hides it: `p` is not bound.
        let sel = app.sel;
        assert!(key(&mut app, 'p').is_empty());
        assert_eq!(app.sel, sel);
        assert_eq!(pane_at(&app, &grid(&app, w, h)), Some(want), "{w}x{h}");
    }
}

#[tokio::test]
async fn the_tree_never_spills_into_the_pane() {
    for decision in [Decision::Accepted, Decision::Investigate] {
        for (w, h) in SIZES {
            let (mut app, _r) = app().await;
            for u in app.review.units.iter_mut() {
                u.decision = decision;
            }
            let g = grid(&app, w, h);
            let (lx, lw) = list_x_w(&app);
            let row = g[1][lx as usize..(lx + lw) as usize].concat();
            let want = match decision {
                Decision::Investigate => "▌┃ checkout.py ⚑",
                _ => "▌┃ checkout.py",
            };
            assert_eq!(row.trim_end(), want, "{w}x{h}\n{}", text(&g));
            for glyph in ["✓", "+", "~", "−", "│"] {
                assert!(!row.contains(glyph), "{w}x{h} {glyph} in {row:?}");
            }
            let footer = g.last().unwrap().concat();
            assert!(
                !footer.contains("marked") && !footer.contains("clear"),
                "{w}x{h}: {footer:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_pane_names_each_unit_once_and_not_its_file() {
    for w in [60u16, 80, 120] {
        let (app, _r) = app().await;
        let g = grid(&app, w, 80);
        let pane = pane_text(&app, &g, w, 80);
        for name in ["charge_order", "old_helper", "new_helper", "Cart::add"] {
            let heads = pane
                .lines()
                .filter(|l| l.trim_end().ends_with(&format!(" {name}")))
                .count();
            assert_eq!(heads, 1, "{w}: {name}\n{pane}");
        }
        // The tree names the file, the code its lines.
        assert!(!pane.contains("checkout.py"), "{w}\n{pane}");
    }
}

#[tokio::test]
async fn the_pane_starts_at_the_files_first_unit_or_at_the_unit_a_jump_went_to() {
    for (w, h) in [(120u16, 40u16), (160, 50), (80, 32), (80, 24), (60, 20)] {
        let (mut app, _r) = app().await;
        // The first content row of the pane: under the rule when it is below.
        let top = |app: &App| {
            let pane = pane_text(app, &grid(app, w, h), w, h);
            let skip = usize::from(w < 110);
            pane.lines().nth(skip).unwrap().trim_end().to_string()
        };
        // On the file's row: its first unit, which needs the most attention.
        assert_eq!(app.selected().unwrap().symbol, "charge_order");
        assert!(
            top(&app).ends_with("▌┃ charge_order"),
            "{w}x{h}: {}",
            top(&app)
        );
        // A jump to a unit further down: it is in view, with the bar that
        // says the keys act on it. Where the pane is too short for the
        // whole file, it is scrolled there, under a line that says what
        // is above.
        jump(&mut app, "new_helper");
        let pane = pane_text(&app, &grid(&app, w, h), w, h);
        let lines: Vec<&str> = pane.lines().map(str::trim_end).collect();
        let head = (lines.iter().position(|l| l.ends_with("▌┃ new_helper"))).expect(&pane);
        if h <= 24 {
            let first = usize::from(w < 110);
            assert!(lines[first].contains("lines above"), "{w}x{h}\n{pane}");
            // The scroll keys still move it from there.
            v2::press(&mut app, "<c-u>");
            let pane = pane_text(&app, &grid(&app, w, h), w, h);
            let moved = pane.lines().nth(head).unwrap();
            assert!(!moved.contains("new_helper"), "{w}x{h}\n{pane}");
        }
        // Back from the unit view on a unit that isn't the first: at it.
        jump(&mut app, "old_helper");
        key(&mut app, 'l');
        assert_eq!(app.screen, Screen::Unit);
        grid(&app, w, h);
        key(&mut app, 'h');
        assert_eq!(app.screen, Screen::Queue);
        let pane = pane_text(&app, &grid(&app, w, h), w, h);
        let lines: Vec<&str> = pane.lines().map(str::trim_end).collect();
        assert!(
            lines.iter().any(|l| l.ends_with("▌┃ old_helper")),
            "{w}x{h}\n{pane}"
        );
    }
}

#[tokio::test]
async fn help_lists_the_queue_keys_and_the_legend() {
    for (w, h) in [(80u16, 24u16), (120, 40)] {
        let (mut app, _r) = app().await;
        app.on_action(Action::HelpOpen);
        assert_eq!(app.overlay, Overlay::Help);
        let s = text(&grid(&app, w, h));
        // Every key from the keymap, with what it does.
        for want in [
            // Cut at 80 columns.
            "a f  accept / flag the f",
            "n N  next / previous pen",
            "l/⏎  look closer or open",
        ] {
            assert!(s.contains(want), "{w}x{h} missing {want:?}\n{s}");
        }
        for gone in [
            "mark / unmark row",
            "range select",
            "dive",
            "toggle preview",
            "open / close",
            "list by file",
            // One file and no directory: nothing to fold, no file to go to.
            "fold",
            "next / previous file",
        ] {
            assert!(!s.contains(gone), "{w}x{h} still lists {gone:?}\n{s}");
        }
    }
    // The legend goes under the keys when there is room for it: the
    // levels' icons and what a row can end with.
    let (mut app, _r) = app().await;
    app.on_action(Action::HelpOpen);
    let s = text(&grid(&app, 120, 40));
    for want in [
        "legend  ┃ close look  ┃ worth a look  ┃ routine  ○ accepted for you",
        "⚑ flagged  ⚠ disputed  ↻ changed  ✓ accepted",
    ] {
        assert!(s.contains(want), "120x40 missing {want:?}\n{s}");
    }
    for gone in ["needs fix", "has a note", "pending  accepted"] {
        assert!(!s.contains(gone), "120x40: {gone}\n{s}");
    }
}

#[tokio::test]
async fn help_in_a_60x20_terminal_is_cut_with_a_note_and_its_border() {
    let (mut app, _r) = app().await;
    app.on_action(Action::HelpOpen);
    let s = text(&grid(&app, 60, 20));
    // Too many keys for 60x20: the last row says so, the border is whole.
    assert!(s.contains("… a taller window shows more"), "{s}");
    assert!(s.contains("any key closes ╯"), "{s}");
    assert!(s.contains("j/↓ k/↑  move down / up"), "{s}");
    assert!(!s.contains("toggle preview"), "{s}");
}

#[tokio::test]
async fn confirm_reset_overlay_renders() {
    for (w, h) in [(60u16, 20u16), (80, 24), (120, 40)] {
        let (mut app, _r) = app().await;
        app.on_action(Action::ReviewReset);
        assert_eq!(app.overlay, Overlay::ConfirmReset);
        let s = text(&grid(&app, w, h));
        let ask = format!("Reset all {} decisions? y/n", app.review.units.len());
        assert!(s.contains(&ask), "{w}x{h}\n{s}");
    }
}

#[tokio::test]
async fn clicking_the_pane_does_nothing() {
    for (w, h) in [(120u16, 40u16), (160, 50), (80, 32), (80, 24), (60, 20)] {
        let (mut app, _r) = app().await;
        jump(&mut app, "old_helper");
        let at = app.sel;
        // Saved where the cursor is, as a key would.
        app.review.cursor = app.selected_id();
        let g = grid(&app, w, h);
        let (lx, lw) = list_x_w(&app);
        let pts: Vec<(u16, u16)> = match pane_at(&app, &g).expect("a pane") {
            Pane::Side => (1..h - 1)
                .map(|y| (lx + lw + 2, y))
                .chain([(w - 1, 2), (lx + lw, 1)])
                .collect(),
            Pane::Bottom => {
                let y0 = pane_rect(&app, w, h).unwrap().0.y;
                (y0..h - 1)
                    .map(|y| (5, y))
                    .chain([(w - 1, h - 2)])
                    .collect()
            }
        };
        for (x, y) in pts {
            let effects = mouse(&mut app, x, y);
            assert_eq!(app.sel, at, "{w}x{h} click ({x},{y})");
            assert!(effects.is_empty(), "{w}x{h} ({x},{y}): {effects:?}");
            assert_eq!(app.overlay, Overlay::None);
            grid(&app, w, h); // still renders
        }
    }
}
