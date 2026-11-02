//! Phase 7, the fix loop (L1–L7 of `docs/v2-map.md` §11): the notes go to
//! the agent CLI, in the session that wrote the change; teer re-runs when
//! the agent exits (Q13 a), or, when the notes went somewhere teer doesn't
//! run (copied, or the model API), once the tree has been quiet for
//! `fix_loop.quiet_seconds`, with a waiting line `q` stops; only the
//! changed units come back, pending, their notes kept and marked; `a`, `f`
//! and `i` on such a note; `:e` and `:e!`.
//!
//! Reviews run as `tui::run` runs them (tests/common/run.rs): real git in
//! temp repos, the agent CLI a script that edits a file and exits, the
//! model a mock. After the agent exits the session calls
//! `Runtime::agent_exited`, as the run loop does, and its tick drives
//! `Runtime::watch`.
//!
//! Names Phase 7's code may choose differently are kept in the helpers
//! right below, so they can be adapted in one place.

mod common;

use std::time::{Duration, Instant};

use common::run::{ConfigFile, Fakes, GithubPr, Session, launch, model, repo, wrote_session};
use serde_json::Value;
use teer::config::{Config, Provider};
use teer::keymap::Action;
use teer::review::model::{Decision, Unit};
use teer::review::{BuildOptions, build_review};
use teer::tui::app::{Effect, Overlay, Screen};

use Decision::{Accepted as A, Investigate as F, Pending as P};

// ---- Guesses about names Phase 7 chooses -----------------------------------------

/// The waiting line, while teer waits for changes it doesn't make itself:
/// it says how many notes are being worked on and ends "q to stop
/// waiting" (the plan: "Claude Code is working on 3 notes… (q to stop
/// waiting)"). Looked for on the screen, the status line included.
fn waiting_line(s: &Session) -> Option<String> {
    let want = format!("{} to stop waiting", s.key(Action::ViewClose));
    s.render(140, 30)
        .lines()
        .find(|l| l.contains(&want))
        .map(|l| l.trim().to_string())
}

/// The waiting line counts the notes sent.
fn counts_notes(line: &str, n: usize) -> bool {
    line.contains(&notes(n))
}

/// Is the note on `u` marked "code changed since this note"? Today's
/// marker is the fingerprint the note was written on (`Dive.note_on`).
fn marked_changed(u: &Unit) -> bool {
    u.note_is_stale()
}

/// What the unit view and the sheet say under such a note.
const CHANGED_MARK: &str = "code changed since this note";

/// What teer says after a re-run: "3 units changed by Claude Code".
fn changed_by(n: usize, by: &str) -> String {
    format!("{} changed by {by}", units(n))
}

/// `a` on a changed note archives it: it no longer goes anywhere (the
/// notes, `teer notes`, the agent), yet the unit keeps its text somewhere
/// in its saved form other than `dive.note`.
fn archived(u: &Unit, note: &str) -> bool {
    let mut v = serde_json::to_value(u).unwrap();
    v["dive"]["note"] = Value::Null;
    v.to_string().contains(note)
}

/// What `:e!` asks.
fn reset_question(n: usize) -> String {
    format!("Reset all {n} decisions? y/n")
}

/// `fix_loop.quiet_seconds`, short for the tests.
const QUIET_1S: &str = "[fix_loop]\nquiet_seconds = 1\n";

fn units(n: usize) -> String {
    if n == 1 {
        "1 unit".into()
    } else {
        format!("{n} units")
    }
}

fn notes(n: usize) -> String {
    if n == 1 {
        "1 note".into()
    } else {
        format!("{n} notes")
    }
}

// ---- Fixtures --------------------------------------------------------------------------

const CALC_OLD: &str = "def total(a, b):\n    return a + b\n\n\ndef fee(x):\n    return x + 1\n";
const CALC_NEW: &str =
    "def total(a, b):\n    return a + b + fee(a)\n\n\ndef fee(x):\n    return x + 2\n";
const SHIP_OLD: &str = "def ship(w):\n    return w * 3\n";
const SHIP_NEW: &str = "def ship(w):\n    return w * 4\n";
const SHIP_FIXED: &str = "def ship(w):\n    return w * 5\n";
const TAX_OLD: &str = "def tax(x):\n    return x * 0.2\n";
const TAX_NEW: &str = "def tax(x):\n    return x * 0.3\n";
const TAX_FIXED: &str = "def tax(x):\n    return x * 0.25\n";

/// Four changed functions, not committed: `total` and `fee` in `calc.py`,
/// `ship` in `ship.py`, `tax` in `tax.py`.
fn shop() -> common::FixtureRepo {
    let r = repo();
    r.write("calc.py", CALC_OLD)
        .write("ship.py", SHIP_OLD)
        .write("tax.py", TAX_OLD)
        .commit("init");
    r.write("calc.py", CALC_NEW)
        .write("ship.py", SHIP_NEW)
        .write("tax.py", TAX_NEW);
    r
}

/// The agent's work: `cat > tax.py` (and more) in the repo, then exit.
fn fixes(files: &[(&str, &str)]) -> String {
    files
        .iter()
        .map(|(f, c)| format!("cat > {f} <<'EOF'\n{c}EOF"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `shop` reviewed without a model, as `teer --no-agent` with `cfg`:
/// `total` flagged ("charges the fee twice"), `tax` flagged ("rate is
/// wrong"), `fee` and `ship` accepted.
fn decided(repo: &common::FixtureRepo, cfg: Config, fakes: &Fakes) -> Session {
    let review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    assert_eq!(review.units.len(), 4, "{:?}", review.units);
    let mut s = Session::start(review, cfg, None, None, fakes.tools());
    s.go_to("total");
    s.keys("f");
    s.keys("charges the fee twice<cr>");
    s.go_to("tax");
    s.keys("f");
    s.keys("rate is wrong<cr>");
    s.go_to("fee");
    s.keys("a");
    s.go_to("ship");
    s.keys("a");
    assert_eq!(s.counts(), (2, 2, 0));
    s
}

/// `:w`, then the default step: the notes go to the agent (or, with the
/// model API, on the clipboard).
fn send(s: &mut Session) -> Vec<Effect> {
    s.keys(":w<cr>");
    s.keys("<cr>")
}

fn hand_off(effects: &[Effect]) -> Option<(Provider, Option<String>, std::path::PathBuf)> {
    effects.iter().find_map(|e| match e {
        Effect::OpenAgent {
            cli, resume, cwd, ..
        } => Some((*cli, resume.clone(), cwd.clone())),
        _ => None,
    })
}

fn unit<'a>(s: &'a Session, symbol: &str) -> &'a Unit {
    let id = s.id(symbol);
    s.app.review.units.iter().find(|u| u.id == id).unwrap()
}

/// The decided `shop` review after the agent changed `tax.py` and exited.
async fn after_fix() -> (common::FixtureRepo, Fakes, Session) {
    let repo = shop();
    let fakes = Fakes::new();
    fakes.agent(&fixes(&[("tax.py", TAX_FIXED)]));
    let mut s = decided(&repo, Config::default(), &fakes);
    send(&mut s);
    assert!(
        s.until(Duration::from_secs(20), |s| s.reruns() == 1).await,
        "re-ran: {:?}",
        s.events
    );
    s.settle().await;
    (repo, fakes, s)
}

// ---- L1 ----------------------------------------------------------------------------------

#[tokio::test]
async fn hand_off_uses_found_session() {
    let repo = repo();
    repo.write("calc.py", CALC_OLD).commit("init");
    repo.write("calc.py", CALC_NEW);
    wrote_session(&repo.path(), "s-wrote", "calc.py");
    let config = ConfigFile::new("provider = \"claude\"\nsecond_opinion = false\n");
    let fakes = Fakes::new();
    let l = launch(&repo.path(), &[], &config.path())
        .await
        .unwrap()
        .expect("a review");
    assert_eq!(l.session_arg.as_deref(), Some("s-wrote"));
    let mut s = Session::launched(l, model(&["total"]), fakes.tools());
    s.settle().await;
    s.go_to("total");
    s.keys("f");
    s.keys("charges the fee twice<cr>");
    s.go_to("fee");
    s.keys("a");
    let prompt = teer::notes::agent_prompt(&s.app.review);
    let effects = send(&mut s);
    // Claude Code, resuming (a fork of) the session that wrote the change,
    // where it was started.
    let (cli, resume, cwd) = hand_off(&effects).expect("handed to the agent");
    assert_eq!(cli, Provider::Claude);
    assert_eq!(resume.as_deref(), Some("s-wrote"));
    assert_eq!(cwd, repo.path());
    let args = fakes.agent_args().expect("the agent ran").join("\n");
    assert!(args.starts_with("--resume\ns-wrote\n"), "{args}");
    assert_eq!(fakes.agent_prompt(), Some(prompt.clone()), "the notes");
    assert!(prompt.contains("charges the fee twice"));
    assert_eq!(fakes.agent_cwd(), Some(repo.path()));

    // Codex: no session to resume, in the repo.
    let config = ConfigFile::new("provider = \"codex\"\nsecond_opinion = false\n");
    let fakes = Fakes::new();
    let l = launch(&repo.path(), &[], &config.path())
        .await
        .unwrap()
        .expect("a review");
    let mut s = Session::launched(l, model(&["total"]), fakes.tools());
    s.settle().await;
    s.go_to("total");
    s.keys("f");
    s.keys("charges the fee twice<cr>");
    s.go_to("fee");
    s.keys("a");
    let effects = send(&mut s);
    let (cli, resume, cwd) = hand_off(&effects).expect("handed to the agent");
    assert_eq!((cli, resume), (Provider::Codex, None));
    assert_eq!(cwd, repo.path());
    assert_eq!(fakes.agent_name().as_deref(), Some("codex"));
}

// ---- L2 ----------------------------------------------------------------------------------

#[tokio::test]
async fn waiting_and_q() {
    // With the model API the notes are copied (Q11), and teer waits.
    let repo = shop();
    let fakes = Fakes::new();
    let cfg = ConfigFile::new(&format!("provider = \"api\"\n{QUIET_1S}")).load();
    let mut s = decided(&repo, cfg, &fakes);
    let effects = send(&mut s);
    assert!(
        !s.copied.is_empty(),
        "the notes are on the clipboard: {effects:?}"
    );
    assert_eq!(hand_off(&effects), None);
    let line = waiting_line(&s).unwrap_or_else(|| panic!("waiting:\n{}", s.render(140, 30)));
    assert!(counts_notes(&line, 2), "{line}");
    // The queue stays on show, read-only.
    assert_eq!(s.app.screen, Screen::Queue);
    let before: Vec<Decision> = s.app.review.units.iter().map(|u| u.decision).collect();
    for key in ["a", "f", "F", "A"] {
        s.keys(key);
        if matches!(s.app.overlay, Overlay::Note { .. }) {
            s.keys("<esc>");
        }
    }
    let after: Vec<Decision> = s.app.review.units.iter().map(|u| u.decision).collect();
    assert_eq!(after, before, "nothing is decided while teer waits");

    // `q` stops waiting: the queue again, teer still running.
    let effects = s.keys("q");
    assert!(!effects.contains(&Effect::Quit) && !s.quit, "{effects:?}");
    assert_eq!(waiting_line(&s), None);
    assert_eq!(s.app.screen, Screen::Queue);
    // Changes no longer bring a re-run...
    repo.write("tax.py", TAX_FIXED);
    s.wait(Duration::from_millis(2500)).await;
    assert_eq!(s.reruns(), 0, "{:?}", s.events);
    // ...`:e` does, later.
    let effects = s.keys(":e<cr>");
    assert!(effects.contains(&Effect::Rerun), "{effects:?}");
    assert!(s.until(Duration::from_secs(20), |s| s.reruns() == 1).await);
    assert_eq!(s.decision("tax"), P);
    // And keys decide again.
    s.go_to("tax");
    s.keys("a");
    assert_eq!(s.decision("tax"), A);
}

#[tokio::test]
async fn yanked_notes_wait_too() {
    // `y` on the sheet: the notes went somewhere teer doesn't run.
    let repo = shop();
    let fakes = Fakes::new();
    let mut s = decided(&repo, ConfigFile::new(QUIET_1S).load(), &fakes);
    s.keys(":w<cr>");
    s.keys("y");
    assert!(!s.copied.is_empty());
    let line = waiting_line(&s).unwrap_or_else(|| panic!("waiting:\n{}", s.render(140, 30)));
    assert!(counts_notes(&line, 2), "{line}");
    assert!(!matches!(s.app.overlay, Overlay::Finish { .. }));
}

// ---- L3 ----------------------------------------------------------------------------------

#[tokio::test]
async fn completion_by_signal() {
    // The agent CLI exits: teer re-runs at once, without a key.
    let repo = shop();
    let fakes = Fakes::new();
    fakes.agent(&fixes(&[("tax.py", TAX_FIXED)]));
    let mut s = decided(&repo, Config::default(), &fakes);
    let keys_before = s.effects.len();
    send(&mut s);
    assert!(fakes.agent_args().is_some(), "the agent ran");
    assert_eq!(repo.file("tax.py"), TAX_FIXED);
    assert!(
        s.until(Duration::from_secs(20), |s| s.reruns() == 1).await,
        "{:?}",
        s.events
    );
    assert!(s.effects.len() > keys_before);
    assert_eq!(s.decision("tax"), P);
    assert!(marked_changed(unit(&s, "tax")));
    // No waiting line: the agent's exit was the signal.
    assert_eq!(waiting_line(&s), None);
}

#[tokio::test]
async fn completion_by_quiet() {
    // Copied for the agent (the model API): teer watches the tree and
    // re-runs once it has been quiet for `fix_loop.quiet_seconds`.
    let repo = shop();
    let fakes = Fakes::new();
    let cfg = ConfigFile::new(&format!("provider = \"api\"\n{QUIET_1S}")).load();
    let mut s = decided(&repo, cfg, &fakes);
    send(&mut s);
    assert!(waiting_line(&s).is_some());
    // Nothing changed yet: nothing to re-run.
    s.wait(Duration::from_millis(1500)).await;
    assert_eq!(s.reruns(), 0, "{:?}", s.events);
    // The user's agent edits the file; teer re-runs a quiet second later.
    repo.write("tax.py", TAX_FIXED);
    let wrote = Instant::now();
    assert!(
        s.until(Duration::from_secs(15), |s| s.reruns() == 1).await,
        "{:?}",
        s.events
    );
    assert!(
        wrote.elapsed() >= Duration::from_millis(900),
        "only after a quiet second: {:?}",
        wrote.elapsed()
    );
    s.settle().await;
    assert_eq!(s.decision("tax"), P);
    assert!(marked_changed(unit(&s, "tax")));
    assert_eq!(waiting_line(&s), None, "done waiting");
}

#[test]
fn quiet_seconds_default_to_five() {
    // The key is read, and teer's default is 5 s: unknown keys would be
    // ignored, so read what a config with the key writes back.
    let v: toml::Value = toml::from_str(QUIET_1S).unwrap();
    assert_eq!(v["fix_loop"]["quiet_seconds"].as_integer(), Some(1));
    let cfg = ConfigFile::new(QUIET_1S).load();
    let shown = format!("{cfg:?}");
    assert!(shown.contains("quiet_seconds: 1"), "{shown}");
    let shown = format!("{:?}", Config::default());
    assert!(shown.contains("quiet_seconds: 5"), "{shown}");
}

// ---- L4 ----------------------------------------------------------------------------------

#[tokio::test]
async fn rerun_changed_vs_unchanged() {
    let (_repo, _fakes, mut s) = after_fix().await;
    // The changed unit: pending again, its note kept and marked.
    let tax = unit(&s, "tax");
    assert_eq!(tax.decision, P);
    assert_eq!(tax.dive.note, "rate is wrong");
    assert!(marked_changed(tax));
    // The others keep their decisions and notes, unmarked.
    assert_eq!(s.decision("total"), F);
    assert_eq!(s.note("total"), "charges the fee twice");
    assert!(!marked_changed(unit(&s, "total")));
    assert_eq!(s.decision("fee"), A);
    assert_eq!(s.decision("ship"), A);
    // On top of its level.
    let tax_at = s
        .app
        .order
        .iter()
        .position(|&i| s.app.review.units[i].symbol == "tax")
        .unwrap();
    let level = s.app.shown_level(s.app.order[tax_at]);
    let first_of_level = s
        .app
        .order
        .iter()
        .position(|&i| s.app.shown_level(i) == level)
        .unwrap();
    assert_eq!(tax_at, first_of_level, "{:?}", s.listed());
    // The mark shows with the note.
    s.go_to("tax");
    s.keys("l");
    let screen = s.render(140, 60);
    assert!(screen.contains(CHANGED_MARK), "{screen}");
}

// ---- L5 ----------------------------------------------------------------------------------

#[tokio::test]
async fn cursor_and_status_after_rerun() {
    let repo = shop();
    let fakes = Fakes::new();
    fakes.agent(&fixes(&[("tax.py", TAX_FIXED), ("ship.py", SHIP_FIXED)]));
    let mut s = decided(&repo, Config::default(), &fakes);
    s.go_to("total");
    send(&mut s);
    assert!(s.until(Duration::from_secs(20), |s| s.reruns() == 1).await);
    s.settle().await;
    let changed = ["ship", "tax"];
    let first = s
        .listed()
        .into_iter()
        .find(|u| changed.contains(&u.as_str()))
        .unwrap();
    assert_eq!(s.selected(), Some(first));
    let want = changed_by(2, "Claude Code");
    let screen = s.render(140, 30);
    assert!(
        screen.contains(&want) || s.status().contains(&want),
        "{want}:\n{screen}"
    );
}

// ---- L6 ----------------------------------------------------------------------------------

#[tokio::test]
async fn notes_on_changed_code() {
    // `i`: the note is edited, the decision stays.
    let (_repo, _fakes, mut s) = after_fix().await;
    s.go_to("tax");
    s.keys("i");
    assert!(
        matches!(&s.app.overlay, Overlay::Note { text, .. } if text == "rate is wrong"),
        "prefilled: {:?}",
        s.app.overlay
    );
    s.keys(" still<cr>");
    assert_eq!(s.decision("tax"), P);
    assert_eq!(s.note("tax"), "rate is wrong still");

    // `f`: flagged, and the note is edited.
    let (_repo, _fakes, mut s) = after_fix().await;
    s.go_to("tax");
    s.keys("f");
    assert!(
        matches!(&s.app.overlay, Overlay::Note { text, .. } if text == "rate is wrong"),
        "prefilled: {:?}",
        s.app.overlay
    );
    s.keys("<c-u>use 0.25<cr>");
    assert_eq!(s.decision("tax"), F);
    assert_eq!(s.note("tax"), "use 0.25");
    assert!(!marked_changed(unit(&s, "tax")), "written on this code");

    // `a`: accepted, and the note archived.
    let (_repo, _fakes, mut s) = after_fix().await;
    s.go_to("tax");
    s.keys("a");
    assert_eq!(s.decision("tax"), A);
    let tax = unit(&s, "tax");
    assert!(tax.dive.note.is_empty(), "{:?}", tax.dive);
    assert!(archived(tax, "rate is wrong"), "{:?}", tax.dive);
    let notes = teer::notes::markdown(&s.app.review);
    assert!(!notes.contains("rate is wrong"), "{notes}");
    assert!(notes.contains("charges the fee twice"), "{notes}");
    // `u` brings the note back with the decision.
    s.keys("u");
    assert_eq!(s.decision("tax"), P);
    assert_eq!(s.note("tax"), "rate is wrong");
}

// ---- L7 ----------------------------------------------------------------------------------

#[tokio::test]
async fn rerun_and_reset_commands() {
    let repo = shop();
    let fakes = Fakes::new();
    let mut s = decided(&repo, Config::default(), &fakes);

    // `:e` re-runs now and keeps the decisions of unchanged code.
    let effects = s.keys(":e<cr>");
    assert!(effects.contains(&Effect::Rerun), "{effects:?}");
    assert!(s.until(Duration::from_secs(20), |s| s.reruns() == 1).await);
    assert_eq!(s.counts(), (2, 2, 0));
    // ...and brings back what the user changed meanwhile.
    repo.write("ship.py", SHIP_FIXED);
    s.keys(":e<cr>");
    assert!(s.until(Duration::from_secs(20), |s| s.reruns() == 2).await);
    assert_eq!(s.decision("ship"), P);
    assert_eq!(s.counts(), (1, 2, 1));
    s.go_to("ship");
    s.keys("a");

    // `:e!` asks first; anything but yes changes nothing.
    let effects = s.keys(":e!<cr>");
    assert!(!effects.contains(&Effect::Rerun), "{effects:?}");
    let screen = s.render(140, 30);
    assert!(screen.contains(&reset_question(4)), "{screen}");
    let effects = s.keys("n");
    assert!(!effects.contains(&Effect::Rerun), "{effects:?}");
    assert_eq!(s.counts(), (2, 2, 0));

    // Yes: every decision reset, and the review re-run.
    s.keys(":e!<cr>");
    let effects = s.keys("y");
    assert!(effects.contains(&Effect::Rerun), "{effects:?}");
    assert!(s.until(Duration::from_secs(20), |s| s.reruns() == 3).await);
    s.settle().await;
    assert_eq!(s.counts(), (0, 0, 4));
}

#[tokio::test]
async fn reset_evaluates_what_is_accepted_for_you_again() {
    // `checkout.py` of the common fixture has a unit that only changed
    // formatting: accepted for you, before and after `:e!`.
    let r = repo();
    r.write("checkout.py", common::PY_OLD).commit("init");
    r.write("checkout.py", common::PY_NEW);
    let review = build_review(&r.path(), &BuildOptions::default()).unwrap();
    let fakes = Fakes::new();
    let mut s = Session::start(review, Config::default(), None, None, fakes.tools());
    let auto: Vec<String> = s
        .app
        .review
        .units
        .iter()
        .filter(|u| u.auto_accepted())
        .map(|u| u.id.clone())
        .collect();
    assert!(!auto.is_empty(), "a unit is accepted for you");
    while s.counts().2 > 0 {
        s.keys("a");
    }
    s.keys(":e!<cr>");
    s.keys("y");
    assert!(s.until(Duration::from_secs(20), |s| s.reruns() == 1).await);
    s.settle().await;
    for u in &s.app.review.units {
        if auto.contains(&u.id) {
            assert!(u.auto_accepted() && u.decision == A, "{}", u.id);
        } else {
            assert_eq!(u.decision, P, "{}", u.id);
        }
    }
}

#[tokio::test]
async fn rerun_on_a_pr_fetches_its_head_again() {
    // Q14 (a): `:e` on a pull request fetches the head first, so a pushed
    // fix shows up.
    let mut pr = GithubPr::new(&[("tax.py", TAX_OLD)], &[("tax.py", TAX_NEW)]);
    let config = ConfigFile::new("second_opinion = false\n");
    let fakes = Fakes::new();
    let l = launch(&pr.repo.path(), &[&pr.url(), "--no-agent"], &config.path())
        .await
        .unwrap()
        .expect("a review");
    let mut s = Session::launched(l, model(&[]), fakes.tools());
    assert_eq!(s.app.review.head, pr.head);
    s.keys("f");
    s.keys("rate is wrong<cr>");
    assert_eq!(s.decision("tax"), F);
    let new_head = pr.push(&[("tax.py", TAX_FIXED)]);
    s.keys(":e<cr>");
    assert!(s.until(Duration::from_secs(30), |s| s.reruns() == 1).await);
    s.settle().await;
    assert_eq!(s.app.review.head, new_head);
    assert_eq!(s.decision("tax"), P);
    assert!(marked_changed(unit(&s, "tax")));
}
