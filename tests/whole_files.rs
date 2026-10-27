//! Nothing that changed is invisible: files teer can't parse are one unit
//! for the whole file, and files it can't show at all (binary, not UTF-8,
//! too large) are listed as not shown.

mod common;

use std::collections::BTreeMap;
use std::process::Command;
use std::sync::Arc;

use common::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};
use teer::agent::mock::MockAdapter;
use teer::config::{Config, KeyMap};
use teer::git::FileStatus;
use teer::review::attention::Level;
use teer::review::model::{Decision, Prep, Review, SessionRef, UnitKind, UnitStatus};
use teer::review::{BuildOptions, build_review};
use teer::tui::app::{App, Overlay};
use teer::tui::ui;

fn lines(n: usize, word: &str) -> String {
    (0..n).map(|i| format!("{word} {i}\n")).collect()
}

/// A change touching a Python function, Markdown (modified, renamed with a
/// change, deleted, and one too large to describe), JSON, a lock file, a
/// test snapshot, a binary file and a file that isn't UTF-8.
fn fixture() -> FixtureRepo {
    let repo = FixtureRepo::new();
    repo.write("a.py", "def a():\n    return 1\n")
        .write("README.md", "# Shop\n\nRun it with `make`.\n")
        .write(
            "docs/old.md",
            "# Guide\n\nStep one.\nStep two.\nStep three.\n",
        )
        .write("NOTES.md", "remember\n")
        .write("Cargo.lock", &lines(60, "dep"))
        .write("tests/__snapshots__/help.test.ts.snap", "usage: old\n")
        .commit("init");
    repo.write("a.py", "def a():\n    return 2\n")
        .write("README.md", "# Shop\n\nRun it with `make run`.\n")
        .write("config.json", "{\n  \"retries\": 3\n}\n")
        .write("big.md", &lines(400, "line"))
        .write("Cargo.lock", &lines(60, "pkg"))
        .write("tests/__snapshots__/help.test.ts.snap", "usage: new\n")
        .write("logo.png", "PNG\0\u{1}\u{2}")
        .remove("NOTES.md");
    repo.git(&["mv", "docs/old.md", "docs/new.md"]);
    repo.write(
        "docs/new.md",
        "# Guide\n\nStep one.\nStep two.\nStep three, carefully.\n",
    );
    std::fs::write(repo.dir.path().join("latin1.txt"), b"caf\xe9\n").unwrap();
    repo
}

/// The fixture's review. Drawing and deciding never read the repository again.
fn review() -> Review {
    let _env = isolate_env();
    let repo = fixture();
    build_review(&repo.path(), &BuildOptions::default()).unwrap()
}

fn whole(r: &Review) -> BTreeMap<String, (UnitStatus, Prep)> {
    r.units
        .iter()
        .filter(|u| u.kind == UnitKind::File)
        .map(|u| (u.file.display().to_string(), (u.status, u.prep)))
        .collect()
}

#[test]
fn every_changed_text_file_is_a_unit_and_the_rest_are_listed() {
    let r = review();
    let got = whole(&r);
    let want: BTreeMap<String, (UnitStatus, Prep)> = [
        ("Cargo.lock", (UnitStatus::Changed, Prep::Skipped)),
        ("NOTES.md", (UnitStatus::Removed, Prep::NotRequested)),
        ("README.md", (UnitStatus::Changed, Prep::NotRequested)),
        ("big.md", (UnitStatus::Added, Prep::Skipped)),
        ("config.json", (UnitStatus::Added, Prep::NotRequested)),
        ("docs/new.md", (UnitStatus::Changed, Prep::NotRequested)),
        (
            "tests/__snapshots__/help.test.ts.snap",
            (UnitStatus::Changed, Prep::Skipped),
        ),
    ]
    .into_iter()
    .map(|(f, s)| (f.to_string(), s))
    .collect();
    assert_eq!(got, want);
    // Parsed code is still split into functions.
    assert!(
        r.units
            .iter()
            .any(|u| u.symbol == "a" && u.kind == UnitKind::Function)
    );
    // Whole files have no callers or callees.
    for u in r.units.iter().filter(|u| u.kind == UnitKind::File) {
        assert!(u.callers.is_empty() && u.callees.is_empty(), "{}", u.id);
        assert_eq!(u.symbol, "(whole file)");
    }
    let renamed = r.files.iter().find(|f| f.path.ends_with("new.md")).unwrap();
    assert_eq!(renamed.status, FileStatus::Renamed);

    let skipped: Vec<(String, &str)> = r
        .skipped
        .iter()
        .map(|f| (f.path.display().to_string(), f.why.as_str()))
        .collect();
    assert_eq!(
        skipped,
        [
            ("latin1.txt".to_string(), "not UTF-8"),
            ("logo.png".to_string(), "binary")
        ]
    );
    assert!(r.files.iter().all(|f| !f.path.ends_with("logo.png")));
}

#[test]
fn levels_name_lock_files_snapshots_and_files_too_large_to_describe() {
    let r = review();
    let a = App::new(r, Config::default(), KeyMap::default());
    let level = |file: &str| {
        let i = a
            .review
            .units
            .iter()
            .position(|u| u.file.display().to_string() == file)
            .unwrap();
        a.levels[i].clone()
    };
    // Facts decide: 120 lines of lock file changed, still routine.
    let lock = level("Cargo.lock");
    assert_eq!(lock.level, Level::Routine);
    assert_eq!(lock.reasons, ["lock file"]);
    let snap = level("tests/__snapshots__/help.test.ts.snap");
    assert_eq!(snap.level, Level::Routine);
    assert_eq!(snap.reasons, ["test snapshot"]);
    let big = level("big.md");
    assert_eq!(big.level, Level::Look);
    assert!(
        big.reasons
            .contains(&"too large to describe; read the diff".to_string()),
        "{:?}",
        big.reasons
    );
    let gone = level("NOTES.md");
    assert!(gone.reasons.contains(&"file deleted".to_string()));
}

#[test]
fn json_output_has_the_whole_file_units_and_the_skipped_files() {
    let env = isolate_env();
    let repo = fixture();
    let out = Command::new(teer_bin())
        .args(["--json", "--no-agent", "-C"])
        .arg(repo.path())
        .env("TEER_DATA_DIR", env.path().join("data"))
        .env("TEER_CONFIG", env.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let readme = v["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == "README.md::(whole file)")
        .expect("README.md is a unit");
    assert_eq!(readme["kind"], "File");
    assert_eq!(readme["new_range"], json!({"start": 1, "end": 3}));
    assert_eq!(
        v["skipped"],
        json!([
            {"path": "latin1.txt", "status": "Added", "why": "not UTF-8"},
            {"path": "logo.png", "status": "Added", "why": "binary"},
        ])
    );
}

fn draw(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn key(app: &mut App, c: char) {
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )));
}

#[test]
fn the_list_shows_whole_files_and_the_files_it_cant_show_last() {
    let a = App::new(review(), Config::default(), KeyMap::default());
    let t = draw(&a, 120, 50);
    // A whole file goes by its file name, and the preview says it is one.
    assert!(t.lines().any(|l| l.starts_with("│ README.md")), "{t}");
    assert!(t.lines().any(|l| l.starts_with("│▌  ◆ README.md")), "{t}");
    assert!(t.contains("  README.md  whole file"), "{t}");
    // Files teer can't show are muted rows with why, in a box of their own
    // after every other, and can't be selected.
    let rows: Vec<&str> = t.lines().collect();
    let latin1 = rows
        .iter()
        .position(|l| l.starts_with("│   latin1.txt  not UTF-8"))
        .unwrap_or_else(|| panic!("{t}"));
    let logo = rows
        .iter()
        .position(|l| l.starts_with("│   logo.png  binary"))
        .unwrap_or_else(|| panic!("{t}"));
    let title = rows
        .iter()
        .position(|l| l.starts_with("╭─ Not shown · 2 files"))
        .unwrap_or_else(|| panic!("{t}"));
    let last_unit = rows
        .iter()
        .rposition(|l| l.chars().nth(4).is_some_and(|c| "▲◆●".contains(c)))
        .unwrap();
    assert!(title > last_unit && latin1 > title && logo > latin1, "{t}");
    let targets = a.targets.borrow();
    for y in [latin1, logo] {
        assert!(
            !targets.iter().any(|(r, _)| r.y == y as u16 && r.x == 0),
            "{y}"
        );
    }
    for &i in &a.order {
        assert!(!a.review.units[i].file.ends_with("logo.png"));
    }
}

#[test]
fn a_change_of_only_a_binary_file_still_says_so() {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("a.py", "def a():\n    pass\n").commit("init");
    repo.write("logo.png", "PNG\0");
    let r = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    assert!(r.units.is_empty());
    let a = App::new(r, Config::default(), KeyMap::default());
    let t = draw(&a, 80, 20);
    assert!(t.contains("╭─ Not shown · 1 file"), "{t}");
    assert!(
        t.lines().any(|l| l.starts_with("│   logo.png  binary")),
        "{t}"
    );
}

#[test]
fn a_file_too_large_to_describe_says_so_and_shows_its_diff() {
    let mut a = App::new(review(), Config::default(), KeyMap::default());
    a.sel = a
        .order
        .iter()
        .position(|&i| a.review.units[i].file.ends_with("big.md"))
        .unwrap();
    let t = draw(&a, 200, 40);
    assert!(t.contains("too large to describe; read the diff"), "{t}");
    assert!(t.contains("code change"), "{t}");
    assert!(t.contains("+ line 0"), "{t}");
}

#[test]
fn all_done_waits_for_whole_files_and_names_what_was_not_shown() {
    let mut a = App::new(review(), Config::default(), KeyMap::default());
    // Every unit accepted but one whole file.
    let last = a
        .review
        .units
        .iter()
        .position(|u| u.file.ends_with("config.json"))
        .unwrap();
    for (i, u) in a.review.units.iter_mut().enumerate() {
        if i != last {
            u.decision = Decision::Accepted;
        }
    }
    assert!(!a.all_reviewed(), "a pending whole file holds it back");
    a.sel = a.order.iter().position(|&i| i == last).unwrap();
    key(&mut a, 'l');
    assert_eq!(a.overlay, Overlay::AllDone);
    let t = draw(&a, 100, 30);
    assert!(t.contains("Hurray! All changes reviewed."), "{t}");
    assert!(
        t.contains("2 files were not shown: latin1.txt, logo.png"),
        "{t}"
    );
}

#[test]
fn e_opens_a_whole_file_at_its_first_change() {
    let mut a = App::new(review(), Config::default(), KeyMap::default());
    a.sel = a
        .order
        .iter()
        .position(|&i| a.review.units[i].file.ends_with("new.md"))
        .unwrap();
    let effects = a.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('e'),
        KeyModifiers::NONE,
    )));
    assert_eq!(
        effects,
        [teer::tui::app::Effect::Edit {
            file: "docs/new.md".into(),
            line: 5
        }]
    );
}

/// Answers every unit in the prompt with one statement citing its first new line.
fn responder(prompt: &str, _schema: &Value) -> anyhow::Result<Value> {
    let mut units = Vec::new();
    let mut id = None;
    for line in prompt.lines() {
        if let Some(rest) = line.strip_prefix("### Unit ") {
            id = Some(rest.to_string());
        }
        if let (Some(uid), Some(r)) = (&id, line.strip_prefix("new lines: ")) {
            let first: u32 = r.split('-').next().unwrap().parse().unwrap_or(1);
            units.push(json!({
                "id": uid,
                "summary": "Says something new.",
                "changes": [{"marker": "changed", "statement": "Readers see new text.",
                    "risk": "low", "evidence": [{"side": "new", "line": first}]}],
                "undo": {"expensive": false, "why": "", "evidence": []}
            }));
            id = None;
        }
    }
    Ok(json!({ "units": units }))
}

#[tokio::test]
async fn whole_files_are_described_but_not_lock_files_snapshots_or_huge_ones() {
    let _env = isolate_env();
    let repo = fixture();
    let mut r = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let mock = MockAdapter::new(responder);
    let prompts = mock.prompts.clone();
    let session = SessionRef::fresh("mock", repo.path());
    teer::cli::prepare_all(&mut r, Arc::new(mock), session, 2).await;
    let all = prompts.lock().unwrap().join("\n=====\n");

    assert!(all.contains("### Unit README.md::(whole file)"), "{all}");
    assert!(all.contains("symbol: (whole file) (Markdown file, changed)"));
    assert!(all.contains("(JSON file, added)"));
    assert!(all.contains("(Markdown file, renamed from docs/old.md, changed)"));
    assert!(all.contains(teer::review::prompts::WHOLE_FILE_RULE));
    assert!(all.contains("    3 | Run it with `make run`."));
    for never in ["big.md", "Cargo.lock", "help.test.ts.snap", "logo.png"] {
        assert!(
            !all.contains(&format!("### Unit {never}")),
            "{never} was sent"
        );
    }
    let unit = |file: &str| r.units.iter().find(|u| u.file.ends_with(file)).unwrap();
    assert_eq!(unit("README.md").prep, Prep::Ready);
    assert_eq!(
        unit("README.md").changes[0].statement,
        "Readers see new text."
    );
    assert_eq!(unit("big.md").prep, Prep::Skipped);
    assert_eq!(unit("Cargo.lock").prep, Prep::Skipped);
}

#[test]
fn a_long_whole_file_sends_only_its_changes() {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("CHANGELOG.md", &lines(900, "entry"))
        .commit("init");
    let mut text = lines(900, "entry");
    text = text.replace("entry 700\n", "entry 700, fixed\n");
    repo.write("CHANGELOG.md", &text);
    let r = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let todo = r.to_prepare(None);
    let refs: Vec<_> = todo.iter().map(|(u, f)| (u, f)).collect();
    let p = teer::review::prompts::units_prompt(&refs, &Default::default());
    assert!(p.contains("  701 | entry 700, fixed"), "{p}");
    assert!(p.contains("  698 | entry 697"), "{p}");
    assert!(!p.contains("| entry 10\n"), "{p}");
    assert!(p.contains("  ...\n"), "{p}");
}

#[test]
fn an_old_saved_review_without_the_new_fields_still_loads() {
    let old: Review = serde_json::from_value(json!({
        "id": "r", "repo_root": "/r", "base": "b", "head": "h", "head_commit": "h",
        "agent_session": null, "created_at": "2026-10-01T00:00:00Z",
        "files": [], "units": [{
            "id": "a.rs::f", "file": "a.rs", "symbol": "f", "kind": "Function",
            "status": "Changed", "old_range": null, "new_range": null,
            "fingerprint": "x", "changes": [], "callers": [], "callees": [],
            "decision": "Accepted"
        }]
    }))
    .unwrap();
    assert!(old.skipped.is_empty());
    assert_eq!(old.units[0].decision, Decision::Accepted);
    // And the new shape round-trips.
    let r = review();
    let back: Review = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back.skipped, r.skipped);
    assert_eq!(whole(&back), whole(&r));
}

#[test]
fn approving_says_which_files_were_not_shown() {
    let r = review();
    assert_eq!(
        teer::approve::intro_for(&r),
        "Reviewed with teer. 2 files were not shown: latin1.txt (not UTF-8), logo.png (binary)."
    );
    let mut none = r.clone();
    none.skipped.clear();
    assert_eq!(
        teer::approve::intro_for(&none),
        teer::approve::DEFAULT_INTRO
    );
}
