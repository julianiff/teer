//! Story order in the list: inside a file, units that call each other stay
//! together, the caller first, callees indented under it with `↳`.
//! `order = "line"` keeps line order.

mod common;

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::config::{Config, KeyMap, Order};
use teer::review::model::{Decision, Prep, Range, Review, Unit, UnitKind, UnitStatus};
use teer::tui::app::App;
use teer::tui::ui;

/// A routine unit `name` at `line` of `file`, with the units it calls and is
/// called by.
fn unit(file: &str, name: &str, line: u32, callees: &[&str], callers: &[&str]) -> Unit {
    Unit {
        id: name.into(),
        file: PathBuf::from(file),
        symbol: name.into(),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: None,
        new_range: Some(Range::new(line, line + 5)),
        renamed_from: None,
        fingerprint: String::new(),
        changes: Vec::new(),
        prep: Prep::Skipped,
        callers: callers.iter().map(|s| s.to_string()).collect(),
        callees: callees.iter().map(|s| s.to_string()).collect(),
        decision: Decision::Pending,
        summary: Some("summary".into()),
        nbc_reason: None,
        dive: Default::default(),
        dive_pending: Default::default(),
        checks: Default::default(),
    }
}

/// handler → service → helper, and `other` on its own, in one file. In line
/// order the story is scattered: helper, handler, other, service.
fn units() -> Vec<Unit> {
    vec![
        unit("src/api.rs", "helper", 1, &[], &["service"]),
        unit("src/api.rs", "handler", 20, &["service"], &[]),
        unit("src/api.rs", "other", 40, &[], &[]),
        unit("src/api.rs", "service", 60, &["helper"], &["handler"]),
    ]
}

fn app(cfg: Config) -> App {
    app_of(units(), cfg)
}

fn app_of(units: Vec<Unit>, cfg: Config) -> App {
    common::isolate_env();
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
        files: Vec::new(),
        units,
        skipped: Vec::new(),
        graph: Default::default(),
        questions: Vec::new(),
    };
    App::new(review, cfg, KeyMap::default())
}

fn names(a: &App) -> Vec<&str> {
    a.order
        .iter()
        .map(|&i| a.review.units[i].symbol.as_str())
        .collect()
}

fn screen(a: &App) -> String {
    let mut t = Terminal::new(TestBackend::new(80, 20)).unwrap();
    t.draw(|f| ui::draw(f, a)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

fn depth(a: &App, name: &str) -> usize {
    let i = a
        .review
        .units
        .iter()
        .position(|u| u.symbol == name)
        .unwrap();
    a.depth[i]
}

#[test]
fn callers_come_first_and_callees_sit_under_them() {
    let a = app(Config::default());
    assert_eq!(names(&a), ["handler", "service", "helper", "other"]);
    let text = screen(&a);
    assert!(text.contains("│▌  ● handler"), "{text}");
    assert!(text.contains("\n│   ● ↳ service"), "{text}");
    assert!(text.contains("\n│   ●   ↳ helper"), "{text}");
    assert!(text.contains("\n│   ● other"), "{text}");
}

#[test]
fn line_order_is_still_there_when_asked_for() {
    let a = app(Config {
        order: Order::Line,
        ..Config::default()
    });
    assert_eq!(names(&a), ["helper", "handler", "other", "service"]);
    assert!(!screen(&a).contains('↳'));
    let cfg: Config = toml::from_str("order = \"line\"").unwrap();
    assert_eq!(cfg.order, Order::Line);
}

#[test]
fn a_caller_in_another_file_keeps_the_order_but_not_the_indent() {
    let mut units = units();
    units[1].file = PathBuf::from("src/b.rs");
    let a = app_of(units, Config::default());
    assert_eq!(names(&a), ["service", "helper", "other", "handler"]);
    assert_eq!(depth(&a, "service"), 0, "its caller is in another file");
    assert_eq!(depth(&a, "helper"), 1);
}

#[test]
fn a_file_lists_its_most_critical_units_first_and_indents_only_within_a_level() {
    // helper has no description now, so it is worth a look: it comes before
    // the routine story, and is not indented under service, which is routine.
    let mut a = app(Config::default());
    a.review.units[0].prep = Prep::Unavailable;
    a.rebuild_order();
    assert_ne!(a.levels[0].level, a.levels[3].level);
    assert_eq!(names(&a), ["helper", "handler", "service", "other"]);
    assert_eq!(depth(&a, "helper"), 0, "its caller is routine");
    assert_eq!(depth(&a, "service"), 1);
    // The selection stays on handler, which moved down.
    let text = screen(&a);
    assert!(text.contains("\n│   ◆ helper"), "{text}");
    assert!(text.contains("\n│▌  ● handler"), "{text}");
    assert!(text.contains("\n│   ● ↳ service"), "{text}");
}
