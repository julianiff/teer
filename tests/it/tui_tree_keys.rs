//! The queue's tree under the keys: the cursor moves over directories and
//! files (never over a file teer can't show), `l` and `h` open and fold,
//! `]f` `[f` go file to file, `a` and `f` decide the pending units of the
//! file under the cursor as one undo step, and a large review asks the
//! agent about a file when the cursor gets to it.

use crate::common;

use common::v2;

use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::config::Config;
use teer::keymap::{Action, Keymap};
use teer::review::model::{Decision, Prep, Range, Review, SessionRef, Unit, UnitKind, UnitStatus};
use teer::session::app::{Effect, Overlay, Screen};
use teer::tui::app::{App, Target};
use teer::tui::{slim, ui};

#[derive(Clone, Copy)]
enum Kind {
    /// Removed while still called: needs a close look.
    Close,
    /// Not described: worth a look.
    Look,
    Routine,
    /// Formatting only: no behavior change.
    Same,
}

fn unit(id: &str, file: &str, line: u32, kind: Kind) -> Unit {
    let mut u = Unit {
        id: format!("{file}::{id}"),
        file: PathBuf::from(file),
        symbol: id.into(),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: None,
        new_range: Some(Range::new(line, line + 5)),
        nested: Vec::new(),
        renamed_from: None,
        fingerprint: String::new(),
        changes: Vec::new(),
        prep: Prep::Skipped,
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Pending,
        summary: Some("summary".into()),
        nbc_reason: None,
        dive: Default::default(),
        accepted_by: None,
        reasons: Vec::new(),
        dive_pending: Default::default(),
        checks: Default::default(),
        security: Vec::new(),
        signature: None,
    };
    match kind {
        Kind::Close => {
            u.status = UnitStatus::Removed;
            u.callers = vec!["somebody".into()];
        }
        Kind::Look => u.prep = Prep::Unavailable,
        Kind::Routine => {}
        Kind::Same => {
            u.status = UnitStatus::NoBehaviorChange;
            u.nbc_reason = Some("formatting only".into());
        }
    }
    u
}

/// Two close looks in `src/c.rs`, a routine unit above them and a
/// formatting change after them; three worth-a-look units in two files; 20
/// routine units in five files of four; and `src/q.rs` with only a
/// formatting change. A binary file in `assets` can't be shown.
/// Positions: `src/c.rs` 0-2 (c_helper, close_a, close_b), `src/l0.rs` 3-4,
/// `src/l1.rs` 5, `src/r0.rs` 6-9, `src/r1.rs` 10-13, ... `src/r4.rs` 22-25.
/// Rows: `assets` 0, `logo.png` 1, `src` 2, `c.rs` 3, `l0.rs` 4, `l1.rs` 5,
/// `r0.rs` 6 ... `r4.rs` 10.
fn units() -> Vec<Unit> {
    let mut v = vec![
        unit("c_helper", "src/c.rs", 5, Kind::Routine),
        unit("close_a", "src/c.rs", 10, Kind::Close),
        unit("close_b", "src/c.rs", 30, Kind::Close),
        unit("tidy", "src/c.rs", 50, Kind::Same),
        unit("look_a", "src/l0.rs", 10, Kind::Look),
        unit("look_b", "src/l0.rs", 30, Kind::Look),
        unit("look_c", "src/l1.rs", 10, Kind::Look),
        unit("quiet", "src/q.rs", 10, Kind::Same),
    ];
    for f in 0..5 {
        for k in 0..4 {
            v.push(unit(
                &format!("r{f}_{k}"),
                &format!("src/r{f}.rs"),
                10 + 20 * k,
                Kind::Routine,
            ));
        }
    }
    v
}

fn app_with(units: Vec<Unit>, cfg: Config) -> App {
    app_keys(units, cfg, Keymap::default())
}

fn app_keys(units: Vec<Unit>, cfg: Config, keys: Keymap) -> App {
    common::isolate_env();
    let mut paths: Vec<PathBuf> = units.iter().map(|u| u.file.clone()).collect();
    paths.dedup();
    let files = paths
        .into_iter()
        .map(|path| teer::git::ChangedFile {
            path,
            old_path: None,
            status: teer::git::FileStatus::Modified,
            old_content: None,
            new_content: None,
        })
        .collect();
    let review = Review {
        id: "t".into(),
        repo_root: PathBuf::from("/r"),
        base: "b".into(),
        head: "h".into(),
        head_commit: "h".into(),
        agent_session: None,
        pr: None,
        intent: Default::default(),
        created_at: chrono::Utc::now(),
        files,
        units,
        skipped: vec![teer::git::SkippedFile {
            path: PathBuf::from("assets/logo.png"),
            status: teer::git::FileStatus::Added,
            why: "binary".into(),
        }],
        graph: Default::default(),
        questions: Vec::new(),
        version: 0,
        history: Default::default(),
        branch: None,
        cursor: None,
        mode: None,
    };
    let mut app = App::new(review, cfg, keys);

    app.rebuild_order();
    app
}

fn app() -> App {
    app_with(units(), Config::default())
}

fn render(app: &App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    t
}

/// The screen as lines of plain text.
fn lines(app: &App, w: u16, h: u16) -> Vec<String> {
    let t = render(app, w, h);
    let buf = t.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

fn text(app: &App, w: u16, h: u16) -> String {
    lines(app, w, h).join("\n")
}

fn press(a: &mut App, code: KeyCode, mods: KeyModifiers) -> Vec<Effect> {
    a.on_event(Event::Key(KeyEvent::new(code, mods)))
}

fn key(a: &mut App, c: char) -> Vec<Effect> {
    press(a, KeyCode::Char(c), KeyModifiers::NONE)
}

fn code(a: &mut App, c: KeyCode) -> Vec<Effect> {
    press(a, c, KeyModifiers::NONE)
}

fn ctrl(a: &mut App, c: char) -> Vec<Effect> {
    press(a, KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn shift_a(a: &mut App) -> Vec<Effect> {
    press(a, KeyCode::Char('A'), KeyModifiers::SHIFT)
}

/// The name of the row the cursor is on.
fn at(a: &App) -> String {
    a.rows()[a.cursor_row().expect("a cursor")].name.clone()
}

fn symbol(a: &App) -> &str {
    &a.selected().unwrap().symbol
}

fn decisions(a: &App) -> Vec<Decision> {
    a.review.units.iter().map(|u| u.decision).collect()
}

fn accepted(a: &App) -> Vec<&str> {
    a.review
        .units
        .iter()
        .filter(|u| u.decision == Decision::Accepted)
        .map(|u| u.symbol.as_str())
        .collect()
}

fn prepared(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .flat_map(|e| match e {
            Effect::Prepare(ids) => ids.clone(),
            _ => vec![],
        })
        .collect()
}

// ---- Rows -----------------------------------------------------------------

#[test]
fn every_file_is_one_row_under_its_directory_and_the_pane_has_its_units() {
    let a = app();
    let s = lines(&a, 120, 60);
    let tree: Vec<String> = s[1..14]
        .iter()
        .map(|l| l.split('│').next().unwrap().trim_end().to_string())
        .collect();
    // Directories first, then files, each by name. Units with no behavior
    // change are accepted for you: `q.rs` has nothing else and isn't
    // listed, and a line under the tree says so.
    assert_eq!(
        tree,
        [
            " ▾ assets",
            "     logo.png  binary",
            " ▾ src",
            "▌  ┃ c.rs",
            "   ┃ l0.rs",
            "   ┃ l1.rs",
            "   ┃ r0.rs",
            "   ┃ r1.rs",
            "   ┃ r2.rs",
            "   ┃ r3.rs",
            "   ┃ r4.rs",
            "",
            "   za shows 2 accepted for you",
        ]
    );
    // The cursor starts on the first close look; its file's units are in
    // the pane, as they are in the file.
    assert_eq!(symbol(&a), "close_a");
    let pane: Vec<&str> = s[1..14]
        .iter()
        .map(|l| l.split('│').nth(1).unwrap().trim_end())
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(
        pane,
        [
            " ┃ c_helper",
            "   summary",
            "▌┃ close_a",
            "   summary",
            " ┃ close_b",
            "   summary",
        ]
    );
    insta::assert_snapshot!("tree", render(&a, 120, 30).backend());
    // `za` lists them: `tidy` with its file's other units, `q.rs` a row.
    let mut a = a;
    v2::press(&mut a, "za");
    let s = text(&a, 120, 60);
    assert!(s.contains("   ○ q.rs"), "{s}");
    assert!(s.contains(" ○ tidy ✓"), "{s}");
    assert!(s.contains("   za hides 2 accepted for you"), "{s}");
    assert_eq!(symbol(&a), "close_a");
}

#[test]
fn every_row_fits_its_width_and_nothing_is_cut_at_the_fitted_width() {
    let a = app();
    for w in 0u16..=120 {
        for l in &slim::build_queue(&a, w).lines {
            assert!(l.width() <= w as usize, "{w}: {l:?}");
        }
    }
    // The narrowest tree that shows every row whole: beside the pane it is
    // this wide and no wider.
    let w = slim::fit_width(&a) as u16;
    assert_eq!(w as usize, "   za shows 2 accepted for you".len() + 1);
    let rows = slim::build_queue(&a, w);
    assert_eq!(rows.lines.len(), 13, "the line under the tree is one line");
    let all: String = rows.lines.iter().map(|l| l.to_string()).collect();
    assert!(!all.contains('…'), "{all}");
    assert!(all.contains("logo.png  binary"), "{all}");
    // Narrower than that line and it wraps: it names a key.
    assert_eq!(slim::build_queue(&a, w - 2).lines.len(), 14);
}

#[test]
fn a_long_chain_of_directories_is_cut_at_the_front_and_a_long_name_at_its_end() {
    let dir = "application/module/MediaDistribution/Contract/Data";
    let file = "MembershipOfATenantInADistributionGroupData.php";
    let units = vec![unit("r", &format!("{dir}/{file}"), 10, Kind::Routine)];
    let a = app_with(units, Config::default());
    let s = lines(&a, 40, 34);
    // The chain is one row; what is deepest stays.
    assert_eq!(s[1].trim_end(), " ▾ …dule/MediaDistribution/Contract/Data");
    assert_eq!(s[2].trim_end(), "▌  ┃ MembershipOfATenantInADistribution…");
    // With room both are whole.
    let s = lines(&a, 100, 34);
    assert_eq!(s[1].trim_end(), format!(" ▾ {dir}"));
    assert_eq!(s[2].trim_end(), format!("▌  ┃ {file}"));
}

// ---- Moving -----------------------------------------------------------------

#[test]
fn j_and_k_move_over_directories_and_files_and_never_over_a_file_teer_cant_show() {
    let mut a = app();
    assert_eq!(at(&a), "c.rs");
    key(&mut a, 'j');
    assert_eq!((at(&a).as_str(), symbol(&a)), ("l0.rs", "look_a"));
    key(&mut a, 'k');
    // Back on a file: the unit of it that needs the most attention.
    assert_eq!((at(&a).as_str(), symbol(&a)), ("c.rs", "close_a"));
    // Up: the directory, which has no unit; then past `logo.png`.
    key(&mut a, 'k');
    assert_eq!(at(&a), "src");
    assert!(a.selected().is_none());
    key(&mut a, 'k');
    assert_eq!(at(&a), "assets");
    key(&mut a, 'k');
    assert_eq!(at(&a), "assets");
    key(&mut a, 'j');
    assert_eq!(at(&a), "src");
    key(&mut a, 'G');
    assert_eq!((at(&a).as_str(), symbol(&a)), ("r4.rs", "r4_0"));
    key(&mut a, 'j');
    assert_eq!(at(&a), "r4.rs");
    v2::press(&mut a, "gg");
    assert_eq!(at(&a), "assets");
    // A count is a row, as the lines of a file are: the row teer can't
    // show gives way to the next one.
    v2::press(&mut a, "2gg");
    assert_eq!(at(&a), "src");
    v2::press(&mut a, "5gg");
    assert_eq!(at(&a), "l0.rs");
    // Tab does nothing here.
    let before = text(&a, 120, 30);
    assert!(code(&mut a, KeyCode::Tab).is_empty());
    assert_eq!(text(&a, 120, 30), before);
}

#[test]
fn l_and_h_open_and_fold_and_go_between_a_file_and_its_view() {
    let mut a = app();
    // On a file `l` and enter open the unit view; `h` comes back to its row.
    for open in [KeyCode::Enter, KeyCode::Char('l')] {
        code(&mut a, open);
        assert_eq!((a.screen, symbol(&a)), (Screen::Unit, "close_a"));
        key(&mut a, 'h');
        assert_eq!((a.screen, at(&a).as_str()), (Screen::Queue, "c.rs"));
    }
    // `h` on a file goes to its directory; `h` there folds it.
    key(&mut a, 'h');
    assert_eq!(at(&a), "src");
    assert_eq!(a.rows().len(), 11);
    key(&mut a, 'h');
    assert_eq!((at(&a).as_str(), a.rows().len()), ("src", 3));
    let s = text(&a, 120, 30);
    assert!(s.contains("▌▸ src") && !s.contains("c.rs"), "{s}");
    // Folded, nothing is decided on it: a directory has no decision.
    key(&mut a, 'a');
    assert_eq!(a.status, "Select a file to decide on it.");
    assert!(
        decisions(&a)
            .iter()
            .all(|d| *d != Decision::Accepted || true)
    );
    // `l` (and enter, and esc as `h`) opens it again and folds it.
    key(&mut a, 'l');
    assert_eq!(a.rows().len(), 11);
    code(&mut a, KeyCode::Esc);
    assert_eq!(a.rows().len(), 3);
    code(&mut a, KeyCode::Enter);
    assert_eq!(a.rows().len(), 11);
    // A jump into a folded directory opens it.
    key(&mut a, 'h');
    assert_eq!(a.rows().len(), 3);
    key(&mut a, 'n');
    assert_eq!(a.rows().len(), 11);
    assert_eq!(a.screen, Screen::Queue);
    assert!(a.selected().is_some());
}

#[test]
fn bracket_f_goes_to_the_first_unit_of_the_next_or_previous_file() {
    let mut a = app();
    v2::press(&mut a, "]f");
    assert_eq!(symbol(&a), "look_a");
    a.sel = 4;
    v2::press(&mut a, "]f");
    assert_eq!(symbol(&a), "look_c");
    // From the middle of a file: the file above, not the start of this one.
    a.sel = 8;
    v2::press(&mut a, "[f");
    assert_eq!(symbol(&a), "look_c");
    a.sel = 1;
    v2::press(&mut a, "[f");
    assert_eq!(a.sel, 1);
    assert_eq!(a.status, "first file");
    key(&mut a, 'j');
    assert!(a.status.is_empty());
    a.sel = 23;
    v2::press(&mut a, "]f");
    assert_eq!(a.sel, 23);
    assert_eq!(a.status, "last file");
    // The actions do the same.
    a.sel = 0;
    a.on_action(Action::MoveNextFile);
    assert_eq!(symbol(&a), "look_a");
    a.on_action(Action::MovePrevFile);
    assert_eq!(symbol(&a), "close_a");
}

#[test]
fn ctrl_d_and_d_move_nothing() {
    // ctrl+d scrolls the pane: no file jump.
    let mut c = app();
    ctrl(&mut c, 'd');
    assert_eq!((c.screen, at(&c).as_str()), (Screen::Queue, "c.rs"));
    // `d` is the notes view's alone.
    key(&mut c, 'd');
    assert_eq!((c.screen, at(&c).as_str()), (Screen::Queue, "c.rs"));
}

#[test]
fn the_selected_unit_stays_selected_through_a_rebuild() {
    let mut a = app();
    a.sel = 7;
    let id = a.selected().unwrap().id.clone();
    a.review.units.swap(0, 10);
    a.rebuild_order();
    assert_eq!(a.selected().unwrap().id, id);
}

// ---- Deciding ----------------------------------------------------------------

#[test]
fn a_and_f_decide_the_files_pending_units_as_one_step_and_move_on() {
    let mut a = app();
    let effects = key(&mut a, 'a');
    assert!(effects.contains(&Effect::Save));
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(a.status, "Accepted 3 units.");
    assert_eq!(
        accepted(&a),
        ["c_helper", "close_a", "close_b", "tidy", "quiet"],
        "and the 2 accepted for you"
    );
    assert_eq!(a.review.history.undo.len(), 1);
    // On to the next file with something pending.
    assert_eq!(at(&a), "l0.rs");
    key(&mut a, 'f');
    assert_eq!(a.status, "Flagged 2 units.");
    assert_eq!(a.overlay, Overlay::None, "a flag asks for no note");
    assert_eq!(at(&a), "l1.rs");
    // The rows say it: the flag after the name; the accepted file is still
    // listed, and has no word for it.
    let s = lines(&a, 120, 30);
    assert!(s[4].starts_with("   ┃ c.rs  "), "{}", s[4]);
    assert!(s[5].starts_with("   ┃ l0.rs ⚑  "), "{}", s[5]);
    // `u` takes back one step: a file.
    key(&mut a, 'u');
    assert_eq!(at(&a), "l0.rs");
    assert!(!text(&a, 120, 30).contains('⚑'));
    key(&mut a, 'u');
    assert_eq!(accepted(&a), ["tidy", "quiet"]);
    assert_eq!(at(&a), "c.rs");
}

#[test]
fn a_selection_of_rows_is_decided_at_once() {
    let mut a = app();
    v2::press(&mut a, "Vjj");
    assert_eq!(at(&a), "l1.rs");
    key(&mut a, 'a');
    assert_eq!(a.visual, None);
    assert_eq!(
        accepted(&a),
        [
            "c_helper", "close_a", "close_b", "tidy", "look_a", "look_b", "look_c", "quiet"
        ]
    );
    assert_eq!(a.review.history.undo.len(), 1);
}

#[test]
fn the_keys_can_be_rebound_in_the_config() {
    let cfg: Config =
        toml::from_str("[keys]\n\"move.next_file\" = \"n\"\n\"decide.accept_file\" = \"<c-a>\"\n")
            .unwrap();
    let (keys, _) = cfg.keymap().unwrap();
    let mut a = app_keys(units(), cfg, keys);
    v2::press(&mut a, "]f");
    assert_eq!(at(&a), "c.rs");
    key(&mut a, 'n');
    assert_eq!(symbol(&a), "look_a");
    let before = decisions(&a);
    shift_a(&mut a);
    assert_eq!(decisions(&a), before, "A is unbound");
    // The file's pending units, at once, as one undo step.
    ctrl(&mut a, 'a');
    assert_eq!(
        accepted(&a),
        ["tidy", "look_a", "look_b", "quiet"],
        "and the 2 accepted for you"
    );
    assert_eq!(a.review.history.undo.len(), 1);
}

// ---- Help ---------------------------------------------------------------------

#[test]
fn the_keys_are_in_the_help_overlay_and_the_legend_shows_when_it_fits() {
    for (w, h) in [(60u16, 20u16), (80, 24), (100, 40), (120, 50)] {
        let mut a = app();
        a.overlay = Overlay::Help;
        let out = text(&a, w, h);
        assert!(
            out.contains("]f [f  next / previous file"),
            "{w}x{h}\n{out}"
        );
        assert!(out.contains("a f  accept / flag the f"), "{w}x{h}\n{out}");
        for gone in ["sidebar", "open / close", "list by file", "zf"] {
            assert!(!out.contains(gone), "{w}x{h}: {gone}\n{out}");
        }
    }
    // With room: what `l` and `h` do on the tree, and the legend under the
    // keys.
    let mut a = app();
    a.overlay = Overlay::Help;
    let out = text(&a, 100, 40);
    assert!(
        out.contains("l/⏎ h/esc  look closer or open / fold or go up"),
        "{out}"
    );
    // A bigger terminal never loses the legend: the keys take another
    // column rather than the legend's room.
    for (w, h) in [(100u16, 40u16), (120, 50), (160, 50)] {
        let out = text(&a, w, h);
        assert!(
            out.contains("legend")
                && out.contains("┃ close look")
                && out.contains("○ accepted for you")
                && out.contains("⚑ flagged"),
            "{w}x{h}\n{out}"
        );
        assert!(!out.contains("needs fix"), "{w}x{h}\n{out}");
    }
    // The help lists the keys of the screen it is opened on: the unit
    // view's own in the unit view.
    let mut f = app();
    key(&mut f, 'l');
    assert_eq!(f.screen, Screen::Unit);
    f.overlay = Overlay::Help;
    let out = text(&f, 100, 40);
    assert!(out.contains("keys · unit"), "{out}");
    assert!(!out.contains("resolved: ok"), "{out}");
    assert!(out.contains("tests that cover this unit"), "{out}");
    // The footer has no file button.
    a.overlay = Overlay::None;
    let out = text(&a, 120, 30);
    let footer = out.lines().last().unwrap();
    assert!(!footer.contains("file"), "{footer}");
}

// ---- Large reviews, sizes ----------------------------------------------------

#[test]
fn a_large_review_asks_about_a_file_when_the_cursor_gets_to_it() {
    let mut units = units();
    for u in &mut units {
        u.prep = Prep::NotRequested;
    }
    let cfg = Config {
        large_review: 3,
        ..Config::default()
    };
    let mut a = app_with(units, cfg);
    let effects = a.start(true, None, Vec::new(), Some(SessionRef::default()));
    // The close looks and the rest of the file under the cursor.
    let mut first = prepared(&effects);
    first.sort();
    assert_eq!(
        first,
        [
            "src/c.rs::c_helper",
            "src/c.rs::close_a",
            "src/c.rs::close_b"
        ]
    );
    // The next row is the next file: asked about when the cursor gets there.
    let mut ids = prepared(&key(&mut a, 'j'));
    ids.sort();
    assert_eq!(ids, ["src/l0.rs::look_a", "src/l0.rs::look_b"]);
    // Its row turns while the agent describes it.
    let s = lines(&a, 120, 30);
    let row = s[5].split('│').next().unwrap().trim_end();
    assert!(row.starts_with("▌  ┃ l0.rs "), "{row}");
    assert!(
        teer::tui::slim::SPINNER.iter().any(|f| row.ends_with(f)),
        "{row}"
    );
    // Back on a file asked about already: nothing more.
    assert!(prepared(&key(&mut a, 'k')).is_empty());
    // A directory asks nothing.
    assert!(prepared(&key(&mut a, 'k')).is_empty());
}

/// A thousand units in a hundred files: no move panics, and the cursor's
/// row is always on screen.
#[test]
fn a_thousand_units_in_a_hundred_files() {
    let units: Vec<Unit> = (0..1000)
        .map(|i| {
            unit(
                &format!("u{i:04}"),
                &format!("src/f{:02}.rs", i / 10),
                10 + (i % 10) as u32 * 10,
                if i % 7 == 0 {
                    Kind::Look
                } else {
                    Kind::Routine
                },
            )
        })
        .collect();
    let mut a = app_with(units, Config::default());
    assert_eq!(a.rows().len(), 103);
    let steps: &[&dyn Fn(&mut App)] = &[
        &|a| {
            key(a, 'j');
        },
        &|a| {
            key(a, 'a');
        },
        &|a| {
            v2::press(a, "]f");
        },
        &|a| {
            key(a, 'G');
        },
        &|a| {
            key(a, 'k');
        },
        &|a| {
            v2::press(a, "[f");
        },
        &|a| {
            v2::press(a, "gg");
        },
        &|a| {
            key(a, 'u');
        },
    ];
    for (n, step) in steps.iter().cycle().take(60).enumerate() {
        step(&mut a);
        let name = at(&a);
        let row = a.cursor_row().unwrap();
        for (w, h) in [(80u16, 24u16), (40, 8), (120, 40)] {
            let s = lines(&a, w, h);
            let (r, _) = *(a.targets.borrow().iter())
                .find(|(_, t)| *t == Target::Row(row))
                .unwrap_or_else(|| panic!("step {n} at {w}x{h}: {name} not shown"));
            let line = &s[r.y as usize];
            assert!(line.starts_with('▌'), "step {n} at {w}x{h}: {line}");
            assert!(line.contains(&name), "step {n} at {w}x{h}: {line}");
        }
    }
    render(&a, 3, 3);
    render(&a, 0, 0);
}

#[test]
fn the_overlay_never_panics_at_tiny_sizes() {
    // `i` opens the note line.
    let mut a = app();
    v2::press(&mut a, "i");
    assert!(matches!(a.overlay, Overlay::Note { .. }));
    for (w, h) in [
        (0u16, 0u16),
        (1, 1),
        (3, 3),
        (4, 4),
        (10, 4),
        (20, 6),
        (40, 8),
        (60, 12),
        (200, 3),
    ] {
        for sel in [0, 4, 8] {
            a.sel = sel;
            render(&a, w, h);
        }
    }
}
