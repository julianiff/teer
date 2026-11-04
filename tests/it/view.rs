//! The view model (`session::view`) on the Python fixture: what a front
//! end other than the terminal gets to draw, snapshot tested as JSON.

use teer::config::Config;
use teer::session::view::{OverlayView, TileBody, TileId, View, snapshot};
use teer::tui::app::App;

use crate::tui::{app_for, cited, fixture_review, select};

fn view(app: &App) -> View {
    snapshot(app, Some(&app.syntax))
}

fn json(app: &App) -> String {
    serde_json::to_string_pretty(&view(app)).unwrap()
}

fn keys(app: &mut App, keys: &str) {
    for k in teer::keymap::parse_keys(keys).unwrap() {
        app.on_key(k.event());
    }
}

#[tokio::test]
async fn the_queue_as_data() {
    let (review, _repo) = fixture_review().await;
    let app = app_for(review, Config::default());
    insta::assert_snapshot!("view_queue", json(&app));
    let v = view(&app);
    // The preview is the unit under the cursor; its code is coloured.
    let preview = v.preview.expect("a preview");
    assert_eq!(preview.id, app.selected_id().unwrap());
    let code = preview.code.expect("its code");
    assert!(
        code.rows
            .iter()
            .flat_map(|r| &r.pieces)
            .any(|p| p.class.is_some()),
        "syntax classes"
    );
}

#[tokio::test]
async fn the_unit_view_as_data() {
    let (mut review, _repo) = fixture_review().await;
    cited(&mut review);
    let mut app = app_for(review, Config::default());
    select(&mut app, "charge_order");
    keys(&mut app, "l");
    keys(&mut app, "<space>t");
    insta::assert_snapshot!("view_unit", json(&app));
    let v = view(&app);
    let unit = v.unit.expect("the unit view");
    // Every statement is written in under the row it cites.
    let code = unit.card.code.as_ref().expect("code");
    assert_eq!(code.notes.len(), unit.card.statements.len());
    assert!(code.notes.iter().all(|n| n.after.is_some()));
    assert!(!code.hunks.is_empty());
    // The advice tile is open with the unit, the tests one was asked for.
    let ids: Vec<TileId> = unit.tiles.iter().map(|t| t.id).collect();
    assert!(ids.contains(&TileId::Tests), "{ids:?}");
    let tests = unit.tiles.iter().find(|t| t.id == TileId::Tests).unwrap();
    assert!(matches!(tests.body, TileBody::Answer(_)));
}

#[tokio::test]
async fn flagged_units_the_sheet_and_the_notes_as_data() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    select(&mut app, "charge_order");
    keys(&mut app, "f");
    keys(&mut app, "<space>f");
    let v = view(&app);
    let flagged = v.flagged.expect("the flagged list");
    assert_eq!(flagged.entries.len(), 1);
    assert_eq!(flagged.entries[0].name, "charge_order");
    keys(&mut app, "h:w<cr>");
    insta::assert_snapshot!("view_finish", json(&app));
    let OverlayView::Finish(f) = view(&app).overlay else {
        panic!("the sheet");
    };
    assert_eq!(f.notes.len(), 1);
}

fn decision(app: &App, symbol: &str) -> teer::review::model::Decision {
    let u = app
        .review
        .units
        .iter()
        .find(|u| u.symbol == symbol)
        .unwrap();
    u.decision
}

#[tokio::test]
async fn the_buttons_have_their_keys_their_words_and_their_state() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    let v = view(&app);
    let accept = &v.actions["decide.accept"];
    assert_eq!((accept.keys.as_str(), accept.on), ("a", None));
    assert_eq!(v.actions["toggle.by_file"].on, Some(false));
    keys(&mut app, "zf");
    assert_eq!(view(&app).actions["toggle.by_file"].on, Some(true));
    // A note and a flag: the bar counts them, for its links to their lists.
    select(&mut app, "charge_order");
    keys(&mut app, "f");
    keys(&mut app, "ichecked<cr>");
    let v = view(&app);
    assert_eq!((v.bar.flagged, v.bar.notes), (1, 1));
    // On the flagged list, the buttons say what they do there.
    keys(&mut app, "<space>f");
    let v = view(&app);
    assert_eq!(
        v.actions["decide.accept"].what,
        "accept it: it leaves the list"
    );
    assert!(v.panel.is_empty(), "no panel open");
}

#[tokio::test]
async fn the_unit_views_back_and_a_tiles_close_are_clicks() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    select(&mut app, "charge_order");
    keys(&mut app, "l<space>tzz");
    let tiles =
        |app: &App| -> Vec<TileId> { view(app).unit.unwrap().tiles.iter().map(|t| t.id).collect() };
    assert!(tiles(&app).contains(&TileId::Tests));
    // The close button closes its tile, zoomed or not.
    app.unit_click_close(TileId::Tests.panel());
    assert!(!tiles(&app).contains(&TileId::Tests));
    assert!(!view(&app).unit.unwrap().zoom);
    // Back goes back from a tile in focus, where `h` would close it.
    keys(&mut app, "<space>t");
    app.unit_click_back();
    assert_eq!(view(&app).screen, teer::session::Screen::Queue);
    assert_eq!(app.selected().unwrap().symbol, "charge_order");
}

#[tokio::test]
async fn a_click_presses_the_key_where_it_is() {
    use teer::review::model::Decision;
    use teer::session::Overlay;
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    let press = |app: &mut App, id: &str| {
        let a = teer::keymap::Action::from_id(id).unwrap();
        app.press(a, None)
    };

    // On the queue it runs.
    select(&mut app, "charge_order");
    press(&mut app, "decide.accept");
    assert_eq!(decision(&app, "charge_order"), Decision::Accepted);

    // The help closes first, then it runs on what the help was over; each
    // key of a pair is on its row.
    press(&mut app, "history.undo");
    keys(&mut app, "?");
    let OverlayView::Help(help) = view(&app).overlay else {
        panic!("the help");
    };
    let row = help
        .groups
        .iter()
        .flat_map(|g| &g.rows)
        .find(|r| r.each.iter().any(|k| k.action == "decide.flag"))
        .unwrap();
    let each: Vec<&str> = row.each.iter().map(|k| k.action).collect();
    assert_eq!(each, ["decide.accept", "decide.flag"]);
    select(&mut app, "charge_order");
    press(&mut app, "decide.flag");
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(decision(&app, "charge_order"), Decision::Investigate);

    // A note being typed keeps its letters: the click waits.
    keys(&mut app, "i");
    press(&mut app, "decide.accept");
    assert!(
        matches!(app.overlay, Overlay::Note { .. }),
        "{:?}",
        app.overlay
    );
    assert_eq!(decision(&app, "charge_order"), Decision::Investigate);
    keys(&mut app, "look<cr>");

    // An open panel takes it as its own key: the notes' delete asks.
    keys(&mut app, "<space>n");
    let panel: Vec<&str> = view(&app).panel.iter().map(|h| h.action).collect();
    assert!(panel.contains(&"note.delete"), "{panel:?}");
    assert!(!panel.contains(&"move.down"), "{panel:?}");
    press(&mut app, "note.delete");
    assert!(
        matches!(app.overlay, Overlay::ConfirmDeleteNote { .. }),
        "{:?}",
        app.overlay
    );
    keys(&mut app, "n<esc>");

    // Finding, open is the query's ⏎: the cursor goes to what it found.
    app.sel = (0..app.order.len())
        .find(|&p| !app.review.units[app.order[p]].symbol.contains("charge"))
        .unwrap();
    keys(&mut app, "/charge");
    let panel: Vec<(String, &str)> = view(&app)
        .panel
        .into_iter()
        .map(|h| (h.keys, h.action))
        .collect();
    assert_eq!(
        panel,
        [
            ("⏎".to_string(), "view.open"),
            ("esc".to_string(), "view.back")
        ]
    );
    press(&mut app, "view.open");
    assert_eq!(app.overlay, Overlay::None);
    let at = &app.review.units[app.order[app.sel]];
    assert!(at.symbol.contains("charge"), "{}", at.symbol);
}

#[tokio::test]
async fn the_sheets_keys_are_pressed_by_a_click() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    keys(&mut app, ":w<cr>");
    let OverlayView::Finish(f) = view(&app).overlay else {
        panic!("the sheet");
    };
    let commit = f.steps.iter().position(|s| s.verb == "commit").unwrap();
    app.pick(commit);
    let edit = teer::keymap::Action::from_id("note.edit").unwrap();
    let effects = app.press(edit, None);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, teer::session::Effect::EditComment { .. })),
        "{effects:?}"
    );
}

#[tokio::test]
async fn the_windows_help_leaves_out_the_way_to_the_window() {
    let (review, _repo) = fixture_review().await;
    let mut app = app_for(review, Config::default());
    keys(&mut app, "?");
    let OverlayView::Help(help) = view(&app).overlay else {
        panic!("the help");
    };
    let actions: Vec<&str> = help
        .groups
        .iter()
        .flat_map(|g| &g.rows)
        .flat_map(|r| &r.each)
        .map(|k| k.action)
        .collect();
    assert!(actions.contains(&"finish.open"), "{actions:?}");
    assert!(!actions.contains(&"finish.app"), "{actions:?}");
}
