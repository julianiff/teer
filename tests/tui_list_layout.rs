//! Full-frame render tests for the Slim list at several terminal sizes:
//! unit rows under their file's name inside their level's box, always-on
//! preview placement, overlays, mouse.

mod common;

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
use teer::config::{Action, Config, KeyMap};
use teer::review::model::{Decision, Review, SessionRef};
use teer::review::{BuildOptions, build_review};
use teer::tui::app::{App, Effect, Overlay, Screen, Target};
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
    teer::cli::prepare_all(
        &mut review,
        Arc::new(MockAdapter::new(responder)),
        session(),
        2,
    )
    .await;
    review.repo_root = PathBuf::from("/work/shop");
    review.base = "a1b2c3d4e5f6".into();
    review.agent_session = Some(session());
    let mut app = App::new(review, Config::default(), KeyMap::default());
    app.start(true, None, Vec::new(), Some(session()), false);
    assert_eq!(app.screen, Screen::Slim);
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

/// The list rect (x, width, and the rows used) from the click targets.
fn list_x_w(app: &App) -> (u16, u16) {
    let r = unit_rects(app);
    (r[0].0.x, r[0].0.width)
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Where {
    Side,
    Bottom,
    Absent,
}

/// The pane rect the layout gives a `w` x `h` terminal (header and footer
/// excluded; no banner).
/// Where the preview pane is: a pane showing code is taller at the bottom.
fn pane_rect(app: &App, w: u16, h: u16) -> Option<(Rect, teer::tui::preview::Pane)> {
    teer::tui::preview::layout_of(app, Rect::new(0, 1, w, h - 2)).1
}

/// Where the preview is, read from the rendered frame: a blank column next
/// to the list with the preview beyond it, or a horizontal rule across the
/// full width.
fn preview_at(app: &App, g: &[Vec<String>]) -> Where {
    let h = g.len();
    let w = g[0].len();
    let (lx, lw) = list_x_w(app);
    let col = (lx + lw) as usize;
    if col + 1 < w
        && (1..h - 1).all(|y| g[y][col] == " ")
        && (1..h - 1).any(|y| g[y][col + 1..].iter().any(|c| c != " "))
    {
        return Where::Side;
    }
    if g.iter().any(|r| r.iter().all(|c| c == "─")) {
        return Where::Bottom;
    }
    Where::Absent
}

/// The text of the preview pane, from the rendered frame.
fn preview_text(app: &App, g: &[Vec<String>], w: u16, h: u16) -> String {
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

fn select(app: &mut App, symbol: &str) {
    app.sel = app
        .order
        .iter()
        .position(|&i| app.review.units[i].symbol == symbol)
        .unwrap();
}

fn decide_all(app: &mut App, d: Decision) {
    for u in app.review.units.iter_mut() {
        u.decision = d;
    }
}

#[tokio::test]
async fn every_unit_row_is_boxed_with_its_icon_and_name_and_no_badge_or_status_glyph() {
    for decision in [Decision::Accepted, Decision::Investigate] {
        for (w, h) in SIZES {
            let (mut app, _r) = app().await;
            decide_all(&mut app, decision);
            for sel in 0..app.order.len() {
                app.sel = sel;
                let g = grid(&app, w, h);
                let rects = unit_rects(&app);
                assert!(!rects.is_empty());
                for (r, _) in &rects {
                    let cells = &g[r.y as usize][r.x as usize..(r.x + r.width) as usize];
                    // Inside the box: the selection gutter, two blanks, then
                    // the level's icon.
                    assert_eq!(cells[0], "│", "{w}x{h} sel {sel}\n{}", text(&g));
                    assert_eq!(cells[cells.len() - 1], "│", "{w}x{h} sel {sel}");
                    assert!(
                        ["▌", " "].contains(&cells[1].as_str()),
                        "{w}x{h} sel {sel}\n{}",
                        text(&g)
                    );
                    assert_eq!(cells[2..4].concat(), "  ", "{w}x{h} sel {sel}");
                    assert!(
                        ["▲", "◆", "●"].contains(&cells[4].as_str()),
                        "{w}x{h} sel {sel}"
                    );
                    let inner = cells.concat();
                    for glyph in ["✓", "⚑", "+", "~", "−"] {
                        assert!(!inner.contains(glyph), "{w}x{h} {glyph} in {inner:?}");
                    }
                }
                // Nothing of the list spills into the cell right of it when there is a pane.
                let (lx, lw) = list_x_w(&app);
                if lx + lw < w {
                    let next = (lx + lw) as usize;
                    for (r, _) in &rects {
                        let c = &g[r.y as usize][next];
                        assert_eq!(c, " ", "{w}x{h} spill\n{}", text(&g));
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn preview_placement_follows_the_breakpoints() {
    for (w, h, want) in [
        (60, 20, Where::Bottom),
        (80, 24, Where::Bottom),
        (109, 30, Where::Bottom),
        (109, 31, Where::Bottom),
        (80, 32, Where::Bottom),
        (110, 20, Where::Side),
        (110, 30, Where::Side),
        (120, 40, Where::Side),
        (160, 50, Where::Side),
    ] {
        let (app, _r) = app().await;
        let g = grid(&app, w, h);
        assert_eq!(preview_at(&app, &g), want, "{w}x{h}\n{}", text(&g));
        // The list gets its rows: the pane sits where the layout says.
        let (r, _) = pane_rect(&app, w, h).unwrap();
        if want == Where::Bottom {
            assert_eq!(g[r.y as usize].concat(), "─".repeat(w as usize), "{w}x{h}");
            // About 60% of the height for a unit with code (every change now).
            assert!(r.height >= 5 && r.height <= 24, "{w}x{h}: {r:?}");
        }
    }
}

#[tokio::test]
async fn p_no_longer_toggles_anything() {
    for (w, h, on) in [(120, 40, Where::Side), (80, 24, Where::Bottom)] {
        let (mut app, _r) = app().await;
        assert_eq!(preview_at(&app, &grid(&app, w, h)), on);
        let sel = app.sel;
        assert!(key(&mut app, 'p').is_empty());
        assert_eq!(app.sel, sel);
        let g = grid(&app, w, h);
        assert_eq!(preview_at(&app, &g), on, "{w}x{h}\n{}", text(&g));
    }
}

#[tokio::test]
async fn rows_show_the_name_and_the_preview_where_it_is() {
    for w in [20u16, 60, 80, 120] {
        let (mut app, _r) = app().await;
        select(&mut app, "new_helper");
        // Judge a row that isn't selected.
        let g = grid(&app, w, 20);
        let other = app
            .order
            .iter()
            .position(|&i| app.review.units[i].symbol == "charge_order")
            .unwrap();
        let r = unit_rects(&app)
            .iter()
            .find(|(_, p)| *p == other)
            .unwrap()
            .0;
        let line = g[r.y as usize][r.x as usize..(r.x + r.width) as usize].concat();
        // At 20 columns the name may be cut.
        let name = if w == 20 { "cha" } else { "charge_order" };
        assert!(line.contains(name), "{w}: {line}");
        assert!(!line.contains("checkout"), "{w}: {line}");
    }
    // The selected unit's file and lines are in the preview.
    let (mut app, _r) = app().await;
    select(&mut app, "charge_order");
    let all: String = grid(&app, 120, 30).iter().map(|l| l.concat()).collect();
    assert!(all.contains("charge_order  checkout.py:8-12"), "{all}");
}

#[tokio::test]
async fn preview_shows_the_selected_symbol_and_follows_selection() {
    for (w, h) in [(120u16, 40u16), (160, 50), (80, 32), (80, 24), (60, 20)] {
        let (mut app, _r) = app().await;
        let region = |app: &App| preview_text(app, &grid(app, w, h), w, h);
        select(&mut app, "new_helper");
        let a = region(&app);
        assert!(
            a.contains(" new_helper ") && !a.contains(" charge_order "),
            "{w}x{h}\n{a}"
        );
        select(&mut app, "charge_order");
        let b = region(&app);
        assert!(
            b.contains(" charge_order ") && !b.contains(" new_helper "),
            "{w}x{h}\n{b}"
        );
        // The reasons come first, right under the head.
        let lines: Vec<&str> = b.lines().collect();
        let head = lines
            .iter()
            .position(|l| l.contains(" charge_order "))
            .unwrap();
        assert!(lines[head + 1].contains(" callers"), "{w}x{h}\n{b}");
        // Moving with the keyboard updates it too: `g` is the first unit,
        // `j` the next.
        key(&mut app, 'g');
        assert_eq!(app.sel, 0);
        let first = app.selected().unwrap().symbol.clone();
        assert!(region(&app).contains(&format!(" {first} ")), "{w}x{h}");
        key(&mut app, 'j');
        let second = app.selected().unwrap().symbol.clone();
        assert_ne!(first, second);
        assert!(region(&app).contains(&format!(" {second} ")), "{w}x{h}");
    }
}

#[tokio::test]
async fn rows_start_with_border_and_gutter_and_the_footer_has_no_marks() {
    for (w, h) in SIZES {
        let (app, _r) = app().await;
        let g = grid(&app, w, h);
        let rows = unit_rects(&app);
        assert!(!rows.is_empty(), "{w}x{h}");
        for (r, pos) in &rows {
            let border = g[r.y as usize][r.x as usize].as_str();
            assert_eq!(border, "│", "{w}x{h} row {pos}");
            let gutter = g[r.y as usize][r.x as usize + 1].as_str();
            assert!(
                ["▌", " "].contains(&gutter),
                "{w}x{h} row {pos}: {gutter:?}"
            );
        }
        let footer = g.last().unwrap().concat();
        assert!(
            !footer.contains("marked") && !footer.contains("clear"),
            "{w}x{h}: {footer:?}"
        );
    }
}

#[tokio::test]
async fn help_lists_the_list_keys_and_the_style_legend() {
    for (w, h) in [(80u16, 24u16), (120, 40)] {
        let (mut app, _r) = app().await;
        app.on_action(Action::Help);
        assert_eq!(app.overlay, Overlay::Help);
        let s = text(&grid(&app, w, h));
        for want in [
            "reset all decisions",
            "accept / investigate",
            "next / previous pending",
            "next / previous file",
            "legend",
            "accepted",
            "removed",
            "flagged",
            "needs fix",
        ] {
            assert!(s.contains(want), "{w}x{h} missing {want:?}\n{s}");
        }
        for gone in [
            "mark / unmark row",
            "range select",
            "dive",
            "toggle preview",
            "open / close",
        ] {
            assert!(!s.contains(gone), "{w}x{h} still lists {gone:?}\n{s}");
        }
    }
}

#[tokio::test]
async fn help_fits_in_a_60x20_terminal() {
    let (mut app, _r) = app().await;
    app.on_action(Action::Help);
    let s = text(&grid(&app, 60, 20));
    // Last legend row and the border must not be cut off.
    assert!(s.contains("✗ needs fix"), "legend clipped at 60x20\n{s}");
    assert!(!s.contains("toggle preview"), "{s}");
}

#[tokio::test]
async fn confirm_reset_overlay_renders() {
    for (w, h) in [(60u16, 20u16), (80, 24), (120, 40)] {
        let (mut app, _r) = app().await;
        app.on_action(Action::Reset);
        assert_eq!(app.overlay, Overlay::ConfirmReset);
        let s = text(&grid(&app, w, h));
        assert!(s.contains("Reset the review?"), "{w}x{h}\n{s}");
    }
}

#[tokio::test]
async fn clicking_a_row_in_the_list_selects_it() {
    for (w, h) in SIZES {
        let (mut app, _r) = app().await;
        app.sel = 0;
        let g = grid(&app, w, h);
        let _ = g;
        let target = app.order.len() - 1;
        let r = unit_rects(&app)
            .iter()
            .find(|(_, p)| *p == target)
            .unwrap()
            .0;
        let effects = mouse(&mut app, r.x + 3, r.y);
        assert_eq!(app.sel, target, "{w}x{h}");
        assert!(!effects.iter().any(|e| matches!(e, Effect::Quit)));
        // Rightmost cell of the list row is part of the row too.
        app.sel = 0;
        grid(&app, w, h);
        let r = unit_rects(&app)
            .iter()
            .find(|(_, p)| *p == target)
            .unwrap()
            .0;
        mouse(&mut app, r.x + r.width - 1, r.y);
        assert_eq!(app.sel, target, "{w}x{h} right edge");
    }
}

#[tokio::test]
async fn clicking_the_preview_pane_does_nothing() {
    for (w, h) in [(120u16, 40u16), (160, 50), (80, 32), (80, 24), (60, 20)] {
        let (mut app, _r) = app().await;
        app.sel = 1;
        let g = grid(&app, w, h);
        let at = preview_at(&app, &g);
        let (lx, lw) = list_x_w(&app);
        let pts: Vec<(u16, u16)> = match at {
            Where::Side => (1..h - 1)
                .map(|y| (lx + lw + 2, y))
                .chain([(w - 1, 2), (lx + lw, 1)])
                .collect(),
            Where::Bottom => {
                let y0 = pane_rect(&app, w, h).unwrap().0.y;
                (y0..h - 1)
                    .map(|y| (5, y))
                    .chain([(w - 1, h - 2)])
                    .collect()
            }
            Where::Absent => panic!("no preview at {w}x{h}"),
        };
        for (x, y) in pts {
            let effects = mouse(&mut app, x, y);
            assert_eq!(app.sel, 1, "{w}x{h} click ({x},{y})");
            assert!(effects.is_empty(), "{w}x{h} ({x},{y}): {effects:?}");
            assert_eq!(app.overlay, Overlay::None);
            grid(&app, w, h); // still renders
        }
    }
}
