//! Ask (`/`): a question about the code, the agent's answer checked by
//! teer (the code it shows read from the files, lines that exist, the
//! change's lines marked, units of the change linked), the view, the note
//! on the unit the answer is about, and the chat about a flagged unit.

mod common;

use std::sync::Arc;

use common::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;
use teer::agent::mock::MockAdapter;
use teer::config::{Action, Config, KeyMap};
use teer::review::model::{Answer, Decision, DiveKind, DiveResult, Question, Review, SessionRef};
use teer::review::prompts::ask_prompt;
use teer::review::validate::validate_answer;
use teer::review::{BuildOptions, ask, build_review, store};
use teer::tui::app::{App, AppEvent, Effect, Overlay, Pane, Screen};

const CART: &str = "class Cart:\n    def add(self, item):\n        self.items.append(item)\n";
const CHARGE: &str = "checkout.py::charge_order";

/// `checkout.py` changed (PY_OLD to PY_NEW, uncommitted), `cart.py` not.
fn fixture() -> (FixtureRepo, Review) {
    isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD)
        .write("cart.py", CART)
        .commit("init");
    repo.write("checkout.py", PY_NEW);
    let review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    (repo, review)
}

/// What the agent says to "how is an order charged, and show me Cart".
fn raw() -> serde_json::Value {
    json!({
        "text": "charge_order checks the currency, pays, and saves the payment. Cart only holds items.",
        "show": [
            {"file": "checkout.py", "start": 8, "end": 12, "why": "charging"},
            {"file": "cart.py", "start": 1, "end": 3, "why": "the Cart class"},
            {"file": "../outside.py", "start": 1, "end": 2, "why": "x"},
            {"file": "cart.py", "start": 40, "end": 41, "why": "x"},
        ],
        "related": [
            {"id": CHARGE, "why": "it charges the order"},
            {"id": "nope.py::nope", "why": "x"},
        ]
    })
}

fn answer() -> Answer {
    let (repo, review) = fixture();
    validate_answer(&raw(), &repo.path(), &review.files, &review.units)
        .0
        .unwrap()
}

#[test]
fn an_answer_shows_the_code_teer_read_and_marks_the_change() {
    let (repo, review) = fixture();
    let (a, dropped) = validate_answer(&raw(), &repo.path(), &review.files, &review.units);
    let a = a.unwrap();
    assert_eq!(a.shown.len(), 2);
    // Code of the change: the reviewed version, its changed lines marked.
    let charge = &a.shown[0];
    assert!(charge.in_change);
    assert_eq!(charge.code[0], "def charge_order(order):");
    assert_eq!(charge.changed, [9, 10, 12]);
    assert_eq!(charge.why, "charging");
    // Code outside the change, read from the file.
    let cart = &a.shown[1];
    assert!(!cart.in_change && cart.changed.is_empty());
    assert_eq!(cart.code, CART.lines().collect::<Vec<_>>());
    // Only units of this change are related.
    let ids: Vec<&str> = a.related.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, [CHARGE]);
    let why: Vec<&str> = dropped.iter().map(|d| d.reason.as_str()).collect();
    assert_eq!(
        why,
        [
            "outside the repository",
            "no such lines",
            "not a unit of this change"
        ]
    );
}

#[test]
fn code_of_the_change_relates_its_unit_and_long_regions_are_cut() {
    let (repo, review) = fixture();
    let raw = json!({
        "text": "",
        "show": [{"file": "checkout.py", "start": 1, "end": 500, "why": "all of it"}],
        "related": []
    });
    let a = validate_answer(&raw, &repo.path(), &review.files, &review.units)
        .0
        .unwrap();
    // Cut at the file's end; every unit it shows is related, not (top level).
    assert_eq!(a.shown[0].cite.end as usize, PY_NEW.lines().count());
    let ids: Vec<&str> = a.related.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&CHARGE), "{ids:?}");
    assert!(ids.iter().all(|id| !id.contains("(top level)")), "{ids:?}");
    assert!(a.related.iter().all(|r| r.why == "its code is shown below"));
    // Nothing at all is no answer.
    let empty = json!({"text": " ", "show": [], "related": []});
    assert!(
        validate_answer(&empty, &repo.path(), &review.files, &review.units)
            .0
            .is_err()
    );
}

#[test]
fn the_prompt_carries_the_change_the_unit_on_show_and_what_was_asked_before() {
    let (_repo, mut review) = fixture();
    review.questions.push(Question {
        text: "what does pay do?".into(),
        answer: Some(Answer {
            text: "It charges the card.".into(),
            ..Default::default()
        }),
        ..Default::default()
    });
    review.questions.push(Question {
        text: "and who calls this?".into(),
        unit: Some(CHARGE.into()),
        ..Default::default()
    });
    let p = ask_prompt(&review, 1);
    assert!(p.starts_with("ASK\n"), "{p}");
    assert!(
        p.contains(&format!("- {CHARGE} (changed) checkout.py:8-12")),
        "{p}"
    );
    assert!(p.contains(&format!("looking at {CHARGE}")), "{p}");
    assert!(
        p.contains("Q: what does pay do?\nA: It charges the card."),
        "{p}"
    );
    assert!(p.trim_end().ends_with("and who calls this?"), "{p}");
    assert!(!ask_prompt(&review, 0).contains("Asked before"));
}

#[tokio::test]
async fn the_agent_answers_read_only_and_without_the_web() {
    let (repo, mut review) = fixture();
    review.questions.push(Question {
        text: "show me the Cart class".into(),
        ..Default::default()
    });
    let mock = MockAdapter::new(|prompt, _| {
        assert!(prompt.starts_with("ASK"));
        Ok(json!({
            "text": "Here it is.",
            "show": [{"file": "cart.py", "start": 1, "end": 3, "why": "Cart"}],
            "related": []
        }))
    });
    let job = ask::Job::new(&review, 0, repo.path());
    let session = SessionRef::fresh("mock", repo.path());
    let a = ask::answer(Arc::new(mock.clone()), session, job)
        .await
        .unwrap();
    assert_eq!(a.shown[0].code[0], "class Cart:");
    assert_eq!(*mock.web_calls.lock().unwrap(), [false]);
}

// ---- The view ---------------------------------------------------------------

fn app(review: Review) -> App {
    let mut a = App::new(review, Config::default(), KeyMap::default());
    a.has_agent = true;
    a
}

fn code(a: &mut App, code: KeyCode) -> Vec<Effect> {
    a.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

fn typed(a: &mut App, text: &str) {
    for c in text.chars() {
        code(a, KeyCode::Char(c));
    }
}

fn screen(a: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| teer::tui::ui::draw(f, a)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
        .collect()
}

fn select(a: &mut App, id: &str) {
    a.sel = a
        .order
        .iter()
        .position(|&i| a.review.units[i].id == id)
        .unwrap();
}

#[test]
fn slash_asks_about_the_unit_on_show_and_shows_the_answer() {
    let (_repo, review) = fixture();
    let mut a = app(review);
    select(&mut a, CHARGE);
    assert!(code(&mut a, KeyCode::Char('/')).is_empty());
    assert_eq!(a.overlay, Overlay::Ask);
    assert!(screen(&a, 120, 40).contains("Ask anything about the code"));
    typed(&mut a, "how is an order charged?");
    assert_eq!(a.ask.input, "how is an order charged?");
    let effects = code(&mut a, KeyCode::Enter);
    assert_eq!(effects, [Effect::Save, Effect::Ask { index: 0 }]);
    assert!(a.ask.input.is_empty());
    let q = &a.review.questions[0];
    assert_eq!(q.unit.as_deref(), Some(CHARGE));
    let text = screen(&a, 120, 40);
    assert!(text.contains("› how is an order charged?"), "{text}");
    assert!(text.contains(teer::tui::ask::ASKING), "{text}");
    // A second Enter on nothing asks nothing.
    assert!(code(&mut a, KeyCode::Enter).is_empty());

    assert_eq!(
        a.on_app_event(AppEvent::Answer {
            index: 0,
            result: Ok(answer()),
        }),
        [Effect::Save]
    );
    let text = screen(&a, 120, 50);
    assert!(text.contains("noted on charge_order"), "{text}");
    assert!(
        text.contains("┃ agent (Claude Code's default model)"),
        "{text}"
    );
    assert!(text.contains("charge_order checks the currency"), "{text}");
    assert!(text.contains("In this change"), "{text}");
    assert!(text.contains("it charges the order"), "{text}");
    assert!(
        text.contains("checkout.py:8-12 charging · in this change"),
        "{text}"
    );
    assert!(text.contains("cart.py:1-3 the Cart class"), "{text}");
    // The lines the change added carry a +, the others don't.
    let row = |n: &str| {
        text.lines()
            .find(|l| l.contains(n))
            .unwrap_or_else(|| panic!("{n}:\n{text}"))
            .to_string()
    };
    assert!(row("raise ValueError").contains("10 +"), "{text}");
    assert!(row("result = pay(order)").contains("11  "), "{text}");
    assert!(row("class Cart:").contains("  1  "), "{text}");
}

#[test]
fn links_go_to_the_unit_in_the_list_and_open_code_in_the_editor() {
    let (_repo, mut review) = fixture();
    review.questions.push(Question {
        text: "how is an order charged?".into(),
        unit: None,
        answer: Some(answer()),
        error: None,
    });
    let mut a = app(review);
    code(&mut a, KeyCode::Char('/'));
    // tab: the newest answer's first link, the unit; then its two pieces of code.
    code(&mut a, KeyCode::Tab);
    assert_eq!(a.ask.link, Some(0));
    let text = screen(&a, 120, 50);
    assert!(
        text.contains("▌ ◆ charge_order  checkout.py:8-12"),
        "{text}"
    );
    code(&mut a, KeyCode::Char('j'));
    assert_eq!(
        code(&mut a, KeyCode::Enter),
        [Effect::Edit {
            file: "checkout.py".into(),
            line: 8
        }]
    );
    code(&mut a, KeyCode::Down);
    code(&mut a, KeyCode::Down);
    assert_eq!(a.ask.link, Some(2), "stops at the last link");
    // Back to typing, and letters go into the question again.
    code(&mut a, KeyCode::Tab);
    assert_eq!(a.ask.link, None);
    typed(&mut a, "j");
    assert_eq!(a.ask.input, "j");
    // A unit: the view closes on it in the list.
    a.screen = Screen::Flagged;
    code(&mut a, KeyCode::Tab);
    code(&mut a, KeyCode::Enter);
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(a.screen, Screen::Slim);
    assert_eq!(a.selected().unwrap().id, CHARGE);
}

#[test]
fn an_answer_is_noted_on_its_unit_where_it_is_seen() {
    let (_repo, review) = fixture();
    let mut a = app(review);
    // Asked on nothing in particular: it goes with the unit it relates to most.
    a.review.questions.push(Question {
        text: "how is an order charged?".into(),
        ..Default::default()
    });
    a.on_app_event(AppEvent::Answer {
        index: 0,
        result: Ok(answer()),
    });
    assert_eq!(a.review.questions[0].unit.as_deref(), Some(CHARGE));
    select(&mut a, CHARGE);
    let text = screen(&a, 140, 50);
    assert!(text.contains("╭ ? how is an order charged?"), "{text}");
    assert!(text.contains("charge_order checks the currency"), "{text}");
    assert!(text.contains("2 pieces of code · / shows them"), "{text}");
    // Not on other units.
    a.sel = (a.sel + 1) % a.order.len();
    assert!(!screen(&a, 140, 50).contains("? how is an order charged?"));
}

#[test]
fn without_an_agent_or_when_it_fails_the_question_says_why() {
    let (_repo, review) = fixture();
    let mut a = app(review);
    a.has_agent = false;
    code(&mut a, KeyCode::Char('/'));
    typed(&mut a, "anything?");
    assert_eq!(code(&mut a, KeyCode::Enter), [Effect::Save]);
    assert!(screen(&a, 120, 40).contains("✗ no agent"));
    // With one, a failure says so and ctrl+r asks again.
    a.has_agent = true;
    let ctrl_r = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert_eq!(
        a.on_event(ctrl_r.clone()),
        [Effect::Save, Effect::Ask { index: 0 }]
    );
    assert!(a.on_event(ctrl_r.clone()).is_empty(), "on its way already");
    a.on_app_event(AppEvent::Answer {
        index: 0,
        result: Err("boom".into()),
    });
    assert!(screen(&a, 120, 40).contains("✗ boom · ctrl+r asks again"));
    code(&mut a, KeyCode::Esc);
    assert_eq!(a.overlay, Overlay::None);
}

#[test]
fn questions_are_saved_and_survive_a_rerun() {
    let (repo, mut review) = fixture();
    review.questions.push(Question {
        text: "how is an order charged?".into(),
        unit: Some(CHARGE.into()),
        answer: Some(answer()),
        error: Some("not saved".into()),
    });
    store::save(&review).unwrap();
    let back = store::load(
        &review.repo_root,
        None,
        &review.base,
        &review.head,
        &review.head_commit,
    )
    .unwrap();
    assert_eq!(back.questions[0].answer, review.questions[0].answer);
    assert_eq!(back.questions[0].error, None);
    let mut again = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    assert!(again.questions.is_empty());
    store::merge_previous(&mut again, &back);
    assert_eq!(again.questions, back.questions);
}

// ---- The chat on the Flagged screen --------------------------------------------

/// The app on the Flagged screen, on `charge_order`.
fn flagged(review: Review) -> App {
    let mut a = app(review);
    a.review.unit_mut(CHARGE).unwrap().decision = Decision::Investigate;
    a.on_action(Action::Flagged);
    assert_eq!(a.screen, Screen::Flagged);
    a
}

#[test]
fn slash_on_a_flagged_unit_opens_its_chat_and_keeps_the_conversation() {
    let (_repo, review) = fixture();
    let mut a = flagged(review);
    // Offered in the tile that asks for more.
    let text = screen(&a, 160, 50);
    assert!(text.contains(" /  Chat "), "{text}");
    assert!(text.contains("ask about it in your own words"), "{text}");
    assert!(code(&mut a, KeyCode::Char('/')).is_empty());
    assert_eq!(a.flagged.pane, Pane::Chat);
    assert!(a.flagged.typing);
    let text = screen(&a, 160, 50);
    assert!(text.contains("/ Chat"), "{text}");
    assert!(
        text.contains("Ask about this unit in your own words"),
        "{text}"
    );
    // Letters go into the question, not to the screen's keys (q quits, d, r…).
    typed(&mut a, "does it charge twice? q");
    assert_eq!(a.ask.draft, "does it charge twice? q");
    assert_eq!(a.screen, Screen::Flagged);
    assert_eq!(
        code(&mut a, KeyCode::Enter),
        [Effect::Save, Effect::Ask { index: 0 }]
    );
    assert!(a.ask.draft.is_empty());
    assert_eq!(a.review.questions[0].unit.as_deref(), Some(CHARGE));
    assert!(screen(&a, 160, 50).contains(teer::tui::ask::ASKING));
    a.on_app_event(AppEvent::Answer {
        index: 0,
        result: Ok(answer()),
    });
    let text = screen(&a, 160, 50);
    assert!(text.contains("› does it charge twice? q"), "{text}");
    assert!(text.contains("charge_order checks the currency"), "{text}");
    assert!(text.contains("checkout.py:8-12 charging"), "{text}");
    // The chat is about this unit: where the answer is noted goes unsaid,
    // and it is not noted again above the tiles.
    assert!(!text.contains("noted on"), "{text}");
    assert!(!text.contains("╭ ? does it charge twice"), "{text}");

    // A follow-up: the agent gets the conversation so far and the unit.
    typed(&mut a, "and if pay fails?");
    assert_eq!(
        code(&mut a, KeyCode::Enter),
        [Effect::Save, Effect::Ask { index: 1 }]
    );
    let p = ask_prompt(&a.review, 1);
    assert!(
        p.contains("Q: does it charge twice? q\nA: charge_order checks the currency"),
        "{p}"
    );
    assert!(
        p.contains("Shown: checkout.py:8-12 (charging); cart.py:1-3 (the Cart class)"),
        "{p}"
    );
    assert!(p.contains("## The unit they are looking at"), "{p}");
    assert!(p.contains("def charge_order(order):"), "{p}");

    // esc stops typing: the screen's keys work again; the chat stays, with
    // its conversation, on the way back to the unit.
    code(&mut a, KeyCode::Esc);
    assert!(!a.flagged.typing);
    assert_eq!(a.screen, Screen::Flagged);
    code(&mut a, KeyCode::Esc);
    assert_eq!(a.screen, Screen::Slim);
    a.on_action(Action::Flagged);
    let u = a.flagged_unit().unwrap();
    assert!(a.flagged_panes(u).contains(&Pane::Chat));
    a.flagged.pane = Pane::Chat;
    let text = screen(&a, 160, 50);
    assert!(text.contains("› does it charge twice? q"), "{text}");
    assert!(text.contains("› and if pay fails?"), "{text}");
    // Both are in the Ask view too, noted on the unit.
    a.on_action(Action::Back);
    code(&mut a, KeyCode::Char('/'));
    assert_eq!(a.overlay, Overlay::Ask);
    assert!(screen(&a, 120, 50).contains("noted on charge_order"));
}

#[test]
fn an_empty_chat_closes_when_left_and_keys_move_in_and_out_of_it() {
    let (_repo, review) = fixture();
    let mut a = flagged(review);
    code(&mut a, KeyCode::Char('/'));
    typed(&mut a, "x");
    // tab leaves for the next tile; nothing was asked, so the chat goes.
    code(&mut a, KeyCode::Tab);
    assert_ne!(a.flagged.pane, Pane::Chat);
    assert!(!a.flagged.typing);
    let u = a.flagged_unit().unwrap();
    assert!(!a.flagged_panes(u).contains(&Pane::Chat));
    // The question typed so far waits for the next time.
    code(&mut a, KeyCode::Char('/'));
    assert_eq!(a.ask.draft, "x");
    code(&mut a, KeyCode::Backspace);
    // Not typing, q quits as anywhere on the screen; enter types again.
    code(&mut a, KeyCode::Esc);
    assert_eq!(a.flagged.pane, Pane::Chat);
    code(&mut a, KeyCode::Enter);
    assert!(a.flagged.typing);
    code(&mut a, KeyCode::Esc);
    assert_eq!(
        code(&mut a, KeyCode::Char('q')),
        [Effect::Save, Effect::Quit]
    );
}

#[test]
fn a_chat_follows_on_from_its_own_answers_more_than_from_others() {
    let (_repo, mut review) = fixture();
    let long = "x".repeat(2000);
    let mut ask = |text: &str, unit: Option<&str>, answer: &str| {
        review.questions.push(Question {
            text: text.into(),
            unit: unit.map(String::from),
            answer: Some(Answer {
                text: answer.into(),
                ..Default::default()
            }),
            error: None,
        });
    };
    for i in 0..10 {
        ask(&format!("chat {i}"), Some(CHARGE), &long);
    }
    for i in 0..5 {
        ask(&format!("other {i}"), None, &long);
    }
    ask("now?", Some(CHARGE), "");
    review.questions.last_mut().unwrap().answer = None;
    review.unit_mut(CHARGE).unwrap().dive.results.insert(
        DiveKind::Recommend,
        DiveResult {
            text: "Add a test for the EUR case.".into(),
            ..Default::default()
        },
    );
    let p = ask_prompt(&review, review.questions.len() - 1);
    // The chat's last 8 and the other last 3, in the order they were asked.
    let asked: Vec<&str> = p.lines().filter_map(|l| l.strip_prefix("Q: ")).collect();
    assert_eq!(
        asked,
        [
            "chat 2", "chat 3", "chat 4", "chat 5", "chat 6", "chat 7", "chat 8", "chat 9",
            "other 2", "other 3", "other 4"
        ]
    );
    let answers: Vec<usize> = p
        .lines()
        .filter_map(|l| l.strip_prefix("A: "))
        .map(str::len)
        .collect();
    assert_eq!(answers[0], 1500, "a chat answer, cut longer");
    assert_eq!(answers[10], 400);
    assert!(
        p.contains("- Recommend: Add a test for the EUR case."),
        "{p}"
    );
}

/// One question to the real Claude Code about teer's own code (read-only,
/// costs a few cents):
///
/// ```sh
/// cargo test --test ask live_ask -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore]
async fn live_ask() {
    use teer::agent::claude::ClaudeAdapter;
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut review = build_review(&root, &BuildOptions::default()).unwrap();
    review.questions.push(Question {
        text: "Show me the KeyMap struct and how a key is matched to an action.".into(),
        ..Default::default()
    });
    let started = std::time::Instant::now();
    let job = ask::Job::new(&review, 0, root.clone());
    let a = ask::answer(
        Arc::new(ClaudeAdapter::default()),
        SessionRef::fresh("claude", root),
        job,
    )
    .await
    .unwrap();
    println!("[{:.0?}] {}", started.elapsed(), a.text);
    for s in &a.shown {
        println!(
            "  {}:{}-{} {} (in change: {}, changed: {:?})",
            s.cite.file.display(),
            s.cite.start,
            s.cite.end,
            s.why,
            s.in_change,
            s.changed
        );
    }
    for r in &a.related {
        println!("  related {} — {}", r.id, r.why);
    }
    assert!(!a.shown.is_empty());
}
