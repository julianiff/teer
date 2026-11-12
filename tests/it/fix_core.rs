//! Proposed changes: fix_core (see FEATURES.md, proposed changes).
//!
//! The core with a scripted agent: what a proposal answers (a standing
//! finding over the reviewer's note over the advice), the prompt, what
//! teer keeps of the hunks (each found exactly once, inside the
//! repository, the file still parsing), the second model's word, writing
//! hunks to the working tree all or nothing, and the runtime carrying
//! both out.

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::isolate_env;
use common::v2::review;
use serde_json::{Value, json};
use teer::agent::mock::MockAdapter;
use teer::config::Config;
use teer::fix::apply::apply;
use teer::fix::propose::{self, Job, target};
use teer::fix::{HunkState, Proposal, ProposedHunk, Reason};
use teer::git::{ChangedFile, FileStatus};
use teer::keymap::Keymap;
use teer::review::BuildOptions;
use teer::review::dive::DiveEvent;
use teer::review::model::{
    Cite, Decision, DiveKind, DiveResult, PrInfo, Prep, Range, Review, Risk, SessionRef, Unit,
    UnitKind, UnitStatus,
};
use teer::security::{Audit, Class, Finding, SecondCheck};
use teer::session::Runtime;
use teer::session::app::{App, AppEvent, Effect};

const REPO: &str = "<?php
class AssetRepository {
    public function byId($id) {
        $sql = \"SELECT * FROM assets WHERE id = \" . $id;
        return $this->db->query($sql);
    }
}
";

const ROUTES: &str = "<?php
class AssetController {
    public function show($request) {
        $id = $request->get('id');
        return $this->repo->byId($id);
    }
}
";

const APP_TS: &str = "export function total(items: number[]): number {
  return items.reduce((a, b) => a + b, 0);
}
";

const BY_ID: &str = "src/repo.php::AssetRepository::byId";

/// The query, bound: lines 4-5 of the repository.
const QUERY_OLD: &str = "        $sql = \"SELECT * FROM assets WHERE id = \" . $id;\n        return $this->db->query($sql);";
const QUERY_NEW: &str = "        $sql = \"SELECT * FROM assets WHERE id = ?\";\n        return $this->db->query($sql, [$id]);";

fn unit() -> Unit {
    Unit {
        id: BY_ID.into(),
        file: PathBuf::from("src/repo.php"),
        symbol: "AssetRepository::byId".into(),
        kind: UnitKind::Method,
        status: UnitStatus::Changed,
        old_range: Some(Range::new(3, 6)),
        new_range: Some(Range::new(3, 6)),
        renamed_from: None,
        fingerprint: "fp-byId".into(),
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

fn cite(file: &str, start: u32, end: u32) -> Cite {
    Cite {
        file: file.into(),
        start,
        end,
    }
}

/// A finding titled `title`: its path verified or not, the second model's
/// word on it or none.
fn finding(title: &str, severity: Risk, verified: bool, agree: Option<bool>) -> Finding {
    Finding {
        class: Class::Injection,
        severity,
        confidence: Risk::High,
        title: title.into(),
        source: cite("src/routes.php", 4, 4),
        path: Vec::new(),
        sink: cite("src/repo.php", 4, 5),
        missing: "a bound parameter".into(),
        exploit: "Anyone sends id=1 OR 1=1 and reads every asset.".into(),
        path_verified: verified,
        second: agree.map(|agree| SecondCheck {
            agree,
            why: "said so".into(),
            evidence: None,
        }),
        proof: None,
    }
}

fn with_audit(u: &mut Unit, findings: Vec<Finding>) {
    u.dive.results.insert(
        DiveKind::Security,
        DiveResult {
            text: "audit".into(),
            audit: Some(Audit {
                findings,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
}

/// The unit with a high finding that stands: the reason to fix.
fn unit_to_fix() -> Unit {
    let mut u = unit();
    with_audit(
        &mut u,
        vec![finding(
            "The asset id reaches the query unescaped",
            Risk::High,
            true,
            Some(true),
        )],
    );
    u
}

/// A checkout under `root` with the repository's file as reviewed, the
/// controller and a TypeScript file; a review of `u` in it.
fn fixture(root: &Path, u: Unit) -> Review {
    for (path, text) in [
        ("src/repo.php", REPO),
        ("src/routes.php", ROUTES),
        ("web/app.ts", APP_TS),
    ] {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let mut r = review(vec![u]);
    r.repo_root = root.to_path_buf();
    r.files = vec![ChangedFile {
        path: "src/repo.php".into(),
        old_path: None,
        status: FileStatus::Modified,
        old_content: Some(REPO.replace(" . $id", " . intval($id)")),
        new_content: Some(REPO.into()),
    }];
    r
}

fn job(r: &Review, second: bool) -> Job {
    let u = r.unit(BY_ID).unwrap().clone();
    let file = r.files[0].clone();
    Job::new(r, u, file, r.repo_root.clone(), second)
}

fn hunk(file: &str, old: &str, new: &str) -> Value {
    json!({ "file": file, "old": old, "new": new, "why": "Binds the id\nas a parameter." })
}

fn answer(hunks: Vec<Value>) -> Value {
    json!({ "summary": "Binds the id instead of concatenating it.", "hunks": hunks })
}

/// An agent that answers the proposal with `fix` and the second model
/// with `check`.
fn agent(fix: Value, check: Value) -> MockAdapter {
    MockAdapter::new(move |prompt, _| {
        if prompt.starts_with("DIVE:Fix") {
            Ok(fix.clone())
        } else if prompt.starts_with(propose::CHECK_MARKER) {
            Ok(check.clone())
        } else {
            anyhow::bail!("not a fix prompt: {prompt}")
        }
    })
}

fn agrees() -> Value {
    json!({ "agree": true, "why": "The id is bound, nothing else changes.",
            "evidence": { "file": "src/repo.php", "start": 4, "end": 5 } })
}

/// Run the proposal of `job` with `m`; every event it sent.
async fn run(m: &MockAdapter, job: Job) -> Vec<DiveEvent> {
    // Dropped hunks are logged under the data dir.
    isolate_env();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let session = SessionRef::fresh("mock", job.root.clone());
    propose::run(Arc::new(m.clone()), session, job, tx).await;
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

/// The proposal the last event carries.
fn proposal(events: &[DiveEvent]) -> Proposal {
    match events.last() {
        Some(DiveEvent::Proposed { proposal, text, .. }) => {
            assert_eq!(text, &proposal.summary);
            (**proposal).clone()
        }
        other => panic!("no proposal: {other:?}\n{events:?}"),
    }
}

// ---- What a proposal answers ---------------------------------------------------

#[test]
fn a_standing_finding_comes_before_the_note_and_the_note_before_the_advice() {
    let mut u = unit();
    with_audit(
        &mut u,
        vec![
            finding("Medium one", Risk::Medium, true, None),
            finding("Unverified high", Risk::High, false, None),
            finding("High one", Risk::High, true, Some(true)),
            finding("Disputed high", Risk::High, true, Some(false)),
        ],
    );
    u.dive.note = "Bind the id.".into();
    u.dive.results.insert(
        DiveKind::Recommend,
        DiveResult {
            text: "1. Use a prepared statement.".into(),
            ..Default::default()
        },
    );

    // The most severe finding that stands; all that stand in the context.
    let (reason, context) = target(&u).unwrap();
    assert_eq!(
        reason,
        Reason::Finding {
            index: 2,
            title: "High one".into(),
            severity: Risk::High
        }
    );
    let at = |s: &str| context.find(s).unwrap_or_else(|| panic!("{s}:\n{context}"));
    assert!(at("1. [high severity, injection] High one") < at("2. [medium severity"));
    assert!(context.contains("Medium one"), "{context}");
    assert!(context.contains("sink: src/repo.php:4-5"), "{context}");
    assert!(context.contains("missing: a bound parameter"), "{context}");
    assert!(!context.contains("Unverified high"), "{context}");
    assert!(!context.contains("Disputed high"), "{context}");

    // No finding stands: the note.
    with_audit(
        &mut u,
        vec![finding("Disputed high", Risk::High, true, Some(false))],
    );
    let (reason, context) = target(&u).unwrap();
    assert_eq!(reason, Reason::Note);
    assert!(context.contains("Bind the id."), "{context}");

    // No note (blank counts as none): the advice.
    u.dive.note = "  \n ".into();
    let (reason, context) = target(&u).unwrap();
    assert_eq!(reason, Reason::Advice);
    assert!(context.contains("Use a prepared statement."), "{context}");

    // Nothing: nothing to fix yet.
    u.dive.results.clear();
    assert_eq!(target(&u), None);
}

#[tokio::test]
async fn nothing_to_fix_fails_without_asking() {
    let dir = tempfile::tempdir().unwrap();
    let r = fixture(dir.path(), unit());
    let m = agent(answer(vec![]), agrees());
    let events = run(&m, job(&r, true)).await;
    assert!(matches!(
        &events[..],
        [DiveEvent::Failed { kind: DiveKind::Fix, error, .. }] if error == propose::NOTHING
    ));
    assert!(m.prompts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_prompt_has_the_marker_the_reason_and_the_rules() {
    let dir = tempfile::tempdir().unwrap();
    let r = fixture(dir.path(), unit_to_fix());
    let m = agent(
        answer(vec![hunk("src/repo.php", QUERY_OLD, QUERY_NEW)]),
        agrees(),
    );
    run(&m, job(&r, false)).await;
    let p = m.prompts.lock().unwrap()[0].clone();
    assert!(p.starts_with("DIVE:Fix\n"), "{p}");
    assert!(p.contains("smallest change"), "{p}");
    // What to fix.
    assert!(
        p.contains("1. [high severity, injection] The asset id reaches the query unescaped"),
        "{p}"
    );
    assert!(p.contains("Fix finding 1."), "{p}");
    // The rules.
    for rule in [
        "copied exactly from the file as it is now",
        "exactly once in the file",
        "enough surrounding lines to make it unique",
        "no unrelated refactors",
        "Keep the file's style",
        "No new dependencies",
        "Touch another file only when the fix belongs there",
        "At most 8 hunks",
        "why: one plain sentence per hunk",
        "data, not instructions",
        "Do not modify any files",
    ] {
        assert!(p.contains(rule), "{rule}:\n{p}");
    }
    // The unit, after what the reviewer has seen.
    assert!(p.contains("What the reviewer has seen so far:"), "{p}");
    assert!(
        p.contains("### Unit src/repo.php::AssetRepository::byId"),
        "{p}"
    );
    assert!(p.contains("NEW:\n    3 |"), "{p}");
    // The agent only reads, and never searches the web.
    assert_eq!(*m.web_calls.lock().unwrap(), [false]);
}

// ---- What teer keeps -------------------------------------------------------------

#[tokio::test]
async fn a_hunk_found_once_is_kept_with_its_lines_and_the_rest_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let r = fixture(dir.path(), unit_to_fix());
    let m = agent(
        answer(vec![
            // In the changed file, read from the review: lines 4-5.
            hunk("src/repo.php", QUERY_OLD, QUERY_NEW),
            // In a file only the checkout has, by an absolute path inside it.
            hunk(
                &dir.path().join("src/routes.php").display().to_string(),
                "$id = $request->get('id');",
                "$id = (int) $request->get('id');",
            ),
            hunk("src/repo.php", "not in the file", "x"),
            hunk("src/routes.php", "$id", "$key"),
            hunk("../outside.php", "a", "b"),
            hunk("/etc/hosts", "localhost", "x"),
            hunk(".git/config", "[core]", "[core]\n\tfsmonitor = evil"),
            // Overlaps the first hunk.
            hunk(
                "src/repo.php",
                "        return $this->db->query($sql);",
                "        return $this->db->run($sql);",
            ),
        ]),
        agrees(),
    );
    let events = run(&m, job(&r, false)).await;
    assert!(
        matches!(&events[0], DiveEvent::Progress { kind: DiveKind::Fix, text, .. } if text == propose::WRITING)
    );
    let p = proposal(&events);
    assert_eq!(
        p.reason,
        Reason::Finding {
            index: 0,
            title: "The asset id reaches the query unescaped".into(),
            severity: Risk::High
        }
    );
    assert_eq!(p.summary, "Binds the id instead of concatenating it.");
    assert_eq!(p.hunks.len(), 2, "{:#?}", p.hunks);
    assert_eq!(p.dropped, 6);
    // In file order, then line order.
    let h = &p.hunks[0];
    assert_eq!(h.file, PathBuf::from("src/repo.php"));
    assert_eq!(h.lines, Range::new(4, 5));
    assert_eq!(h.old, QUERY_OLD, "the code is kept as written");
    assert_eq!(h.new, QUERY_NEW);
    assert_eq!(h.why, "Binds the id as a parameter.", "why is one line");
    assert!(h.parses);
    assert_eq!(h.state, HunkState::Proposed);
    let h = &p.hunks[1];
    assert_eq!(h.file, PathBuf::from("src/routes.php"));
    assert_eq!(h.lines, Range::new(4, 4));
    assert!(p.second.is_none());
    assert_eq!(
        *m.second_calls.lock().unwrap(),
        0,
        "the second model is off"
    );
}

/// What [`propose::validate`] drops of `hunks`, as `(statement, reason)`,
/// and how many it keeps.
fn dropped(r: &Review, hunks: Vec<Value>) -> (usize, Vec<(String, String)>) {
    let (p, dropped) = propose::validate(&job(r, false), Reason::Note, &answer(hunks));
    let kept = p.map(|p| p.hunks.len()).unwrap_or(0);
    let d = dropped
        .into_iter()
        .map(|d| (d.statement, d.reason))
        .collect();
    (kept, d)
}

#[test]
fn every_hunk_dropped_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let r = fixture(dir.path(), unit_to_fix());
    let reasons =
        |d: &[(String, String)]| -> Vec<String> { d.iter().map(|(_, r)| r.clone()).collect() };
    let (kept, d) = dropped(
        &r,
        vec![
            hunk("src/repo.php", QUERY_OLD, QUERY_NEW),
            hunk("src/repo.php", "not in the file", "x"),
            hunk("src/routes.php", "$id", "$key"),
            hunk("../outside.php", "a", "b"),
            hunk("/etc/hosts", "localhost", "x"),
            hunk(".git/config", "[core]", "x"),
            hunk("src/missing.php", "a", "b"),
            hunk("src/repo.php", "query($sql);", "run($sql);"),
        ],
    );
    assert_eq!(kept, 1);
    assert_eq!(d[0].0, "hunk 2 in src/repo.php");
    assert_eq!(
        reasons(&d),
        [
            "its old text isn't in the file",
            "its old text is in the file more than once",
            "'../outside.php' is outside the repository",
            "'/etc/hosts' is outside the repository",
            "'.git/config' is outside the repository",
            "src/missing.php: can't be read (unreadable)",
            "it overlaps a hunk before it",
        ]
    );

    let long = "x".repeat(propose::MAX_HUNK_BYTES + 1);
    let (kept, d) = dropped(
        &r,
        vec![
            hunk("src/repo.php", "    }\n}", "    }\n}"),
            hunk("src/repo.php", "", "x"),
            hunk("web/app.ts", "a + b", "a +\0 b"),
            hunk("web/app.ts", "a + b", "a \u{1b}[2J+ b"),
            hunk("web/app.ts", "a + b", "a\u{202E} + b"),
            hunk("web/app.ts", "a + b", &long),
            hunk("web/app.ts", "a + b", "a - b"),
        ],
    );
    assert_eq!(kept, 1, "{d:?}");
    assert_eq!(
        reasons(&d),
        [
            "it changes nothing",
            "no old text",
            "a control character (U+0000)",
            "a control character (U+001B)",
            "it adds a bidirectional control character",
            format!("longer than {} bytes", propose::MAX_HUNK_BYTES).as_str(),
        ]
    );

    // At most 8: the ninth goes, however good.
    let mut hunks = vec![hunk("src/repo.php", QUERY_OLD, QUERY_NEW)];
    hunks.extend((0..7).map(|_| hunk("src/repo.php", "not there", "x")));
    hunks.push(hunk("web/app.ts", "a + b", "a - b"));
    let (kept, d) = dropped(&r, hunks);
    assert_eq!(kept, 1);
    assert_eq!(d.len(), 8);
    assert_eq!(d[7].1, "more than 8 hunks");
}

#[tokio::test]
async fn a_deletion_is_kept_and_a_hunk_that_breaks_the_syntax_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let r = fixture(dir.path(), unit_to_fix());
    let m = agent(
        answer(vec![
            // A deletion: the line and its newline.
            hunk("src/routes.php", "        $id = $request->get('id');\n", ""),
            // A closing parenthesis lost, in PHP and in TypeScript.
            hunk(
                "src/repo.php",
                "return $this->db->query($sql);",
                "return $this->db->query($sql;",
            ),
            hunk(
                "web/app.ts",
                "items.reduce((a, b) => a + b, 0);",
                "items.reduce((a, b) => a + b, 0;",
            ),
        ]),
        agrees(),
    );
    let p = proposal(&run(&m, job(&r, false)).await);
    assert_eq!(p.dropped, 0);
    let by = |f: &str| p.hunks.iter().find(|h| h.file == Path::new(f)).unwrap();
    let del = by("src/routes.php");
    assert_eq!(del.new, "");
    assert_eq!(del.lines, Range::new(4, 4));
    assert!(del.parses, "{del:?}");
    assert_eq!(del.diff_lines(), ["-        $id = $request->get('id');"]);
    assert!(!by("src/repo.php").parses);
    assert!(!by("web/app.ts").parses);
}

#[tokio::test]
async fn an_answer_with_no_hunk_that_applies_fails() {
    let dir = tempfile::tempdir().unwrap();
    let r = fixture(dir.path(), unit_to_fix());
    let m = agent(
        answer(vec![
            hunk("src/repo.php", "not there", "x"),
            hunk("src/routes.php", "$id", "$key"),
        ]),
        agrees(),
    );
    let events = run(&m, job(&r, true)).await;
    let Some(DiveEvent::Failed {
        kind: DiveKind::Fix,
        error,
        ..
    }) = events.last()
    else {
        panic!("{events:?}")
    };
    assert!(
        error.starts_with("the agent's change didn't apply: "),
        "{error}"
    );
    assert!(error.contains("isn't in the file"), "{error}");
    assert!(error.contains("more than once"), "{error}");
    assert_eq!(*m.second_calls.lock().unwrap(), 0);

    // None proposed: the agent's reason.
    let m = agent(
        json!({ "summary": "Only a schema change fixes it.", "hunks": [] }),
        agrees(),
    );
    let events = run(&m, job(&r, true)).await;
    assert!(matches!(
        events.last(),
        Some(DiveEvent::Failed { error, .. })
            if error == "the agent's change didn't apply: it proposed none: Only a schema change fixes it."
    ));

    // No list, or a failed question.
    let m = agent(json!({ "summary": "x" }), agrees());
    let events = run(&m, job(&r, true)).await;
    assert!(matches!(events.last(), Some(DiveEvent::Failed { .. })));
    let m = MockAdapter::new(|_, _| anyhow::bail!("boom"));
    let events = run(&m, job(&r, true)).await;
    assert!(matches!(
        events.last(),
        Some(DiveEvent::Failed { kind: DiveKind::Fix, error, .. }) if error.contains("boom")
    ));
}

// ---- The second model ------------------------------------------------------------

#[tokio::test]
async fn the_second_model_agrees_with_the_change_it_was_shown() {
    let dir = tempfile::tempdir().unwrap();
    let r = fixture(dir.path(), unit_to_fix());
    let m = agent(
        answer(vec![hunk("src/repo.php", QUERY_OLD, QUERY_NEW)]),
        agrees(),
    );
    let events = run(&m, job(&r, true)).await;
    let said: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            DiveEvent::Progress { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(said, [propose::WRITING, propose::CHECKING]);
    let p = proposal(&events);
    let s = p.second.expect("checked");
    assert!(s.agree);
    assert_eq!(s.why, "The id is bound, nothing else changes.");
    assert_eq!(s.evidence, Some(cite("src/repo.php", 4, 5)));
    assert_eq!(*m.second_calls.lock().unwrap(), 1);

    // It saw the reason, the change as a diff, and the code around it.
    let c = m.prompts.lock().unwrap()[1].clone();
    assert!(c.starts_with(propose::CHECK_MARKER), "{c}");
    assert!(
        c.contains("Does it fix the reason, and change nothing else?"),
        "{c}"
    );
    assert!(
        c.contains("## The reason: the security finding \"The asset id reaches the query unescaped\" (high severity)"),
        "{c}"
    );
    assert!(c.contains("--- src/repo.php:4-5"), "{c}");
    assert!(
        c.contains("+        return $this->db->query($sql, [$id]);"),
        "{c}"
    );
    assert!(c.contains("Around it, as src/repo.php is now:"), "{c}");
    assert!(c.contains("    2 | class AssetRepository {"), "{c}");
    assert!(
        c.contains("### Unit src/repo.php::AssetRepository::byId"),
        "{c}"
    );
}

#[tokio::test]
async fn the_second_model_may_dispute_the_change_or_fail() {
    let dir = tempfile::tempdir().unwrap();
    let r = fixture(dir.path(), unit_to_fix());
    let fix = answer(vec![hunk("src/repo.php", QUERY_OLD, QUERY_NEW)]);
    // Disputed, with evidence that doesn't exist (dropped, the word kept).
    let m = agent(
        fix.clone(),
        json!({ "agree": false, "why": "byName still concatenates the id.",
                "evidence": { "file": "src/repo.php", "start": 90, "end": 91 } }),
    );
    let p = proposal(&run(&m, job(&r, true)).await);
    let s = p.second.expect("checked");
    assert!(!s.agree);
    assert_eq!(s.why, "byName still concatenates the id.");
    assert_eq!(s.evidence, None);

    // A check that fails, or says neither yes nor no: unchecked.
    let m = MockAdapter::new({
        let fix = fix.clone();
        move |p, _| {
            if p.starts_with("DIVE:Fix") {
                Ok(fix.clone())
            } else {
                anyhow::bail!("the second model is down")
            }
        }
    });
    let p = proposal(&run(&m, job(&r, true)).await);
    assert!(p.second.is_none());
    assert_eq!(p.hunks.len(), 1);
    let m = agent(fix, json!({ "why": "Hard to say." }));
    let p = proposal(&run(&m, job(&r, true)).await);
    assert!(p.second.is_none());
}

// ---- Applying ----------------------------------------------------------------------

fn proposed(file: &str, old: &str, new: &str) -> ProposedHunk {
    ProposedHunk {
        file: file.into(),
        lines: Range::new(1, 1),
        old: old.into(),
        new: new.into(),
        why: String::new(),
        parses: true,
        state: HunkState::Proposed,
    }
}

fn of(hunks: Vec<ProposedHunk>) -> Proposal {
    Proposal {
        reason: Reason::Note,
        summary: "s".into(),
        hunks,
        second: None,
        dropped: 0,
    }
}

/// Temporary files left beside the files written.
fn leftovers(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in ["src", "web"] {
        for e in std::fs::read_dir(root.join(dir)).unwrap() {
            let p = e.unwrap().path();
            if p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".teer-")
            {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn applying_writes_the_chosen_hunks_and_leaves_the_rest_alone() {
    use std::os::unix::fs::PermissionsExt;
    isolate_env();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fixture(root, unit());
    let routes = root.join("src/routes.php");
    std::fs::set_permissions(&routes, std::fs::Permissions::from_mode(0o750)).unwrap();
    let mut p = of(vec![
        proposed("src/repo.php", QUERY_OLD, QUERY_NEW),
        proposed(
            "src/repo.php",
            "class AssetRepository {",
            "final class AssetRepository {",
        ),
        proposed("src/routes.php", "$id = $request", "$id = (int) $request"),
        proposed("web/app.ts", "a + b", "a - b"),
        proposed("src/routes.php", "class AssetController", "class Applied"),
    ]);
    p.hunks[4].state = HunkState::Applied;

    let written = apply(root, &p, &[0, 1, 2, 4]).unwrap();
    assert_eq!(
        written,
        [
            PathBuf::from("src/repo.php"),
            PathBuf::from("src/routes.php")
        ]
    );
    // Two hunks of one file in one pass.
    let repo = std::fs::read_to_string(root.join("src/repo.php")).unwrap();
    assert_eq!(
        repo,
        REPO.replace("class AssetRepository {", "final class AssetRepository {")
            .replace(QUERY_OLD, QUERY_NEW)
    );
    let now = std::fs::read_to_string(&routes).unwrap();
    assert_eq!(
        now,
        ROUTES.replace("$id = $request", "$id = (int) $request")
    );
    // An applied hunk is skipped; a hunk not chosen and its file untouched.
    assert!(now.contains("class AssetController"));
    assert_eq!(
        std::fs::read_to_string(root.join("web/app.ts")).unwrap(),
        APP_TS
    );
    // The permissions stay, and no temporary file is left.
    let mode = std::fs::metadata(&routes).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o750);
    assert!(leftovers(root).is_empty(), "{:?}", leftovers(root));

    // Applied already: nothing left.
    let mut done = p.clone();
    done.hunks[3].state = HunkState::Applied;
    assert!(apply(root, &done, &[3]).is_err());
    assert!(apply(root, &p, &[9]).is_err());
}

#[test]
fn a_file_changed_since_the_proposal_stops_every_file_from_being_written() {
    isolate_env();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fixture(root, unit());
    let p = of(vec![
        proposed("src/repo.php", QUERY_OLD, QUERY_NEW),
        proposed("src/routes.php", "$id = $request", "$id = (int) $request"),
    ]);
    // Its old text is gone from the second file.
    let changed = ROUTES.replace("$id = $request", "$key = $request");
    std::fs::write(root.join("src/routes.php"), &changed).unwrap();
    let e = apply(root, &p, &[0, 1]).unwrap_err();
    assert_eq!(e, "src/routes.php changed since the change was proposed");
    assert_eq!(
        std::fs::read_to_string(root.join("src/repo.php")).unwrap(),
        REPO
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/routes.php")).unwrap(),
        changed
    );
    // Or there twice now.
    let twice = format!("{ROUTES}// $id = $request\n");
    std::fs::write(root.join("src/routes.php"), &twice).unwrap();
    let e = apply(root, &p, &[0, 1]).unwrap_err();
    assert!(e.contains("changed since"), "{e}");
    assert_eq!(
        std::fs::read_to_string(root.join("src/repo.php")).unwrap(),
        REPO
    );
    assert!(leftovers(root).is_empty());
}

#[test]
fn a_symlink_out_of_the_repository_is_never_written() {
    isolate_env();
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = dir.path();
    fixture(root, unit());
    let secret = outside.path().join("secret.php");
    std::fs::write(&secret, ROUTES).unwrap();
    std::os::unix::fs::symlink(&secret, root.join("src/link.php")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("lib")).unwrap();
    for file in [
        "src/link.php",
        "lib/secret.php",
        "../secret.php",
        ".git/config",
    ] {
        let p = of(vec![
            proposed("src/repo.php", QUERY_OLD, QUERY_NEW),
            proposed(file, "$id = $request", "$id = (int) $request"),
        ]);
        let e = apply(root, &p, &[0, 1]).unwrap_err();
        assert!(
            e.contains("symlink") || e.contains("outside the repository"),
            "{file}: {e}"
        );
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), ROUTES, "{file}");
        assert_eq!(
            std::fs::read_to_string(root.join("src/repo.php")).unwrap(),
            REPO,
            "{file}: nothing is written"
        );
    }
}

// ---- The runtime ---------------------------------------------------------------------

/// Effects run by a runtime on `m`; the app events they send, until `done`
/// says one is the last.
async fn exec(
    app: &mut App,
    m: &MockAdapter,
    effect: Effect,
    done: impl Fn(&AppEvent) -> bool,
) -> Vec<AppEvent> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let rt = Runtime {
        adapter: Some(Arc::new(m.clone())),
        tx,
        build: BuildOptions::default(),
        tools: Default::default(),
    };
    rt.exec(app, effect);
    let mut out = Vec::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("the runtime answers")
            .expect("the channel stays open");
        let last = done(&ev);
        out.push(ev);
        if last {
            return out;
        }
    }
}

#[tokio::test]
async fn the_runtime_asks_for_a_change_and_writes_it_to_the_working_tree() {
    isolate_env();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut r = fixture(&root, unit_to_fix());
    r.agent_session = Some(SessionRef::fresh("mock", root.clone()));
    let mut app = App::new(r, Config::default(), Keymap::default());
    app.has_agent = true;
    let m = agent(
        answer(vec![
            hunk("src/repo.php", QUERY_OLD, QUERY_NEW),
            hunk("src/routes.php", "$id = $request", "$id = (int) $request"),
        ]),
        agrees(),
    );

    let events = exec(
        &mut app,
        &m,
        Effect::Dive {
            unit_id: BY_ID.into(),
            kind: DiveKind::Fix,
            web: false,
        },
        |ev| {
            matches!(
                ev,
                AppEvent::Dive(DiveEvent::Proposed { .. } | DiveEvent::Failed { .. })
            )
        },
    )
    .await;
    // The second model is on by default.
    assert_eq!(*m.second_calls.lock().unwrap(), 1);
    for ev in events {
        app.on_app_event(ev);
    }
    let u = app.review.unit(BY_ID).unwrap();
    assert!(!u.dive_pending.contains(&DiveKind::Fix));
    let result = &u.dive.results[&DiveKind::Fix];
    assert_eq!(result.text, "Binds the id instead of concatenating it.");
    let p = result.proposal.as_ref().unwrap();
    assert_eq!(p.hunks.len(), 2);
    assert!(p.second.as_ref().unwrap().agree);
    assert!(app.progress.is_empty(), "{:?}", app.progress);

    // `P`: written to the working tree.
    let events = exec(
        &mut app,
        &m,
        Effect::ApplyHunks {
            unit_id: BY_ID.into(),
            hunks: vec![0, 1],
        },
        |ev| matches!(ev, AppEvent::FixApplied { .. }),
    )
    .await;
    let [
        AppEvent::FixApplied {
            unit_id,
            hunks,
            result,
        },
    ] = &events[..]
    else {
        panic!("{events:?}")
    };
    assert_eq!(unit_id, BY_ID);
    assert_eq!(hunks, &[0, 1]);
    assert_eq!(
        result.as_deref().unwrap(),
        [
            PathBuf::from("src/repo.php"),
            PathBuf::from("src/routes.php")
        ]
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/repo.php")).unwrap(),
        REPO.replace(QUERY_OLD, QUERY_NEW)
    );
    assert!(
        std::fs::read_to_string(root.join("src/routes.php"))
            .unwrap()
            .contains("$id = (int) $request")
    );
}

#[tokio::test]
async fn the_runtime_never_writes_a_pull_requests_code() {
    isolate_env();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut r = fixture(&root, unit_to_fix());
    r.pr = Some(PrInfo::default());
    r.unit_mut(BY_ID).unwrap().dive.results.insert(
        DiveKind::Fix,
        DiveResult {
            text: "s".into(),
            proposal: Some(of(vec![proposed("src/repo.php", QUERY_OLD, QUERY_NEW)])),
            ..Default::default()
        },
    );
    let mut app = App::new(r, Config::default(), Keymap::default());
    let m = agent(json!({}), json!({}));
    let events = exec(
        &mut app,
        &m,
        Effect::ApplyHunks {
            unit_id: BY_ID.into(),
            hunks: vec![0],
        },
        |ev| matches!(ev, AppEvent::FixApplied { .. }),
    )
    .await;
    assert!(matches!(
        &events[..],
        [AppEvent::FixApplied { result: Err(e), .. }] if e == "a pull request's code isn't changed here"
    ));
    assert_eq!(
        std::fs::read_to_string(root.join("src/repo.php")).unwrap(),
        REPO
    );
}
