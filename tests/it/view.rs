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
