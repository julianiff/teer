//! Behavior end to end: a scripted agent writes a Python probe, teer runs it
//! in the sandbox on its probe checkout before and after an uncommitted
//! change, and the table comes from what the runs printed. macOS only (the
//! sandbox), and skipped where python3 is missing.
#![cfg(target_os = "macos")]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{FixtureRepo, isolate_env};
use serde_json::{Value, json};
use teer::agent::mock::MockAdapter;
use teer::review::behavior::{Job, run};
use teer::review::dive::DiveEvent;
use teer::review::model::{Outcome, Review, SessionRef};
use teer::review::{BuildOptions, build_review};

const OLD: &str =
    "def discount(amount):\n    if amount > 100:\n        return amount * 0.9\n    return amount\n";
const NEW: &str = "def discount(amount):\n    if amount < 0:\n        raise ValueError(\"negative amount\")\n    if amount >= 100:\n        return amount * 0.9\n    return amount\n";

const PROBE: &str = r#"import json, sys
sys.path.insert(0, ".")
from pricing import discount

for name, amount in [("big", 150), ("edge", 100), ("neg", -5), ("small", 5)]:
    try:
        print("TEER_CASE " + json.dumps({"case": name, "output": discount(amount)}))
    except Exception as e:
        print("TEER_CASE " + json.dumps({"case": name, "error": type(e).__name__ + ": " + str(e)}))
"#;

fn has_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn plan(probe: &str) -> Value {
    json!({
        "files": [{"path": "teer_probe.py", "content": probe}],
        "command": ["python3", "teer_probe.py"],
        "cases": [
            {"name": "big", "input": "amount=150", "expect": "same", "why": "above both thresholds"},
            {"name": "edge", "input": "amount=100", "expect": "different", "why": "> became >="},
            {"name": "neg", "input": "amount=-5", "expect": "same", "why": "negatives pass through"},
            {"name": "small", "input": "amount=5", "expect": "same", "why": "below the threshold"}
        ]
    })
}

fn repo() -> (FixtureRepo, Review) {
    let r = FixtureRepo::new();
    r.write("pricing.py", OLD).commit("base");
    r.write("pricing.py", NEW);
    let review = build_review(&r.path(), &BuildOptions::default()).unwrap();
    (r, review)
}

fn job(review: &Review) -> Job {
    let unit = review
        .units
        .iter()
        .find(|u| u.symbol == "discount")
        .unwrap()
        .clone();
    let file = review.file_of(&unit).unwrap().clone();
    Job {
        repo: review.repo_root.clone(),
        base: review.base.clone(),
        files: review.files.clone(),
        review_id: review.id.clone(),
        unit,
        file,
        timeout: Duration::from_secs(30),
    }
}

/// The answer, after the progress the run reported (which must come first).
async fn ask(m: &MockAdapter, review: &Review) -> DiveEvent {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let session = SessionRef::fresh("mock", review.repo_root.clone());
    run(Arc::new(m.clone()), session, job(review), tx).await;
    let mut said = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        match ev {
            DiveEvent::Progress { text, .. } => said.push(text),
            other => {
                assert!(!said.is_empty(), "no progress before the answer");
                assert!(rx.try_recv().is_err(), "nothing after the answer");
                return other;
            }
        }
    }
    panic!("no answer; progress: {said:?}")
}

#[tokio::test]
async fn the_table_comes_from_running_the_code_before_and_after() {
    if !has_python() {
        return;
    }
    let env = isolate_env();
    let (r, review) = repo();
    let m = MockAdapter::new(|prompt, schema| {
        assert!(prompt.starts_with("DIVE:Behavior"), "{prompt}");
        assert!(
            prompt.contains("TEER_CASE"),
            "the protocol is in the prompt"
        );
        assert!(schema["properties"]["cases"].is_object());
        Ok(plan(PROBE))
    });
    let ev = ask(&m, &review).await;
    let DiveEvent::Ready {
        text,
        behavior: Some(b),
        ..
    } = &ev
    else {
        panic!("{ev:?}")
    };
    assert_eq!(*m.web_calls.lock().unwrap(), [false], "never the web");
    let case = |n: &str| b.cases.iter().find(|c| c.name == n).unwrap();
    assert_eq!(case("big").before, Outcome::Value("135.0".into()));
    assert!(!case("big").changed());
    assert_eq!(case("edge").before, Outcome::Value("100".into()));
    assert_eq!(case("edge").after, Outcome::Value("90.0".into()));
    assert!(case("edge").changed() && !case("edge").surprise());
    assert_eq!(case("neg").before, Outcome::Value("-5".into()));
    assert_eq!(
        case("neg").after,
        Outcome::Error("ValueError: negative amount".into())
    );
    assert!(case("neg").surprise(), "predicted the same, but it changed");
    assert!(!case("small").changed());
    assert_eq!(
        text,
        "4 cases ran before and after the change: 2 behave differently, 1 not as the agent predicted."
    );

    // The receipt: the probe and what each run printed.
    assert!(b.dir.starts_with(env.path().join("data/probes")), "{b:?}");
    assert_eq!(
        std::fs::read_to_string(b.dir.join("probe/teer_probe.py")).unwrap(),
        PROBE
    );
    let before = std::fs::read_to_string(b.dir.join("before.stdout")).unwrap();
    assert!(
        before.contains("\"case\": \"neg\", \"output\": -5"),
        "{before}"
    );

    // The user's checkout was never touched.
    assert_eq!(r.file("pricing.py"), NEW);
    assert!(!r.path().join("teer_probe.py").exists());
    assert_eq!(r.git(&["status", "--porcelain"]).trim(), "M pricing.py");
}

#[tokio::test]
async fn a_probe_that_fails_goes_back_to_the_agent_with_its_output() {
    if !has_python() {
        return;
    }
    isolate_env();
    let (_r, review) = repo();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let m = MockAdapter::new(move |prompt, _| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            // Imports a module that doesn't exist.
            return Ok(plan(&PROBE.replace("from pricing", "from pricingx")));
        }
        assert!(
            prompt.contains("your previous probe did not work"),
            "{prompt}"
        );
        assert!(prompt.contains("ModuleNotFoundError"), "{prompt}");
        Ok(plan(PROBE))
    });
    let ev = ask(&m, &review).await;
    assert!(
        matches!(
            &ev,
            DiveEvent::Ready {
                behavior: Some(_),
                ..
            }
        ),
        "{ev:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // Never more than the allowed tries.
    let m = MockAdapter::new(|_, _| Ok(plan("import sys\nsys.exit(3)\n")));
    let ev = ask(&m, &review).await;
    match &ev {
        DiveEvent::Failed { error, .. } => {
            assert!(error.contains("no working probe after 3 tries"), "{error}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(m.prompts.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn a_probe_cannot_reach_outside_its_checkout() {
    if !has_python() {
        return;
    }
    isolate_env();
    let (r, review) = repo();
    let home = std::env::var("HOME").unwrap();
    let escape = format!(
        r#"import json, os, urllib.request
def attempt(f):
    try:
        f()
        return "done"
    except Exception as e:
        return type(e).__name__
print("TEER_CASE " + json.dumps({{"case": "big", "output": attempt(lambda: os.listdir({home:?}))}}))
print("TEER_CASE " + json.dumps({{"case": "edge", "output": attempt(lambda: open({out:?}, "w").write("x"))}}))
print("TEER_CASE " + json.dumps({{"case": "neg", "output": attempt(lambda: urllib.request.urlopen("https://example.com", timeout=5))}}))
print("TEER_CASE " + json.dumps({{"case": "small", "output": attempt(lambda: open("inside.txt", "w").write("x"))}}))
"#,
        out = r.path().join("escaped.txt").display().to_string(),
    );
    let m = MockAdapter::new(move |_, _| Ok(plan(&escape)));
    let ev = ask(&m, &review).await;
    let DiveEvent::Ready {
        behavior: Some(b), ..
    } = &ev
    else {
        panic!("{ev:?}")
    };
    let case = |n: &str| b.cases.iter().find(|c| c.name == n).unwrap().after.clone();
    assert_eq!(case("big"), Outcome::Value("\"PermissionError\"".into()));
    assert_eq!(case("edge"), Outcome::Value("\"PermissionError\"".into()));
    assert_ne!(case("neg"), Outcome::Value("\"done\"".into()));
    assert_eq!(case("small"), Outcome::Value("\"done\"".into()));
    assert!(!r.path().join("escaped.txt").exists());
}

#[tokio::test]
async fn t_on_the_flagged_screen_runs_the_probe_and_shows_the_table() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use teer::config::{Config, KeyMap};
    use teer::review::model::{Decision, DiveKind};
    use teer::tui::Runtime;
    use teer::tui::app::{App, Effect, Screen};

    if !has_python() {
        return;
    }
    isolate_env();
    let (_r, mut review) = repo();
    review.agent_session = Some(SessionRef::fresh("mock", review.repo_root.clone()));
    for u in review.units.iter_mut().filter(|u| u.symbol == "discount") {
        u.decision = Decision::Investigate;
    }
    let mut app = App::new(review, Config::default(), KeyMap::default());
    let press = |app: &mut App, c: char| {
        app.on_event(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )))
    };
    press(&mut app, 'd');
    assert_eq!(app.screen, Screen::Flagged);
    let effects = press(&mut app, 't');
    assert!(
        matches!(
            &effects[..],
            [Effect::Dive {
                kind: DiveKind::Behavior,
                ..
            }]
        ),
        "{effects:?}"
    );
    // Zoomed, its tile is as wide as the screen.
    press(&mut app, 'z');
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let m = MockAdapter::new(|_, _| Ok(plan(PROBE)));
    let rt = Runtime {
        adapter: Some(Arc::new(m.clone())),
        tx,
        build: BuildOptions::default(),
        tools: Default::default(),
    };
    for e in effects {
        rt.exec(&mut app, e);
    }
    let mut saw_progress = false;
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(60), rx.recv())
            .await
            .expect("the run finishes")
            .unwrap();
        let said = match &ev {
            teer::tui::app::AppEvent::Dive(DiveEvent::Progress { text, .. }) => Some(text.clone()),
            _ => None,
        };
        let done = said.is_none();
        if let Some(said) = said.filter(|_| !saw_progress) {
            saw_progress = true;
            app.on_app_event(ev);
            // The tile says what is happening while it runs.
            let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
            t.draw(|f| teer::tui::ui::draw(f, &app)).unwrap();
            let buf = t.backend().buffer().clone();
            let text: String = (0..40)
                .map(|y| (0..120).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
                .collect();
            let said = said.trim_end_matches('…');
            assert!(text.contains(said), "{said}:\n{text}");
            continue;
        }
        app.on_app_event(ev);
        if done {
            break;
        }
    }
    assert!(
        app.progress.is_empty(),
        "progress is cleared with the answer"
    );
    let u = app.flagged_unit().unwrap();
    assert!(u.dive_pending.is_empty());
    assert!(u.dive.results[&DiveKind::Behavior].behavior.is_some());
    let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
    t.draw(|f| teer::tui::ui::draw(f, &app)).unwrap();
    let buf = t.backend().buffer().clone();
    let text: String = (0..40)
        .map(|y| (0..120).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
        .collect();
    assert!(text.contains("2 behave differently"), "{text}");
    assert!(text.contains("! neg"), "{text}");
    // Whatever the screen allowed, the probe is written without the web.
    assert_eq!(*m.web_calls.lock().unwrap(), [false]);
}
