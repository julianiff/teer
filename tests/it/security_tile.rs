//! Security lens: security_tile (see docs/security-plan.md).
//!
//! The Security tile (`<space>x`) with a scripted agent: findings keep
//! their source and sink only with real lines, a path stands only when
//! teer's call graph confirms each call, a finding that stands raises its
//! unit's level, the second model may dispute one, and the tile draws
//! teer's own facts, the findings, what holds and what was checked.

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::isolate_env;
use common::v2::{press, review, screen};
use serde_json::{Value, json};
use teer::agent::mock::MockAdapter;
use teer::config::Config;
use teer::git::{ChangedFile, FileStatus};
use teer::graph::{GraphData, SymbolNode};
use teer::keymap::Keymap;
use teer::review::attention::{Level, assess};
use teer::review::dive::DiveEvent;
use teer::review::model::{
    Decision, DiveKind, Prep, Range, Review, SessionRef, Side, Unit, UnitKind, UnitStatus,
};
use teer::security::audit::{self, Job};
use teer::security::{Class, Fact, Origin, SecurityConfig, Weight};
use teer::session::app::{AppEvent, Effect};
use teer::session::unit::Panel;
use teer::session::view::{TileBody, TileId, snapshot};
use teer::tui::app::App;

const ROUTES: &str = "<?php
class AssetController {
    public function show($request) {
        $id = $request->get('id');
        return $this->repo->byId($id);
    }
}
";

const REPO: &str = "<?php
class AssetRepository {
    public function byId($id) {
        $sql = \"SELECT * FROM assets WHERE id = \" . $id;
        return $this->db->query($sql);
    }
}
";

const LOG: &str = "<?php
function audit_log($line) {
    file_put_contents('/tmp/audit', $line);
}
";

const SHOW: &str = "src/routes.php::AssetController::show";
const BY_ID: &str = "src/repo.php::AssetRepository::byId";

fn unit(id: &str, file: &str, symbol: &str) -> Unit {
    Unit {
        id: id.into(),
        file: PathBuf::from(file),
        symbol: symbol.into(),
        kind: UnitKind::Method,
        status: UnitStatus::Changed,
        old_range: Some(Range::new(3, 6)),
        new_range: Some(Range::new(3, 6)),
        renamed_from: None,
        fingerprint: format!("fp-{symbol}"),
        changes: Vec::new(),
        prep: Prep::Skipped,
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Pending,
        summary: Some("Loads an asset by id".into()),
        nbc_reason: None,
        dive: Default::default(),
        accepted_by: None,
        reasons: Vec::new(),
        dive_pending: Default::default(),
        checks: Default::default(),
        security: Vec::new(),
    }
}

fn node(id: &str, file: &str, symbol: &str) -> SymbolNode {
    SymbolNode {
        id: id.into(),
        file: file.into(),
        symbol: symbol.into(),
        kind: UnitKind::Method,
        range: Range::new(3, 6),
        in_review: true,
    }
}

/// A review of two units in a caller → callee graph: the controller's
/// `show` calls the repository's `byId`, the unit under review. The
/// repository's file is the change (read from the review); the
/// controller and a logging file are only in the checkout under `root`.
fn fixture(root: &Path) -> Review {
    for (path, text) in [("src/routes.php", ROUTES), ("src/log.php", LOG)] {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let mut by_id = unit(BY_ID, "src/repo.php", "AssetRepository::byId");
    by_id.callers = vec![SHOW.into()];
    let mut show = unit(SHOW, "src/routes.php", "AssetController::show");
    show.callees = vec![BY_ID.into()];
    let mut r = review(vec![by_id, show]);
    r.repo_root = root.to_path_buf();
    r.files = vec![ChangedFile {
        path: "src/repo.php".into(),
        old_path: None,
        status: FileStatus::Modified,
        old_content: Some(REPO.replace(" . $id", " . intval($id)")),
        new_content: Some(REPO.into()),
    }];
    r.graph = GraphData {
        nodes: vec![
            node(SHOW, "src/routes.php", "AssetController::show"),
            node(BY_ID, "src/repo.php", "AssetRepository::byId"),
        ],
        edges: vec![(0, 1)],
    };
    r
}

fn job(r: &Review, root: &Path, config: SecurityConfig, second: bool) -> Job {
    let unit = r.unit(BY_ID).unwrap().clone();
    let file = r.files[0].clone();
    Job::new(r, unit, file, root.to_path_buf(), config, second)
}

fn cite(file: &str, start: u32, end: u32) -> Value {
    json!({ "file": file, "start": start, "end": end })
}

/// A finding with source, path and sink as given.
fn finding(title: &str, source: Value, path: Value, sink: Value) -> Value {
    json!({
        "class": "injection",
        "severity": "high",
        "confidence": "high",
        "title": title,
        "source": source,
        "path": path,
        "sink": sink,
        "missing": "a bound parameter: the id is put into the SQL as it came",
        "exploit": "Anyone who may call show sends id=1 OR 1=1 and reads every asset."
    })
}

/// The finding that holds: the request's id, through `byId`, into the query.
fn sqli() -> Value {
    finding(
        "The asset id from the request reaches the query unescaped",
        cite("src/routes.php", 4, 4),
        json!([{ "symbol": "AssetRepository::byId", "file": "src/repo.php", "start": 3, "end": 6 }]),
        cite("src/repo.php", 4, 5),
    )
}

fn answer(findings: Vec<Value>) -> Value {
    json!({
        "text": "The id reaches a query built by hand.",
        "findings": findings,
        "holds": [
            { "what": "show is behind the login", "file": "src/routes.php", "start": 3, "end": 3 },
            { "what": "made up", "file": "src/routes.php", "start": 90, "end": 91 }
        ]
    })
}

/// An agent that answers the audit with `audit` and the second model's
/// check with `check`.
fn agent(audit: Value, check: Value) -> MockAdapter {
    MockAdapter::new(move |prompt, _| {
        if prompt.starts_with("DIVE:Security") {
            Ok(audit.clone())
        } else if prompt.starts_with(audit::CHECK_MARKER) {
            Ok(check.clone())
        } else {
            anyhow::bail!("not a security prompt: {prompt}")
        }
    })
}

fn agrees() -> Value {
    json!({ "checks": [{ "index": 1, "agree": true, "why": "The id goes into the SQL as it came.",
                         "evidence": cite("src/repo.php", 4, 4) }] })
}

/// Run the audit of `job` with `m`; every event it sent.
async fn run(m: &MockAdapter, job: Job) -> Vec<DiveEvent> {
    // Dropped findings are logged under the data dir.
    isolate_env();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let session = SessionRef::fresh("mock", job.root.clone());
    audit::run(Arc::new(m.clone()), session, job, tx).await;
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

/// Apply `events` to `r`; the level `BY_ID` is assessed at then.
fn applied(r: &mut Review, events: &[DiveEvent]) -> Level {
    r.unit_mut(BY_ID)
        .unwrap()
        .dive_pending
        .insert(DiveKind::Security);
    for ev in events {
        r.apply_dive(ev);
    }
    assess(r, r.unit(BY_ID).unwrap()).level
}

fn audit_of(r: &Review) -> &teer::security::Audit {
    r.unit(BY_ID).unwrap().dive.results[&DiveKind::Security]
        .audit
        .as_ref()
        .unwrap()
}

#[tokio::test]
async fn a_finding_with_a_path_the_graph_confirms_stands_and_raises_the_unit() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = fixture(dir.path());
    let before = assess(&r, r.unit(BY_ID).unwrap()).level;
    assert_ne!(before, Level::Attention);
    let m = agent(answer(vec![sqli()]), agrees());
    let events = run(&m, job(&r, dir.path(), SecurityConfig::default(), true)).await;
    // What it is doing, the audit (the second model on its way), its word.
    assert!(matches!(&events[0], DiveEvent::Progress { text, .. } if text.contains("call chain")));
    let DiveEvent::Audit { audit: a, text, .. } = &events[1] else {
        panic!("{events:?}")
    };
    assert!(a.checking, "the second model checks it");
    assert_eq!(text, "The id reaches a query built by hand.");
    assert!(matches!(&events[2], DiveEvent::AuditChecked { checks: Some(c), .. } if c.len() == 1));
    assert_eq!(*m.second_calls.lock().unwrap(), 1);

    assert_eq!(applied(&mut r, &events), Level::Attention);
    let a = audit_of(&r);
    assert!(!a.checking);
    let f = &a.findings[0];
    assert!(f.path_verified && f.stands(), "{f:?}");
    assert_eq!(f.class, Class::Injection);
    assert_eq!(f.path[0].symbol, "AssetRepository::byId");
    assert_eq!(f.sink.file, PathBuf::from("src/repo.php"));
    assert!(f.second.as_ref().unwrap().agree);
    // The made-up hold is gone; what was checked and what not is said.
    assert_eq!(a.holds.len(), 1);
    assert_eq!(a.holds[0].what, "show is behind the login");
    assert_eq!(a.checked, Class::ALL);
    assert!(a.not_checked.contains(&"denial of service".to_string()));
    assert_eq!(a.dropped, 0);
    let reasons = assess(&r, r.unit(BY_ID).unwrap()).reasons;
    assert!(
        reasons.iter().any(|x| x.contains("reaches the query")),
        "{reasons:?}"
    );
}

#[tokio::test]
async fn a_finding_whose_lines_dont_exist_is_dropped_and_counted() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = fixture(dir.path());
    let m = agent(
        answer(vec![
            sqli(),
            finding(
                "Lines past the end of the file",
                cite("src/routes.php", 40, 41),
                json!([]),
                cite("src/repo.php", 4, 4),
            ),
            finding(
                "Outside the repository",
                cite("src/routes.php", 4, 4),
                json!([]),
                cite("../../etc/passwd", 1, 1),
            ),
            finding(
                "An absolute path elsewhere",
                cite("/etc/passwd", 1, 1),
                json!([]),
                cite("src/repo.php", 4, 4),
            ),
            json!({ "class": "nonsense", "title": "x", "source": cite("src/routes.php", 4, 4),
                    "sink": cite("src/repo.php", 4, 4) }),
        ]),
        json!({}),
    );
    let events = run(&m, job(&r, dir.path(), SecurityConfig::default(), false)).await;
    applied(&mut r, &events);
    let a = audit_of(&r);
    assert_eq!(a.findings.len(), 1, "{:?}", a.findings);
    assert_eq!(a.dropped, 4);
    assert_eq!(
        *m.second_calls.lock().unwrap(),
        0,
        "the second model is off"
    );
}

#[tokio::test]
async fn an_unverified_path_shows_but_raises_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = fixture(dir.path());
    let before = assess(&r, r.unit(BY_ID).unwrap()).level;
    let m = agent(
        answer(vec![
            // A hop whose lines don't exist: removed, the path unverified.
            finding(
                "A hop that isn't there",
                cite("src/routes.php", 4, 4),
                json!([{ "symbol": "Ghost::call", "file": "src/routes.php", "start": 70, "end": 72 }]),
                cite("src/repo.php", 4, 5),
            ),
            // The sink is in code nothing on the path calls.
            finding(
                "The id ends up in the audit log",
                cite("src/repo.php", 4, 4),
                json!([]),
                cite("src/log.php", 3, 3),
            ),
            // Not through the unit at all: about other code, dropped.
            finding(
                "Elsewhere",
                cite("src/routes.php", 4, 4),
                json!([]),
                cite("src/log.php", 3, 3),
            ),
            // The wrong way round: the callee doesn't call its caller.
            finding(
                "Backwards",
                cite("src/repo.php", 4, 4),
                json!([]),
                cite("src/routes.php", 5, 5),
            ),
        ]),
        json!({}),
    );
    let events = run(&m, job(&r, dir.path(), SecurityConfig::default(), false)).await;
    assert_eq!(applied(&mut r, &events), before);
    let a = audit_of(&r);
    assert_eq!(a.findings.len(), 3);
    assert_eq!(a.dropped, 1, "the finding about other code");
    assert!(a.findings.iter().all(|f| !f.path_verified && !f.stands()));
    let ghost = a
        .findings
        .iter()
        .find(|f| f.title == "A hop that isn't there")
        .unwrap();
    assert!(ghost.path.is_empty(), "the hop with no lines is removed");

    // In the tile: "path not verified".
    let mut app = app_on(r, true);
    press(&mut app, "<space>xzz");
    let out = screen(&app, 120, 60);
    assert!(out.contains("path not verified"), "{out}");
}

#[tokio::test]
async fn a_finding_the_second_model_disputes_doesnt_stand() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = fixture(dir.path());
    let before = assess(&r, r.unit(BY_ID).unwrap()).level;
    let dispute = json!({ "checks": [
        { "index": 1, "agree": false, "why": "The query binds the id on line 5.",
          "evidence": cite("src/repo.php", 5, 5) },
        { "index": 9, "agree": true, "why": "No such finding." }
    ] });
    let m = agent(answer(vec![sqli()]), dispute);
    let events = run(&m, job(&r, dir.path(), SecurityConfig::default(), true)).await;
    assert_eq!(applied(&mut r, &events), before);
    let f = &audit_of(&r).findings[0];
    assert!(f.path_verified);
    let s = f.second.as_ref().unwrap();
    assert!(!s.agree && !f.stands());
    assert_eq!(s.why, "The query binds the id on line 5.");

    // A failed check leaves the finding as it was, and stops waiting.
    let mut r = fixture(dir.path());
    let m = MockAdapter::new(|p, _| {
        if p.starts_with("DIVE:Security") {
            Ok(answer(vec![sqli()]))
        } else {
            anyhow::bail!("the second model is down")
        }
    });
    let events = run(&m, job(&r, dir.path(), SecurityConfig::default(), true)).await;
    assert!(matches!(
        events.last(),
        Some(DiveEvent::AuditChecked { checks: None, .. })
    ));
    assert_eq!(applied(&mut r, &events), Level::Attention);
    assert!(!audit_of(&r).checking);
}

#[tokio::test]
async fn the_prompt_names_the_checks_teers_catalog_finds_along_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let r = fixture(dir.path());
    // The route checks access, on a line of its own range (3-6).
    let routes = ROUTES.replace(
        "$id = $request->get('id');",
        "$this->denyAccessUnlessGranted('VIEW'); $id = $request->get('id');",
    );
    std::fs::write(dir.path().join("src/routes.php"), routes).unwrap();
    let m = agent(answer(vec![]), agrees());
    run(&m, job(&r, dir.path(), SecurityConfig::default(), false)).await;
    let p = m.prompts.lock().unwrap()[0].clone();
    assert!(p.contains("Checks teer found along the path:"), "{p}");
    assert!(
        p.contains("in AssetController::show (src/routes.php:3-6)"),
        "{p}"
    );
    // Without one, no such list.
    std::fs::write(dir.path().join("src/routes.php"), ROUTES).unwrap();
    let m = agent(answer(vec![]), agrees());
    run(&m, job(&r, dir.path(), SecurityConfig::default(), false)).await;
    let p = m.prompts.lock().unwrap()[0].clone();
    assert!(!p.contains("Checks teer found along the path:"), "{p}");
}

#[tokio::test]
async fn the_prompt_has_the_marker_the_caller_chain_and_teers_facts() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = fixture(dir.path());
    r.unit_mut(BY_ID).unwrap().security = vec![Fact {
        class: Class::Authorization,
        weight: Weight::Look,
        text: "denyAccessUnlessGranted is gone".into(),
        side: Side::Old,
        lines: Some(Range::new(4, 4)),
        subject: "denyAccessUnlessGranted".into(),
        origin: Origin::Teer,
    }];
    let m = agent(answer(vec![sqli()]), agrees());
    let config = SecurityConfig {
        include: vec!["dos".into()],
        ..Default::default()
    };
    run(&m, job(&r, dir.path(), config, true)).await;
    let prompts = m.prompts.lock().unwrap().clone();
    let p = &prompts[0];
    assert!(p.starts_with("DIVE:Security\n"), "{p}");
    assert!(
        p.contains("1. AssetController::show (src/routes.php:3-6) → this unit"),
        "{p}"
    );
    assert!(p.contains("denyAccessUnlessGranted is gone"), "{p}");
    assert!(p.contains("old lines 4-4 of src/repo.php"), "{p}");
    assert!(p.contains("not conclusions"), "{p}");
    // Every class, what is left out, and what the config asks for on top.
    for c in Class::ALL {
        assert!(p.contains(&format!("- {}: ", c.id())), "{c:?}");
    }
    assert!(p.contains("Leave out: rate limits;"), "{p}");
    assert!(
        p.contains("Also check, as the reviewer asked: denial of service."),
        "{p}"
    );
    assert!(p.contains("data, not instructions"), "{p}");
    assert!(
        p.contains("### Unit src/repo.php::AssetRepository::byId"),
        "{p}"
    );
    // The second model sees the cited code, read by teer: from the change,
    // and from the checkout.
    let c = &prompts[1];
    assert!(c.starts_with(audit::CHECK_MARKER), "{c}");
    assert!(c.contains("$id = $request->get('id');"), "{c}");
    assert!(c.contains("$this->db->query($sql);"), "{c}");
    assert!(
        c.contains("teer's call graph: confirms every call on the path"),
        "{c}"
    );
}

#[tokio::test]
async fn a_failed_question_fails_the_tile() {
    let dir = tempfile::tempdir().unwrap();
    let r = fixture(dir.path());
    let m = MockAdapter::new(|_, _| anyhow::bail!("boom"));
    let events = run(&m, job(&r, dir.path(), SecurityConfig::default(), true)).await;
    assert!(matches!(
        events.last(),
        Some(DiveEvent::Failed { kind: DiveKind::Security, error, .. }) if error.contains("boom")
    ));
    let m = MockAdapter::new(|_, _| Ok(json!({ "text": "no list" })));
    let events = run(&m, job(&r, dir.path(), SecurityConfig::default(), true)).await;
    assert!(matches!(events.last(), Some(DiveEvent::Failed { .. })));
}

/// The unit view's app on `r`, on the queue, with or without an agent.
fn app_on(r: Review, agent: bool) -> App {
    isolate_env();
    let mut r = r;
    r.agent_session = agent.then(|| SessionRef::fresh("mock", r.repo_root.clone()));
    let mut app = App::new(r, Config::default(), Keymap::default());
    app.has_agent = agent;
    app.sel = app
        .order
        .iter()
        .position(|&i| app.review.units[i].id == BY_ID)
        .unwrap();
    press(&mut app, "l");
    app
}

#[tokio::test]
async fn space_x_asks_and_the_tile_draws_facts_findings_holds_and_what_was_checked() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = fixture(dir.path());
    r.unit_mut(BY_ID).unwrap().security = vec![Fact {
        class: Class::Injection,
        weight: Weight::Note,
        text: "a query is built from a parameter".into(),
        side: Side::New,
        lines: Some(Range::new(4, 4)),
        subject: String::new(),
        origin: Origin::Teer,
    }];
    let events = {
        let m = agent(answer(vec![sqli()]), agrees());
        run(&m, job(&r, dir.path(), SecurityConfig::default(), true)).await
    };
    let mut app = app_on(r, true);
    let effects = press(&mut app, "<space>x");
    assert_eq!(
        effects,
        [Effect::Dive {
            unit_id: BY_ID.into(),
            kind: DiveKind::Security,
            web: false
        }]
    );
    assert_eq!(app.unit.panel, Some(Panel::Tool(DiveKind::Security)));
    press(&mut app, "zz");
    // teer's facts first, the agent on its way.
    let out = screen(&app, 120, 60);
    assert!(out.contains("␣x Security"), "{out}");
    assert!(out.contains("teer found"), "{out}");
    assert!(out.contains("a query is built from a parameter"), "{out}");
    assert!(out.contains("asking the agent"), "{out}");

    // The answer, then the second model's word.
    let (first, rest) = events.split_at(2);
    for ev in first {
        app.on_app_event(AppEvent::Dive(ev.clone()));
    }
    let out = screen(&app, 120, 60);
    assert!(out.contains(audit::CHECKING), "{out}");
    for ev in rest {
        app.on_app_event(AppEvent::Dive(ev.clone()));
    }
    let out = screen(&app, 120, 60);
    let at = |s: &str| out.find(s).unwrap_or_else(|| panic!("{s} missing:\n{out}"));
    assert!(at("teer found") < at("1 finding · 1 check holds"));
    assert!(at("1 finding · 1 check holds") < at("The asset id from the request"));
    assert!(out.contains("(high · injection)"), "{out}");
    assert!(
        at("source") < at("→ AssetRepository::byId") && at("→ AssetRepository::byId") < at("sink")
    );
    assert!(out.contains("AssetController::show"), "{out}");
    assert!(out.contains("routes.php:4"), "{out}");
    assert!(out.contains("repo.php:4-5"), "{out}");
    assert!(out.contains("✗ a bound parameter"), "{out}");
    assert!(out.contains("second model agrees"), "{out}");
    assert!(!out.contains("path not verified"), "{out}");
    assert!(at("second model agrees") < at("✓ show is behind the login"));
    assert!(out.contains("checked: auth · entry points"), "{out}");
    assert!(out.contains("not: denial of service"), "{out}");
    assert!(!out.contains(audit::CHECKING), "{out}");
    assert!(!out.contains("safe"), "{out}");
    // Focused, its keys: prove the findings; who wrote them.
    assert!(out.contains("␣p prove"), "{out}");
    assert!(out.contains("agent (Claude Code's default model)"), "{out}");
    // The finding stands: the unit is now one to look at closely.
    assert_eq!(app.assessment(BY_ID).unwrap().level, Level::Attention);

    // The desktop's view has the tile, teer's facts and the audit.
    let v = snapshot(&app, None);
    let tile = v
        .unit
        .unwrap()
        .tiles
        .into_iter()
        .find(|t| t.id == TileId::Security)
        .expect("the Security tile");
    let TileBody::Answer(a) = tile.body else {
        panic!("an answer tile")
    };
    assert_eq!(a.facts.len(), 1);
    assert_eq!(a.result.unwrap().audit.unwrap().findings.len(), 1);
    assert_eq!(a.ask.unwrap().action, "tool.prove");
}

#[test]
fn without_an_agent_the_tile_shows_teers_facts_and_says_findings_need_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = fixture(dir.path());
    r.unit_mut(BY_ID).unwrap().security = vec![Fact {
        class: Class::Secret,
        weight: Weight::Attention,
        text: "an API key in the code".into(),
        side: Side::New,
        lines: Some(Range::new(4, 4)),
        subject: String::new(),
        origin: Origin::Teer,
    }];
    let mut app = app_on(r, false);
    let effects = press(&mut app, "<space>x");
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(app.unit.panel, Some(Panel::Tool(DiveKind::Security)));
    let out = screen(&app, 120, 50);
    assert!(out.contains("an API key in the code"), "{out}");
    assert!(out.contains(audit::NO_AGENT), "{out}");
    assert!(!out.contains("␣p prove"), "{out}");
}
