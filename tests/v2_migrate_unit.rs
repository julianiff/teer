//! Phase 8, migrating v1 users (M2–M6, the implementation's side; the
//! plan's own tests are `tests/v2_migrate.rs`): v1's forms on the command
//! line, a review v1 saved, v1's `[keys]` names and key strings, the `?`
//! help grouped as Move, Decide, Look closer, More, Finish from the keymap,
//! and what `teer --help` lists.

mod common;

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;
use teer::config::Config;
use teer::keymap::{KeySpec, Keymap};
use teer::review::model::{Decision, Review};
use teer::review::{BuildOptions, build_review, store};
use teer::tui::app::{Overlay, Screen};

use common::FixtureRepo;
use common::sandbox::{Sandbox, stderr, stdout};
use common::v2::{Driver, find, review, units};

// ---- A review v1 saved (M3) ----------------------------------------------

/// A review teer 0.1.1 saved of `src/lib.rs` (made with that release's
/// `teer --json --no-agent`, then given v1 decisions): `total` flagged and
/// resolved "needs fix" with a note, `refund` flagged and resolved "ok",
/// `label` accepted, `retry` flagged without a note, `audit` pending with a
/// note. No `version`, no history, no cursor.
const V1_REVIEW: &str = include_str!("samples/v1_saved_review.json");

/// A repo with the change the v1 review is of, and that review saved for
/// it where teer looks, as v1 left it.
fn repo_with_v1_review() -> FixtureRepo {
    common::isolate_env();
    let mut v: Value = serde_json::from_str(V1_REVIEW).unwrap();
    let file = &v["files"][0];
    let repo = FixtureRepo::new();
    repo.write("src/lib.rs", file["old_content"].as_str().unwrap())
        .commit("init");
    repo.write("src/lib.rs", file["new_content"].as_str().unwrap());
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    v["repo_root"] = repo.path().to_string_lossy().into();
    v["base"] = head.clone().into();
    v["head_commit"] = head.clone().into();
    let path = store::review_path(&repo.path(), None, &head, "WORKTREE", &head);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    std::fs::write(
        path.with_file_name("latest"),
        path.to_string_lossy().as_bytes(),
    )
    .unwrap();
    repo
}

/// The review `teer` builds in `repo` and the state it restores, as
/// `cli::run_on` does for plain `teer` on a dirty tree.
fn restored(repo: &Path) -> Review {
    let mut r = build_review(repo, &BuildOptions::default()).unwrap();
    teer::cli::restore(&mut r);
    r
}

fn by_symbol<'a>(units: &'a [Value], symbol: &str) -> &'a Value {
    units
        .iter()
        .find(|u| u["symbol"] == symbol)
        .unwrap_or_else(|| panic!("no unit {symbol}"))
}

#[test]
fn the_v1_fixture_is_what_v1_wrote() {
    let v: Value = serde_json::from_str(V1_REVIEW).unwrap();
    assert!(v.get("version").is_none() && v.get("history").is_none());
    let units = v["units"].as_array().unwrap();
    assert_eq!(by_symbol(units, "total")["dive"]["resolution"], "NeedsFix");
    assert_eq!(by_symbol(units, "refund")["dive"]["resolution"], "Ok");
    assert!(units.iter().all(|u| u.get("accepted_by").is_none()));
}

#[test]
fn a_v1_review_loads_in_v2_states() {
    let sb = Sandbox::new();
    let repo = repo_with_v1_review();
    let out = sb.run(&repo.path(), &["--json", "--no-agent"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        !stderr(&out).contains("teer:"),
        "a v1 review loads without a word: {}",
        stderr(&out)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let units = v["units"].as_array().unwrap();
    let state = |s: &str| {
        let u = by_symbol(units, s);
        (
            u["decision"].as_str().unwrap().to_string(),
            u["dive"]["resolution"].clone(),
            u["dive"]["note"].as_str().unwrap().to_string(),
        )
    };
    let st = |d: &str, note: &str| (d.to_string(), Value::Null, note.to_string());
    // Needs fix: flagged, the note kept.
    assert_eq!(
        state("total"),
        st("Investigate", "The + 1 is wrong: totals must stay the sum.")
    );
    // Ok: flagged.
    assert_eq!(state("refund"), st("Investigate", ""));
    // Flagged without a note: flagged.
    assert_eq!(state("retry"), st("Investigate", ""));
    assert_eq!(state("label"), st("Accepted", ""));
    assert_eq!(by_symbol(units, "label")["accepted_by"], "reviewer");
    assert_eq!(state("audit"), st("Pending", "Where does 1000 come from?"));
    // What the agent wrote then came along.
    assert_eq!(
        by_symbol(units, "total")["changes"][0]["statement"],
        "Every total is now one more than the sum of the items."
    );
    assert!(by_symbol(units, "total")["dive"]["results"]["Recommend"].is_object());
    // Saved again in this teer's format.
    assert_eq!(v["version"], store::CURRENT);
}

#[test]
fn a_v1_review_continues_in_the_queue() {
    let repo = repo_with_v1_review();
    let r = restored(&repo.path());
    assert_eq!(r.cursor, None, "v1 saved no cursor");
    let mut d = Driver::open(r, Config::default());
    assert_eq!(d.screen(), Screen::Queue);
    let id = |s: &str| format!("src/lib.rs::{s}");
    assert_eq!(d.decision_of(&id("total")), Decision::Investigate);
    assert_eq!(d.decision_of(&id("refund")), Decision::Investigate);
    assert_eq!(d.decision_of(&id("retry")), Decision::Investigate);
    assert_eq!(d.decision_of(&id("label")), Decision::Accepted);
    assert_eq!(d.decision_of(&id("audit")), Decision::Pending);
    let saved = serde_json::to_string(&d.app.review).unwrap();
    assert!(!saved.contains("\"resolution\""), "{saved}");

    // v1 kept no place: the queue opens where it opens on a new review,
    // and `n` finds the one pending unit.
    d.keys("n");
    assert_eq!(d.selected().as_deref(), Some(id("audit").as_str()));
    let s = d.render(120, 30);
    assert!(
        s.contains("total flagged · “The + 1 is wron"),
        "the flag's note shows:\n{s}"
    );
    assert!(!s.contains("needs fix"), "{s}");

    // Deciding goes on from there, and finishing lists the flags' notes.
    d.keys("a");
    assert_eq!(d.decision_of(&id("audit")), Decision::Accepted);
    assert!(
        d.bar(120, 30).contains("Nothing pending. :w to finish"),
        "{}",
        d.bar(120, 30)
    );
    d.keys(":w<cr>");
    assert!(
        matches!(d.overlay(), Overlay::Finish { .. }),
        "{:?}",
        d.overlay()
    );
    let s = d.render(120, 40);
    assert!(
        s.contains("The + 1 is wrong: totals must stay the sum."),
        "{s}"
    );
}

// ---- v1's [keys] (M4) ----------------------------------------------------

/// A v1 `[keys]` table: two keys of its own (`accept`, `next_group`, in
/// v1's `ctrl+n` form), one that only repeats v1's default
/// (`investigate`), and three of actions v2 dropped.
const V1_KEYS: &str = "provider = \"claude\"\n\n[keys]\naccept = \"e\"\nnext_group = \"ctrl+n\"\ninvestigate = \"i\"\nswipe = \"w\"\nzoom = \"z\"\nresolve_fix = \"x\"\n";

#[test]
fn v1_keys_map_over_and_removed_ones_warn_once_each() {
    let sb = Sandbox::new();
    sb.config(V1_KEYS);
    let repo = FixtureRepo::new();
    repo.write("a.py", "def f():\n    return 1\n")
        .commit("init");
    repo.write("a.py", "def f():\n    return 2\n");

    let out = sb.run(&repo.path(), &["--json", "--no-agent"]);
    assert!(out.status.success(), "teer continues: {}", stderr(&out));
    let path = sb.config_path();
    let said: Vec<String> = stderr(&out)
        .lines()
        .filter(|l| l.starts_with("teer:"))
        .map(String::from)
        .collect();
    let warn = |name: &str| {
        format!(
            "teer: `[keys].{name}` is no longer used (in {})",
            path.display()
        )
    };
    // Only the removed ones, each once, in the order of their names.
    assert_eq!(said, vec![warn("resolve_fix"), warn("swipe"), warn("zoom")]);
    // Nothing written back.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), V1_KEYS);

    // The keys a review then has.
    let cfg = Config::load(&path).unwrap();
    let (keys, _) = cfg.keymap().unwrap();
    let mut d = Driver::with(review(units(6)), keys);
    d.keys("e");
    assert_eq!(d.decision(0), Decision::Accepted, "accept is e");
    d.keys("<c-n>");
    assert_eq!(d.sel(), 3, "next_group is ctrl+n");
    d.keys("i");
    assert!(
        matches!(d.overlay(), Overlay::Note { .. }),
        "investigate = \"i\" was v1's default: i writes a note, as in v2: {:?}",
        d.overlay()
    );
    d.keys("<esc>f");
    assert_eq!(d.decision(3), Decision::Investigate, "f still flags");
}

#[test]
fn v1_key_strings_work_for_v2_ids() {
    let over = HashMap::from([
        (
            "scroll.half_down".to_string(),
            KeySpec::One("ctrl+f".into()),
        ),
        ("view.open".to_string(), KeySpec::One("enter".into())),
    ]);
    let (keys, warnings) = Keymap::new(&over, 1000).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(keys.labels(teer::keymap::Action::ScrollHalfDown), "^f");
    assert_eq!(keys.labels(teer::keymap::Action::ViewOpen), "⏎");
}

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
        ("a f", "accept / flag the unit"),
        ("V", "select lines"),
        ("l/⏎", "open"),
        ("K", "ask about the unit"),
        ("␣t", "tests that cover this unit"),
        ("za", "show units accepted for you"),
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
        ("]c [c", "next / previous hunk"),
        ("h/esc", "back"),
        ("gx", "open the picture in the browser"),
        ("␣s", "settings"),
        (":e!", "re-run and reset every decision"),
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
    assert!(!s.contains("select lines"), "{s}");
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
fn the_help_shows_rebound_and_v1_keys() {
    let over = HashMap::from([
        ("accept".to_string(), KeySpec::One("e".into())),
        ("tool.notes".to_string(), KeySpec::One("<space>N".into())),
    ]);
    let (keys, _) = Keymap::new(&over, 1000).unwrap();
    let mut d = Driver::with(review(units(6)), keys);
    d.keys("?");
    let s = d.render(160, 50);
    assert!(has_row(&s, "e f", "accept / flag the unit"), "{s}");
    assert!(has_row(&s, "␣N", "every note"), "{s}");
    assert!(!has_row(&s, "a f", "accept / flag the unit"), "{s}");
}

// ---- The command line (M2, M6) ---------------------------------------------

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
    assert_eq!(section("Commands"), vec!["pr", "notes"], "{help}");
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
fn old_forms_work_for_one_release_with_a_hint() {
    const GH_LIST: &str = r#"echo "$@" >> "$(dirname "$0")/gh.log"
echo '[{"number":7,"title":"Charge in one place","url":"https://github.com/acme/shop/pull/7","author":{"login":"ana"},"isDraft":false,"reviewDecision":"REVIEW_REQUIRED","updatedAt":"2026-11-01T10:00:00Z","headRefName":"refunds"}]'"#;
    let sb = Sandbox::new();
    sb.gh(GH_LIST);
    let repo = FixtureRepo::new();
    repo.write("a.py", "x = 1\n").commit("init");
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/shop.git",
    ]);

    let new = sb.run(&repo.path(), &["pr", "--json"]);
    assert!(new.status.success(), "{}", stderr(&new));
    assert!(!stderr(&new).contains("is now"), "{}", stderr(&new));
    for old in [&["pr", "list", "--json"][..], &["pr", "ls", "--json"]] {
        let out = sb.run(&repo.path(), old);
        assert!(out.status.success(), "{old:?}: {}", stderr(&out));
        assert_eq!(stdout(&out), stdout(&new), "{old:?} is teer pr");
        assert_eq!(
            stderr(&out).lines().collect::<Vec<_>>(),
            vec!["teer: `teer pr list` is now `teer pr`; the old form goes in the next release."],
            "{old:?}"
        );
    }

    // `teer pr <url>`: the hint first, then what `teer <url>` does (this
    // one isn't a pull request).
    let url = "https://github.com/acme/shop/issues/7";
    let old = sb.run(&repo.path(), &["pr", url, "--no-agent"]);
    let new = sb.run(&repo.path(), &[url, "--no-agent"]);
    let (old_err, new_err) = (stderr(&old), stderr(&new));
    assert_eq!(
        old_err.lines().next(),
        Some(
            format!(
                "teer: `teer pr <url>` is now `teer {url}`; the old form goes in the next release."
            )
            .as_str()
        ),
        "{old_err}"
    );
    assert!(new_err.contains("not a pull request URL"), "{new_err}");
    assert_eq!(
        old_err.lines().skip(1).collect::<Vec<_>>(),
        new_err.lines().collect::<Vec<_>>()
    );

    // `--base` is a v2 flag: no hint.
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("a.py", "x = 2\n").commit("two");
    let out = sb.run(&repo.path(), &["--base", "main", "--json", "--no-agent"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stderr(&out).contains("is now"), "{}", stderr(&out));

    // `teer help` still prints the help, and a command's.
    let out = sb.run(&repo.path(), &["help", "pr"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("Usage: teer pr"), "{}", stdout(&out));
}
