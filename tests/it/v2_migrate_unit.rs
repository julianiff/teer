//! What v2 left of v1 (the implementation's side; the plan's own tests are
//! `tests/it/v2_migrate.rs`): the `?` help grouped as Move, Decide, Look
//! closer, More, Finish from the keymap, and what `teer --help` lists.

use crate::common;

use std::collections::HashMap;

use teer::keymap::{KeySpec, Keymap};
use teer::session::app::{Overlay, Screen};

use common::FixtureRepo;
use common::sandbox::{Sandbox, stderr, stdout};
use common::v2::{Driver, find, review, units};

// ---- `?` (M5) --------------------------------------------------------------

/// The groups the help shows, in reading order (columns left to right,
/// each top to bottom).
fn groups_in(help: &str) -> Vec<&'static str> {
    let mut at: Vec<((u16, u16), &'static str)> =
        ["Move", "Decide", "Look closer", "More", "Finish"]
            .into_iter()
            .filter_map(|g| find(help, &format!("  {g}  ")).map(|p| (p, g)))
            .collect();
    at.sort();
    at.into_iter().map(|(_, g)| g).collect()
}

/// A help row: `keys  what it does`, as one line of the help draws it.
fn has_row(help: &str, keys: &str, what: &str) -> bool {
    help.contains(&format!("{keys}  {what}"))
}

#[test]
fn the_help_groups_the_queues_keys() {
    let mut d = Driver::new(6);
    d.keys("?");
    let s = d.render(160, 50);
    assert!(s.contains("keys · queue"), "{s}");
    assert_eq!(
        groups_in(&s),
        vec!["Move", "Decide", "Look closer", "More", "Finish"],
        "{s}"
    );
    for (keys, what) in [
        ("j/↓ k/↑", "move down / up"),
        ("/", "find a unit by file or name"),
        ("a f", "accept / flag the file's pending units"),
        ("V", "select rows"),
        ("l/⏎", "look closer or open"),
        ("␣t", "tests that cover this unit"),
        ("␣s", "settings"),
        (":w", "finish"),
        ("q/:q", "close; in the list, quit"),
    ] {
        assert!(has_row(&s, keys, what), "{keys}  {what}:\n{s}");
    }
}

#[test]
fn the_help_groups_the_unit_views_keys() {
    let mut d = Driver::new(6);
    d.keys("l?");
    assert_eq!(d.screen(), Screen::Unit);
    let s = d.render(160, 50);
    assert!(s.contains("keys · unit"), "{s}");
    assert_eq!(
        groups_in(&s),
        vec!["Move", "Decide", "Look closer", "More", "Finish"],
        "{s}"
    );
    for (keys, what) in [
        ("h/esc", "back"),
        ("␣t", "tests that cover this unit"),
        ("␣s", "settings"),
        (":e", "re-run, decisions kept"),
    ] {
        assert!(has_row(&s, keys, what), "{keys}  {what}:\n{s}");
    }
}

#[test]
fn the_help_of_the_finish_sheet_is_its_own_and_goes_back_to_it() {
    let mut d = Driver::new(3);
    d.keys(":w<cr>");
    assert!(matches!(d.overlay(), Overlay::Finish { .. }));
    d.keys("?");
    assert_eq!(*d.overlay(), Overlay::Help);
    let s = d.render(160, 50);
    assert!(s.contains("keys · finish"), "{s}");
    assert_eq!(
        groups_in(&s),
        vec!["Move", "Decide", "More", "Finish"],
        "{s}"
    );
    for (keys, what) in [
        ("i", "edit the commit message or summary"),
        ("y", "copy the notes"),
        ("h/esc", "back"),
    ] {
        assert!(has_row(&s, keys, what), "{keys}  {what}:\n{s}");
    }
    // Not the queue's.
    assert!(!s.contains("select rows"), "{s}");
    // Any key closes it, back on the sheet.
    d.keys("x");
    assert!(
        matches!(d.overlay(), Overlay::Finish { .. }),
        "{:?}",
        d.overlay()
    );
    d.keys("<esc>");
    assert_eq!(*d.overlay(), Overlay::None);
}

#[test]
fn the_help_shows_rebound_keys() {
    let over = HashMap::from([
        ("decide.accept".to_string(), KeySpec::One("e".into())),
        ("tool.notes".to_string(), KeySpec::One("<space>N".into())),
    ]);
    let (keys, _) = Keymap::new(&over, 1000).unwrap();
    let mut d = Driver::with(review(units(6)), keys);
    // A note, so the notes have something to list.
    d.keys("ia note<cr>?");
    let s = d.render(160, 50);
    assert!(
        has_row(&s, "e f", "accept / flag the file's pending units"),
        "{s}"
    );
    assert!(has_row(&s, "␣N", "every note"), "{s}");
    assert!(
        !has_row(&s, "a f", "accept / flag the file's pending units"),
        "{s}"
    );
}

// ---- The command line (M6) -------------------------------------------------

#[test]
fn teer_help_lists_only_the_v2_surface() {
    let sb = Sandbox::new();
    let repo = FixtureRepo::new();
    let out = sb.run(&repo.path(), &["--help"]);
    assert!(out.status.success());
    let help = stdout(&out);
    for usage in [
        "Usage: teer [--base <REF>] [--no-agent] [--json]",
        "       teer <URL>",
        "       teer pr\n",
        "       teer notes\n",
    ] {
        assert!(help.contains(usage), "{usage:?}:\n{help}");
    }
    // The entries of each section: the first word of its indented lines.
    let section = |name: &str| -> Vec<String> {
        help.split(&format!("\n{name}:\n"))
            .nth(1)
            .unwrap_or_else(|| panic!("no {name}:\n{help}"))
            .lines()
            .take_while(|l| l.is_empty() || l.starts_with(' '))
            // Entries start within the first six columns; their text
            // starts further in.
            .filter(|l| l.len() > 6 && !l[..6].trim().is_empty() || l.starts_with("      -"))
            .filter_map(|l| l.split_whitespace().next().map(String::from))
            .collect()
    };
    assert_eq!(
        section("Commands"),
        vec!["review", "security", "improve", "pr", "notes", "app"],
        "{help}"
    );
    assert_eq!(
        section("Options"),
        vec!["--base", "--json", "--no-agent", "-h,"],
        "{help}"
    );
    // `setup` stays hidden as a command (the text above names it).
    for gone in ["--swipe", "--session", "-C", "--version", "  setup", "list"] {
        assert!(!help.contains(gone), "{gone}:\n{help}");
    }
}

#[test]
fn teer_help_prints_a_commands_help() {
    let sb = Sandbox::new();
    let repo = FixtureRepo::new();
    let out = sb.run(&repo.path(), &["help", "pr"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("Usage: teer pr"), "{}", stdout(&out));
}
