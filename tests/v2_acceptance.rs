//! The plan's acceptance checklist (A1–A12 of `docs/v2-map.md` §11), each
//! scenario end to end: `teer` started as `cli::run_on` starts it (start
//! detection, saved state, agent and session discovery) in a fixture repo
//! with real git, then run as `tui::run` runs it (tests/common/run.rs),
//! with the keys of the scenario typed in Neovim notation. gh is a
//! recording script, the agent CLI a script that edits a file and exits,
//! the model a mock. Commits, posts, the hand-off and re-runs really run.
//!
//! Names later phases may choose differently are kept in the helpers right
//! below, so they can be adapted in one place.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::run::{
    ANSWER, ConfigFile, DRAFT_BODY, DRAFT_SUBJECT, Fakes, GithubPr, RECOMMEND, Session, launch,
    model, repo, save_v1_review, v1_fixture, v1_repo,
};
use teer::config::Config;
use teer::keymap::{Keymap, parse_keys};
use teer::review::model::{Decision, Unit};
use teer::tui::app::{Effect, Overlay, Screen};
use teer::tui::pr_pick::{Picked, Picker};
use teer::tui::unit::Panel;

use Decision::{Accepted as A, Investigate as F, Pending as P};

// ---- Guesses about names later phases choose -------------------------------------------

/// Is the note on `u` marked "code changed since this note" (Phase 7)?
fn marked_changed(u: &Unit) -> bool {
    u.note_is_stale()
}

/// What the sheet says first while units are pending (Phase 6).
fn units_pending(n: usize) -> String {
    format!("{n} units pending")
}

/// The warning for a v1 `[keys]` entry whose action is gone (Phase 8).
fn removed_warning(name: &str) -> String {
    format!("`[keys].{name}` is no longer used")
}

/// The unit view's sections that need an agent, and the ones that don't.
const AGENT_SECTIONS: [&str; 2] = ["What to do about it", "Second model"];
const CODE_SECTIONS: [&str; 2] = ["Why this level", "What changed"];

// ---- Fixtures --------------------------------------------------------------------------

const CALC_OLD: &str = "def total(a, b):\n    return a + b\n\n\ndef fee(x):\n    return x + 1\n\n\ndef tax(x):\n    return x * 0.2\n";
const CALC_NEW: &str = "def total(a, b):\n    return a + b + fee(a) + fee(b)\n\n\ndef fee(x):\n    return x + 2\n\n\ndef tax(x):\n    return x * 0.25\n";
/// What the agent makes of the note on `total`: `fee` and `tax` as they were.
const CALC_FIXED: &str = "def total(a, b):\n    return a + b + fee(a)\n\n\ndef fee(x):\n    return x + 2\n\n\ndef tax(x):\n    return x * 0.25\n";

/// `calc.py` changed and not committed: `total` (calls `fee`), `fee`, `tax`.
fn calc() -> common::FixtureRepo {
    let r = repo();
    r.write("calc.py", CALC_OLD).commit("init");
    r.write("calc.py", CALC_NEW);
    r
}

const SHOP_OLD: &str = "def price(q):\n    return q * 10\n\n\ndef rate(q):\n    return 0.2\n";
const SHOP_NEW: &str = "def price(q):\n    return q * 12\n\n\ndef rate(q):\n    return 0.3\n";

/// Claude Code is the agent; no second model, no web search (no confirm).
fn config() -> ConfigFile {
    ConfigFile::new("provider = \"claude\"\nsecond_opinion = false\ndive_web = false\n")
}

/// `teer <args>` in `dir`, the model describing the units in `high` as ▲,
/// once everything it started has come back.
async fn teer(
    dir: &Path,
    args: &[&str],
    cfg: &ConfigFile,
    fakes: &Fakes,
    high: &[&str],
) -> Session {
    let l = launch(dir, args, &cfg.path())
        .await
        .unwrap()
        .expect("a review");
    let mut s = Session::launched(l, model(high), fakes.tools());
    s.settle().await;
    s
}

fn sheet_open(s: &Session) -> bool {
    matches!(s.app.overlay, Overlay::Finish { .. })
}

/// `:w`, and the sheet's work done (the agent's draft of a commit message).
async fn finish(s: &mut Session) {
    if !sheet_open(s) {
        s.keys(":w<cr>");
    }
    assert!(sheet_open(s), "{:?} {}", s.app.overlay, s.status());
    s.settle().await;
}

fn unit<'a>(s: &'a Session, symbol: &str) -> &'a Unit {
    let id = s.id(symbol);
    s.app.review.unit(&id).unwrap()
}

fn commits(r: &common::FixtureRepo) -> usize {
    r.git(&["rev-list", "--count", "HEAD"])
        .trim()
        .parse()
        .unwrap()
}

/// The line teer leaves with, once it quit.
fn exit_line(s: &Session) -> Option<String> {
    s.quit.then(|| s.app.finish.exit_line.clone()).flatten()
}

/// Each heading of `want` on `screen` that has something under it before
/// the next heading.
fn sections_with_content(screen: &str, want: &[&str]) -> Vec<String> {
    let rows: Vec<&str> = screen.lines().collect();
    let mut shown = Vec::new();
    for h in want {
        let Some(y) = rows.iter().position(|r| r.contains(h)) else {
            continue;
        };
        let next = rows[y + 1..]
            .iter()
            .map(|r| r.trim_matches(|c: char| c.is_whitespace() || "│─┃".contains(c)))
            .find(|r| !r.is_empty());
        let filled = next.is_some_and(|r| !want.iter().any(|w| r.starts_with(w)));
        assert!(filled, "{h} is empty:\n{screen}");
        shown.push(h.to_string());
    }
    shown
}

/// Effects that ask the model something.
fn asks_model(e: &Effect) -> bool {
    matches!(
        e,
        Effect::Prepare(_)
            | Effect::Dive { .. }
            | Effect::InterpretTests { .. }
            | Effect::DraftCommitMessage(_)
            | Effect::Summarize
            | Effect::Ask { .. }
    )
}

// ---- A1 --------------------------------------------------------------------------------

#[tokio::test]
async fn agent_changes_nothing_wrong() {
    let r = calc();
    let fakes = Fakes::new();
    let cfg = config();
    let mut s = teer(&r.path(), &[], &cfg, &fakes, &["total"]).await;
    assert_eq!(s.counts(), (0, 0, 3));
    s.keys("3a");
    assert_eq!(s.counts(), (3, 0, 0));
    finish(&mut s).await;
    let before = commits(&r);
    s.keys("<cr>");
    assert_eq!(commits(&r), before + 1);
    let message = r.git(&["log", "-1", "--format=%B"]);
    assert!(message.starts_with(DRAFT_SUBJECT), "drafted: {message}");
    assert!(message.contains(DRAFT_BODY), "{message}");
    assert_eq!(r.git(&["show", "HEAD:calc.py"]), CALC_NEW);
    let line = exit_line(&s).expect("teer is done");
    assert!(line.starts_with("Committed "), "{line}");
}

// ---- A2 --------------------------------------------------------------------------------

#[tokio::test]
async fn agent_changes_one_fix() {
    let r = calc();
    let fakes = Fakes::new();
    fakes.agent(&format!("cat > calc.py <<'EOF'\n{CALC_FIXED}EOF"));
    let cfg = config();
    let mut s = teer(&r.path(), &[], &cfg, &fakes, &["total"]).await;
    // The ▲ comes first.
    assert_eq!(s.selected().as_deref(), Some("total"));
    s.keys("f");
    s.keys("charges the fee twice<cr>");
    s.keys("2a");
    assert_eq!(s.counts(), (2, 1, 0));
    finish(&mut s).await;
    s.keys("<cr>");
    // The agent edited the file, and teer re-ran without a key.
    assert!(
        fakes
            .agent_prompt()
            .is_some_and(|p| p.contains("charges the fee twice"))
    );
    assert_eq!(r.file("calc.py"), CALC_FIXED);
    assert!(
        s.until(Duration::from_secs(20), |s| s.reruns() == 1).await,
        "{:?}",
        s.events
    );
    s.settle().await;
    // On that unit, its note marked as changed; the rest as decided.
    assert_eq!(s.selected().as_deref(), Some("total"));
    let total = unit(&s, "total");
    assert_eq!(total.decision, P);
    assert_eq!(total.dive.note, "charges the fee twice");
    assert!(marked_changed(total));
    assert_eq!((s.decision("fee"), s.decision("tax")), (A, A));
    s.keys("a");
    finish(&mut s).await;
    let before = commits(&r);
    s.keys("<cr>");
    assert_eq!(commits(&r), before + 1);
    assert_eq!(r.git(&["show", "HEAD:calc.py"]), CALC_FIXED);
    assert!(exit_line(&s).is_some());
}

// ---- A3 --------------------------------------------------------------------------------

#[tokio::test]
async fn branch_before_pr() {
    let r = repo();
    r.write("calc.py", CALC_OLD).commit("init");
    r.git(&["checkout", "-q", "-b", "feature/fees"]);
    r.write("calc.py", CALC_NEW).commit("charge fees");
    r.write("README.md", "fees\n").commit("say so");
    assert_eq!(r.git(&["status", "--porcelain"]), "");
    let fakes = Fakes::new();
    let s = teer(&r.path(), &["--no-agent"], &config(), &fakes, &[]).await;
    let main = r.git(&["rev-parse", "main"]).trim().to_string();
    assert_eq!(s.app.review.base, main);
    assert_eq!(s.app.review.head, r.git(&["rev-parse", "HEAD"]).trim());
    let header = s.render(120, 12).lines().next().unwrap().to_string();
    assert!(
        header.contains("feature/fees since main, 2 commits"),
        "{header}"
    );
}

// ---- A4 --------------------------------------------------------------------------------

#[tokio::test]
async fn pr_approve() {
    let pr = GithubPr::new(&[("shop.py", SHOP_OLD)], &[("shop.py", SHOP_NEW)]);
    let fakes = Fakes::new();
    let mut s = teer(
        &pr.repo.path(),
        &[&pr.n.to_string()],
        &config(),
        &fakes,
        &[],
    )
    .await;
    assert_eq!(s.app.review.pr.as_ref().map(|p| p.number), Some(pr.n));
    assert_eq!(s.counts(), (0, 0, 2));
    s.keys("A");
    assert_eq!(s.counts(), (2, 0, 0));
    finish(&mut s).await;
    s.keys("<cr>");
    assert!(
        s.until(Duration::from_secs(20), |s| s.quit).await,
        "{}",
        s.status()
    );
    assert_eq!(fakes.gh_calls(), 1, "after the head check");
    let sent = fakes.gh_stdin(1);
    assert_eq!(sent["event"], "APPROVE");
    assert_eq!(sent["commit_id"], pr.head.as_str());
    assert_eq!(
        exit_line(&s).as_deref(),
        Some(format!("PR #{} approved on GitHub.", pr.n).as_str())
    );
    // The clone is as it was.
    assert_eq!(pr.repo.git(&["status", "--porcelain"]), "");
}

// ---- A5 --------------------------------------------------------------------------------

#[tokio::test]
async fn pr_request_changes() {
    let pr = GithubPr::new(&[("shop.py", SHOP_OLD)], &[("shop.py", SHOP_NEW)]);
    let fakes = Fakes::new();
    fakes.pr_list(&format!(
        r#"[{{"number":{n},"title":"Raise prices","url":"{url}","author":{{"login":"ana"}},"isDraft":false,"reviewDecision":"REVIEW_REQUIRED","updatedAt":"2026-11-01T10:00:00Z","headRefName":"pr"}}]"#,
        n = pr.n,
        url = pr.url()
    ));
    // `teer pr`: the picker, its list from gh, `⏎` on the pull request.
    let target = teer::pr_list::target_repo(&pr.repo.path()).unwrap();
    let listing = teer::pr_list::fetch(&fakes.tools().gh, &pr.repo.path(), target.clone())
        .await
        .unwrap();
    let mut picker = Picker::new(
        target,
        teer::tui::theme::Theme::new(Config::default().theme),
        Keymap::default(),
        chrono::Utc::now(),
    );
    picker.loaded(listing);
    let enter = parse_keys("<cr>").unwrap()[0].event();
    let Some(Picked::Pr(picked)) = picker.on_key(enter) else {
        panic!("picked a pull request");
    };
    assert_eq!(picked.url, pr.url());

    let mut s = teer(&pr.repo.path(), &[&picked.url], &config(), &fakes, &[]).await;
    s.keys("f");
    s.keys("why 12?<cr>");
    s.keys("f");
    s.keys("the rate is wrong<cr>");
    assert_eq!(s.counts(), (0, 2, 0));
    finish(&mut s).await;
    s.keys("<cr>");
    assert!(
        s.until(Duration::from_secs(20), |s| s.quit).await,
        "{}",
        s.status()
    );
    assert_eq!(fakes.gh_calls(), 1);
    let sent = fakes.gh_stdin(1);
    assert_eq!(sent["event"], "REQUEST_CHANGES");
    let comments = sent["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2, "inline: {sent}");
    let bodies: Vec<&str> = comments.iter().filter_map(|c| c["body"].as_str()).collect();
    assert!(bodies.iter().any(|b| b.contains("why 12?")), "{bodies:?}");
    assert!(bodies.iter().any(|b| b.contains("the rate is wrong")));
    assert!(
        !sent["body"].as_str().unwrap().trim().is_empty(),
        "a summary"
    );
}

// ---- A6 --------------------------------------------------------------------------------

#[tokio::test]
async fn look_closer() {
    let r = calc();
    let fakes = Fakes::new();
    let mut s = teer(&r.path(), &[], &config(), &fakes, &["total"]).await;
    assert_eq!(s.selected().as_deref(), Some("total"));
    let row = s.app.sel;
    // `l` on the ▲: the recommendation shows without a key.
    s.keys("l");
    assert_eq!(s.app.screen, Screen::Unit);
    assert!(
        s.until(Duration::from_secs(10), |s| s
            .render(140, 60)
            .contains(RECOMMEND))
            .await,
        "{}",
        s.render(140, 60)
    );
    // `K` streams an answer.
    s.keys("K");
    assert_eq!(s.app.unit.panel, Some(Panel::Ask));
    s.keys("what does it add?<cr>");
    assert!(
        s.until(Duration::from_secs(10), |s| s.events.contains(&"Answer"))
            .await
    );
    let answered = s.events.iter().position(|e| *e == "Answer").unwrap();
    assert!(
        s.events[..answered].contains(&"Asking"),
        "streamed: {:?}",
        s.events
    );
    assert!(s.render(140, 60).contains(ANSWER), "{}", s.render(140, 60));
    s.keys("<esc>");
    assert_eq!(s.app.screen, Screen::Unit);
    // `gd`: the definition of what it calls, in the review.
    s.keys("gd");
    if matches!(s.app.overlay, Overlay::Jump { .. }) {
        s.keys("<cr>");
    }
    assert_eq!(s.app.unit.id, s.id("fee"));
    // `h`: back on the row it was opened from.
    for _ in 0..3 {
        if s.app.screen == Screen::Unit {
            s.keys("h");
        }
    }
    assert_eq!(s.app.screen, Screen::Queue);
    assert_eq!(s.app.sel, row);
}

// ---- A7 --------------------------------------------------------------------------------

#[tokio::test]
async fn bulk_and_undo() {
    let r = v1_repo();
    let fakes = Fakes::new();
    let mut s = teer(&r.path(), &["--no-agent"], &config(), &fakes, &[]).await;
    assert_eq!(s.counts(), (0, 0, 5));
    s.keys("gg");
    let listed = s.listed();
    let three = &listed[..3];
    s.keys("Vjjf");
    s.keys("one note for three<cr>");
    for u in three {
        assert_eq!((s.decision(u), s.note(u)), (F, "one note for three".into()));
    }
    assert_eq!(s.counts(), (0, 3, 2));
    s.keys("u");
    assert_eq!(s.counts(), (0, 0, 5), "all three undone");
    for u in three {
        assert_eq!(s.note(u), "", "{u}");
    }
    s.keys("<c-r>");
    for u in three {
        assert_eq!((s.decision(u), s.note(u)), (F, "one note for three".into()));
    }
    // `.` on the next unit (as in Vim, it may take as many units as the
    // selection had: only the next one is checked).
    s.go_to(&listed[3]);
    s.keys(".");
    if matches!(s.app.overlay, Overlay::Note { .. }) {
        s.keys("<esc>");
    }
    assert_eq!(s.decision(&listed[3]), F);
    for u in three {
        assert_eq!(s.decision(u), F, "{u}");
    }
}

// ---- A8 --------------------------------------------------------------------------------

#[tokio::test]
async fn pending_warning() {
    let r = calc();
    let fakes = Fakes::new();
    let mut s = teer(&r.path(), &["--no-agent"], &config(), &fakes, &[]).await;
    s.keys("gg");
    s.keys("a");
    finish(&mut s).await;
    let screen = s.render(120, 40);
    assert!(screen.contains(&units_pending(2)), "{screen}");
    s.keys("n");
    assert!(!sheet_open(&s));
    let first = s.listed().into_iter().find(|u| s.decision(u) == P).unwrap();
    assert_eq!(s.selected(), Some(first));
}

// ---- A9 --------------------------------------------------------------------------------

#[tokio::test]
async fn resume() {
    let r = v1_repo();
    let fakes = Fakes::new();
    let mut s = teer(&r.path(), &["--no-agent"], &config(), &fakes, &[]).await;
    let listed = s.listed();
    s.go_to(&listed[0]);
    s.keys("a");
    s.go_to(&listed[1]);
    s.keys("f");
    s.keys("look again<cr>");
    s.go_to(&listed[3]);
    let decisions: Vec<Decision> = listed.iter().map(|u| s.decision(u)).collect();
    s.keys("q");
    assert!(s.quit);

    let s = teer(&r.path(), &["--no-agent"], &config(), &fakes, &[]).await;
    assert_eq!(s.selected().as_deref(), Some(listed[3].as_str()));
    let again: Vec<Decision> = listed.iter().map(|u| s.decision(u)).collect();
    assert_eq!(again, decisions);
    assert_eq!(s.note(&listed[1]), "look again");
}

// ---- A10 -------------------------------------------------------------------------------

/// Every `.rs` file under `dir`.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn keymap_rules() {
    let broken = Keymap::default().check_rules();
    assert!(broken.is_empty(), "{broken:#?}");
    // No key literal outside the keymap module (test modules aside).
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    let found: Vec<String> = files
        .iter()
        .filter(|p| !p.ends_with("src/keymap.rs"))
        .flat_map(|p| {
            let text = std::fs::read_to_string(p).unwrap();
            let code = text
                .split("#[cfg(test)]")
                .next()
                .unwrap_or_default()
                .to_string();
            code.lines()
                .enumerate()
                .filter(|(_, l)| l.contains("KeyCode::"))
                .map(|(i, l)| format!("{}:{}: {}", p.display(), i + 1, l.trim()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(found.is_empty(), "{found:#?}");
}

// ---- A11 -------------------------------------------------------------------------------

#[tokio::test]
async fn v1_user() {
    let r = v1_repo();
    save_v1_review(&r);
    let cfg = ConfigFile::new(&std::fs::read_to_string(v1_fixture("config.toml")).unwrap());
    // Warnings for the removed actions only.
    let (_, warnings) = cfg.load().keymap().expect("the v1 keys load");
    let mut want = vec![removed_warning("prev_card"), removed_warning("swipe")];
    want.sort();
    let mut got: Vec<String> = warnings
        .iter()
        .map(|w| {
            want.iter()
                .find(|x| w.contains(x.as_str()))
                .cloned()
                .unwrap_or_else(|| w.clone())
        })
        .collect();
    got.sort();
    assert_eq!(got, want, "{warnings:?}");

    // The saved review goes on in the queue, with the v1 keys.
    let fakes = Fakes::new();
    let mut s = teer(&r.path(), &["--no-agent"], &cfg, &fakes, &[]).await;
    assert_eq!(s.app.screen, Screen::Queue);
    assert_eq!(
        ["total", "fee", "tax", "discount", "rounding"].map(|u| s.decision(u)),
        [F, F, F, A, P]
    );
    assert_eq!(s.note("total"), "charges the fee twice");
    s.go_to("rounding");
    s.keys("e");
    assert_eq!(s.decision("rounding"), A, "v1's `accept = \"e\"`");
    finish(&mut s).await;
    assert!(s.render(140, 40).contains("charges the fee twice"));
}

// ---- A12 -------------------------------------------------------------------------------

#[tokio::test]
async fn no_agent() {
    let r = calc();
    let fakes = Fakes::new();
    // An agent is set up; `--no-agent` leaves it out.
    let l = launch(&r.path(), &["--no-agent"], &config().path())
        .await
        .unwrap()
        .expect("a review");
    let m = model(&["total"]);
    let prompts = m.prompts.clone();
    let mut s = Session::launched(l, m, fakes.tools());
    s.settle().await;
    // The unit view: no empty sections, none that need the agent.
    s.keys("gg");
    s.keys("l");
    assert_eq!(s.app.screen, Screen::Unit);
    let screen = s.render(140, 60);
    let all = [CODE_SECTIONS, AGENT_SECTIONS].concat();
    assert_eq!(
        sections_with_content(&screen, &all),
        CODE_SECTIONS,
        "{screen}"
    );
    s.keys("h");
    // The agent-changes scenario: `a` presses, `:w`, `⏎`.
    s.keys("3a");
    finish(&mut s).await;
    let before = commits(&r);
    s.keys("<cr>");
    assert_eq!(commits(&r), before + 1);
    assert!(exit_line(&s).is_some());
    let asked: Vec<&Effect> = s.effects.iter().filter(|e| asks_model(e)).collect();
    assert!(asked.is_empty(), "{asked:?}");
    assert!(prompts.lock().unwrap().is_empty());
}
