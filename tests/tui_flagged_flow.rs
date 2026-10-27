//! The Flagged flow end to end with the mock agent: flag units, open the
//! screen, ask questions, web confirmation, results, resolution, notes.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::agent::mock::{MockAdapter, dive_responder};
use teer::config::{Config, KeyMap};
use teer::review::dive::{DiveEvent, run_dive};
use teer::review::model::{Decision, DiveKind, Review, SessionRef};
use teer::review::{BuildOptions, build_review};
use teer::tui::app::{App, AppEvent, Effect, Overlay, Screen};
use teer::tui::ui;

const W: u16 = 180;
const H: u16 = 40;

fn session() -> SessionRef {
    SessionRef {
        agent: "mock".into(),
        id: "s1".into(),
        cwd: PathBuf::from("/work/shop"),
    }
}

struct Fixture {
    app: App,
    adapter: MockAdapter,
    _repo: FixtureRepo,
}

fn fixture_with(cfg: Config, adapter: MockAdapter) -> Fixture {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD).commit("init");
    repo.write("checkout.py", PY_NEW);
    let mut review: Review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    review.agent_session = Some(session());
    let app = App::new(review, cfg, KeyMap::default());
    assert_eq!(app.screen, Screen::Slim);
    Fixture {
        app,
        adapter,
        _repo: repo,
    }
}

fn fixture() -> Fixture {
    fixture_with(Config::default(), MockAdapter::new(dive_responder))
}

fn key(a: &mut App, c: char) -> Vec<Effect> {
    a.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )))
}

fn code(a: &mut App, c: KeyCode) -> Vec<Effect> {
    a.on_event(Event::Key(KeyEvent::new(c, KeyModifiers::NONE)))
}

fn screen_text(app: &App) -> String {
    let mut t = Terminal::new(TestBackend::new(W, H)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..H)
        .map(|y| (0..W).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The resolution glyph of every list row (gutter, space, glyph).
/// The header row of each flagged unit in turn: the screen shows one.
fn heads(app: &mut App) -> Vec<String> {
    let at = app.flagged.sel;
    let heads = (0..app.flagged_units().len())
        .map(|i| {
            app.flagged.sel = i;
            screen_text(app)
                .lines()
                .nth(1)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    app.flagged.sel = at;
    heads
}

/// How each flagged unit stands, as its header row says.
fn row_glyphs(app: &mut App) -> Vec<char> {
    heads(app)
        .iter()
        .filter_map(|h| h.chars().nth(1).filter(|g| "⚑●✗".contains(*g)))
        .collect()
}

fn dive_effects(effects: &[Effect]) -> Vec<(String, DiveKind, bool)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Dive { unit_id, kind, web } => Some((unit_id.clone(), *kind, *web)),
            _ => None,
        })
        .collect()
}

/// Run the agent for every Dive effect and return the events, undelivered.
async fn run_effects(f: &Fixture, effects: &[Effect]) -> Vec<DiveEvent> {
    let adapter: Arc<dyn teer::agent::AgentAdapter> = Arc::new(f.adapter.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    for (id, kind, web) in dive_effects(effects) {
        let unit = f.app.review.unit(&id).unwrap().clone();
        let file = f
            .app
            .review
            .files
            .iter()
            .find(|c| c.path == unit.file)
            .unwrap()
            .clone();
        // As the runtime does: a picture gets the code it may cite.
        let target = (kind == DiveKind::Visualize)
            .then(|| teer::review::visual::target(&f.app.review, &unit));
        run_dive(
            adapter.clone(),
            session(),
            unit,
            file,
            kind,
            web,
            target,
            tx.clone(),
        )
        .await;
    }
    drop(tx);
    let mut out = Vec::new();
    while let Some(e) = rx.recv().await {
        out.push(e);
    }
    out
}

fn deliver(app: &mut App, events: Vec<DiveEvent>) {
    for e in events {
        app.on_app_event(AppEvent::Dive(e));
    }
}

/// Swipe through everything: the first card flagged with `←`, the second
/// with `i`, the rest accepted with `l`. Returns the number flagged.
fn swipe_and_flag(app: &mut App) -> Vec<String> {
    key(app, 's');
    assert_eq!(app.screen, Screen::Swipe);
    let mut n = 0;
    let mut flagged = Vec::new();
    while let Some(id) = app.swipe_current().cloned() {
        match n {
            0 => code(app, KeyCode::Left),
            1 => key(app, 'i'),
            _ => key(app, 'l'),
        };
        if n < 2 {
            flagged.push(id);
        }
        n += 1;
        assert!(n < 50, "swipe stack never empties");
    }
    assert!(n >= 3, "fixture needs at least three decidable units");
    flagged
}

#[tokio::test]
async fn swiping_flags_units_and_the_summary_offers_the_flagged_screen() {
    let mut f = fixture();
    let flagged = swipe_and_flag(&mut f.app);
    assert_eq!(f.app.flagged_units().len(), 2);
    for id in &flagged {
        assert_eq!(
            f.app.review.unit(id).unwrap().decision,
            Decision::Investigate
        );
    }
    let text = screen_text(&f.app);
    assert!(text.contains("2 flagged — press d to see them"), "{text}");
}

#[tokio::test]
async fn d_opens_the_flagged_screen_listing_exactly_the_flagged_units() {
    let mut f = fixture();
    let flagged = swipe_and_flag(&mut f.app);
    key(&mut f.app, 'd');
    assert_eq!(f.app.screen, Screen::Flagged);
    let heads = heads(&mut f.app);
    assert_eq!(heads.len(), flagged.len());
    for id in &flagged {
        let u = f.app.review.unit(id).unwrap();
        assert!(
            heads.iter().any(|h| h.contains(u.symbol.as_str())),
            "{} missing: {heads:#?}",
            u.symbol
        );
    }
    assert!(heads[0].contains("1/2 · j k other units"), "{heads:#?}");
    assert_eq!(row_glyphs(&mut f.app), ['⚑', '⚑']);
}

#[tokio::test]
async fn visualize_is_asked_once_and_the_answer_shows_after_delivery() {
    let mut f = fixture();
    swipe_and_flag(&mut f.app);
    key(&mut f.app, 'd');

    let effects = key(&mut f.app, 'v');
    let asked = dive_effects(&effects);
    assert_eq!(asked.len(), 1, "{effects:?}");
    assert_eq!(asked[0].1, DiveKind::Visualize);
    assert_eq!(
        f.app.overlay,
        Overlay::None,
        "Visualize needs no confirmation"
    );
    let id = asked[0].0.clone();
    assert!(
        f.app
            .review
            .unit(&id)
            .unwrap()
            .dive_pending
            .contains(&DiveKind::Visualize)
    );
    let text = screen_text(&f.app);
    assert!(
        text.contains("asking the agent"),
        "spinner while pending:\n{text}"
    );
    assert!(!text.contains("mock picture"));

    let events = run_effects(&f, &effects).await;
    assert_eq!(events.len(), 1);
    // Nothing shows until the event is delivered.
    assert!(!screen_text(&f.app).contains("mock picture"));
    deliver(&mut f.app, events);
    let text = screen_text(&f.app);
    assert!(text.contains("mock picture"), "{text}");
    assert!(
        text.contains("caller ─calls→ this unit"),
        "edges shown:\n{text}"
    );
    assert!(!text.contains("asking the agent"), "{text}");
    assert!(f.app.review.unit(&id).unwrap().dive_pending.is_empty());

    // Cached: a second v asks nothing.
    let again = key(&mut f.app, 'v');
    assert!(dive_effects(&again).is_empty(), "{again:?}");
    assert_eq!(f.adapter.web_calls.lock().unwrap().len(), 1);
    assert_eq!(f.adapter.prompts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_pending_question_is_not_asked_twice() {
    let mut f = fixture();
    swipe_and_flag(&mut f.app);
    key(&mut f.app, 'd');
    assert_eq!(dive_effects(&key(&mut f.app, 'v')).len(), 1);
    assert!(dive_effects(&key(&mut f.app, 'v')).is_empty());
}

#[tokio::test]
async fn web_kinds_confirm_once_then_only_they_use_the_web() {
    let mut f = fixture();
    swipe_and_flag(&mut f.app);
    key(&mut f.app, 'd');

    // First web-backed request: confirmation, nothing sent yet.
    let effects = key(&mut f.app, 'n');
    assert!(dive_effects(&effects).is_empty(), "{effects:?}");
    assert_eq!(f.app.overlay, Overlay::ConfirmWeb);
    let text = screen_text(&f.app);
    assert!(text.contains("never your code"), "{text}");

    // y proceeds with the request.
    let effects = key(&mut f.app, 'y');
    assert_eq!(f.app.overlay, Overlay::None);
    let asked = dive_effects(&effects);
    assert_eq!(asked.len(), 1, "{effects:?}");
    assert_eq!(asked[0].1, DiveKind::Recommend);
    assert!(asked[0].2);
    let mut events = run_effects(&f, &effects).await;

    // Later web requests (the next unit) don't ask again.
    key(&mut f.app, 'j');
    let effects = key(&mut f.app, 'n');
    assert_eq!(f.app.overlay, Overlay::None);
    assert_eq!(dive_effects(&effects).len(), 1);
    assert_eq!(dive_effects(&effects)[0].1, DiveKind::Recommend);
    events.extend(run_effects(&f, &effects).await);

    // Visualize after confirmation: the agent is called with web = false.
    let effects = key(&mut f.app, 'v');
    events.extend(run_effects(&f, &effects).await);
    assert_eq!(*f.adapter.web_calls.lock().unwrap(), [true, true, false]);

    deliver(&mut f.app, events);
    key(&mut f.app, 'n');
    assert!(screen_text(&f.app).contains("mock Recommend answer"));
    key(&mut f.app, 'v');
    assert!(screen_text(&f.app).contains("mock picture"));
    key(&mut f.app, 'k');
    key(&mut f.app, 'n');
    assert!(screen_text(&f.app).contains("mock Recommend answer"));
}

#[tokio::test]
async fn declining_the_web_confirmation_asks_nothing_and_clears_the_spinner() {
    let mut f = fixture();
    swipe_and_flag(&mut f.app);
    key(&mut f.app, 'd');
    key(&mut f.app, 'n');
    assert_eq!(f.app.overlay, Overlay::ConfirmWeb);
    let effects = key(&mut f.app, 'q'); // anything but y / enter declines
    assert!(dive_effects(&effects).is_empty());
    assert_eq!(f.app.overlay, Overlay::None);
    let u = f.app.flagged_unit().unwrap();
    assert!(u.dive_pending.is_empty());
    assert!(u.dive.results.is_empty());
    assert!(f.adapter.web_calls.lock().unwrap().is_empty());
    // Asking again re-prompts.
    key(&mut f.app, 'n');
    assert_eq!(f.app.overlay, Overlay::ConfirmWeb);
}

#[tokio::test]
async fn dive_web_off_never_asks_and_never_uses_the_web() {
    let cfg = Config {
        dive_web: false,
        ..Config::default()
    };
    let mut f = fixture_with(cfg, MockAdapter::new(dive_responder));
    swipe_and_flag(&mut f.app);
    key(&mut f.app, 'd');
    let effects = key(&mut f.app, 'n');
    assert_eq!(f.app.overlay, Overlay::None);
    let asked = dive_effects(&effects);
    assert_eq!(asked.len(), 1);
    assert!(!asked[0].2);
    run_effects(&f, &effects).await;
    assert_eq!(*f.adapter.web_calls.lock().unwrap(), [false]);
}

#[tokio::test]
async fn pr_reviews_never_use_the_web_and_never_ask() {
    let mut f = fixture();
    f.app.review.pr = Some(teer::review::model::PrInfo {
        host: "github.com".into(),
        port: None,
        base_assumed: None,
        owner: "acme".into(),
        repo: "shop".into(),
        number: 7,
        ..Default::default()
    });
    swipe_and_flag(&mut f.app);
    key(&mut f.app, 'd');
    let effects = key(&mut f.app, 'n');
    assert_eq!(f.app.overlay, Overlay::None, "no web confirmation for a PR");
    let asked = dive_effects(&effects);
    assert_eq!(asked.len(), 1);
    assert!(!asked[0].2, "web must be off");
    assert!(f.app.status.contains("disabled for PR"), "{}", f.app.status);
    run_effects(&f, &effects).await;
    // Visualize never uses the web anyway.
    let effects = key(&mut f.app, 'v');
    assert!(!dive_effects(&effects)[0].2);
    run_effects(&f, &effects).await;
    assert_eq!(*f.adapter.web_calls.lock().unwrap(), [false, false]);
}

#[tokio::test]
async fn a_failed_dive_shows_the_error_and_clears_the_spinner() {
    let broken = MockAdapter::new(|_, _| anyhow::bail!("agent exploded"));
    let mut f = fixture_with(Config::default(), broken);
    swipe_and_flag(&mut f.app);
    key(&mut f.app, 'd');
    let effects = key(&mut f.app, 'v');
    assert_eq!(dive_effects(&effects).len(), 1);
    assert!(screen_text(&f.app).contains("asking the agent"));
    let events = run_effects(&f, &effects).await;
    assert!(matches!(
        events[0],
        DiveEvent::Failed {
            kind: DiveKind::Visualize,
            ..
        }
    ));
    deliver(&mut f.app, events);
    let text = screen_text(&f.app);
    assert!(text.contains("failed"), "{text}");
    assert!(text.contains("agent exploded"), "{text}");
    assert!(!text.contains("asking the agent"), "{text}");
    let u = f.app.flagged_unit().unwrap();
    assert!(u.dive_pending.is_empty());
    assert!(u.dive.results.is_empty());
    // It can be asked again.
    assert_eq!(dive_effects(&key(&mut f.app, 'v')).len(), 1);
}

#[tokio::test]
async fn r_asks_the_open_tab_again() {
    let mut f = fixture();
    swipe_and_flag(&mut f.app);
    key(&mut f.app, 'd');
    let effects = key(&mut f.app, 'v');
    let events = run_effects(&f, &effects).await;
    deliver(&mut f.app, events);
    let again = key(&mut f.app, 'r');
    assert_eq!(dive_effects(&again).len(), 1, "{again:?}");
    assert_eq!(dive_effects(&again)[0].1, DiveKind::Visualize);
    assert_eq!(
        f.app.screen,
        Screen::Flagged,
        "r must not re-run the review"
    );
    assert!(!again.contains(&Effect::Rerun));
}

#[tokio::test]
async fn o_and_f_toggle_the_resolution_glyph() {
    let mut f = fixture();
    swipe_and_flag(&mut f.app);
    key(&mut f.app, 'd');
    assert_eq!(row_glyphs(&mut f.app), ['⚑', '⚑']);

    key(&mut f.app, 'o');
    let text = screen_text(&f.app);
    assert_eq!(row_glyphs(&mut f.app), ['●', '⚑'], "{text}");
    assert!(text.contains("resolved: ok"), "{text}");

    key(&mut f.app, 'f');
    let text = screen_text(&f.app);
    assert_eq!(row_glyphs(&mut f.app), ['✗', '⚑'], "{text}");
    assert!(text.contains("needs fix"), "{text}");

    // Same key again clears it.
    key(&mut f.app, 'f');
    let text = screen_text(&f.app);
    assert_eq!(row_glyphs(&mut f.app), ['⚑', '⚑'], "{text}");
    assert_eq!(f.app.flagged_unit().unwrap().dive.resolution, None);
}

#[tokio::test]
async fn note_round_trip() {
    let mut f = fixture();
    swipe_and_flag(&mut f.app);
    key(&mut f.app, 'd');
    key(&mut f.app, 'm');
    assert!(matches!(f.app.overlay, Overlay::Note { .. }));
    assert!(screen_text(&f.app).contains(" Note "));
    for c in "ask Sam".chars() {
        key(&mut f.app, c);
    }
    // Typed letters are text, not commands.
    assert_eq!(f.app.screen, Screen::Flagged);
    assert!(matches!(&f.app.overlay, Overlay::Note { text, .. } if text == "ask Sam"));
    let effects = code(&mut f.app, KeyCode::Enter);
    assert!(effects.contains(&Effect::Save));
    assert_eq!(f.app.overlay, Overlay::None);
    assert_eq!(f.app.flagged_unit().unwrap().dive.note, "ask Sam");
    assert!(screen_text(&f.app).contains("ask Sam"));

    // Reopening starts from the stored note; Esc cancels edits.
    key(&mut f.app, 'm');
    assert!(matches!(&f.app.overlay, Overlay::Note { text, .. } if text == "ask Sam"));
    key(&mut f.app, '!');
    code(&mut f.app, KeyCode::Esc);
    assert_eq!(f.app.overlay, Overlay::None);
    assert_eq!(f.app.flagged_unit().unwrap().dive.note, "ask Sam");
    assert_eq!(f.app.screen, Screen::Flagged, "Esc closes the editor only");
}

#[tokio::test]
async fn esc_returns_to_where_the_dive_started() {
    let mut f = fixture();
    swipe_and_flag(&mut f.app);
    assert_eq!(f.app.screen, Screen::Swipe);
    key(&mut f.app, 'd');
    assert_eq!(f.app.screen, Screen::Flagged);
    code(&mut f.app, KeyCode::Esc);
    assert_eq!(f.app.screen, Screen::Swipe);

    // From the list too.
    code(&mut f.app, KeyCode::Esc);
    if f.app.screen == Screen::Slim {
        key(&mut f.app, 'd');
        assert_eq!(f.app.screen, Screen::Flagged);
        code(&mut f.app, KeyCode::Esc);
        assert_eq!(f.app.screen, Screen::Slim);
    }
}

#[tokio::test]
async fn d_with_nothing_flagged_stays_put_with_a_message() {
    let mut f = fixture();
    assert!(f.app.flagged_units().is_empty());
    key(&mut f.app, 'd');
    assert_eq!(f.app.screen, Screen::Slim);
    assert!(
        f.app.status.to_lowercase().contains("nothing flagged"),
        "{}",
        f.app.status
    );
    assert!(
        screen_text(&f.app)
            .to_lowercase()
            .contains("nothing flagged")
    );
}
