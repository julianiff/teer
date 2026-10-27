//! The Visualize question end to end with the mock agent: `v` asks (never
//! with the web), the picture is validated and saved as an HTML page, the tab
//! shows the claim and the edges in its tile, `b` opens the page, and it
//! survives a reload.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui_image::picker::{Picker, ProtocolType};
use teer::agent::mock::{MockAdapter, dive_responder};
use teer::approve::Tools;
use teer::config::{Config, KeyMap};
use teer::review::dive::{DiveEvent, run_dive};
use teer::review::model::{Decision, DiveKind, SessionRef, Visual};
use teer::review::{BuildOptions, build_review, store, visual};
use teer::tui::Runtime;
use teer::tui::app::{App, AppEvent, Effect, Overlay, Pane, Screen};
use teer::tui::picture::Pictures;
use teer::tui::ui;

struct Fixture {
    app: App,
    adapter: MockAdapter,
    repo: FixtureRepo,
}

/// The Python fixture with its first two units flagged, on the Flagged screen.
fn fixture(keys: KeyMap) -> Fixture {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD).commit("init");
    repo.write("checkout.py", PY_NEW);
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    review.agent_session = Some(session(&repo.path()));
    for u in review.units.iter_mut().filter(|u| u.needs_agent()).take(2) {
        u.decision = Decision::Investigate;
    }
    let mut app = App::new(review, Config::default(), keys);
    key(&mut app, 'd');
    assert_eq!(app.screen, Screen::Flagged);
    Fixture {
        app,
        adapter: MockAdapter::new(dive_responder),
        repo,
    }
}

fn session(cwd: &Path) -> SessionRef {
    SessionRef {
        agent: "mock".into(),
        id: "s1".into(),
        cwd: cwd.to_path_buf(),
    }
}

fn key(a: &mut App, c: char) -> Vec<Effect> {
    a.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )))
}

fn screen(app: &App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    t
}

fn screen_text(app: &App) -> String {
    text_at(app, 120, 40)
}

fn text_at(app: &App, w: u16, h: u16) -> String {
    let t = screen(app, w, h);
    let buf = t.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// As if the terminal had answered that it speaks the Kitty graphics protocol.
fn kitty() -> Pictures {
    let mut picker = Picker::halfblocks();
    picker.set_protocol_type(ProtocolType::Kitty);
    Pictures::new(picker)
}

/// Kitty draws an image as cells holding this placeholder character.
const PLACEHOLDER: char = '\u{10EEEE}';

/// Run the Dive effects the way the runtime does (Visualize gets its target)
/// and deliver the answers.
async fn answer(f: &mut Fixture, effects: &[Effect]) {
    let adapter: Arc<dyn teer::agent::AgentAdapter> = Arc::new(f.adapter.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    for e in effects {
        let Effect::Dive { unit_id, kind, web } = e else {
            continue;
        };
        let unit = f.app.review.unit(unit_id).unwrap().clone();
        let file = f.app.review.file_of(&unit).unwrap().clone();
        let target = (*kind == DiveKind::Visualize).then(|| visual::target(&f.app.review, &unit));
        run_dive(
            adapter.clone(),
            session(&f.repo.path()),
            unit,
            file,
            *kind,
            *web,
            target,
            tx.clone(),
        )
        .await;
    }
    drop(tx);
    while let Some(ev) = rx.recv().await {
        if let DiveEvent::Failed { error, .. } = &ev {
            panic!("{error}");
        }
        f.app.on_app_event(AppEvent::Dive(ev));
    }
}

fn picture(app: &App) -> Visual {
    app.flagged_unit().unwrap().dive.results[&DiveKind::Visualize]
        .visual
        .clone()
        .unwrap()
}

/// An opener script that appends what it was asked to open to `opened`.
fn tools(dir: &Path) -> Tools {
    let p = dir.join("opener");
    std::fs::write(
        &p,
        "#!/bin/sh\nd=$(dirname \"$0\"); printf '%s\\n' \"$@\" >> \"$d/opened\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    Tools {
        opener: p.into(),
        ..Tools::default()
    }
}

/// The lines the opener got, once it has run `n` times (it runs detached).
async fn opened(dir: &Path, n: usize) -> Vec<String> {
    for _ in 0..100 {
        if let Ok(s) = std::fs::read_to_string(dir.join("opened"))
            && s.lines().count() >= n
        {
            return s.lines().map(String::from).collect();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Vec::new()
}

fn runtime(tools: Tools) -> Runtime {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    Runtime {
        adapter: None,
        tx,
        build: BuildOptions::default(),
        tools,
    }
}

#[tokio::test]
async fn v_asks_without_the_web_and_the_tab_shows_the_picture() {
    let mut f = fixture(KeyMap::default());
    let effects = key(&mut f.app, 'v');
    assert_eq!(f.app.flagged.pane, Pane::Answer(DiveKind::Visualize));
    assert!(
        matches!(
            &effects[..],
            [Effect::Dive {
                kind: DiveKind::Visualize,
                ..
            }]
        ),
        "{effects:?}"
    );
    assert_eq!(f.app.overlay, Overlay::None, "no web, so no confirmation");
    assert!(screen_text(&f.app).contains("asking the agent"));
    // Asked again while in flight: nothing new.
    assert!(key(&mut f.app, 'v').is_empty());

    answer(&mut f, &effects).await;
    assert_eq!(*f.adapter.web_calls.lock().unwrap(), [false]);
    let prompt = f.adapter.prompts.lock().unwrap()[0].clone();
    assert!(prompt.starts_with("DIVE:Visualize"), "{prompt}");
    assert!(prompt.contains("Code you may cite"), "{prompt}");

    assert!(screen_text(&f.app).contains("╭ v Visualize ✓"));
    // Zoomed, the tile is as wide as the screen.
    key(&mut f.app, 'z');
    let text = screen_text(&f.app);
    for want in [
        "mock picture: the unit now writes to a store.",
        "caller ─calls→ this unit  [unchanged]",
        "this unit ─writes→ store  [added]",
        "saved to",
        "b opens it in the browser",
        "╭ v Visualize ✓",
    ] {
        assert!(text.contains(want), "{want:?} missing:\n{text}");
    }

    // The page: under the data dir, one per review and unit, self-contained.
    let v = picture(&f.app);
    let id = f.app.flagged_unit_id().unwrap();
    assert_eq!(v.path, visual::page_path(&f.app.review.id, &id));
    assert!(v.path.starts_with(store::data_dir().join("visuals")));
    let html = std::fs::read_to_string(&v.path).unwrap();
    assert!(html.contains("<svg role=\"img\" aria-label=\"mock picture"));
    assert!(html.contains("<figcaption>mock picture"));
    assert!(!html.contains("<script"));
}

#[tokio::test]
async fn visualize_tab_snapshot() {
    let mut f = fixture(KeyMap::default());
    let effects = key(&mut f.app, 'v');
    answer(&mut f, &effects).await;
    // Fixed paths, so the snapshot is the same on every machine.
    f.app.review.repo_root = PathBuf::from("/work/shop");
    f.app.review.base = "a1b2c3d4e5f6".into();
    let id = f.app.flagged_unit_id().unwrap();
    let u = f.app.review.unit_mut(&id).unwrap();
    let res = u.dive.results.get_mut(&DiveKind::Visualize).unwrap();
    res.visual.as_mut().unwrap().path = PathBuf::from("/data/visuals/r1/checkout.py_charge.html");
    insta::assert_snapshot!("flagged_visualize", screen(&f.app, 100, 30).backend());
}

#[tokio::test]
async fn b_opens_the_page_and_writes_it_again_when_it_is_gone() {
    let mut f = fixture(KeyMap::default());
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime(tools(dir.path()));

    // Nothing drawn yet: say how to draw one.
    assert!(key(&mut f.app, 'b').is_empty());
    assert_eq!(f.app.status, "No picture yet: press v to draw one.");

    let effects = key(&mut f.app, 'v');
    answer(&mut f, &effects).await;
    // `b` works from any tab.
    key(&mut f.app, 'x');
    let id = f.app.flagged_unit_id().unwrap();
    let effects = key(&mut f.app, 'b');
    assert_eq!(
        effects,
        vec![Effect::OpenVisual {
            unit_id: id.clone()
        }]
    );
    for e in effects {
        rt.exec(&mut f.app, e);
    }
    let path = picture(&f.app).path;
    assert_eq!(opened(dir.path(), 1).await, [path.display().to_string()]);
    assert!(f.app.status.starts_with("Opened "), "{}", f.app.status);

    // Deleted: written again from the saved picture, then opened.
    std::fs::remove_file(&path).unwrap();
    for e in key(&mut f.app, 'b') {
        rt.exec(&mut f.app, e);
    }
    assert!(path.is_file());
    assert_eq!(opened(dir.path(), 2).await.len(), 2);

    // A stored path outside teer's visuals folder is never written to.
    let outside = dir.path().join("elsewhere.html");
    let u = f.app.review.unit_mut(&id).unwrap();
    let res = u.dive.results.get_mut(&DiveKind::Visualize).unwrap();
    res.visual.as_mut().unwrap().path = outside.clone();
    for e in key(&mut f.app, 'b') {
        rt.exec(&mut f.app, e);
    }
    assert!(!outside.exists());
    assert_eq!(picture(&f.app).path, path);
    assert_eq!(opened(dir.path(), 3).await[2], path.display().to_string());
}

#[tokio::test]
async fn r_on_the_visualize_tab_asks_again() {
    let mut f = fixture(KeyMap::default());
    let effects = key(&mut f.app, 'v');
    answer(&mut f, &effects).await;
    let effects = key(&mut f.app, 'r');
    assert!(
        matches!(
            &effects[..],
            [Effect::Dive {
                kind: DiveKind::Visualize,
                ..
            }]
        ),
        "{effects:?}"
    );
    let u = f.app.flagged_unit().unwrap();
    assert!(!u.dive.results.contains_key(&DiveKind::Visualize));
    assert!(u.dive_pending.contains(&DiveKind::Visualize));
}

#[tokio::test]
async fn the_keys_can_be_rebound() {
    let keys = KeyMap::with_overrides(&HashMap::from([
        ("dive_visualize".to_string(), "V".to_string()),
        ("open_visual".to_string(), "ctrl+o".to_string()),
    ]))
    .unwrap();
    let mut f = fixture(keys);
    assert!(key(&mut f.app, 'v').is_empty());
    assert!(key(&mut f.app, 'b').is_empty());
    assert!(f.app.status.is_empty(), "{}", f.app.status);
    let effects = key(&mut f.app, 'V');
    assert!(matches!(
        &effects[..],
        [Effect::Dive {
            kind: DiveKind::Visualize,
            ..
        }]
    ));
    answer(&mut f, &effects).await;
    assert!(screen_text(&f.app).contains("^o opens it in the browser"));
    let effects = f.app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(
        matches!(&effects[..], [Effect::OpenVisual { .. }]),
        "{effects:?}"
    );
}

#[tokio::test]
async fn the_picture_survives_a_save_and_reload() {
    let mut f = fixture(KeyMap::default());
    let effects = key(&mut f.app, 'v');
    answer(&mut f, &effects).await;
    let before = picture(&f.app);
    store::save(&f.app.review).unwrap();
    let back = store::load_latest(&f.app.review.repo_root, None).unwrap();
    let id = f.app.flagged_unit_id().unwrap();
    let res = &back.unit(&id).unwrap().dive.results[&DiveKind::Visualize];
    assert_eq!(res.visual.as_ref(), Some(&before));
    assert_eq!(res.text, before.claim);

    let mut app = App::new(back, Config::default(), KeyMap::default());
    key(&mut app, 'd');
    key(&mut app, 'v');
    key(&mut app, 'z');
    let text = screen_text(&app);
    assert!(text.contains("this unit ─writes→ store  [added]"), "{text}");
}

#[tokio::test]
async fn with_images_the_tile_shows_the_picture_instead_of_the_list() {
    let mut f = fixture(KeyMap::default());
    let effects = key(&mut f.app, 'v');
    answer(&mut f, &effects).await;
    f.app.pictures = Some(kitty());
    // In its tile, with or without the focus.
    assert!(screen_text(&f.app).contains(PLACEHOLDER));
    code_right(&mut f.app);
    assert!(screen_text(&f.app).contains(PLACEHOLDER));
    code_right(&mut f.app);
    key(&mut f.app, 'v');
    key(&mut f.app, 'z');
    let text = screen_text(&f.app);
    assert!(text.contains(PLACEHOLDER), "{text}");
    assert!(
        text.contains("mock picture: the unit now writes to a store."),
        "{text}"
    );
    assert!(
        !text.contains("─calls→"),
        "the picture replaces the list:\n{text}"
    );
    assert!(text.contains("b opens it in the browser · "), "{text}");
    // Drawn again from the same pixels on the next frame.
    assert!(screen_text(&f.app).contains(PLACEHOLDER));

    // Other tiles are text.
    code_right(&mut f.app);
    assert_ne!(
        f.app.flagged.pane,
        teer::tui::app::Pane::Answer(DiveKind::Visualize)
    );
    assert!(!screen_text(&f.app).contains(PLACEHOLDER));
    key(&mut f.app, 'v');

    // Too little room for a picture: the list again.
    let small = text_at(&f.app, 120, 14);
    assert!(!small.contains(PLACEHOLDER), "{small}");
    assert!(small.contains("─calls→"), "{small}");

    // A re-ask in flight shows the spinner, not the old picture.
    key(&mut f.app, 'r');
    let text = screen_text(&f.app);
    assert!(
        !text.contains(PLACEHOLDER) && text.contains("asking the agent"),
        "{text}"
    );
}

#[tokio::test]
async fn without_images_the_tab_keeps_the_list() {
    let mut f = fixture(KeyMap::default());
    let effects = key(&mut f.app, 'v');
    answer(&mut f, &effects).await;
    assert!(f.app.pictures.is_none(), "tests never ask a real terminal");
    key(&mut f.app, 'z');
    let text = screen_text(&f.app);
    assert!(!text.contains(PLACEHOLDER));
    assert!(text.contains("this unit ─writes→ store  [added]"), "{text}");
}

fn code_right(a: &mut App) {
    a.on_event(Event::Key(KeyEvent::new(
        KeyCode::Right,
        KeyModifiers::NONE,
    )));
}
