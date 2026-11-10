//! Finishing from the queue: there is no "all reviewed" popup. Once every
//! unit is accepted the bottom bar says `:w` finishes, and `:w` opens the
//! sheet (see `tui::finish`).

use crate::common;

use std::path::PathBuf;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use teer::config::Config;
use teer::keymap::Keymap;
use teer::review::model::{Decision, Prep, Range, Review, Unit, UnitKind, UnitStatus};
use teer::session::app::{Effect, Overlay, Screen};
use teer::tui::app::App;
use teer::tui::ui;

/// Unit `i` lands in a group by `i % 4`: routine (0), worth a look (1),
/// close look (2), no behavior change (3). With 12 units the list is three
/// boxes of three: positions 0-2 close look, 3-5 worth a look, 6-8 routine.
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
        summary: Some("summary".into()),
        nbc_reason: None,
        dive: Default::default(),
        accepted_by: None,
        reasons: Vec::new(),
        dive_pending: Default::default(),
        checks: Default::default(),
        security: Vec::new(),
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

fn app_with(n: usize, cfg: Config, keys: Keymap) -> App {
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
    };
    App::new(review, cfg, keys)
}

fn app(n: usize) -> App {
    app_with(n, Config::default(), Keymap::default())
}

fn draw(app: &App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    t.backend().buffer().clone()
}

fn rows(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

fn text(app: &App, w: u16, h: u16) -> String {
    rows(&draw(app, w, h)).join("\n")
}

fn press(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Vec<Effect> {
    app.on_event(Event::Key(KeyEvent::new(code, mods)))
}

fn key(app: &mut App, c: char) -> Vec<Effect> {
    press(app, KeyCode::Char(c), KeyModifiers::NONE)
}

/// Type `k` in Neovim notation (`:e!<cr>`).
fn keys(app: &mut App, k: &str) -> Vec<Effect> {
    common::v2::press(app, k)
}

fn shift_a(app: &mut App) -> Vec<Effect> {
    press(app, KeyCode::Char('A'), KeyModifiers::SHIFT)
}

fn decisions(a: &App) -> Vec<Decision> {
    a.review.units.iter().map(|u| u.decision).collect()
}

fn mouse_up(a: &mut App) -> Vec<Effect> {
    a.on_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 5,
        row: 5,
        modifiers: KeyModifiers::NONE,
    }))
}

/// Three units that need a look (no behavior-change one), in the queue.
fn three() -> App {
    app(3)
}

/// Units 0..=n-1 accepted except the last, which is pending.
fn all_but_last() -> App {
    let mut a = three();
    for u in a.review.units.iter_mut().take(2) {
        u.decision = Decision::Accepted;
    }
    a
}

/// Accept the one pending unit through the queue.
fn accept_last(a: &mut App) {
    let pos = a
        .order
        .iter()
        .position(|&i| a.review.units[i].decision == Decision::Pending)
        .unwrap();
    a.sel = pos;
    key(a, 'a');
}

/// The bottom row of the screen at 90x24.
fn bar(a: &App) -> String {
    rows(&draw(a, 90, 24)).pop().unwrap_or_default()
}

/// The review is done: every unit accepted, no popup, and the bottom bar
/// says how to finish.
fn says_done(a: &App) -> bool {
    a.overlay == Overlay::None
        && a.all_reviewed()
        && bar(a).contains("Nothing pending. :w to finish")
}

// ---- Getting there ------------------------------------------------------

#[test]
fn accepting_the_last_pending_unit_in_the_queue_says_how_to_finish() {
    let mut a = three();
    for _ in 0..2 {
        key(&mut a, 'a');
        assert!(!says_done(&a));
    }
    key(&mut a, 'a');
    assert!(says_done(&a), "{:?} {}", a.overlay, a.status);
}

#[test]
fn a_no_behavior_change_unit_does_not_hold_it_back() {
    let mut a = app(4);
    // It is accepted for you.
    assert_eq!(a.review.units[3].decision, Decision::Accepted);
    assert!(a.review.units[3].auto_accepted());
    for u in a.review.units.iter_mut().take(2) {
        u.decision = Decision::Accepted;
    }
    accept_last(&mut a);
    assert!(says_done(&a), "{}", a.status);
}

#[test]
fn not_while_a_unit_is_flagged_and_never_a_sheet_at_launch() {
    let mut a = all_but_last();
    a.review.units[0].decision = Decision::Investigate;
    accept_last(&mut a);
    assert!(!says_done(&a));
    // Nothing is pending, but the bar counts the flagged unit.
    assert!(bar(&a).contains("1 flagged"), "{}", bar(&a));
    // Accepting the flagged one too finishes it.
    a.sel = a
        .order
        .iter()
        .position(|&i| a.review.units[i].decision == Decision::Investigate)
        .unwrap();
    key(&mut a, 'a');
    assert!(says_done(&a), "{}", bar(&a));
    // A review accepted before it was opened opens on the queue, no sheet.
    let mut b = three();
    for u in b.review.units.iter_mut() {
        u.decision = Decision::Accepted;
    }
    b.start(false, None, Vec::new(), None);
    assert_eq!(b.screen, Screen::Queue);
    assert_eq!(b.overlay, Overlay::None);
}

#[test]
fn undo_and_reset_never_say_it() {
    let mut a = all_but_last();
    accept_last(&mut a);
    assert!(says_done(&a));
    key(&mut a, 'u');
    assert!(!says_done(&a));
    assert!(!a.all_reviewed());
    // Accepted again: said again.
    accept_last(&mut a);
    assert!(says_done(&a));
    keys(&mut a, ":e!<cr>");
    key(&mut a, 'y');
    assert!(!says_done(&a));
    assert!(
        a.review
            .units
            .iter()
            .all(|u| u.decision == Decision::Pending)
    );
}

#[test]
fn a_rebound_finish_key_is_the_one_named() {
    let toml = r#"
[keys]
"finish.open" = ":done"
"decide.accept" = "e"
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    let (keys_, _) = cfg.keymap().unwrap();
    let mut a = app_with(3, cfg, keys_);
    for u in a.review.units.iter_mut().take(2) {
        u.decision = Decision::Accepted;
    }
    a.sel = a
        .order
        .iter()
        .position(|&i| a.review.units[i].decision == Decision::Pending)
        .unwrap();
    key(&mut a, 'e');
    assert!(
        bar(&a).contains("Nothing pending. :done to finish"),
        "{}",
        bar(&a)
    );
    keys(&mut a, ":done<cr>");
    assert!(matches!(a.overlay, Overlay::Finish { .. }));
}

// ---- The sheet from the queue -------------------------------------------

#[test]
fn colon_w_opens_the_sheet_from_the_queue() {
    let mut a = all_but_last();
    accept_last(&mut a);
    keys(&mut a, ":w<cr>");
    assert!(matches!(a.overlay, Overlay::Finish { .. }));
    // A branch, everything accepted: done.
    assert_eq!(a.finish_step(), Some(teer::session::finish::Step::Done));
    let t = text(&a, 90, 24);
    assert!(t.contains("All 3 units accepted."), "{t}");
    // `q` and esc close it and quit nothing; ctrl+c still quits.
    assert!(key(&mut a, 'q').is_empty());
    assert_eq!(a.overlay, Overlay::None);
    keys(&mut a, ":w<cr>");
    assert!(press(&mut a, KeyCode::Esc, KeyModifiers::NONE).is_empty());
    assert_eq!(a.overlay, Overlay::None);
    keys(&mut a, ":w<cr>");
    assert_eq!(
        press(&mut a, KeyCode::Char('c'), KeyModifiers::CONTROL),
        vec![Effect::Save, Effect::Quit]
    );
}

#[test]
fn keys_in_the_sheet_never_decide_or_move_the_list() {
    for c in ['l', 's', 'u', 'a', 'f', 'x'] {
        let mut a = all_but_last();
        accept_last(&mut a);
        keys(&mut a, ":w<cr>");
        let (sel, before) = (a.sel, decisions(&a));
        let n = a.review.history.undo.len();
        key(&mut a, c);
        assert_eq!(a.sel, sel, "{c}");
        assert_eq!(decisions(&a), before, "{c}");
        assert_eq!(a.review.history.undo.len(), n, "{c} must not undo");
        assert_eq!(a.screen, Screen::Queue, "{c}");
    }
    // A click outside its keys does nothing either.
    let mut a = all_but_last();
    accept_last(&mut a);
    keys(&mut a, ":w<cr>");
    draw(&a, 80, 24);
    assert!(mouse_up(&mut a).is_empty());
    assert!(a.all_reviewed());
    // `A` on what is left accepts it at once and finishes.
    let mut b = all_but_last();
    b.sel = b
        .order
        .iter()
        .position(|&i| b.review.units[i].decision == Decision::Pending)
        .unwrap();
    shift_a(&mut b);
    assert!(says_done(&b), "{}", bar(&b));
}

#[test]
fn the_sheet_renders_inside_the_screen() {
    let mut a = all_but_last();
    accept_last(&mut a);
    keys(&mut a, ":w<cr>");
    for (w, h) in [(60u16, 20u16), (80, 24), (120, 40)] {
        let r = rows(&draw(&a, w, h));
        let joined = r.join("\n");
        assert!(joined.contains("Finish"), "{w}x{h}\n{joined}");
        assert!(joined.contains('╭') && joined.contains('╯'));
        assert_eq!(r.len(), h as usize);
        assert!(r.iter().all(|l| l.chars().count() == w as usize));
    }
    for (w, h) in [(0u16, 0u16), (1, 1), (10, 3), (3, 10), (20, 6), (200, 3)] {
        draw(&a, w, h);
    }
}
