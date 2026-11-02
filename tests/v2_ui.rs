//! v2 keys on screen: the bottom bar while a sequence waits, while a command
//! is typed and in visual mode; the help built from the keymap for each
//! screen; the jump list; sheets and prompts naming their keys; hints a
//! click runs.

mod common;

use std::collections::HashMap;
use std::time::Instant;

use common::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;
use teer::config::Config;
use teer::keymap::{Action, KeySpec, Keymap, Outcome, parse_keys};
use teer::review::model::{Decision, Review};
use teer::review::{BuildOptions, build_review};
use teer::tui::app::{App, DraftState, JumpKind, Overlay, Screen, Target};
use teer::tui::ui;

/// A review of the Python fixture's uncommitted changes, not prepared.
fn review() -> (Review, FixtureRepo) {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD).commit("init");
    repo.write("checkout.py", PY_NEW);
    let review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    (review, repo)
}

fn app_with(keys: Keymap) -> (App, FixtureRepo) {
    let (review, repo) = review();
    let mut app = App::new(review, Config::default(), keys);
    // The v1 list's drawing (`--swipe`); the queue's is tests/v2_queue.rs.
    app.open_v1_list();
    assert!(app.order.len() >= 2, "the fixture lists units");
    (app, repo)
}

fn app() -> (App, FixtureRepo) {
    app_with(Keymap::default())
}

fn rebound(over: &[(&str, &str)]) -> Keymap {
    let over: HashMap<String, KeySpec> = over
        .iter()
        .map(|(id, k)| (id.to_string(), KeySpec::One(k.to_string())))
        .collect();
    Keymap::new(&over, 1000).unwrap().0
}

fn draw(app: &App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    t.backend().buffer().clone()
}

fn rows(buf: &Buffer) -> Vec<String> {
    let a = buf.area;
    (0..a.height)
        .map(|y| (0..a.width).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

fn text(app: &App, w: u16, h: u16) -> String {
    rows(&draw(app, w, h)).join("\n")
}

/// The bottom bar as drawn at `w` cells.
fn bar(app: &App, w: u16) -> String {
    rows(&draw(app, w, 30)).pop().unwrap()
}

/// Feed `keys` (Neovim notation) to the app's keymap, as normal mode does;
/// what the last one did.
fn feed(app: &mut App, keys: &str) -> Outcome {
    let now = Instant::now();
    let mut last = Outcome::None;
    for k in parse_keys(keys).unwrap() {
        last = app.keys.feed(&k.event(), now);
    }
    last
}

fn key(s: &str) -> Target {
    Target::Key(parse_keys(s).unwrap()[0].event())
}

fn targets(app: &App) -> Vec<Target> {
    app.targets.borrow().iter().map(|(_, t)| *t).collect()
}

// ---- The bottom bar ----------------------------------------------------

#[test]
fn a_prefix_shows_each_key_that_can_follow() {
    let (mut app, _repo) = app();
    assert_eq!(feed(&mut app, "g"), Outcome::Pending);
    let line = bar(&app, 200);
    assert!(
        line.starts_with(
            " g  g first unit · d what this unit calls · r what calls this unit · \
             f open in the editor · x open the picture in the browser · t next tile"
        ),
        "{line}"
    );
    // Cut to a narrow terminal.
    let narrow = bar(&app, 50);
    assert!(narrow.starts_with(" g  g first unit · d what"), "{narrow}");
    assert!(narrow.trim_end().ends_with('…'), "{narrow}");

    let (mut app, _repo) = self::app();
    feed(&mut app, "<space>");
    let line = bar(&app, 200);
    assert!(
        line.contains("␣  t tests that cover this unit · r run old and new"),
        "{line}"
    );
}

#[test]
fn a_count_shows_while_it_waits() {
    let (mut app, _repo) = app();
    feed(&mut app, "3");
    assert!(bar(&app, 80).starts_with(" 3 "), "{}", bar(&app, 80));
    feed(&mut app, "]");
    let line = bar(&app, 120);
    assert!(line.starts_with(" 3]  c next hunk · f next file"), "{line}");
}

#[test]
fn the_command_line_shows_what_is_typed_and_where_it_goes() {
    let (mut app, _repo) = app();
    assert_eq!(feed(&mut app, ":w"), Outcome::Command);
    let buf = draw(&app, 100, 30);
    let line = rows(&buf).pop().unwrap();
    assert!(line.starts_with(" :w    :w finish"), "{line}");
    // The cursor after the text.
    assert!(
        buf[(3, 29)]
            .style()
            .add_modifier
            .contains(Modifier::REVERSED)
    );

    let (mut app, _repo) = self::app();
    feed(&mut app, ":e");
    let line = bar(&app, 120);
    assert!(
        line.contains(":e re-run, decisions kept · :e! re-run and reset every decision"),
        "{line}"
    );
    assert!(!line.contains(":w"), "{line}");
}

#[test]
fn an_unknown_command_shows_in_the_bar_whole() {
    let (mut app, _repo) = app();
    let Outcome::Error(e) = feed(&mut app, ":nope<cr>") else {
        panic!("not an error");
    };
    // As normal mode puts it: the bar shows it, not cut for the buttons.
    app.status = e;
    let line = bar(&app, 80);
    assert!(line.trim_end().ends_with("Not a command: :nope"), "{line}");
    assert!(line.starts_with(" gf open in editor"), "{line}");
}

#[test]
fn visual_mode_says_so_and_marks_its_rows() {
    let (mut app, _repo) = app();
    app.visual = Some(0);
    app.sel = 1;
    let buf = draw(&app, 120, 30);
    let line = rows(&buf).pop().unwrap();
    assert!(
        line.starts_with(" -- VISUAL --  a accept   f flag   h leave"),
        "{line}"
    );
    assert!(line.trim_end().ends_with("2 units"), "{line}");
    // The rows from the anchor to the cursor, and no other.
    let visual = app.theme.visual().bg;
    assert!(visual.is_some());
    let marked: Vec<usize> = app
        .targets
        .borrow()
        .iter()
        .filter_map(|(r, t)| match t {
            Target::Unit(pos) if buf[(r.x + 6, r.y)].style().bg == visual => Some(*pos),
            _ => None,
        })
        .collect();
    assert_eq!(marked, vec![0, 1]);
    // The buttons apply to the selection.
    let t = targets(&app);
    assert!(t.contains(&Target::Action(Action::DecideAccept)));
    assert!(t.contains(&Target::Action(Action::ViewBack)));
}

#[test]
fn the_list_footer_names_v2_keys() {
    let (mut app, _repo) = app();
    let line = bar(&app, 160);
    for hint in [
        "gf open in editor",
        "s swipe",
        "? help",
        "f flag",
        "K ask",
        "i note",
    ] {
        assert!(line.contains(hint), "{hint}: {line}");
    }
    assert!(
        !line.contains("l open"),
        "only a flagged unit opens: {line}"
    );
    let i = app.order[app.sel];
    app.review.units[i].decision = Decision::Investigate;
    let line = bar(&app, 160);
    assert!(line.contains("l open"), "{line}");
}

// ---- The help ----------------------------------------------------------

#[test]
fn the_help_lists_the_lists_keys_from_the_keymap() {
    let (mut app, _repo) = app();
    app.overlay = Overlay::Help;
    let screen = text(&app, 160, 50);
    for want in [
        "keys · list",
        "move",
        "decide",
        "view",
        "go to",
        "tools",
        "finish",
        "j/↓ k/↑  move down / up",
        "gg/home G/end  first / last unit",
        "a f  accept / flag the unit",
        "A F  accept / flag the file's pending units",
        "V  select lines",
        "u ^r  undo the last decision / redo",
        "␣t  tests that cover this unit",
        ":e!  re-run and reset every decision",
        "s  swipe through the cards (until v2)",
        "zf  list by file or by level",
        "any key closes",
    ] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    // Not the Flagged screen's keys, nor the panels'.
    assert!(!screen.contains("zoom the tile"), "{screen}");
    assert!(!screen.contains("delete the note"), "{screen}");
}

#[test]
fn the_help_follows_the_screen() {
    let (mut app, _repo) = app();
    app.overlay = Overlay::Help;
    app.screen = Screen::Flagged;
    let screen = text(&app, 160, 50);
    for want in [
        "keys · Flagged",
        "gt/tab/→ gT/⇧tab/←  next / previous tile (until v2)",
        "zz  zoom the tile (until v2)",
        "o x  resolved: ok / needs fix (until v2)",
        "r  ask the tile again (until v2)",
    ] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    assert!(!screen.contains("list by file or by level"), "{screen}");

    app.screen = Screen::Swipe;
    let screen = text(&app, 160, 50);
    assert!(screen.contains("keys · swipe"), "{screen}");
    assert!(screen.contains("k/↑  move up"), "{screen}");
    assert!(!screen.contains("select lines"), "{screen}");
    assert!(!screen.contains("next tile"), "{screen}");
}

#[test]
fn the_help_shows_rebound_keys() {
    let (mut app, _repo) = app_with(rebound(&[("decide.accept", "y"), ("visual.start", "v")]));
    app.overlay = Overlay::Help;
    let screen = text(&app, 160, 50);
    assert!(screen.contains("y f  accept / flag the unit"), "{screen}");
    assert!(screen.contains("v  select lines"), "{screen}");
}

#[test]
fn the_help_fits_a_small_terminal() {
    let (mut app, _repo) = app();
    app.overlay = Overlay::Help;
    for (w, h) in [(80, 24), (60, 20), (40, 12)] {
        let screen = text(&app, w, h);
        assert!(screen.contains("keys · list"), "{w}x{h}:\n{screen}");
        assert!(screen.contains("move"), "{w}x{h}:\n{screen}");
    }
}

// ---- The jump list -----------------------------------------------------

#[test]
fn the_find_list_shows_its_query_and_what_it_found() {
    let (mut app, _repo) = app();
    app.overlay = Overlay::Jump {
        kind: JumpKind::Find,
        query: "zzz_nothing".into(),
        sel: 0,
    };
    let screen = text(&app, 100, 30);
    assert!(screen.contains("Find a unit"), "{screen}");
    assert!(screen.contains("/ zzz_nothing"), "{screen}");
    assert!(
        screen.contains("Nothing matches \"zzz_nothing\"."),
        "{screen}"
    );
    assert!(screen.contains("↑ ↓ move · ⏎ go · esc close"), "{screen}");

    // An empty query: every unit, or none while the list is a stub.
    app.overlay = Overlay::Jump {
        kind: JumpKind::Find,
        query: String::new(),
        sel: 0,
    };
    let items = app.jump_items();
    let screen = text(&app, 100, 30);
    match items.first() {
        None => assert!(screen.contains("No units to find."), "{screen}"),
        Some(&i) => {
            let name = &app.review.units[i].symbol;
            let row = screen.lines().find(|l| l.contains(name.as_str())).unwrap();
            assert!(row.contains('▌'), "the first is selected: {row}");
        }
    }
}

#[test]
fn the_callers_list_is_titled_by_the_unit() {
    let (mut app, _repo) = app();
    let u = &app.review.units[app.order[0]];
    let (id, symbol) = (u.id.clone(), u.symbol.clone());
    app.overlay = Overlay::Jump {
        kind: JumpKind::Callers { of: id.clone() },
        query: String::new(),
        sel: 0,
    };
    let screen = text(&app, 100, 30);
    assert!(screen.contains(&format!("What calls {symbol}")), "{screen}");
    assert!(screen.contains("j k move · l go · q close"), "{screen}");
    assert!(!screen.contains("/ "), "no query line: {screen}");
    app.overlay = Overlay::Jump {
        kind: JumpKind::Definition { of: id },
        query: String::new(),
        sel: 0,
    };
    let screen = text(&app, 100, 30);
    assert!(screen.contains(&format!("What {symbol} calls")), "{screen}");
}

// ---- Sheets, prompts and clicks -----------------------------------------

#[test]
fn prompts_and_sheets_name_their_own_keys() {
    let (mut app, _repo) = app();
    app.overlay = Overlay::ConfirmReset;
    let screen = text(&app, 100, 30);
    assert!(screen.contains("⏎ reset · esc cancel"), "{screen}");
    assert!(targets(&app).contains(&key("<cr>")));
    assert!(targets(&app).contains(&key("<esc>")));

    let file = app.review.units[app.order[0]].file.clone();
    app.overlay = Overlay::ConfirmAcceptFile {
        file,
        ids: vec!["x".into()],
    };
    let screen = text(&app, 100, 30);
    assert!(screen.contains("y/⏎ yes · esc no"), "{screen}");
    assert!(!screen.contains("(y/n)"), "{screen}");
    assert!(targets(&app).contains(&key("y")));

    // A review of uncommitted changes, all reviewed: enter commits.
    app.overlay = Overlay::AllDone;
    let screen = text(&app, 100, 30);
    assert!(
        screen.contains("q quit  ·  any other key to continue"),
        "{screen}"
    );
    assert!(screen.contains("⏎ commit these changes"), "{screen}");
    assert!(!screen.contains("reset"), "{screen}");
    assert!(targets(&app).contains(&key("<cr>")));
    assert!(targets(&app).contains(&key("q")));

    app.overlay = Overlay::Commit {
        message: "Charge orders in one currency".into(),
        draft: DraftState::Template,
        scroll: 0,
        editing: None,
        branch: teer::commit::Branch::Unknown,
        others: 0,
    };
    let screen = text(&app, 100, 30);
    assert!(
        screen.contains("⏎ commit · i edit message · j k scroll · esc cancel"),
        "{screen}"
    );
}

#[test]
fn panels_name_keymap_keys_and_clicks_run_sequences() {
    // Rebound to sequences, which a click can't press as one key.
    let keys = rebound(&[("note.delete", "dd"), ("view.close", ":q")]);
    let (mut app, _repo) = app_with(keys);
    let i = app.order[0];
    app.review.units[i].dive.note = "Check the currency.".into();
    app.overlay = Overlay::Notes { sel: 0 };
    let screen = text(&app, 120, 30);
    assert!(
        screen.contains("l go to · i edit · dd delete · y copy all"),
        "{screen}"
    );
    assert!(screen.contains(":q close"), "{screen}");
    let t = targets(&app);
    assert!(t.contains(&key("l")), "{t:?}");
    assert!(t.contains(&key("i")), "{t:?}");
    assert!(t.contains(&Target::Action(Action::NoteDelete)), "{t:?}");
    assert!(t.contains(&Target::Action(Action::ViewClose)), "{t:?}");
}
