//! Phase 8, landing v1 users safely (M1–M6 of `docs/v2-map.md` §11): the
//! v1 surface is gone; `teer --base main` and `teer pr list` still work for
//! one release, with a hint; a review saved by v1 loads and goes on in the
//! queue; a v1 `[keys]` table maps over, with one warning per removed
//! action and nothing written back; `?` groups the keys from the keymap;
//! `teer --help` shows the v2 surface only.
//!
//! The saved review (`tests/fixtures/v1/review.json`) was written by teer
//! v1 (`e394565`, `teer --json --no-agent`) for the `calc.py` change below,
//! then given a v1 user's decisions: `total` flagged, resolved "needs fix",
//! with a note; `fee` flagged, resolved "ok"; `tax` flagged; `discount`
//! accepted; `rounding` pending. The config (`tests/fixtures/v1/config.toml`)
//! uses v1's action names.
//!
//! Names Phase 8's code may choose differently are kept in the helpers
//! right below, so they can be adapted in one place.

use crate::common;

use std::collections::HashMap;
use std::path::Path;

use common::run::{ConfigFile, Fakes, Session, launch, repo, save_v1_review, v1_fixture, v1_repo};
use common::sandbox::{Sandbox, stderr, stdout};
use common::v2::Driver;
use serde_json::Value;
use teer::config::Config;
use teer::keymap::{Action, KeySpec, Keymap};
use teer::review::model::{Decision, Review};
use teer::review::store;
use teer::tui::app::{Overlay, Screen};

use Decision::{Accepted as A, Investigate as F, Pending as P};

// ---- Guesses about names Phase 8 chooses ------------------------------------------

/// The warning for a v1 `[keys]` entry whose action is gone.
fn removed_warning(name: &str) -> String {
    format!("`[keys].{name}` is no longer used")
}

/// The one line an old form prints about the new one: the only line on
/// stderr.
fn hint(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    match lines[..] {
        [one] => Some(one.to_string()),
        _ => None,
    }
}

/// The help's groups, in order (matched without case).
const HELP_GROUPS: [&str; 5] = ["Move", "Decide", "Look closer", "More", "Finish"];

/// The row of the help that describes accepting a unit (from the keymap's
/// descriptions: "accept the unit", or "accept / flag the unit" when the
/// help pairs it with flagging).
fn is_accept_row(what: &str) -> bool {
    what.starts_with("accept") && what.contains("the unit")
}

/// What `teer --help` may show besides the v2 surface: clap's own help.
const CLAP_OWN_COMMANDS: [&str; 1] = ["help"];
const CLAP_OWN_FLAGS: [&str; 2] = ["-h", "--help"];

// ---- Fixtures ------------------------------------------------------------------------

fn decision(r: &Review, symbol: &str) -> Decision {
    r.units
        .iter()
        .find(|u| u.symbol == symbol)
        .unwrap()
        .decision
}

fn note(r: &Review, symbol: &str) -> String {
    r.units
        .iter()
        .find(|u| u.symbol == symbol)
        .unwrap()
        .dive
        .note
        .clone()
}

/// Every `resolution` a saved form of `r` still carries (v1's "ok" and
/// "needs fix").
fn resolutions(r: &Review) -> Vec<Value> {
    let v = serde_json::to_value(r).unwrap();
    v["units"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|u| u["dive"].get("resolution").cloned())
        .filter(|r| !r.is_null())
        .collect()
}

/// A clean branch `feature`, two commits ahead of `main`.
fn feature_branch() -> common::FixtureRepo {
    let r = repo();
    r.write("shop.py", "def price(q):\n    return q * 10\n")
        .commit("init");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("shop.py", "def price(q):\n    return q * 12\n")
        .commit("raise prices");
    r
}

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
    let built = save_v1_review(&r);
    let loaded = store::load(
        &built.repo_root,
        None,
        &built.base,
        &built.head,
        &built.head_commit,
    )
    .expect("loads");
    let saved = serde_json::to_string(&loaded).unwrap();
    assert!(!saved.contains("resolution"), "{saved}");
    let mut d = Driver::open(loaded, Config::default());
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
fn old_forms_hint() {
    // `teer --base main`: still the branch since main, and a hint.
    let sb = Sandbox::new();
    let r = feature_branch();
    let out = sb.run(&r.path(), &["--base", "main", "--json", "--no-agent"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).expect("the review as JSON");
    let main = r.git(&["rev-parse", "main"]).trim().to_string();
    assert_eq!(v["base"], main.as_str());
    let line = hint(&stderr(&out)).unwrap_or_else(|| panic!("one hint: {:?}", stderr(&out)));
    assert!(line.contains("base"), "{line}");

    // `teer pr list`: the list, as `teer pr` prints it without a terminal,
    // and a hint naming `teer pr`.
    let sb = Sandbox::new();
    sb.gh("echo \"$@\" >> \"$(dirname \"$0\")/gh.log\"\necho '[]'");
    let r = repo();
    r.write("shop.py", "x = 1\n").commit("init");
    r.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/shop.git",
    ]);
    for form in [&["pr", "list"][..], &["pr", "ls"][..]] {
        let out = sb.run(&r.path(), form);
        assert!(out.status.success(), "{form:?}: {}", stderr(&out));
        assert!(
            stdout(&out).contains("No open pull requests in acme/shop"),
            "{form:?}: {}",
            stdout(&out)
        );
        let line = hint(&stderr(&out)).unwrap_or_else(|| panic!("{form:?}: {}", stderr(&out)));
        assert!(line.contains("teer pr"), "{form:?}: {line}");
    }
    // `teer pr` itself prints no hint.
    let out = sb.run(&r.path(), &["pr"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out).trim(), "", "no hint for the new form");
}

// ---- M3 ------------------------------------------------------------------------------

#[tokio::test]
async fn v1_review_fixture_loads() {
    let r = v1_repo();
    let built = save_v1_review(&r);
    let loaded = store::load(
        &built.repo_root,
        None,
        &built.base,
        &built.head,
        &built.head_commit,
    )
    .expect("a review saved by v1 loads");
    // Needs fix: flagged, the note kept. Resolved ok: flagged. Flagged
    // without a note: flagged. The rest as they were.
    assert_eq!(decision(&loaded, "total"), F);
    assert_eq!(note(&loaded, "total"), "charges the fee twice");
    assert_eq!(decision(&loaded, "fee"), F);
    assert_eq!(decision(&loaded, "tax"), F);
    assert_eq!(note(&loaded, "tax"), "");
    assert_eq!(decision(&loaded, "discount"), A);
    assert_eq!(decision(&loaded, "rounding"), P);
    let total = loaded.units.iter().find(|u| u.symbol == "total").unwrap();
    assert!(!total.note_is_stale(), "written on this code");
    assert_eq!(resolutions(&loaded), Vec::<Value>::new());

    // `teer` on the same change goes on in the queue with it.
    let config = ConfigFile::new("second_opinion = false\n");
    let fakes = Fakes::new();
    let l = launch(&r.path(), &["--no-agent"], &config.path())
        .await
        .unwrap()
        .expect("a review");
    assert_eq!(decision(&l.review, "total"), F);
    assert_eq!(note(&l.review, "total"), "charges the fee twice");
    let mut s = Session::launched(l, common::run::model(&[]), fakes.tools());
    assert_eq!(s.app.screen, Screen::Queue);
    let screen = s.render(140, 30);
    assert!(screen.contains("charges the fee twice"), "{screen}");
    assert!(!screen.to_lowercase().contains("needs fix"), "{screen}");
    s.go_to("rounding");
    s.keys("a");
    assert_eq!(s.decision("rounding"), A);
    assert_eq!(s.counts(), (2, 3, 0));
    s.keys(":w<cr>");
    assert!(matches!(s.app.overlay, Overlay::Finish { .. }));
    assert!(s.render(140, 40).contains("charges the fee twice"));
    // Saved again in v2's format.
    let path = store::review_path(
        &built.repo_root,
        None,
        &built.base,
        &built.head,
        &built.head_commit,
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(saved["version"].as_u64() >= Some(store::CURRENT as u64));
    let again: Review = serde_json::from_value(saved).unwrap();
    assert_eq!(resolutions(&again), Vec::<Value>::new());
}

// ---- M4 ------------------------------------------------------------------------------

#[test]
fn v1_keys_config() {
    let config = ConfigFile::new(&std::fs::read_to_string(v1_fixture("config.toml")).unwrap());
    let cfg = Config::load(&config.path()).expect("a v1 config loads");
    let (keys, warnings) = cfg.keymap().expect("its keys load");
    // Mapped to their v2 actions.
    assert_eq!(keys.label(Action::DecideAccept), "e");
    assert_eq!(keys.label(Action::HistoryUndo), "U");
    // One warning per removed action, none for the ones that map.
    for gone in ["swipe", "prev_card"] {
        let n = warnings
            .iter()
            .filter(|w| w.contains(&removed_warning(gone)))
            .count();
        assert_eq!(n, 1, "{gone}: {warnings:?}");
    }
    assert_eq!(warnings.len(), 2, "{warnings:?}");

    // On start: the warnings once each, the review, and the config as it was.
    let sb = Sandbox::new();
    sb.config(&std::fs::read_to_string(v1_fixture("config.toml")).unwrap());
    let before = std::fs::read(sb.config_path()).unwrap();
    let listing = |d: &Path| {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    let files = listing(sb.root.path());
    let r = v1_repo();
    let out = sb.run(&r.path(), &["--json", "--no-agent"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let err = stderr(&out);
    for gone in ["swipe", "prev_card"] {
        assert_eq!(err.matches(&removed_warning(gone)).count(), 1, "{err}");
    }
    for kept in ["accept", "undo"] {
        assert!(!err.contains(&format!("[keys].{kept}")), "{kept}: {err}");
    }
    assert_eq!(std::fs::read(sb.config_path()).unwrap(), before);
    assert_eq!(listing(sb.root.path()), files, "nothing written next to it");
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
        assert_eq!(commands, ["pr", "notes"], "{flag}:\n{help}");
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
