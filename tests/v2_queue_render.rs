//! The queue on screen (v2 Phase 4): the header says what is reviewed with
//! the change's summary under it; the rows show each unit's glyph, file,
//! name and state with the start of its note, and `⚠` when disputed; a
//! line counts the units accepted for you; the preview sits beside or
//! below; the bottom bar counts, keeps the mode and says when nothing is
//! pending; the note line; the help.

mod common;

use std::time::Instant;

use common::isolate_env;
use common::v2::{review, unit};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use teer::config::Config;
use teer::keymap::{Action, Keymap, parse_keys};
use teer::review::model::{
    BranchNames, Change, Commit, Decision, LineRef, Marker, PrInfo, PrMeta, Prep, Review, Risk,
    SecondOpinion, Side, Unit, UnitStatus, Verdict,
};
use teer::tui::app::{App, Grouping, Overlay, Screen, Target};
use teer::tui::{preview, ui};

/// The note on the flagged unit: longer than the 40 characters a row shows.
const NOTE: &str = "The retry is gone and nothing replaces it when the call times out.";

/// Unit `u{i}` named `name` in `file`, described with one statement of
/// `risk`: high is ▲, medium ◆, low ●.
fn described(i: usize, file: &str, name: &str, risk: Risk) -> Unit {
    let mut u = unit(i, file);
    u.symbol = name.into();
    u.prep = Prep::Ready;
    u.summary = Some(format!("Summary of {name}."));
    u.changes = vec![Change {
        marker: Marker::Changed,
        statement: format!("{name} changes."),
        evidence: vec![LineRef {
            side: Side::New,
            line: 1,
        }],
        risk,
    }];
    u
}

/// A second model that disputes `u`'s statement.
fn disputed(mut u: Unit) -> Unit {
    let statement = u.changes[0].statement.clone();
    u.checks.second = Some(SecondOpinion {
        risk: u.changes[0].risk,
        verdicts: vec![Verdict {
            statement,
            agree: false,
            why: "the retry moved to client.rs".into(),
            evidence: Vec::new(),
        }],
        missed: Vec::new(),
    });
    u
}

/// A unit with no behavior change: accepted for you.
fn no_change(i: usize, file: &str) -> Unit {
    let mut u = unit(i, file);
    u.status = UnitStatus::NoBehaviorChange;
    u
}

/// ▲ `refund` pending, ◆ `create_order` flagged with [`NOTE`], ● `cart_row`
/// accepted, ● `total` pending; two units accepted for you.
fn units() -> Vec<Unit> {
    let mut flagged = described(1, "src/api/routes.rs", "create_order", Risk::Medium);
    flagged.decision = Decision::Investigate;
    flagged.dive.note = NOTE.into();
    let mut accepted = described(2, "src/ui/cart.rs", "cart_row", Risk::Low);
    accepted.decision = Decision::Accepted;
    vec![
        described(0, "src/billing/charge.rs", "refund", Risk::High),
        flagged,
        accepted,
        described(3, "src/ui/cart.rs", "total", Risk::Low),
        no_change(4, "src/ui/cart.rs"),
        no_change(5, "src/ui/style.rs"),
    ]
}

/// `review` on the queue, by level, the cursor on the first unit.
fn queue_of(review: Review) -> App {
    let _env = isolate_env();
    let mut app = App::new(review, Config::default(), Keymap::default());
    app.screen = Screen::Queue;
    app.grouping = Grouping::Unit;
    app.rebuild_order();
    app.sel = 0;
    app
}

fn queue() -> App {
    queue_of(review(units()))
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

fn screen(app: &App, w: u16, h: u16) -> String {
    rows(&draw(app, w, h)).join("\n")
}

/// The bottom bar drawn `w` wide.
fn bar(app: &App, w: u16) -> String {
    rows(&draw(app, w, 30)).pop().unwrap()
}

/// The row of unit `name` drawn `w` x `h`.
fn row_of(app: &App, name: &str, w: u16, h: u16) -> String {
    let s = screen(app, w, h);
    s.lines()
        .find(|l| l.starts_with("│") && l.contains(name))
        .unwrap_or_else(|| panic!("{name} is listed:\n{s}"))
        .to_string()
}

fn pos_of(app: &App, id: &str) -> usize {
    app.order
        .iter()
        .position(|&i| app.review.units[i].id == id)
        .unwrap_or_else(|| panic!("{id} is listed"))
}

/// Feed `keys` to the keymap, as normal mode does.
fn feed(app: &mut App, keys: &str) {
    let now = Instant::now();
    for k in parse_keys(keys).unwrap() {
        app.keys.feed(&k.event(), now);
    }
}

fn targets(app: &App) -> Vec<(Rect, Target)> {
    app.targets.borrow().clone()
}

fn commit(subject: &str) -> Commit {
    Commit {
        subject: subject.into(),
        ..Default::default()
    }
}

// ---- Header ------------------------------------------------------------

#[test]
fn the_header_says_what_is_reviewed_with_the_summary_under_it() {
    let mut r = review(units());
    r.intent.summary = Some("Adds refunds and\nretries failed charges.".into());
    let app = queue_of(r);
    let s = rows(&draw(&app, 120, 30));
    assert_eq!(s[0].trim_end(), " your changes");
    assert_eq!(s[1].trim_end(), " Adds refunds and retries failed charges.");
    // One line, cut on a narrow terminal.
    let s = rows(&draw(&app, 30, 20));
    assert!(s[1].ends_with('…'), "{}", s[1]);
    assert!(!s[2].contains("charges"), "{}", s[2]);

    // A branch since its base, by its commits.
    let mut r = review(units());
    r.head = "feature".into();
    r.branch = Some(BranchNames {
        head: "feature/refunds".into(),
        base: "main".into(),
    });
    r.intent.commits = (0..6).map(|i| commit(&format!("step {i}"))).collect();
    let app = queue_of(r);
    let s = rows(&draw(&app, 120, 30));
    assert_eq!(s[0].trim_end(), " feature/refunds since main, 6 commits");
    // No summary yet: the list starts under the header.
    assert!(s[1].starts_with("╭─ Needs a close look"), "{}", s[1]);

    // A pull request, by its author, then its title.
    let mut r = review(units());
    r.head = "pr".into();
    r.pr = Some(PrInfo {
        number: 482,
        ..Default::default()
    });
    r.intent.pr = Some(PrMeta {
        title: "Refunds".into(),
        author: "ana".into(),
        body: String::new(),
        size: Default::default(),
    });
    r.intent.summary = Some("Refunds a charge.".into());
    let app = queue_of(r);
    let s = rows(&draw(&app, 120, 30));
    assert_eq!(s[0].trim_end(), " PR #482 by ana · Refunds");
    assert_eq!(s[1].trim_end(), " Refunds a charge.");
}

#[test]
fn the_header_leaves_the_flagged_count_to_the_bar() {
    let app = queue();
    let s = rows(&draw(&app, 120, 30));
    assert!(!s[0].contains("flagged"), "{}", s[0]);
    assert!(s[29].contains("1 flagged"), "{}", s[29]);
}

// ---- Rows --------------------------------------------------------------

#[test]
fn rows_show_glyph_path_name_and_state() {
    let app = queue();
    let refund = row_of(&app, "refund", 100, 40);
    assert!(
        refund.contains("▌  ▲ src/billing/charge.rs  refund"),
        "{refund}"
    );
    assert!(refund.trim_end().ends_with("pending │"), "{refund}");
    let cart = row_of(&app, "cart_row", 100, 40);
    assert!(cart.contains("● src/ui/cart.rs"), "{cart}");
    assert!(cart.trim_end().ends_with("accepted │"), "{cart}");
    // Flagged: the first 40 characters of the note, and no more.
    let first: String = NOTE.chars().take(40).collect();
    let more: String = NOTE.chars().take(41).collect();
    let flagged = row_of(&app, "create_order", 100, 40);
    assert!(flagged.contains("◆ src/api/routes.rs"), "{flagged}");
    assert!(
        flagged.contains(&format!("flagged · “{first}…”")),
        "{flagged}"
    );
    assert!(!flagged.contains(&more), "{flagged}");
    // A short note whole, without the ellipsis.
    let mut app = queue();
    let i = app.order[pos_of(&app, "u1")];
    app.review.units[i].dive.note = "Retry?".into();
    let flagged = row_of(&app, "create_order", 100, 40);
    assert!(flagged.contains("flagged · “Retry?” │"), "{flagged}");
    // No note: the state alone.
    app.review.units[i].dive.note.clear();
    let flagged = row_of(&app, "create_order", 100, 40);
    assert!(flagged.trim_end().ends_with("flagged │"), "{flagged}");
}

#[test]
fn rows_are_grouped_by_level_then_file() {
    let app = queue();
    let s = screen(&app, 160, 40);
    let at = |what: &str| {
        s.find(what)
            .unwrap_or_else(|| panic!("{what} is drawn:\n{s}"))
    };
    assert!(at("Needs a close look · 1 unit") < at("refund"));
    assert!(at("refund") < at("Worth a look · 1 unit"));
    assert!(at("Worth a look") < at("create_order"));
    assert!(at("create_order") < at("Routine · 2 units"));
    // In a level, by file: cart.rs's units together.
    assert!(at("Routine") < at("cart_row") && at("Routine") < at("total"));
}

#[test]
fn a_disputed_unit_shows_the_warning() {
    let mut us = units();
    us[3] = disputed(us[3].clone());
    let app = queue_of(review(us));
    let total = row_of(&app, "total", 100, 40);
    assert!(total.contains("total ⚠"), "{total}");
    for name in ["refund", "create_order", "cart_row"] {
        let row = row_of(&app, name, 160, 40);
        assert!(!row.contains('⚠'), "{row}");
    }
}

#[test]
fn listed_by_file_the_files_are_labels_and_rows_leave_the_path_out() {
    let mut app = queue();
    app.grouping = Grouping::File;
    app.rebuild_order();
    let s = screen(&app, 160, 40);
    assert!(s.contains("│ src/ui/cart.rs"), "{s}");
    let total = row_of(&app, "total", 100, 40);
    assert!(!total.contains("cart.rs"), "{total}");
    assert!(total.trim_end().ends_with("pending │"), "{total}");
}

#[test]
fn the_auto_line_counts_units_accepted_for_you_and_za_shows_them() {
    let mut app = queue();
    let s = screen(&app, 160, 40);
    assert!(
        s.contains(" ○ 2 units accepted for you, za shows them"),
        "{s}"
    );
    assert!(!s.contains("Accepted for you ·"), "{s}");
    app.show_auto = true;
    app.rebuild_order();
    let s = screen(&app, 160, 40);
    assert!(s.contains("Accepted for you · 2 units"), "{s}");
    assert!(
        s.contains("○ 2 units accepted for you, za hides them"),
        "{s}"
    );
    let auto = row_of(&app, "s5", 100, 40);
    assert!(
        auto.contains("○ src/ui/style.rs ") && auto.contains(" s5 "),
        "{auto}"
    );
    assert!(auto.trim_end().ends_with("accepted │"), "{auto}");
    // None accepted for you: no line.
    let app = queue_of(review(units().into_iter().take(4).collect()));
    assert!(!screen(&app, 160, 40).contains("accepted for you"));
}

#[test]
fn visual_rows_are_marked() {
    let mut app = queue();
    app.visual = Some(app.review.units[app.order[0]].id.clone());
    app.sel = 1;
    let buf = draw(&app, 160, 40);
    let visual = app.theme.visual().bg;
    assert!(visual.is_some());
    let marked: Vec<usize> = targets(&app)
        .iter()
        .filter_map(|(r, t)| match t {
            Target::Unit(pos) if buf[(r.x + 6, r.y)].style().bg == visual => Some(*pos),
            _ => None,
        })
        .collect();
    assert_eq!(marked, vec![0, 1]);
}

#[test]
fn a_click_on_a_row_selects_it() {
    let mut app = queue();
    draw(&app, 160, 40);
    let pos = pos_of(&app, "u3");
    let (r, _) = *targets(&app)
        .iter()
        .find(|(_, t)| *t == Target::Unit(pos))
        .expect("the row is a target");
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.on_mouse(MouseEvent {
            kind,
            column: r.x + 8,
            row: r.y,
            modifiers: KeyModifiers::NONE,
        });
    }
    assert_eq!(app.sel, pos);
}

// ---- Preview -----------------------------------------------------------

/// Where the preview was drawn: the area it registered for the wheel.
fn preview_area(app: &App) -> Rect {
    targets(app)
        .iter()
        .find_map(|(r, t)| (*t == Target::Detail).then_some(*r))
        .expect("a preview")
}

#[test]
fn the_preview_sits_beside_on_a_wide_terminal_and_below_on_a_narrow_one() {
    let app = queue();
    let s = screen(&app, 160, 40);
    let wide = preview_area(&app);
    assert!(wide.x > 0 && wide.y == 1, "{wide:?}");
    // Why this level, then what the agent said.
    let why = s.find("Why this level").expect(&s);
    let said = s.find("Summary of refund.").expect(&s);
    assert!(why < said, "{s}");
    let line = s.lines().find(|l| l.contains("Why this level")).unwrap();
    assert!(line.contains("refund"), "the row beside it: {line}");

    let s = screen(&app, 100, 40);
    let narrow = preview_area(&app);
    assert!(narrow.x == 0 && narrow.y > 1, "{narrow:?}");
    assert!(s.contains("Summary of refund."), "{s}");
    let line = s.lines().find(|l| l.contains("Why this level")).unwrap();
    assert!(!line.contains('│'), "below the list: {line}");

    // The layout the queue draws is the one `layout_of` gives it.
    for w in [100u16, 160] {
        let (list, pane) = preview::layout_of(&app, Rect::new(0, 1, w, 38));
        let (pane, _) = pane.unwrap();
        draw(&app, w, 40);
        assert_eq!(preview_area(&app), pane, "{w}");
        assert!(list.width <= w);
    }
}

#[test]
fn beside_the_preview_the_queue_takes_up_to_45_percent() {
    let app = queue();
    let (list, _) = preview::layout_of(&app, Rect::new(0, 1, 120, 38));
    assert!(list.width > 42 && list.width <= 54, "{list:?}");
}

#[test]
fn a_change_with_code_previews_why_then_its_code() {
    let repo = common::FixtureRepo::new();
    repo.write("checkout.py", common::PY_OLD).commit("init");
    repo.write("checkout.py", common::PY_NEW);
    let r = teer::review::build_review(&repo.path(), &Default::default()).unwrap();
    let app = queue_of(r);
    let name = app.selected().unwrap().symbol.clone();
    // Beside the rows: the selected unit's row, then why, then its code.
    let s = screen(&app, 160, 40);
    let lines: Vec<&str> = s.lines().collect();
    let why = lines
        .iter()
        .position(|l| l.contains("Why this level"))
        .expect(&s);
    let code = lines
        .iter()
        .position(|l| l.contains("╭ code change"))
        .expect(&s);
    assert!(why < code, "{s}");
    assert!(lines[why].contains(&format!("{name}  pending │")), "{s}");
    let x = lines[code].find("╭ code change").unwrap();
    assert!(x > lines[code].find('│').unwrap_or(0), "{s}");
    // Below the rows on a narrow terminal.
    let s = screen(&app, 100, 40);
    let lines: Vec<&str> = s.lines().collect();
    let why = lines
        .iter()
        .position(|l| l.contains("Why this level"))
        .expect(&s);
    let code = lines
        .iter()
        .position(|l| l.contains("╭ code change"))
        .expect(&s);
    let last_row = lines.iter().rposition(|l| l.starts_with('╰')).unwrap();
    assert!(last_row < why && why < code, "{s}");
    assert!(lines[code].starts_with(" ╭ code change"), "{s}");
}

// ---- Bottom bar --------------------------------------------------------

#[test]
fn the_bottom_bar_counts_pending_flagged_and_accepted() {
    let app = queue();
    let line = bar(&app, 120);
    // Units accepted for you count as accepted.
    assert!(
        line.starts_with(" 2 pending · 1 flagged · 3 accepted"),
        "{line}"
    );
    // No buttons; the key to the keys on the right.
    assert!(!line.contains("open in editor"), "{line}");
    assert!(line.trim_end().ends_with("? keys"), "{line}");
    assert!(
        targets(&app)
            .iter()
            .any(|(r, t)| *t == Target::Action(Action::HelpOpen) && r.y == 29)
    );
}

#[test]
fn the_bottom_bar_says_when_nothing_is_pending() {
    let mut app = queue();
    for u in &mut app.review.units {
        if u.decision == Decision::Pending {
            u.decision = Decision::Accepted;
        }
    }
    let line = bar(&app, 120);
    assert!(
        line.starts_with(" Nothing pending. :w to finish · 1 flagged · 5 accepted"),
        "{line}"
    );
    // A click on `:w` finishes.
    let (r, _) = *targets(&app)
        .iter()
        .find(|(_, t)| *t == Target::Action(Action::FinishOpen))
        .expect(":w is a target");
    assert_eq!((r.x, r.y, r.width), (18, 29, 2));
    // Nothing flagged: not said.
    let i = app.order[pos_of(&app, "u1")];
    app.review.units[i].decision = Decision::Accepted;
    let line = bar(&app, 120);
    assert!(
        line.starts_with(" Nothing pending. :w to finish · 6 accepted"),
        "{line}"
    );
}

#[test]
fn the_finish_key_in_the_bar_follows_the_keymap() {
    let over = [(
        "finish.open".to_string(),
        teer::keymap::KeySpec::One(":x".into()),
    )]
    .into_iter()
    .collect();
    let mut app = queue();
    app.keys = Keymap::new(&over, 1000).unwrap().0;
    for u in &mut app.review.units {
        u.decision = Decision::Accepted;
    }
    assert!(bar(&app, 120).starts_with(" Nothing pending. :x to finish"));
}

#[test]
fn the_bottom_bar_keeps_the_mode_after_the_counts() {
    let mut app = queue();
    app.visual = Some(app.review.units[app.order[0]].id.clone());
    app.sel = 1;
    let line = bar(&app, 120);
    assert!(
        line.starts_with(" 2 pending · 1 flagged · 3 accepted  -- VISUAL --  a accept"),
        "{line}"
    );
    assert!(line.trim_end().ends_with("2 units"), "{line}");
    let t = targets(&app);
    assert!(
        t.iter()
            .any(|(_, t)| *t == Target::Action(Action::DecideAccept))
    );
    assert!(
        t.iter()
            .any(|(_, t)| *t == Target::Action(Action::ViewBack))
    );

    let mut app = queue();
    feed(&mut app, "g");
    let line = bar(&app, 160);
    assert!(
        line.starts_with(" 2 pending · 1 flagged · 3 accepted  g  g first unit · d what"),
        "{line}"
    );
    let mut app = queue();
    feed(&mut app, "3");
    assert!(bar(&app, 120).starts_with(" 2 pending · 1 flagged · 3 accepted  3 "));

    let mut app = queue();
    feed(&mut app, ":w");
    let buf = draw(&app, 120, 30);
    let line = rows(&buf).pop().unwrap();
    assert!(
        line.starts_with(" 2 pending · 1 flagged · 3 accepted  :w    :w finish"),
        "{line}"
    );
    // The cursor after what is typed.
    assert!(
        buf[(39, 29)]
            .style()
            .add_modifier
            .contains(ratatui::style::Modifier::REVERSED)
    );
    // Narrow: what is typed stays, the counts go.
    let line = bar(&app, 30);
    assert!(line.starts_with(" :w "), "{line}");
}

#[test]
fn the_status_shows_on_the_right_whole() {
    let mut app = queue();
    app.status = "Not a command: :nope".into();
    let line = bar(&app, 80);
    assert!(line.starts_with(" 2 pending"), "{line}");
    assert!(line.trim_end().ends_with("Not a command: :nope"), "{line}");
    assert!(!line.contains("? keys"), "{line}");
}

#[test]
fn the_bar_is_on_the_unit_view_and_under_the_finish_sheet() {
    let mut app = queue();
    app.screen = Screen::Unit;
    assert!(bar(&app, 120).starts_with(" 2 pending · 1 flagged · 3 accepted"));
    let mut app = queue();
    app.overlay = Overlay::Finish { sel: 0 };
    assert!(bar(&app, 120).starts_with(" 2 pending · 1 flagged · 3 accepted"));
}

#[test]
fn a_note_is_typed_in_the_bar() {
    let mut app = queue();
    app.overlay = Overlay::Note {
        id: "u0".into(),
        text: "check the retry".into(),
        back: None,
        also: Vec::new(),
        joins: None,
    };
    let s = rows(&draw(&app, 120, 30));
    let line = s.last().unwrap();
    assert!(
        line.starts_with(" note on refund: check the retry "),
        "{line}"
    );
    assert!(line.trim_end().ends_with("⏎ save · esc cancel"), "{line}");
    // In the bar only: no editor over the queue.
    let elsewhere = s[..29]
        .iter()
        .filter(|l| l.contains("check the retry"))
        .count();
    assert_eq!(elsewhere, 0, "{}", s.join("\n"));
    let keys: Vec<Target> = targets(&app).into_iter().map(|(_, t)| t).collect();
    let key = |s: &str| Target::Key(parse_keys(s).unwrap()[0].event());
    assert!(keys.contains(&key("<cr>")), "{keys:?}");
    assert!(keys.contains(&key("<esc>")), "{keys:?}");
    // A long note keeps its end in view.
    app.overlay = Overlay::Note {
        id: "u0".into(),
        text: format!("{} the end", "x".repeat(200)),
        back: None,
        also: Vec::new(),
        joins: None,
    };
    let line = bar(&app, 80);
    assert!(
        line.starts_with(" note on refund: …xxx") && line.contains("xx the end "),
        "{line}"
    );
}

// ---- Help --------------------------------------------------------------

#[test]
fn the_help_lists_the_queues_keys() {
    let mut app = queue();
    app.overlay = Overlay::Help;
    let s = screen(&app, 160, 50);
    for want in [
        "keys · queue",
        "a f  accept / flag the unit",
        "A F  accept / flag the file's pending units",
        "za  show units accepted for you",
        "zf  list by file or by level",
        "␣c  commits and PR description",
        ":w  finish",
        "○ accepted for you",
        "⚠ disputed",
    ] {
        assert!(s.contains(want), "{want}:\n{s}");
    }
    for gone in ["swipe", "needs fix", "zoom", "next hunk"] {
        assert!(!s.contains(gone), "{gone}:\n{s}");
    }
}

// ---- Empty -------------------------------------------------------------

#[test]
fn an_empty_queue_says_what_is_left() {
    // Only units accepted for you: the line that shows them.
    let app = queue_of(review(vec![no_change(0, "a.rs"), no_change(1, "b.rs")]));
    let s = screen(&app, 100, 20);
    assert!(
        s.contains("○ 2 units accepted for you, za shows them"),
        "{s}"
    );
    assert!(
        bar(&app, 100).starts_with(" Nothing pending. :w to finish · 2 accepted"),
        "{}",
        bar(&app, 100)
    );
    let app = queue_of(review(Vec::new()));
    assert!(screen(&app, 100, 20).contains("Nothing to review in these changes."));
}

#[test]
fn every_size_draws() {
    let mut app = queue();
    app.visual = Some(app.review.units[app.order[0]].id.clone());
    for (w, h) in [
        (20, 6),
        (40, 12),
        (60, 20),
        (80, 24),
        (109, 30),
        (110, 30),
        (200, 60),
    ] {
        let s = rows(&draw(&app, w, h));
        assert_eq!(s.len(), h as usize);
    }
}
