//! The unit view (v2 Phase 5): any unit, its sections, the recommendation
//! asked for on open and prefetched, the `K` panel, go-to, the leader
//! panels, and deciding without leaving it. Models are the mock adapter.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::v2::{self, press, screen};
use common::*;
use serde_json::json;
use teer::agent::mock::MockAdapter;
use teer::approve::Tools;
use teer::config::Config;
use teer::keymap::{Action, Keymap};
use teer::review::dive::DiveEvent;
use teer::review::model::{
    Change, Coverage, Decision, DiveKind, Marker, Prep, Review, Risk, SecondOpinion, SessionRef,
    TestSearch, Unit, Verdict,
};
use teer::review::{BuildOptions, build_review, store};
use teer::tui::Runtime;
use teer::tui::app::{App, AppEvent, Effect, JumpKind, Overlay, Screen};
use teer::tui::unit::{PREFETCH, Panel};

// ---- Helpers -----------------------------------------------------------

/// A description of `risk`: high makes a unit ▲, medium ◆, low ●.
fn change(risk: Risk) -> Change {
    Change {
        marker: Marker::Changed,
        statement: "it changes".into(),
        evidence: Vec::new(),
        risk,
    }
}

/// Unit `u{i}` in `file`, described at `risk`.
fn rated(i: usize, file: &str, risk: Risk) -> Unit {
    let mut u = v2::unit(i, file);
    u.prep = Prep::Ready;
    u.summary = Some(format!("s{i} does a thing"));
    u.changes = vec![change(risk)];
    u
}

/// ▲ ◆ ● in `f0.rs`, ▲ ◆ ▲ in `f1.rs`.
fn mixed() -> Vec<Unit> {
    vec![
        rated(0, "f0.rs", Risk::High),
        rated(1, "f0.rs", Risk::Medium),
        rated(2, "f0.rs", Risk::Low),
        rated(3, "f1.rs", Risk::High),
        rated(4, "f1.rs", Risk::Medium),
        rated(5, "f1.rs", Risk::High),
    ]
}

fn session(cwd: &Path) -> SessionRef {
    SessionRef {
        agent: "mock".into(),
        id: "s1".into(),
        cwd: cwd.to_path_buf(),
    }
}

/// `review` started the way `teer` starts it, with an agent (or not); the
/// app and what the start asked for.
fn started(review: Review, agent: bool) -> (App, Vec<Effect>) {
    let _env = isolate_env();
    let cwd = review.repo_root.clone();
    let mut app = App::new(review, Config::default(), Keymap::default());
    let effects = app.start(agent, None, Vec::new(), Some(session(&cwd)));
    (app, effects)
}

/// The units the recommendation is asked for in `effects`, in order.
fn recommends(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Dive {
                unit_id,
                kind: DiveKind::Recommend,
                ..
            } => Some(unit_id.clone()),
            _ => None,
        })
        .collect()
}

/// Ids in queue order.
fn queue(app: &App) -> Vec<String> {
    app.order
        .iter()
        .map(|&i| app.review.units[i].id.clone())
        .collect()
}

/// The pending units after `id` in queue order.
fn pending_after(app: &App, id: &str) -> Vec<String> {
    let ids = queue(app);
    let at = ids.iter().position(|x| x == id).unwrap();
    ids[at + 1..]
        .iter()
        .filter(|x| app.review.unit(x).unwrap().decision == Decision::Pending)
        .cloned()
        .collect()
}

/// The answer to `kind` on unit `id` of `app`, about its code as it is.
fn ready(app: &App, id: &str, kind: DiveKind, text: &str) -> AppEvent {
    AppEvent::Dive(DiveEvent::Ready {
        id: id.into(),
        fingerprint: app.review.unit(id).unwrap().fingerprint.clone(),
        kind,
        text: text.into(),
        sources: Vec::new(),
        visual: None,
        behavior: None,
        coverage: None,
    })
}

fn text(app: &App) -> String {
    screen(app, 120, 50)
}

/// What the payments module has; `checkout.py` calls `pay` from it.
const PAYMENTS: &str = "def pay(order):\n    total = order.total\n    return charge(total)\n";

/// The Python fixture (`checkout.py` changed, `payments.py` not), with an
/// agent session.
fn fixture() -> (FixtureRepo, Review) {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD)
        .write("payments.py", PAYMENTS)
        .commit("init");
    repo.write("checkout.py", PY_NEW);
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    review.agent_session = Some(session(&repo.path()));
    (repo, review)
}

const CHARGE: &str = "checkout.py::charge_order";
const HELPER: &str = "checkout.py::new_helper";

/// What the mock agent answers: a recommendation, or an answer to a
/// question that links `new_helper` and shows `pay`.
fn responder(prompt: &str, _schema: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    if prompt.contains("DIVE:Recommend") {
        return Ok(json!({ "text": "Check the currency before paying.", "sources": [] }));
    }
    if prompt.contains("\nASK") || prompt.starts_with("ASK") {
        return Ok(json!({
            "text": "It raises on an unsupported currency, then pays.",
            "show": [{ "file": "payments.py", "start": 1, "end": 3, "why": "pay" }],
            "related": [{ "id": HELPER, "why": "it calls charge_order" }],
        }));
    }
    anyhow::bail!("unexpected prompt")
}

/// Run `effects` the way `teer` does, with `mock` as the agent, and hand
/// the app what comes back.
async fn run(app: &mut App, mock: &MockAdapter, effects: Vec<Effect>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let rt = Runtime {
        adapter: Some(Arc::new(mock.clone())),
        tx,
        build: BuildOptions::default(),
        tools: Tools::default(),
    };
    let asked = effects
        .iter()
        .filter(|e| matches!(e, Effect::Dive { .. } | Effect::Ask { .. }))
        .count();
    for e in effects {
        rt.exec(app, e);
    }
    drop(rt);
    let mut done = 0;
    while done < asked {
        let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("an answer in time")
            .expect("the runtime is there");
        if matches!(
            ev,
            AppEvent::Answer { .. } | AppEvent::Dive(DiveEvent::Ready { .. })
        ) {
            done += 1;
        }
        if let AppEvent::Dive(DiveEvent::Failed { error, .. }) = &ev {
            panic!("{error}");
        }
        app.on_app_event(ev);
    }
}

// ---- U1: sections and hunk jumps ----------------------------------------------

#[test]
fn sections_and_hunk_jumps() {
    let (_repo, review) = fixture();
    let (mut app, started_with) = started(review, true);
    let effects = app.open_unit(CHARGE.into());
    assert_eq!(app.screen, Screen::Unit);
    // Its recommendation is on its way: prefetched, or asked on open.
    let asked = [recommends(&started_with), recommends(&effects)].concat();
    assert_eq!(asked.iter().filter(|id| *id == CHARGE).count(), 1);
    let text = text(&app);
    let at = |s: &str| {
        text.find(s)
            .unwrap_or_else(|| panic!("{s:?} missing:\n{text}"))
    };
    // Title row: level, name, file, state.
    let title = text.lines().nth(1).unwrap();
    assert!(title.contains("charge_order"), "{text}");
    assert!(title.contains("checkout.py:8-12"), "{text}");
    assert!(title.trim_end().ends_with("pending"), "{text}");
    // The sections in their order; the second model only when it disputes.
    assert!(at("Why this level") < at("What changed"));
    assert!(at("What changed") < at("raise ValueError"));
    assert!(at("raise ValueError") < at("What to do about it"));
    assert!(text.contains("asking the agent"), "{text}");
    assert!(!text.contains("Second model"), "{text}");

    // A dispute shows the second model under the recommendation.
    let u = app.review.unit_mut(CHARGE).unwrap();
    u.prep = Prep::Ready;
    u.changes = vec![change(Risk::Low)];
    u.checks.second = Some(SecondOpinion {
        risk: Risk::High,
        verdicts: vec![Verdict {
            statement: "it changes".into(),
            agree: false,
            why: "it also stops sending the receipt".into(),
            evidence: Vec::new(),
        }],
        missed: Vec::new(),
    });
    app.rebuild_order();
    let answer = ready(&app, CHARGE, DiveKind::Recommend, "Check the currency.");
    app.on_app_event(answer);
    let text = screen(&app, 120, 60);
    let at = |s: &str| {
        text.find(s)
            .unwrap_or_else(|| panic!("{s:?} missing:\n{text}"))
    };
    assert!(at("Check the currency.") < at("Second model"), "{text}");
    assert!(text.contains("it also stops sending the receipt"), "{text}");

    // ]c and [c: from hunk to hunk, the text scrolled to each.
    screen(&app, 120, 14);
    let hunks = app.unit.hunks.borrow().clone();
    assert!(!hunks.is_empty());
    press(&mut app, "]c");
    assert_eq!(app.detail_offset(), hunks[0].min(app.unit.text_max.get()));
    let short = screen(&app, 120, 14);
    assert!(short.contains("lines above"), "{short}");
    press(&mut app, "[c");
    assert_eq!(app.status, "No more hunks above.");
}

// ---- U2: prefetch and cache -------------------------------------------------

#[test]
fn prefetch_and_cache() {
    let (mut app, effects) = started(v2::review(mixed()), true);
    // The first three pending ▲ and ◆ units in queue order, nothing else.
    let asked = recommends(&effects);
    assert_eq!(asked.len(), PREFETCH);
    let close: Vec<String> = queue(&app)
        .into_iter()
        .filter(|id| {
            let a = app.assessment(id).unwrap();
            a.level >= teer::review::attention::Level::Look
        })
        .take(PREFETCH)
        .collect();
    assert_eq!(asked, close);
    assert!(!asked.contains(&"u2".to_string()), "● is not prefetched");

    // Opening one: it is on its way already; only the next pending unit,
    // if nobody asked yet.
    let first = asked[0].clone();
    let next = pending_after(&app, &first)[0].clone();
    let effects = app.open_unit(first.clone());
    let want: Vec<String> = if asked.contains(&next) {
        Vec::new()
    } else {
        vec![next.clone()]
    };
    assert_eq!(recommends(&effects), want);

    // The answer is kept with the unit and shown.
    let answer = ready(&app, &first, DiveKind::Recommend, "Rename it back.");
    app.on_app_event(answer);
    assert!(text(&app).contains("Rename it back."));
    assert!(recommends(&app.open_unit(first.clone())).is_empty());

    // Saved and run again on the same code: it is still there and not
    // asked again.
    let mut again = v2::review(mixed());
    store::merge_previous(&mut again, &app.review);
    let (mut app2, effects) = started(again, true);
    assert!(!recommends(&effects).contains(&first), "cached");
    assert!(!recommends(&app2.open_unit(first.clone())).contains(&first));
    assert!(text(&app2).contains("Rename it back."));

    // The code changed: asked again.
    let mut units = mixed();
    units
        .iter_mut()
        .find(|u| u.id == first)
        .unwrap()
        .fingerprint = "changed".into();
    let mut changed = v2::review(units);
    store::merge_previous(&mut changed, &app.review);
    let (_, effects) = started(changed, true);
    assert!(recommends(&effects).contains(&first));
}

#[tokio::test]
async fn the_mock_agent_answers_what_was_prefetched_and_opening_shows_it() {
    let (_repo, review) = fixture();
    let (mut app, effects) = started(review, true);
    let mock = MockAdapter::new(responder);
    let asked = recommends(&effects);
    assert!(!asked.is_empty() && asked.len() <= PREFETCH);
    let dives = effects
        .into_iter()
        .filter(|e| matches!(e, Effect::Dive { .. }))
        .collect();
    run(&mut app, &mock, dives).await;
    // Never the web before the reviewer allowed it.
    assert!(mock.web_calls.lock().unwrap().iter().all(|w| !w));
    let effects = app.open_unit(asked[0].clone());
    assert!(!recommends(&effects).contains(&asked[0]));
    assert!(text(&app).contains("Check the currency before paying."));
}

// ---- U3: no agent -------------------------------------------------------------

#[test]
fn no_agent_hides_agent_sections() {
    let mut units = mixed();
    units[0].checks.second = Some(SecondOpinion {
        risk: Risk::Low,
        verdicts: vec![Verdict {
            statement: "it changes".into(),
            agree: false,
            why: "no".into(),
            evidence: Vec::new(),
        }],
        missed: Vec::new(),
    });
    let (mut app, effects) = started(v2::review(units), false);
    assert!(recommends(&effects).is_empty(), "no prefetch");
    let effects = app.open_unit("u0".into());
    assert!(recommends(&effects).is_empty());
    let text = text(&app);
    assert!(text.contains("What changed"), "{text}");
    assert!(!text.contains("What to do about it"), "{text}");
    assert!(!text.contains("Second model"), "{text}");
    // No empty section is left: the text ends with what changed.
    let last = text
        .lines()
        .skip(3)
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>();
    assert!(!last.iter().any(|l| l.trim() == "What to do about it"));
    // The tools that need an agent say so; teer's own test search works.
    assert!(press(&mut app, "<space>r").is_empty());
    assert!(app.status.contains("No agent"), "{}", app.status);
    assert_eq!(app.unit.panel, None);
    let effects = press(&mut app, "<space>t");
    assert!(matches!(
        &effects[..],
        [Effect::Dive {
            kind: DiveKind::Tests,
            ..
        }]
    ));
    assert_eq!(app.unit.panel, Some(Panel::Tool(DiveKind::Tests)));
    // Asking says why it can't.
    press(&mut app, "<esc>K");
    assert!(screen(&app, 120, 40).contains("No agent: asking needs one"));
}

// ---- U4: the ask panel ------------------------------------------------------------

#[tokio::test]
async fn ask_panel_lifecycle() {
    let (_repo, review) = fixture();
    let (mut app, _) = started(review, true);
    app.open_unit(CHARGE.into());
    let mock = MockAdapter::new(responder);

    // K: the panel under the text, typing; before a question, what to ask
    // and the tools that bring more light (Q7).
    assert!(press(&mut app, "K").is_empty());
    assert_eq!(app.unit.panel, Some(Panel::Ask));
    assert!(app.unit.typing);
    let t = text(&app);
    assert!(t.contains("Ask about this unit in your own words"), "{t}");
    assert!(t.contains("Not sure yet? Bring more light into it:"), "{t}");
    assert!(t.contains("␣t  Tests"), "{t}");
    assert!(t.contains("␣r  Behavior"), "{t}");
    assert!(
        t.contains("asks once whether the agent may also search the web"),
        "{t}"
    );

    // Letters are the question, not keys (a, q, h).
    press(&mut app, "what happens on a bad currency? aqh");
    assert_eq!(app.screen, Screen::Unit);
    assert_eq!(app.review.unit(CHARGE).unwrap().decision, Decision::Pending);
    // The first question that could use the web asks first, once.
    assert_eq!(press(&mut app, "<cr>"), [Effect::Save]);
    assert_eq!(app.overlay, Overlay::ConfirmWeb);
    let effects = press(&mut app, "y");
    assert_eq!(effects, [Effect::Save, Effect::Ask { index: 0 }]);
    assert!(app.web_allowed());
    assert_eq!(app.review.questions[0].unit.as_deref(), Some(CHARGE));

    // Streaming: the steps, then the answer as it is written.
    app.on_app_event(AppEvent::Asking {
        index: 0,
        progress: teer::review::ask::Progress::Step("read payments.py".into()),
    });
    assert!(text(&app).contains("step 1: read payments.py"));
    app.on_app_event(AppEvent::Asking {
        index: 0,
        progress: teer::review::ask::Progress::Text("It raises".into()),
    });
    let t = text(&app);
    assert!(t.contains("It raises"), "{t}");
    assert!(t.contains(teer::tui::ask::WRITING), "{t}");
    // The answer, from the mock agent through the runtime, with the web.
    app.ask.pending.remove(&0);
    run(&mut app, &mock, vec![Effect::Ask { index: 0 }]).await;
    assert_eq!(*mock.web_calls.lock().unwrap(), [true]);
    let prompt = mock.prompts.lock().unwrap()[0].clone();
    assert!(prompt.contains("You may search the web"), "{prompt}");
    let t = text(&app);
    assert!(t.contains("It raises on an unsupported currency"), "{t}");
    assert!(t.contains("payments.py:1-3 pay"), "{t}");

    // A follow-up stays in the panel and doesn't ask about the web again.
    press(&mut app, "and when pay fails?");
    assert_eq!(
        press(&mut app, "<cr>"),
        [Effect::Save, Effect::Ask { index: 1 }]
    );
    assert_eq!(app.chat_of(CHARGE), [0, 1]);

    // tab: the links; the code opens in the editor, a unit here.
    app.ask.pending.clear();
    app.on_app_event(AppEvent::Answer {
        index: 1,
        result: Err("timed out".into()),
    });
    press(&mut app, "<tab>");
    assert_eq!(app.ask.link, Some(0));
    assert!(!app.unit.typing);
    let links = app.ask_links();
    assert_eq!(links.len(), 2, "{links:?}");
    press(&mut app, "<tab>");
    assert_eq!(
        press(&mut app, "<cr>"),
        [Effect::Edit {
            file: PathBuf::from("payments.py"),
            line: 1
        }]
    );
    // tab past the last link types again; shift+tab goes back to it.
    press(&mut app, "<tab>");
    assert!(app.unit.typing && app.ask.link.is_none());
    press(&mut app, "<s-tab><s-tab>");
    assert_eq!(app.ask.link, Some(0));
    press(&mut app, "<cr>");
    assert_eq!(app.screen, Screen::Unit);
    assert_eq!(app.unit.id, HELPER);
    assert_eq!(app.unit.panel, None);

    // Back on charge_order, the conversation is still there.
    press(&mut app, "/");
    assert!(matches!(app.overlay, Overlay::Jump { .. }));
    press(&mut app, "charge<cr>");
    assert_eq!(app.unit.id, CHARGE);
    press(&mut app, "K");
    let t = text(&app);
    assert!(t.contains("› what happens on a bad currency? aqh"), "{t}");
    assert!(t.contains("› and when pay fails?"), "{t}");
    // esc closes it; h goes back to the queue on the unit's row.
    press(&mut app, "<esc>");
    assert_eq!(app.unit.panel, None);
    assert_eq!(app.screen, Screen::Unit);
    press(&mut app, "h");
    assert_ne!(app.screen, Screen::Unit);
    assert_eq!(app.selected_id().as_deref(), Some(CHARGE));
}

#[test]
fn saying_no_to_the_web_asks_from_the_code_and_never_again() {
    let (_repo, review) = fixture();
    let (mut app, _) = started(review, true);
    app.open_unit(CHARGE.into());
    press(&mut app, "Kwhy?<cr>");
    assert_eq!(app.overlay, Overlay::ConfirmWeb);
    assert_eq!(
        press(&mut app, "n"),
        [Effect::Save, Effect::Ask { index: 0 }]
    );
    assert!(!app.web_allowed());
    assert_eq!(
        press(&mut app, "and?<cr>"),
        [Effect::Save, Effect::Ask { index: 1 }]
    );
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn a_pull_request_never_asks_about_the_web() {
    let (_repo, mut review) = fixture();
    review.pr = Some(teer::pr::parse_url("https://github.com/acme/shop/pull/7").unwrap());
    let (mut app, _) = started(review, true);
    app.open_unit(CHARGE.into());
    assert_eq!(
        press(&mut app, "Kwhy?<cr>"),
        [Effect::Save, Effect::Ask { index: 0 }]
    );
    assert!(!app.web_allowed());
}

// ---- U5: go-to ------------------------------------------------------------------

#[test]
fn goto_definition_callers_editor() {
    let (repo, review) = fixture();
    let (mut app, _) = started(review, true);

    // gd on new_helper: what it calls in the review, then there.
    app.open_unit(HELPER.into());
    press(&mut app, "gd");
    assert_eq!(
        app.overlay,
        Overlay::Jump {
            kind: JumpKind::Definition { of: HELPER.into() },
            query: String::new(),
            sel: 0
        }
    );
    press(&mut app, "<cr>");
    assert_eq!(app.screen, Screen::Unit);
    assert_eq!(app.unit.id, CHARGE);

    // gd on charge_order: nothing it calls is in the review; pay, outside
    // it, shows read-only under the text.
    press(&mut app, "gd");
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.unit.panel, Some(Panel::Outside));
    let t = text(&app);
    assert!(t.contains("What it calls outside the review"), "{t}");
    assert!(t.contains("payments.py:1-3 pay"), "{t}");
    assert!(t.contains("return charge(total)"), "{t}");
    press(&mut app, "<esc>");
    assert_eq!(app.unit.panel, None);

    // gr: what calls it, in the review.
    press(&mut app, "gr");
    assert!(matches!(
        app.overlay,
        Overlay::Jump {
            kind: JumpKind::Callers { .. },
            ..
        }
    ));
    let items: Vec<String> = app
        .jump_items()
        .iter()
        .map(|&i| app.review.units[i].id.clone())
        .collect();
    assert_eq!(items, [HELPER]);
    press(&mut app, "<esc>");

    // gf: the editor at the first changed line (the new `if`).
    let effects = press(&mut app, "gf");
    assert_eq!(
        effects,
        [Effect::Edit {
            file: PathBuf::from("checkout.py"),
            line: 9
        }]
    );
    assert_eq!(
        repo.file("checkout.py").lines().nth(8).unwrap().trim(),
        "if order.currency not in SUPPORTED:"
    );
}

// ---- U6: leader panels --------------------------------------------------------------

#[test]
fn leader_panels_and_gx() {
    let (_repo, review) = fixture();
    let (mut app, _) = started(review, true);
    app.open_unit(CHARGE.into());

    // <space> alone waits and names what follows.
    press(&mut app, "<space>");
    let hint: Vec<Action> = app.keys.hint().into_iter().map(|(_, a)| a).collect();
    for a in [
        Action::ToolTests,
        Action::ToolRunOldNew,
        Action::ToolDiagram,
    ] {
        assert!(hint.contains(&a), "{hint:?}");
    }
    // t: teer's search at once; the agent reads what it found on its own (Q6).
    let effects = press(&mut app, "t");
    assert!(matches!(
        &effects[..],
        [Effect::Dive {
            kind: DiveKind::Tests,
            ..
        }]
    ));
    assert_eq!(app.unit.panel, Some(Panel::Tool(DiveKind::Tests)));
    let t = text(&app);
    assert!(t.contains("␣t Tests"), "{t}");
    let found = AppEvent::Dive(DiveEvent::Ready {
        id: CHARGE.into(),
        fingerprint: app.review.unit(CHARGE).unwrap().fingerprint.clone(),
        kind: DiveKind::Tests,
        text: String::new(),
        sources: Vec::new(),
        visual: None,
        behavior: None,
        coverage: Some(Box::new(Coverage::found(TestSearch::default()))),
    });
    let effects = app.on_app_event(found);
    assert!(
        effects.contains(&Effect::InterpretTests {
            unit_id: CHARGE.into()
        }),
        "{effects:?}"
    );
    assert!(text(&app).contains("No test calls it"));

    // One panel at a time: run old and new takes its place.
    let effects = press(&mut app, "<space>r");
    assert!(matches!(
        &effects[..],
        [Effect::Dive {
            kind: DiveKind::Behavior,
            ..
        }]
    ));
    assert_eq!(app.unit.panel, Some(Panel::Tool(DiveKind::Behavior)));
    assert!(!text(&app).contains("␣t Tests"));

    // The diagram; gx before there is one says so, after opens it.
    let effects = press(&mut app, "<space>v");
    assert!(matches!(
        &effects[..],
        [Effect::Dive {
            kind: DiveKind::Visualize,
            web: false,
            ..
        }]
    ));
    assert_eq!(app.unit.panel, Some(Panel::Tool(DiveKind::Visualize)));
    assert!(press(&mut app, "gx").is_empty());
    assert!(app.status.starts_with("No picture yet"), "{}", app.status);
    let u = app.review.unit_mut(CHARGE).unwrap();
    u.dive_pending.clear();
    u.dive.results.insert(
        DiveKind::Visualize,
        teer::review::model::DiveResult {
            text: "a picture".into(),
            visual: Some(teer::review::model::Visual {
                claim: "a picture".into(),
                nodes: Vec::new(),
                edges: Vec::new(),
                path: PathBuf::from("/tmp/p.html"),
            }),
            ..Default::default()
        },
    );
    assert_eq!(
        press(&mut app, "gx"),
        [Effect::OpenVisual {
            unit_id: CHARGE.into()
        }]
    );
    // The same key again closes its panel.
    press(&mut app, "<space>v");
    assert_eq!(app.unit.panel, None);
}

// ---- U7: deciding here ----------------------------------------------------------------

#[test]
fn decide_and_advance_in_unit_view() {
    let (mut app, _) = started(v2::review(v2::units(9)), true);
    let ids = queue(&app);
    let first = ids[0].clone();
    app.sel = 0;
    app.open_unit(first.clone());

    // a: accepted, the next pending unit on show, still here.
    press(&mut app, "a");
    assert_eq!(
        app.review.unit(&first).unwrap().decision,
        Decision::Accepted
    );
    assert_eq!(app.screen, Screen::Unit);
    assert_eq!(app.unit.id, ids[1]);
    // .: again.
    press(&mut app, ".");
    assert_eq!(
        app.review.unit(&ids[1]).unwrap().decision,
        Decision::Accepted
    );
    assert_eq!(app.unit.id, ids[2]);
    // u: back to that unit, pending again.
    press(&mut app, "u");
    assert_eq!(
        app.review.unit(&ids[1]).unwrap().decision,
        Decision::Pending
    );
    assert_eq!(app.unit.id, ids[1]);

    // f: flagged, the note line on it while the view shows it; then on.
    press(&mut app, "f");
    assert_eq!(
        app.review.unit(&ids[1]).unwrap().decision,
        Decision::Investigate
    );
    assert!(matches!(&app.overlay, Overlay::Note { id, .. } if *id == ids[1]));
    assert_eq!(app.unit_on_show().unwrap().id, ids[1]);
    press(&mut app, "check this<cr>");
    assert_eq!(app.review.unit(&ids[1]).unwrap().dive.note, "check this");
    assert_eq!(app.unit_on_show().unwrap().id, ids[2]);

    // n and N move between pending units without deciding.
    press(&mut app, "n");
    assert_eq!(app.unit.id, ids[3]);
    press(&mut app, "N");
    assert_eq!(app.unit.id, ids[2]);
    assert_eq!(
        app.review.unit(&ids[2]).unwrap().decision,
        Decision::Pending
    );

    // A: the file's pending units (ids 2 here: f0.rs holds 0-2).
    press(&mut app, "n");
    assert_eq!(app.unit.id, ids[3]);
    press(&mut app, "A");
    for id in &ids[3..6] {
        assert_eq!(
            app.review.unit(id).unwrap().decision,
            Decision::Accepted,
            "{id}"
        );
    }
    assert_eq!(app.unit.id, ids[6]);
    assert_eq!(app.screen, Screen::Unit);
    // F: the same, flagged, with one note line for them.
    press(&mut app, "F");
    assert!(matches!(app.overlay, Overlay::Note { .. }));
    press(&mut app, "<esc>");
    for id in &ids[6..9] {
        assert_eq!(
            app.review.unit(id).unwrap().decision,
            Decision::Investigate,
            "{id}"
        );
    }
    // Only ids[2] is pending now.
    assert_eq!(app.unit.id, ids[2]);

    // q goes back to the queue, on the unit's row; it doesn't quit.
    let effects = press(&mut app, "q");
    assert!(!effects.contains(&Effect::Quit));
    assert_ne!(app.screen, Screen::Unit);
    assert_eq!(app.selected_id().as_deref(), Some(ids[2].as_str()));
}

#[test]
fn k_from_the_queue_opens_the_unit_asking() {
    let (mut app, _) = started(v2::review(mixed()), true);
    app.screen = Screen::Queue;
    let id = app.selected_id().unwrap();
    let effects = press(&mut app, "K");
    assert_eq!(app.screen, Screen::Unit);
    assert_eq!(app.unit.id, id);
    assert_eq!(app.unit.panel, Some(Panel::Ask));
    assert!(recommends(&effects).len() <= 2);
    press(&mut app, "<esc>h");
    assert_eq!(app.screen, Screen::Queue);
}
