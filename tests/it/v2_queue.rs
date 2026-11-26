//! Phase 4, the queue (Q1–Q14 of `docs/v2-map.md` §11, and the acceptance
//! scenarios "bulk and undo" and "resume"): what teer opens on, its rows,
//! header, preview and bottom bar, and deciding in it. Keys go in as Neovim
//! notation through `App::on_key` on a review opened as `teer` opens it
//! (`App::new`, then `App::start`); real git in temp repos where a review is
//! saved and resumed. Nothing calls a model.
//!
//! Names the queue's code may choose differently are kept in the helpers
//! right below, so they can be adapted in one place.

use crate::common;

use common::v2::{Driver, find, review, unit};
use common::{FixtureRepo, isolate_env};
use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use teer::config::Config;
use teer::keymap::Action;
use teer::review::model::{
    Change, Decision, LineRef, Marker, PrInfo, PrMeta, Prep, Range, Review, Risk, SecondOpinion,
    Side, Unit, UnitStatus, Unmentioned, Verdict,
};
use teer::review::{BuildOptions, build_review};
use teer::session::app::{Effect, Overlay, Screen};

use Decision::{Accepted as A, Investigate as F, Pending as P};

// ---- Guesses about names the queue's code chooses ------------------------------

/// The note line open on a unit: its id and the text in it. (Phase 1 opens
/// it as `Overlay::Note`.)
fn note_line(d: &Driver) -> Option<(String, String)> {
    match d.overlay() {
        Overlay::Note { id, text, .. } => Some((id.clone(), text.clone())),
        _ => None,
    }
}

/// The bottom bar's counts (`4 pending · 1 flagged · 37 accepted`), a
/// count of none left out.
fn counts(pending: usize, flagged: usize, accepted: usize) -> String {
    [
        (pending, "pending"),
        (flagged, "flagged"),
        (accepted, "accepted"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, word)| format!("{n} {word}"))
    .collect::<Vec<_>>()
    .join(" · ")
}

/// The line under the list when units were accepted for you.
fn accepted_for_you_line(d: &Driver, n: usize) -> String {
    format!(
        "{} shows {n} accepted for you",
        d.app.keys.label(Action::ToggleAutoAccepted)
    )
}

// ---- Fixtures ---------------------------------------------------------------------

/// Unit `u{i}` named `name` in `file` at line `line`, pending, routine (┃).
fn named(i: usize, file: &str, name: &str, line: u32) -> Unit {
    let mut u = unit(i, file);
    u.symbol = name.into();
    u.new_range = Some(Range::new(line, line + 2));
    u
}

/// The units `names`, three per file (`f0.rs`, `f1.rs`, ...), in that order.
fn units_named(names: &[&str]) -> Vec<Unit> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| named(i, &format!("f{}.rs", i / 3), n, 1 + 10 * (i as u32 % 3)))
        .collect()
}

/// `n` pending routine units, three per file, opened as `teer --no-agent`.
fn queue(n: usize) -> Driver {
    let names: Vec<String> = (0..n).map(|i| format!("s{i}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    Driver::open(review(units_named(&names)), Config::default())
}

/// `n` pending routine units, a file each (a row of the tree per unit),
/// opened as `teer --no-agent`.
fn queue_singles(n: usize) -> Driver {
    Driver::open(review(common::v2::singles(n)), Config::default())
}

/// `units` opened as `teer --no-agent`.
fn queue_of(units: Vec<Unit>) -> Driver {
    Driver::open(review(units), Config::default())
}

fn at(line: u32) -> Vec<LineRef> {
    vec![LineRef {
        side: Side::New,
        line,
    }]
}

/// `u` described by the agent with one statement at `risk`, disputed by
/// the second model when `disputed`.
fn described(mut u: Unit, risk: Risk, disputed: bool) -> Unit {
    let statement = format!("Changes how {} works.", u.symbol);
    u.prep = Prep::Ready;
    u.changes = vec![Change {
        marker: Marker::Changed,
        statement: statement.clone(),
        evidence: at(1),
        risk,
    }];
    if disputed {
        u.checks.second = Some(SecondOpinion {
            risk: Risk::Low,
            verdicts: vec![Verdict {
                statement,
                agree: false,
                why: "the retry moved, it wasn't removed".into(),
                evidence: at(1),
            }],
            missed: Vec::new(),
        });
    }
    u
}

/// Unit `u{i}` with no behavior change: accepted for you.
fn same(i: usize, file: &str, name: &str) -> Unit {
    let mut u = named(i, file, name, 40 + i as u32);
    u.status = UnitStatus::NoBehaviorChange;
    u.nbc_reason = Some("formatting only".into());
    u
}

/// Set unit `id`'s decision without a key (the state a review was left in).
fn set(d: &mut Driver, id: &str, to: Decision) {
    d.app.review.unit_mut(id).unwrap().decision = to;
}

/// The row of the screen that first shows `text`.
fn row_with(screen: &str, text: &str) -> String {
    let (_, y) = find(screen, text).unwrap_or_else(|| panic!("{text:?} not shown:\n{screen}"));
    screen.lines().nth(y as usize).unwrap().to_string()
}

fn mouse(d: &mut Driver, kind: MouseEventKind, (column, row): (u16, u16)) -> Vec<Effect> {
    d.app.on_event(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }))
}

// ---- Q1 ---------------------------------------------------------------------------

#[test]
fn queue_is_the_default_screen() {
    let mut d = queue(6);
    assert_eq!(d.screen(), Screen::Queue);
    // Any unit opens, flagged or not; back is the queue.
    d.keys("l");
    assert_eq!(d.screen(), Screen::Unit);
    d.keys("h");
    assert_eq!(d.screen(), Screen::Queue);
    // `s` was v1's way into swipe; there is no swipe.
    d.keys("s");
    assert_eq!(d.screen(), Screen::Queue, "s opens no swipe from the queue");
}

// ---- Q2 ---------------------------------------------------------------------------

#[test]
fn header_says_what_is_reviewed() {
    let mut r = review(units_named(&["charge", "refund"]));
    r.intent.summary = Some("Adds refunds to the checkout.".into());
    let d = Driver::open(r, Config::default());
    let screen = d.render(120, 30);
    let top: Vec<&str> = screen.lines().take(3).collect();
    assert!(top[0].contains("your changes"), "{screen}");
    assert!(
        top.iter()
            .any(|l| l.contains("Adds refunds to the checkout.")),
        "one line of the change summary under the title:\n{screen}"
    );

    let mut r = review(units_named(&["charge"]));
    r.head = "c0ffee".into();
    r.pr = Some(PrInfo {
        host: "github.com".into(),
        owner: "acme".into(),
        repo: "shop".into(),
        number: 482,
        ..Default::default()
    });
    r.intent.pr = Some(PrMeta {
        title: "Refunds".into(),
        author: "ana".into(),
        ..Default::default()
    });
    let d = Driver::open(r, Config::default());
    let screen = d.render(120, 30);
    assert!(
        screen.lines().next().unwrap().contains("Refunds #482"),
        "{screen}"
    );
}

// ---- Q3 ---------------------------------------------------------------------------

#[test]
fn row_contents() {
    let note = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOP";
    let mut units = vec![
        named(0, "src/pay.rs", "charge", 1),
        named(1, "src/pay.rs", "refund", 10),
        named(2, "src/pay.rs", "retry", 20),
        described(named(3, "src/pay.rs", "settle", 30), Risk::Low, true),
    ];
    units[1].decision = A;
    units[2].decision = F;
    units[2].dive.note = note.into();
    let d = queue_of(units);
    let screen = d.render(100, 40);
    // The file's row: the icon of the most attention in it, its name,
    // then what there is to act on. No path, no count, no word for a state.
    assert_eq!(row_with(&screen, "pay.rs").trim_end(), "▌  ┃ pay.rs ⚑ ⚠");
    // Under the tree, a head line per unit: its icon, its name, and how it
    // was decided; the bar on the one the keys act on.
    for head in [" ┃ charge", " ┃ refund ✓", " ┃ retry ⚑", "▌┃ settle ⚠"] {
        assert!(
            screen.lines().any(|l| l.trim_end() == head),
            "{head}:\n{screen}"
        );
    }
    let body: Vec<&str> = screen.lines().collect();
    let body = body[..body.len() - 1].join("\n");
    for word in ["pending", "accepted", "flagged", "src/pay.rs"] {
        assert!(!body.contains(word), "{word}:\n{screen}");
    }
    // The reviewer's note is whole, on its card.
    assert!(screen.contains(note), "{screen}");
}

// ---- Q4 ---------------------------------------------------------------------------

#[test]
fn za_shows_auto_accepted() {
    let units = vec![
        named(0, "a.rs", "charge", 1),
        named(1, "a.rs", "refund", 10),
        same(2, "a.rs", "fmt_one"),
        same(3, "b.rs", "fmt_two"),
        same(4, "b.rs", "fmt_three"),
    ];
    let mut d = queue_of(units);
    let screen = d.render(120, 30);
    assert!(screen.contains(&accepted_for_you_line(&d, 3)), "{screen}");
    assert!(!screen.contains("fmt_one"), "hidden: {screen}");
    assert_eq!(d.app.order.len(), 2);
    assert!(
        d.bar(120, 30).contains(&counts(2, 0, 3)),
        "they count as accepted"
    );

    d.keys("za");
    assert_eq!(d.app.order.len(), 5);
    let screen = d.render(120, 30);
    // In the tree and the pane, behind their own icon.
    for shown in ["○ b.rs", "○ fmt_one ✓"] {
        assert!(screen.contains(shown), "{shown}: {screen}");
    }
    d.keys("j");
    let screen = d.render(120, 30);
    for name in ["fmt_two", "fmt_three"] {
        assert!(screen.contains(name), "{name}: {screen}");
    }
    d.keys("za");
    assert_eq!(d.app.order.len(), 2);
    assert!(!d.render(120, 30).contains("fmt_one"));
}

// ---- Q5 ---------------------------------------------------------------------------

#[test]
fn preview_side_or_below() {
    let mut units = units_named(&["alpha", "bravo", "charlie"]);
    units[0].checks.unmentioned = Some(Unmentioned {
        text: "Also stops sending the receipt.".into(),
        evidence: at(1),
    });
    let d = queue_of(units);
    assert_eq!(d.selected().as_deref(), Some("u0"));
    // The pane: a head line per unit of the file under the cursor.
    let head = "┃ bravo";

    // Wide: beside the tree.
    let wide = d.render(160, 30);
    let (x, y) = find(&wide, head).unwrap_or_else(|| panic!("{wide}"));
    let (fx, file) = find(&wide, "f0.rs").unwrap();
    assert!(x > fx + 7, "right of the tree: column {x}\n{wide}");
    assert!(y <= file + 2, "next to it, not under it: row {y}\n{wide}");

    // Narrow: below it.
    let narrow = d.render(80, 40);
    let (x, y) = find(&narrow, head).unwrap_or_else(|| panic!("{narrow}"));
    let (_, file) = find(&narrow, "f0.rs").unwrap();
    assert!(x < 80 / 4, "at the left: column {x}\n{narrow}");
    assert!(y > file, "under the tree: row {y}\n{narrow}");
}

// ---- Q12 --------------------------------------------------------------------------

#[test]
fn count_accepts_pending_units() {
    let mut d = queue_singles(6);
    set(&mut d, "u1", A);
    d.keys("3a");
    assert_eq!(d.decisions(), vec![A, A, A, A, P, P]);
    assert_eq!(d.selected().as_deref(), Some("u4"));
    assert_eq!(d.undo_steps(), 1);
    d.keys("u");
    assert_eq!(d.decisions(), vec![P, A, P, P, P, P]);
}

// ---- Q13 --------------------------------------------------------------------------

#[test]
fn mouse_select_and_copy() {
    let mut d = queue_singles(3);
    let screen = d.render(100, 30);
    let at = find(&screen, "f02.rs").unwrap();
    mouse(&mut d, MouseEventKind::Down(MouseButton::Left), at);
    mouse(&mut d, MouseEventKind::Up(MouseButton::Left), at);
    assert_eq!(d.selected().as_deref(), Some("u2"), "a click selects");
    assert_eq!(d.decisions(), vec![P, P, P], "and decides nothing");

    let screen = d.render(100, 30);
    let (x, y) = find(&screen, "f01.rs").unwrap();
    mouse(&mut d, MouseEventKind::Down(MouseButton::Left), (x, y));
    mouse(&mut d, MouseEventKind::Drag(MouseButton::Left), (x + 5, y));
    d.render(100, 30);
    let effects = mouse(&mut d, MouseEventKind::Up(MouseButton::Left), (x + 5, y));
    let copied = effects.iter().find_map(|e| match e {
        Effect::Copy { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert_eq!(copied.as_deref(), Some("f01.rs"), "{effects:?}");
}

// ---- Q14 --------------------------------------------------------------------------

const OLD: &str = "def one(x):\n    return x + 1\n\n\ndef two(x):\n    return x + 2\n\n\n\
def three(x):\n    return x + 3\n\n\ndef four(x):\n    return x + 4\n";
const NEW: &str = "def one(x):\n    return x + 10\n\n\ndef two(x):\n    return x + 20\n\n\n\
def three(x):\n    return x + 30\n\n\ndef four(x):\n    return x + 40\n";

/// A repo with `calc.py` changed in four functions, not committed.
fn changed_repo() -> FixtureRepo {
    isolate_env();
    let r = FixtureRepo::new();
    r.write("calc.py", OLD).commit("init");
    r.write("calc.py", NEW);
    r
}

/// `teer` in `repo`: the review built and merged with the saved one.
fn reopen(repo: &FixtureRepo) -> Driver {
    let mut r: Review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    teer::review::store::restore(&mut r);
    Driver::open(r, Config::default())
}

/// Type `keys` and save when they ask to, as the runtime does.
fn keys_saved(d: &mut Driver, keys: &str) -> Vec<Effect> {
    let effects = d.keys(keys);
    d.save_if_asked(&effects);
    effects
}

fn symbol(d: &Driver) -> String {
    let id = d.selected().unwrap();
    d.app.review.unit(&id).unwrap().symbol.clone()
}

fn decision_by_symbol(d: &Driver, name: &str) -> Decision {
    d.app
        .review
        .units
        .iter()
        .find(|u| u.symbol == name)
        .unwrap()
        .decision
}

#[test]
fn resume_after_quit_and_crash() {
    let repo = changed_repo();
    let mut d = reopen(&repo);
    assert_eq!(d.app.order.len(), 4);
    assert_eq!(symbol(&d), "one");
    // The queue decides on files: unit by unit is the unit view's.
    keys_saved(&mut d, "l");
    keys_saved(&mut d, "a");
    keys_saved(&mut d, "islow path<cr>");
    keys_saved(&mut d, "f");
    assert_eq!(symbol(&d), "three");
    assert_eq!(d.app.review.cursor.as_deref(), d.selected().as_deref());

    // A crash: nothing more is saved; the next start resumes from the last save.
    let mut d = reopen(&repo);
    assert_eq!(d.screen(), Screen::Queue);
    assert_eq!(symbol(&d), "three", "the cursor where it was");
    assert_eq!(decision_by_symbol(&d, "one"), A);
    assert_eq!(decision_by_symbol(&d, "two"), F);
    assert_eq!(decision_by_symbol(&d, "three"), P);
    let two = d
        .app
        .review
        .units
        .iter()
        .find(|u| u.symbol == "two")
        .unwrap();
    assert_eq!(two.dive.note, "slow path");
    assert_eq!(d.undo_steps(), 2, "the undo stack too");

    // `q`: saved where the cursor is.
    keys_saved(&mut d, "ln");
    assert_eq!(symbol(&d), "four");
    keys_saved(&mut d, "q");
    assert_eq!(d.screen(), Screen::Queue);
    let effects = keys_saved(&mut d, "q");
    assert!(effects.contains(&Effect::Quit), "{effects:?}");
    let mut d = reopen(&repo);
    assert_eq!(symbol(&d), "four");
    // Undo after the restart takes back the flag.
    keys_saved(&mut d, "u");
    assert_eq!(decision_by_symbol(&d, "two"), P);
}

// ---- Acceptance: bulk and undo -------------------------------------------------

#[test]
fn bulk_and_undo() {
    let mut d = queue_singles(9);
    d.keys("Vjjf");
    assert_eq!(d.decisions()[..4], [F, F, F, P]);
    d.keys("u");
    assert!(d.decisions().iter().all(|x| *x == P), "{:?}", d.decisions());
    d.keys("<c-r>");
    assert_eq!(d.decisions()[..4], [F, F, F, P]);
    // `.` on the next unit flags it.
    d.keys("n");
    let next = d.selected().unwrap();
    assert_eq!(d.decision_of(&next), P);
    d.keys(".");
    assert_eq!(d.decision_of(&next), F);
    assert_eq!(note_line(&d), None, "notes are not repeated");
    assert_eq!(d.decisions()[..3], [F, F, F]);
}
