//! What v2 left of v1 (M1, M5, M6 of `docs/v2-map.md` §11): the v1 surface
//! is gone, its command forms included; `?` groups the keys from the
//! keymap; `teer --help` shows the v2 surface only.

use crate::common;

use std::collections::HashMap;
use std::path::Path;

use common::run::{repo, v1_repo};
use common::sandbox::{Sandbox, stderr, stdout};
use common::v2::Driver;
use teer::config::Config;
use teer::keymap::{Action, KeySpec, Keymap};
use teer::review::{BuildOptions, build_review};
use teer::session::app::{Overlay, Screen};

// ---- Names the tests match -------------------------------------------------------------

/// The help's groups, in order (matched without case).
const HELP_GROUPS: [&str; 5] = ["Move", "Decide", "Look closer", "More", "Finish"];

/// The row of the help that describes accepting: "accept the unit", or
/// "accept / flag the unit" when the help pairs it with flagging; in the
/// queue, where a row is a file, "accept / flag the file's pending units".
fn is_accept_row(what: &str) -> bool {
    what.starts_with("accept") && (what.contains("the unit") || what.contains("the file's"))
}

/// What `teer --help` may show besides the v2 surface: clap's own help.
const CLAP_OWN_COMMANDS: [&str; 1] = ["help"];
const CLAP_OWN_FLAGS: [&str; 2] = ["-h", "--help"];

// ---- Fixtures ------------------------------------------------------------------------

/// The text of each column of `screen` row by row: rows cut at runs of two
/// or more spaces and at box borders.
fn segments(screen: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for (y, row) in screen.lines().enumerate() {
        let chars: Vec<char> = row.chars().collect();
        let mut x = 0;
        while x < chars.len() {
            let sep = |i: usize| {
                "│╭╮╰╯─".contains(chars[i])
                    || (chars[i] == ' ' && chars.get(i + 1).is_none_or(|c| *c == ' '))
            };
            if sep(x) {
                x += 1;
                continue;
            }
            let start = x;
            while x < chars.len() && !sep(x) {
                x += 1;
            }
            let text: String = chars[start..x].iter().collect();
            out.push((start, y, text.trim().to_string()));
        }
    }
    out
}

/// Where each of [`HELP_GROUPS`] is drawn on `screen` (column, row).
fn group_places(screen: &str) -> Vec<Option<(usize, usize)>> {
    let segs = segments(screen);
    HELP_GROUPS
        .iter()
        .map(|g| {
            segs.iter()
                .find(|(_, _, t)| t.eq_ignore_ascii_case(g))
                .map(|(x, y, _)| (*x, *y))
        })
        .collect()
}

// ---- M1 ------------------------------------------------------------------------------

#[test]
fn v1_surface_removed() {
    // `--swipe` is gone.
    let sb = Sandbox::new();
    let r = v1_repo();
    let out = sb.run(&r.path(), &["--swipe", "--json", "--no-agent"]);
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(stderr(&out).contains("--swipe"), "{}", stderr(&out));
    // The v1 screens and their keys.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for gone in ["src/tui/swipe.rs", "src/tui/flagged.rs"] {
        assert!(!root.join(gone).exists(), "{gone}");
    }
    let v1: Vec<&str> = Action::all()
        .map(|a| a.id())
        .filter(|id| id.starts_with("v1."))
        .collect();
    assert!(v1.is_empty(), "{v1:?}");
    // "Needs fix" is gone: nothing a review saves or shows has it.
    let built = build_review(&r.path(), &BuildOptions::default()).unwrap();
    let saved = serde_json::to_string(&built).unwrap();
    assert!(!saved.contains("resolution"), "{saved}");
    let mut d = Driver::open(built, Config::default());
    let queue = d.render(140, 40).to_lowercase();
    assert!(!queue.contains("needs fix"), "{queue}");
    d.keys("l");
    let unit = d.render(140, 60);
    for glyph in ['$', '≠'] {
        assert!(!unit.contains(glyph), "{glyph}: {unit}");
    }
}

// ---- M2 ------------------------------------------------------------------------------

#[test]
fn v1_forms_removed() {
    let sb = Sandbox::new();
    let r = repo();
    r.write("shop.py", "x = 1\n").commit("init");
    for form in [
        &["pr", "list"][..],
        &["pr", "ls"],
        &["pr", "https://github.com/acme/shop/pull/7"],
    ] {
        let out = sb.run(&r.path(), form);
        assert!(!out.status.success(), "{form:?}: {}", stdout(&out));
        assert!(
            stderr(&out).contains("Usage: teer pr"),
            "{form:?}: {}",
            stderr(&out)
        );
    }
}

// ---- M5 ------------------------------------------------------------------------------

#[test]
fn help_is_generated_from_keymap() {
    let over = HashMap::from([("decide.accept".to_string(), KeySpec::One("e".into()))]);
    let (keys, _) = Keymap::new(&over, 1000).unwrap();
    let mut d = Driver::with(common::v2::review(common::v2::units(6)), keys);
    for screen in [Screen::Queue, Screen::Unit] {
        if screen == Screen::Unit {
            d.keys("l");
        }
        assert_eq!(d.screen(), screen);
        d.keys("?");
        assert!(matches!(d.overlay(), Overlay::Help), "{screen:?}");
        let help = d.render(160, 60);
        // The groups, in order (down a column, then the next column).
        let places = group_places(&help);
        for (g, p) in HELP_GROUPS.iter().zip(&places) {
            assert!(p.is_some(), "{screen:?}: no {g} group:\n{help}");
        }
        let places: Vec<(usize, usize)> = places.into_iter().flatten().collect();
        assert!(
            places
                .windows(2)
                .all(|w| w[1].0 > w[0].0 || (w[1].0 == w[0].0 && w[1].1 > w[0].1)),
            "{screen:?}: in order {places:?}:\n{help}"
        );
        // The rebinding shows: accepting is `e`, no longer `a`.
        let segs = segments(&help);
        let at = segs
            .iter()
            .position(|(_, _, t)| is_accept_row(t))
            .unwrap_or_else(|| panic!("{screen:?}: no accept row:\n{help}"));
        let keys: Vec<&str> = segs[at - 1].2.split([' ', '/']).collect();
        assert!(keys.contains(&"e"), "{screen:?}: {keys:?}\n{help}");
        assert!(!keys.contains(&"a"), "{screen:?}: {keys:?}\n{help}");
        d.keys("<esc>");
    }
}

// ---- M6 ------------------------------------------------------------------------------

#[test]
fn help_lists_v2_surface() {
    let sb = Sandbox::new();
    let dir = tempfile::tempdir().unwrap();
    for flag in ["--help", "-h"] {
        let out = sb.run(dir.path(), &[flag]);
        assert!(out.status.success(), "{}", stderr(&out));
        let help = stdout(&out);
        // Lines of a section: from its title to the next blank-line-separated title.
        let section = |title: &str| -> Vec<String> {
            let mut lines = help.lines().skip_while(|l| !l.starts_with(title)).skip(1);
            let mut out = Vec::new();
            for l in lines.by_ref() {
                if !l.starts_with(' ') && !l.is_empty() {
                    break;
                }
                out.push(l.to_string());
            }
            out
        };
        // The forms: `teer`, `teer <url>` (the argument), `teer pr`, `teer notes`.
        let usage = help.lines().find(|l| l.starts_with("Usage:")).unwrap();
        assert!(usage.starts_with("Usage: teer"), "{usage}");
        let commands: Vec<String> = section("Commands:")
            .iter()
            .filter_map(|l| l.split_whitespace().next().map(String::from))
            .filter(|c| !CLAP_OWN_COMMANDS.contains(&c.as_str()))
            .collect();
        assert_eq!(
            commands,
            ["review", "security", "improve", "pr", "notes", "app"],
            "{flag}:\n{help}"
        );
        let args = section("Arguments:");
        assert_eq!(
            args.iter()
                .filter(|l| l.trim_start().starts_with('['))
                .count(),
            1,
            "one argument, the pull request: {args:?}"
        );
        // The flags: --base, --no-agent, --json, and nothing else.
        let mut flags: Vec<String> = section("Options:")
            .iter()
            .map(|l| l.trim_start())
            .filter(|l| l.starts_with('-'))
            .flat_map(|l| {
                l.split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|w| w.starts_with('-'))
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .filter(|f| !CLAP_OWN_FLAGS.contains(&f.as_str()))
            .collect();
        flags.sort();
        assert_eq!(flags, ["--base", "--json", "--no-agent"], "{flag}:\n{help}");
        assert!(!help.contains("pr list"), "{help}");
        assert!(!help.contains("--swipe") && !help.contains("--session"));
    }
}
