//! The mouse: a drag selects text inside the pane it starts in and copies
//! it, a double click copies a word, and the keys an overlay names can be
//! clicked.

mod common;

use common::*;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use teer::config::{Config, KeyMap};
use teer::review::model::Decision;
use teer::review::{BuildOptions, build_review};
use teer::tui::app::{App, Effect, Overlay, Screen};
use teer::tui::ui;

const W: u16 = 100;
const H: u16 = 24;

/// `total` and `fee` changed, no agent; the cursor on `total`, whose code
/// change shows in the preview.
fn app() -> (App, FixtureRepo) {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write(
        "calc.py",
        "def total(price, qty):\n    return price * qty\n\n\ndef fee(x):\n    return x + 1\n",
    )
    .commit("init");
    repo.write(
        "calc.py",
        "def total(price, qty):\n    subtotal = price * qty\n    return subtotal + fee(subtotal)\n\n\ndef fee(x):\n    return x + 2\n",
    );
    let review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let mut app = App::new(review, Config::default(), KeyMap::default());
    app.start(false, None, Vec::new(), None, false);
    assert_eq!(app.selected().unwrap().symbol, "total");
    (app, repo)
}

/// Draw the app, as the runtime does before each event; the rows, and
/// whether each cell is drawn reversed.
fn draw(app: &App) -> (Vec<String>, Vec<Vec<bool>>) {
    let mut t = Terminal::new(TestBackend::new(W, H)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    let rows = (0..H)
        .map(|y| (0..W).map(|x| buf[(x, y)].symbol()).collect())
        .collect();
    let rev = (0..H)
        .map(|y| {
            (0..W)
                .map(|x| buf[(x, y)].modifier.contains(Modifier::REVERSED))
                .collect()
        })
        .collect();
    (rows, rev)
}

/// Where `text` is drawn first: its column and row.
fn at(rows: &[String], text: &str) -> (u16, u16) {
    for (y, r) in rows.iter().enumerate() {
        if let Some(i) = r.find(text) {
            return (r[..i].chars().count() as u16, y as u16);
        }
    }
    panic!("{text:?} is not on the screen:\n{}", rows.join("\n"))
}

fn mouse(app: &mut App, kind: MouseEventKind, (column, row): (u16, u16)) -> Vec<Effect> {
    app.on_event(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }))
}

fn drag(app: &mut App, from: (u16, u16), to: (u16, u16)) -> Vec<Effect> {
    draw(app);
    mouse(app, MouseEventKind::Down(MouseButton::Left), from);
    mouse(app, MouseEventKind::Drag(MouseButton::Left), to);
    draw(app);
    mouse(app, MouseEventKind::Up(MouseButton::Left), to)
}

fn click(app: &mut App, at: (u16, u16)) -> Vec<Effect> {
    draw(app);
    mouse(app, MouseEventKind::Down(MouseButton::Left), at);
    mouse(app, MouseEventKind::Up(MouseButton::Left), at)
}

fn key(app: &mut App, code: KeyCode) -> Vec<Effect> {
    app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

/// The text the effects put on the clipboard, and how the status names it.
fn copied(effects: &[Effect]) -> Option<(String, String)> {
    effects.iter().find_map(|e| match e {
        Effect::Copy { text, what } => Some((text.clone(), what.clone())),
        _ => None,
    })
}

#[test]
fn a_drag_over_the_code_change_copies_the_code_without_numbers_or_signs() {
    let (mut a, _repo) = app();
    let (rows, _) = draw(&a);
    let (_, y) = at(&rows, "subtotal = price");
    let (end, last) = at(&rows, "fee(subtotal)");
    // From the line numbers to the end of the next row.
    let e = drag(&mut a, (3, y), (end + 12, last));
    let (text, what) = copied(&e).expect("copied");
    assert_eq!(
        text,
        "    subtotal = price * qty\n    return subtotal + fee(subtotal)"
    );
    assert_eq!(what, "2 lines");
}

#[test]
fn a_drag_stays_in_the_pane_it_started_in() {
    let (mut a, _repo) = app();
    let (rows, _) = draw(&a);
    let from = at(&rows, "calc.py");
    // Down into the preview and the footer: the list's rows only.
    let e = drag(&mut a, from, (60, H - 1));
    let (text, _) = copied(&e).expect("copied");
    assert!(text.starts_with("calc.py"), "{text}");
    assert!(text.contains("total") && text.contains("fee"), "{text}");
    assert!(!text.contains("code change"), "{text}");
    assert!(!text.contains("open in editor"), "{text}");
    // Without the boxes' left and right edges.
    assert!(
        !text.lines().any(|l| l.starts_with('│') || l.ends_with('│')),
        "{text}"
    );
}

#[test]
fn the_copied_text_stays_marked_until_the_next_key() {
    let (mut a, _repo) = app();
    let (rows, _) = draw(&a);
    let (x, y) = at(&rows, "subtotal = price");
    copied(&drag(&mut a, (x, y), (x + 7, y))).expect("copied");
    let (_, rev) = draw(&a);
    assert!((x..=x + 7).all(|c| rev[y as usize][c as usize]));
    assert!(!rev[y as usize][(x + 8) as usize]);
    key(&mut a, KeyCode::Down);
    let (_, rev) = draw(&a);
    assert!(!rev[y as usize][x as usize]);
}

#[test]
fn a_double_click_copies_the_word_under_the_pointer() {
    let (mut a, _repo) = app();
    let (rows, _) = draw(&a);
    let (x, y) = at(&rows, "subtotal + fee");
    assert_eq!(
        copied(&click(&mut a, (x + 2, y))),
        None,
        "one click copies nothing"
    );
    let (text, what) = copied(&click(&mut a, (x + 2, y))).expect("copied");
    assert_eq!(text, "subtotal");
    assert_eq!(what, "“subtotal”");
    // A path with its line numbers is one word.
    let (x, y) = at(&rows, "calc.py:1-3");
    click(&mut a, (x, y));
    assert_eq!(copied(&click(&mut a, (x, y))).unwrap().0, "calc.py:1-3");
}

#[test]
fn a_click_without_a_drag_still_selects_a_row() {
    let (mut a, _repo) = app();
    let (rows, _) = draw(&a);
    let e = click(&mut a, at(&rows, "fee"));
    assert_eq!(copied(&e), None);
    assert_eq!(a.selected().unwrap().symbol, "fee");
}

#[test]
fn a_double_click_on_a_button_presses_it_twice_and_copies_nothing() {
    let (mut a, _repo) = app();
    let (rows, _) = draw(&a);
    let help = at(&rows, "? help");
    click(&mut a, help);
    assert_eq!(a.overlay, Overlay::Help);
    // The help covers the screen: a click anywhere closes it.
    let e = click(&mut a, help);
    assert_eq!(copied(&e), None);
    assert_eq!(a.overlay, Overlay::None);
}

#[test]
fn keys_named_in_an_overlay_can_be_clicked() {
    let (mut a, _repo) = app();
    key(&mut a, KeyCode::Char('R'));
    assert_eq!(a.overlay, Overlay::ConfirmReset);
    let (rows, _) = draw(&a);
    click(&mut a, at(&rows, "esc cancel"));
    assert_eq!(a.overlay, Overlay::None);

    // `A` asks before accepting the file; its `y` is a button.
    key(&mut a, KeyCode::Char('A'));
    assert!(matches!(a.overlay, Overlay::ConfirmAcceptFile { .. }));
    let (rows, _) = draw(&a);
    let (x, y) = at(&rows, "(y/n)");
    click(&mut a, (x + 1, y));
    assert!(
        a.review
            .units
            .iter()
            .all(|u| u.decision == Decision::Accepted)
    );
    assert_eq!(a.overlay, Overlay::AllDone);
}

#[test]
fn notes_can_be_picked_and_closed_with_the_mouse() {
    let (mut a, _repo) = app();
    for u in a.review.units.iter_mut() {
        u.dive.note = format!("look at {}", u.symbol);
    }
    key(&mut a, KeyCode::Char('N'));
    assert_eq!(a.overlay, Overlay::Notes { sel: 0 });
    let (rows, _) = draw(&a);
    click(&mut a, at(&rows, "look at fee"));
    assert_eq!(a.overlay, Overlay::Notes { sel: 1 });
    let (rows, _) = draw(&a);
    let e = click(&mut a, at(&rows, "y copy all"));
    assert_eq!(copied(&e).unwrap().1, "2 notes as Markdown");
    let (rows, _) = draw(&a);
    click(&mut a, at(&rows, "esc close"));
    assert_eq!(a.overlay, Overlay::None);
}

#[test]
fn on_the_swipe_card_a_sideways_drag_swipes_and_a_downward_one_copies() {
    let (mut a, _repo) = app();
    key(&mut a, KeyCode::Char('s'));
    assert_eq!(a.screen, Screen::Swipe);
    let first = a.swipe_current().cloned().unwrap();
    let (rows, _) = draw(&a);
    let (x, y) = at(&rows, "subtotal = price");
    let e = drag(&mut a, (x, y), (x, y + 1));
    let (text, _) = copied(&e).expect("copied");
    assert!(text.starts_with("subtotal = price * qty\n"), "{text}");
    assert_eq!(a.review.unit(&first).unwrap().decision, Decision::Pending);

    // Sideways but short of a swipe: nothing.
    let e = drag(&mut a, (x, y), (x + 5, y));
    assert_eq!(copied(&e), None);
    assert_eq!(a.review.unit(&first).unwrap().decision, Decision::Pending);

    let e = drag(&mut a, (x, y), (x + 60, y));
    assert_eq!(copied(&e), None);
    assert_eq!(a.review.unit(&first).unwrap().decision, Decision::Accepted);
}
