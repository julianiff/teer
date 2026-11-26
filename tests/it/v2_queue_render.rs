//! The queue on screen: the header says what is reviewed with the change's
//! summary under it; the tree lists the changed files under their
//! directories, each behind the icon of its level, with `⚑` when flagged
//! and `⚠` when disputed; a line names the key that shows the units
//! accepted for you; the pane with the file's changes sits beside or below;
//! the bottom bar counts, keeps the mode and says when nothing is pending;
//! the note line; the help.

use crate::common;

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
use teer::session::app::{Overlay, Screen};
use teer::tui::app::{App, Target};
use teer::tui::{preview, slim, ui};

/// The note on the flagged unit.
const NOTE: &str = "The retry is gone and nothing replaces it when the call times out.";

/// Unit `u{i}` named `name` in `file`, described with one statement of
/// `risk`: high is ┃, medium ┃, low ┃.
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

/// ┃ `refund` pending, ┃ `create_order` flagged with [`NOTE`], ┃ `cart_row`
/// accepted, ┃ `total` pending; two units accepted for you.
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

    app.rebuild_order();
    app.sel = 0;
    // The rows where they always were: the attack-surface card over them
    // is security_queue.rs's.
    app.security.enabled = false;
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

/// The tree drawn beside the pane, `w` x `h`: its rows without the
/// header, the bar and the blank rows under it.
fn tree(app: &App, w: u16, h: u16) -> Vec<String> {
    assert!(w >= 110, "beside the pane");
    let s = rows(&draw(app, w, h));
    let mut tree: Vec<String> = s[1..s.len() - 1]
        .iter()
        .map(|l| l.split('│').next().unwrap().trim_end().to_string())
        .collect();
    while tree.last().is_some_and(String::is_empty) {
        tree.pop();
    }
    tree
}

/// The tree's row of the file or directory `name`, by its index.
fn row_at(app: &App, name: &str) -> usize {
    (app.rows().iter().position(|r| r.name == name)).unwrap_or_else(|| panic!("{name} is listed"))
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
        app.keys.feed(k, now);
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
    // Two lines on a narrow terminal.
    let s = rows(&draw(&app, 30, 20));
    assert_eq!(s[1].trim_end(), " Adds refunds and retries");
    assert_eq!(s[2].trim_end(), " failed charges.");
    // Never more than two: the second one is cut.
    let s = rows(&draw(&app, 16, 20));
    assert_eq!(s[1].trim_end(), " Adds refunds");
    assert!(s[2].trim_end().ends_with('…'), "{}", s[2]);
    assert!(!s[3].contains("charges"), "{}", s[3]);

    // A branch, by its name.
    let mut r = review(units());
    r.head = "feature".into();
    r.branch = Some(BranchNames {
        head: "feature/refunds".into(),
        base: "main".into(),
    });
    r.intent.commits = (0..6).map(|i| commit(&format!("step {i}"))).collect();
    let app = queue_of(r);
    let s = rows(&draw(&app, 120, 30));
    assert_eq!(s[0].trim_end(), " feature/refunds");
    // No summary yet: the tree starts under the header.
    assert!(s[1].starts_with(" ▾ src"), "{}", s[1]);

    // A pull request, by its title, then its number.
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
    assert_eq!(s[0].trim_end(), " Refunds #482");
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
fn the_tree_lists_directories_and_files_behind_their_marks() {
    let app = queue();
    assert_eq!(
        tree(&app, 160, 40),
        [
            " ▾ src",
            "   ▾ api",
            "▌    ┃ routes.rs ⚑",
            "   ▾ billing",
            "     ┃ charge.rs",
            "   ▾ ui",
            "     ┃ cart.rs",
            "",
            "   za shows 2 accepted for you",
        ]
    );
}

#[test]
fn a_row_says_no_more_than_its_mark_its_name_and_what_to_act_on() {
    let mut app = queue();
    let rows = tree(&app, 160, 40).join("\n");
    // No state in words, no path, no note, no count.
    let first: String = NOTE.chars().take(10).collect();
    for gone in ["pending", "flagged", "src/", &first, "1/", "unit"] {
        assert!(!rows.contains(gone), "{gone}:\n{rows}");
    }
    // A file with nothing left to do steps back: its row is muted.
    let cart = row_at(&app, "cart.rs");
    let name_fg = |app: &App| {
        let buf = draw(app, 160, 40);
        let (r, _) = *targets(app)
            .iter()
            .find(|(_, t)| *t == Target::Row(cart))
            .unwrap();
        let x = rows_text(&buf, r.y).find("cart.rs").unwrap() as u16;
        (buf[(x - 2, r.y)].style().fg, buf[(x, r.y)].style().fg)
    };
    let muted = app.theme.muted().fg;
    assert_ne!(name_fg(&app).1, muted);
    let i = app.order[pos_of(&app, "u3")];
    app.review.units[i].decision = Decision::Accepted;
    assert_eq!(name_fg(&app), (muted, muted));
}

/// Row `y` of `buf`, a byte per cell up to the first wide one.
fn rows_text(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width)
        .map(|x| {
            let s = buf[(x, y)].symbol();
            if s.len() == 1 { s } else { "?" }
        })
        .collect()
}

#[test]
fn the_mark_in_front_has_the_colour_of_the_files_level() {
    let app = queue();
    let buf = draw(&app, 160, 40);
    let mark = |name: &str| {
        let i = row_at(&app, name);
        let (r, _) = *targets(&app)
            .iter()
            .find(|(_, t)| *t == Target::Row(i))
            .unwrap();
        let x = rows_text(&buf, r.y).find(name).unwrap() as u16 - 2;
        (buf[(x, r.y)].symbol().to_string(), buf[(x, r.y)].style().fg)
    };
    let t = &app.theme;
    assert_eq!(mark("charge.rs"), ("┃".into(), Some(t.risk_high)));
    assert_eq!(mark("routes.rs"), ("┃".into(), Some(t.risk_medium)));
    // A routine file is calm.
    assert_eq!(mark("cart.rs"), ("┃".into(), t.muted().fg));
    // An open directory has no colour of its own.
    assert_eq!(mark("billing"), ("▾".into(), t.muted().fg));
}

#[test]
fn a_folded_directory_has_the_colour_of_the_most_attention_under_it() {
    let mut app = queue();
    let billing = row_at(&app, "billing");
    app.select_row(billing);
    common::v2::press(&mut app, "l");
    let rows = tree(&app, 160, 40);
    assert_eq!(rows[3], "▌  ▸ billing");
    assert!(!rows.iter().any(|r| r.contains("charge.rs")), "{rows:?}");
    let buf = draw(&app, 160, 40);
    assert_eq!(buf[(3, 4)].symbol(), "▸");
    assert_eq!(buf[(3, 4)].style().fg, Some(app.theme.risk_high));
    // The pane beside a directory is empty.
    let pane = preview_area(&app);
    for y in pane.y..pane.bottom() {
        let text: String = (pane.x + 1..pane.right())
            .map(|x| buf[(x, y)].symbol())
            .collect();
        assert_eq!(text.trim(), "", "row {y}");
    }
    // `l` opens it again.
    common::v2::press(&mut app, "l");
    assert_eq!(tree(&app, 160, 40)[4], "     ┃ charge.rs");
}

#[test]
fn a_disputed_unit_shows_the_warning() {
    let mut us = units();
    us[3] = disputed(us[3].clone());
    let mut app = queue_of(review(us));
    let rows = tree(&app, 160, 40);
    let cart = row_at(&app, "cart.rs");
    assert!(rows[cart].ends_with("cart.rs ⚠"), "{rows:?}");
    assert_eq!(rows.iter().filter(|r| r.contains('⚠')).count(), 1);
    // In the pane, on the unit it is about.
    app.select_row(cart);
    let s = screen(&app, 160, 40);
    assert!(s.contains("total ⚠"), "{s}");
    assert!(
        !s.contains("cart_row ⚠") && !s.contains("cart_row ✓ ⚠"),
        "{s}"
    );
}

#[test]
fn the_auto_line_names_the_key_that_shows_the_units_accepted_for_you() {
    let mut app = queue();
    let s = screen(&app, 160, 40);
    assert!(s.contains("   za shows 2 accepted for you"), "{s}");
    assert!(!s.contains("style.rs"), "{s}");
    app.show_auto = true;
    app.rebuild_order();
    let rows = tree(&app, 160, 40);
    assert!(rows.contains(&"     ○ style.rs".to_string()), "{rows:?}");
    assert_eq!(rows.last().unwrap(), "   za hides 2 accepted for you");
    // None accepted for you: no line.
    let app = queue_of(review(units().into_iter().take(4).collect()));
    assert!(!screen(&app, 160, 40).contains("accepted for you"));
}

#[test]
fn visual_rows_are_marked() {
    let mut app = queue();
    let (from, to) = (row_at(&app, "routes.rs"), row_at(&app, "charge.rs"));
    app.visual = Some(app.rows()[from].id());
    app.select_row(to);
    let buf = draw(&app, 160, 40);
    let visual = app.theme.visual().bg;
    assert!(visual.is_some());
    let marked: Vec<usize> = targets(&app)
        .iter()
        .filter_map(|(r, t)| match t {
            Target::Row(i) if buf[(r.right() - 1, r.y)].style().bg == visual => Some(*i),
            _ => None,
        })
        .collect();
    assert_eq!(marked, (from..=to).collect::<Vec<_>>());
}

#[test]
fn a_click_on_a_row_selects_it() {
    let mut app = queue();
    let click = |app: &mut App, name: &str| {
        draw(app, 160, 40);
        let i = row_at(app, name);
        let (r, _) = *targets(app)
            .iter()
            .find(|(_, t)| *t == Target::Row(i))
            .expect("the row is a target");
        // Not a double click.
        app.last_click = None;
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
    };
    // A file: the cursor on it, its lead unit selected.
    click(&mut app, "cart.rs");
    assert_eq!(app.selected().map(|u| u.id.as_str()), Some("u3"));
    // A directory: the cursor on it, and it folds; again, it opens.
    click(&mut app, "ui");
    assert_eq!(app.on_dir(), Some(std::path::Path::new("src/ui")));
    assert!(!tree(&app, 160, 40).iter().any(|r| r.contains("cart.rs")));
    click(&mut app, "ui");
    assert!(tree(&app, 160, 40).iter().any(|r| r.contains("cart.rs")));
}

// ---- The pane ------------------------------------------------------------

/// Where the pane was drawn: the area it registered for the wheel.
fn preview_area(app: &App) -> Rect {
    targets(app)
        .iter()
        .find_map(|(r, t)| (*t == Target::Detail).then_some(*r))
        .expect("a pane")
}

#[test]
fn the_pane_sits_beside_on_a_wide_terminal_and_below_on_a_narrow_one() {
    let app = queue();
    let s = screen(&app, 160, 40);
    let wide = preview_area(&app);
    assert!(wide.x > 0 && wide.y == 1, "{wide:?}");
    // The unit's head line, then what the agent says it does.
    let line = s
        .lines()
        .find(|l| l.contains("┃ create_order ⚑"))
        .expect(&s);
    assert!(line.starts_with(" ▾ src"), "beside the tree: {line}");
    assert!(s.contains("│   Summary of create_order."), "{s}");

    let s = screen(&app, 100, 40);
    let narrow = preview_area(&app);
    assert!(narrow.x == 0 && narrow.y > 1, "{narrow:?}");
    let lines: Vec<&str> = s.lines().collect();
    let head = (lines
        .iter()
        .position(|l| l.starts_with(" ┃ create_order ⚑")))
    .expect(&s);
    assert!(lines[head - 1].starts_with("────"), "under the rule: {s}");
    assert!(
        lines[head + 1].starts_with("   Summary of create_order."),
        "{s}"
    );

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
fn the_pane_leaves_what_the_unit_view_is_for() {
    let mut app = queue();
    app.has_agent = true;
    let s = screen(&app, 160, 40);
    for gone in [
        "review",
        "agent",
        "haiku",
        "code change",
        "Why this level",
        "src/api/routes.rs",
        "Nothing selected",
    ] {
        assert!(!s.contains(gone), "{gone}:\n{s}");
    }
}

#[test]
fn beside_the_pane_the_tree_is_as_wide_as_its_rows_and_at_most_45_percent() {
    let app = queue();
    let (list, _) = preview::layout_of(&app, Rect::new(0, 1, 120, 38));
    assert_eq!(list.width as usize, slim::fit_width(&app));
    assert_eq!(list.width, 31);
    // A long name: no wider than 45%.
    let mut us = units();
    us[0].file = format!("src/billing/{}.rs", "long_".repeat(20)).into();
    let app = queue_of(review(us));
    let (list, _) = preview::layout_of(&app, Rect::new(0, 1, 120, 38));
    assert_eq!(list.width, 54);
    // A short tree: no narrower than 24.
    let app = queue_of(review(vec![described(0, "a.rs", "f", Risk::Low)]));
    let (list, _) = preview::layout_of(&app, Rect::new(0, 1, 120, 38));
    assert_eq!(list.width, 24);
}

#[test]
fn the_pane_shows_every_unit_of_the_file_in_line_order_with_its_code() {
    let repo = common::FixtureRepo::new();
    repo.write("checkout.py", common::PY_OLD).commit("init");
    repo.write("checkout.py", common::PY_NEW);
    let r = teer::review::build_review(&repo.path(), &Default::default()).unwrap();
    let app = queue_of(r);
    let listed: Vec<String> = (app.order.iter())
        .map(|&i| app.review.units[i].symbol.clone())
        .collect();
    assert!(listed.len() > 1, "{listed:?}");
    for (w, beside) in [(160u16, true), (100, false)] {
        let s = screen(&app, w, 60);
        let lines: Vec<&str> = s.lines().collect();
        let at = |text: &str| {
            (lines.iter().position(|l| l.contains(text))).unwrap_or_else(|| panic!("{text}:\n{s}"))
        };
        // A head line per unit, top to bottom as in the file, each with
        // its code under it.
        let heads: Vec<usize> = listed.iter().map(|n| at(&format!(" {n}"))).collect();
        assert!(heads.windows(2).all(|p| p[0] < p[1]), "{heads:?}\n{s}");
        let code = at("def charge_order(order):");
        assert!(heads[0] < code && code < heads[1], "{s}");
        assert!(lines[code].contains("  8   def charge_order"), "{s}");
        let tree = at("checkout.py");
        if beside {
            assert_eq!(tree, heads[0], "{s}");
            assert!(lines[code].find('│').is_some(), "{s}");
        } else {
            assert!(tree < heads[0], "{s}");
            assert!(lines[code].starts_with("     8   def"), "{s}");
        }
        // No box around the code, no title, no count of lines.
        assert!(!s.contains("code change") && !s.contains("+2"), "{s}");
    }
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
    app.visual = Some(app.rows()[row_at(&app, "routes.rs")].id());
    let to = row_at(&app, "charge.rs");
    app.select_row(to);
    let line = bar(&app, 120);
    assert!(
        line.starts_with(" 2 pending · 1 flagged · 3 accepted  -- VISUAL --  a accept"),
        "{line}"
    );
    // The marked rows say how much it holds.
    assert!(line.trim_end().ends_with("h leave"), "{line}");
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
        "a f  accept / flag the file's pending units",
        "za  show units accepted for you",
        "l/⏎ h/esc  look closer or open / fold or go up",
        "⚑ flagged",
        "✓ accepted",
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
    assert!(s.contains("   za shows 2 accepted for you"), "{s}");
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
    app.visual = Some(app.rows()[0].id());
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
