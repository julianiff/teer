//! Agent questions asked about a flagged unit (the "dive" naming is kept here;
//! the screen that asks them is the Flagged screen).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use super::model::{
    Behavior, Coverage, DiveKind, DiveResult, Review, SessionRef, TestSearch, Unit, Visual,
};
use super::{prompts, test_search, validate, visual};
use crate::agent::AgentAdapter;
use crate::git::ChangedFile;

#[derive(Debug, Clone, PartialEq)]
pub enum DiveEvent {
    Ready {
        id: String,
        kind: DiveKind,
        text: String,
        sources: Vec<String>,
        /// The picture, for a Visualize answer (`text` is its claim).
        visual: Option<Box<Visual>>,
        /// The table, for a Behavior answer (`text` is its summary).
        behavior: Option<Box<Behavior>>,
        /// Covering tests and gaps, for a Tests answer (`text` is its note).
        coverage: Option<Box<Coverage>>,
    },
    Failed {
        id: String,
        kind: DiveKind,
        error: String,
    },
    /// What a long question is doing now (Behavior), for the tab. Not stored.
    Progress {
        id: String,
        kind: DiveKind,
        text: String,
    },
}

/// Whether a dive of this kind may use the web.
pub fn uses_web(kind: DiveKind) -> bool {
    kind == DiveKind::Recommend
}

fn parse_answer(v: &serde_json::Value) -> Result<(String, Vec<String>), String> {
    let text = v
        .get("text")
        .and_then(|t| t.as_str())
        .map(crate::util::sanitize_agent_text)
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err("the agent returned no answer".into());
    }
    let sources = v
        .get("sources")
        .and_then(|s| s.as_array())
        .map(|a| crate::util::clean_sources(a.iter().filter_map(|x| x.as_str())))
        .unwrap_or_default();
    Ok((text, sources))
}

/// Ask for a picture of `unit`, keep what cites code in `target.regions`, and
/// save the page. The agent only reads; teer writes the file.
async fn visualize(
    adapter: Arc<dyn AgentAdapter>,
    session: &SessionRef,
    unit: &Unit,
    file: &ChangedFile,
    target: visual::Target,
) -> Result<Visual, String> {
    let prompt = prompts::visual_prompt(unit, file, &target.regions);
    let raw = adapter
        .ask_dive(session, &prompt, &prompts::visual_schema(), false)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let (picture, dropped) =
        validate::validate_visual(&unit.id, &raw, &target.regions, &session.cwd);
    validate::log_dropped("from the picture of", &dropped);
    let mut picture = picture?;
    picture.path = target.path;
    let (page, title) = (picture.clone(), target.title);
    tokio::task::spawn_blocking(move || visual::write_page(&page, &title))
        .await
        .map_err(|e| format!("{e}"))?
        .map_err(|e| format!("couldn't save the picture: {e:#}"))?;
    Ok(picture)
}

/// Search the test files under `root` (the repository with the change) for
/// the tests that reach `unit`: no agent. See [`test_search::find`].
pub fn search_tests(root: &Path, unit: &Unit, callers: &[test_search::Caller]) -> DiveEvent {
    let search = test_search::find(root, unit, callers);
    DiveEvent::Ready {
        id: unit.id.clone(),
        kind: DiveKind::Tests,
        text: String::new(),
        sources: Vec::new(),
        visual: None,
        behavior: None,
        coverage: Some(Box::new(Coverage::found(search))),
    }
}

/// The agent's reading of what a Tests search found.
pub struct TestsJob {
    pub unit: Unit,
    pub file: ChangedFile,
    /// The code a gap may cite.
    pub target: visual::Target,
    pub search: TestSearch,
    /// The repository the search ran in: the found tests' code is read here.
    pub root: PathBuf,
}

/// Ask the agent to read the tests the search found: which of them cover the
/// unit, what each checks, and what dangerous cases none does. Keep covering
/// tests that were found or whose lines name the unit, and gaps that cite the
/// target's regions. The agent only reads.
pub async fn interpret_tests(
    adapter: Arc<dyn AgentAdapter>,
    session: SessionRef,
    job: TestsJob,
    tx: UnboundedSender<DiveEvent>,
) {
    let id = job.unit.id.clone();
    let _ = tx.send(DiveEvent::Progress {
        id: id.clone(),
        kind: DiveKind::Tests,
        text: match job.search.total {
            0 => "the agent looks for tests teer can't see…".into(),
            1 => "the agent reads the test teer found…".into(),
            n => format!("the agent reads the {n} tests teer found…"),
        },
    });
    let ev = match interpret(adapter, &session, job).await {
        Ok((text, c)) => DiveEvent::Ready {
            id,
            kind: DiveKind::Tests,
            text,
            sources: Vec::new(),
            visual: None,
            behavior: None,
            coverage: Some(Box::new(c)),
        },
        Err(error) => DiveEvent::Failed {
            id,
            kind: DiveKind::Tests,
            error,
        },
    };
    let _ = tx.send(ev);
}

async fn interpret(
    adapter: Arc<dyn AgentAdapter>,
    session: &SessionRef,
    job: TestsJob,
) -> Result<(String, Coverage), String> {
    let code = {
        let (root, tests) = (job.root.clone(), job.search.tests.clone());
        tokio::task::spawn_blocking(move || {
            tests
                .iter()
                .map(|t| test_search::code(&root, t))
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default()
    };
    let prompt = prompts::tests_prompt(
        &job.unit,
        &job.file,
        &job.target.regions,
        &job.search,
        &code,
    );
    let raw = adapter
        .ask_dive(session, &prompt, &prompts::tests_schema(), false)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let (mut coverage, dropped) = validate::validate_coverage(
        &job.unit,
        &raw,
        &job.target.regions,
        &session.cwd,
        &job.search.tests,
    );
    coverage.search = Some(job.search);
    validate::log_dropped("from the tests of", &dropped);
    let note = validate::one_line(raw["text"].as_str().unwrap_or(""), 300);
    Ok((note, coverage))
}

/// Ask the agent one question about `unit` and send the outcome.
/// Web access is only granted for kinds that need it, and only if `web`.
/// A Visualize question needs `visual` (what it may cite, where the page goes).
#[allow(clippy::too_many_arguments)]
pub async fn run_dive(
    adapter: Arc<dyn AgentAdapter>,
    session: SessionRef,
    unit: Unit,
    file: ChangedFile,
    kind: DiveKind,
    web: bool,
    visual: Option<visual::Target>,
    tx: UnboundedSender<DiveEvent>,
) {
    if kind == DiveKind::Visualize {
        let result = match visual {
            Some(target) => visualize(adapter, &session, &unit, &file, target).await,
            None => Err("nothing to draw: the unit's code is not known".into()),
        };
        let ev = match result {
            Ok(v) => DiveEvent::Ready {
                id: unit.id.clone(),
                kind,
                text: v.claim.clone(),
                sources: Vec::new(),
                visual: Some(Box::new(v)),
                behavior: None,
                coverage: None,
            },
            Err(error) => DiveEvent::Failed {
                id: unit.id.clone(),
                kind,
                error,
            },
        };
        let _ = tx.send(ev);
        return;
    }
    let prompt = prompts::dive_prompt(kind, &unit, &file);
    let web = web && uses_web(kind);
    let ev = match adapter
        .ask_dive(&session, &prompt, &prompts::dive_schema(), web)
        .await
    {
        Ok(v) => match parse_answer(&v) {
            Ok((text, sources)) => DiveEvent::Ready {
                id: unit.id.clone(),
                kind,
                text,
                sources,
                visual: None,
                behavior: None,
                coverage: None,
            },
            Err(error) => DiveEvent::Failed {
                id: unit.id.clone(),
                kind,
                error,
            },
        },
        Err(e) => DiveEvent::Failed {
            id: unit.id.clone(),
            kind,
            error: format!("{e:#}"),
        },
    };
    let _ = tx.send(ev);
}

impl Review {
    /// Store a dive answer, or clear the pending marker when it failed.
    /// Returns whether anything changed (false for an unknown unit).
    pub fn apply_dive(&mut self, ev: &DiveEvent) -> bool {
        match ev {
            DiveEvent::Ready {
                id,
                kind,
                text,
                sources,
                visual,
                behavior,
                coverage,
            } => {
                let Some(u) = self.unit_mut(id) else {
                    return false;
                };
                u.dive_pending.remove(kind);
                u.dive.results.insert(
                    *kind,
                    DiveResult {
                        text: text.clone(),
                        sources: sources.clone(),
                        visual: visual.as_deref().cloned(),
                        behavior: behavior.as_deref().cloned(),
                        coverage: coverage.as_deref().cloned(),
                    },
                );
                true
            }
            DiveEvent::Failed { id, kind, error } => {
                crate::log::write(&format!("dive {kind:?} for {id} failed: {error}"));
                let Some(u) = self.unit_mut(id) else {
                    return false;
                };
                u.dive_pending.remove(kind)
            }
            DiveEvent::Progress { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{Decision, Unit};

    pub(super) fn review() -> Review {
        let u: Unit = serde_json::from_value(serde_json::json!({
            "id": "a::f", "file": "a.rs", "symbol": "f", "kind": "Function",
            "status": "Changed", "old_range": null, "new_range": null,
            "fingerprint": "x", "changes": [], "callers": [], "callees": [],
            "decision": "Investigate"
        }))
        .unwrap();
        Review {
            id: "r".into(),
            repo_root: Default::default(),
            base: String::new(),
            head: String::new(),
            head_commit: String::new(),
            agent_session: None,
            pr: None,
            intent: Default::default(),
            created_at: chrono::Utc::now(),
            files: Vec::new(),
            units: vec![u],
            skipped: Vec::new(),
            graph: Default::default(),
            questions: Vec::new(),
        }
    }

    #[test]
    fn ready_stores_result_and_clears_pending() {
        let mut r = review();
        r.units[0].dive_pending.insert(DiveKind::Recommend);
        let ev = DiveEvent::Ready {
            id: "a::f".into(),
            kind: DiveKind::Recommend,
            text: "why".into(),
            sources: vec!["a.rs:1".into()],
            visual: None,
            behavior: None,
            coverage: None,
        };
        assert!(r.apply_dive(&ev));
        let u = &r.units[0];
        assert!(u.dive_pending.is_empty());
        assert_eq!(u.dive.results[&DiveKind::Recommend].text, "why");
        assert_eq!(u.dive.results[&DiveKind::Recommend].sources, ["a.rs:1"]);
        assert_eq!(u.decision, Decision::Investigate);
    }

    #[test]
    fn failed_clears_pending_only() {
        let mut r = review();
        r.units[0].dive_pending.insert(DiveKind::Recommend);
        let ev = DiveEvent::Failed {
            id: "a::f".into(),
            kind: DiveKind::Recommend,
            error: "boom".into(),
        };
        assert!(r.apply_dive(&ev));
        assert!(r.units[0].dive_pending.is_empty());
        assert!(r.units[0].dive.results.is_empty());
        // Nothing was pending any more: no change.
        assert!(!r.apply_dive(&ev));
    }

    #[test]
    fn unknown_unit_changes_nothing() {
        let mut r = review();
        let ev = DiveEvent::Ready {
            id: "nope".into(),
            kind: DiveKind::Recommend,
            text: String::new(),
            sources: vec![],
            visual: None,
            behavior: None,
            coverage: None,
        };
        assert!(!r.apply_dive(&ev));
    }
}

#[cfg(test)]
fn tests_review() -> Review {
    tests::review()
}

#[cfg(test)]
mod run_tests {
    use super::*;
    use crate::agent::mock::{MockAdapter, dive_responder};

    fn unit() -> Unit {
        serde_json::from_value(serde_json::json!({
            "id": "a::f", "file": "a.rs", "symbol": "f", "kind": "Function",
            "status": "Changed", "old_range": null, "new_range": null,
            "fingerprint": "x", "changes": [], "callers": [], "callees": [],
            "decision": "Investigate"
        }))
        .unwrap()
    }

    fn file() -> ChangedFile {
        ChangedFile {
            path: "a.rs".into(),
            old_path: None,
            status: crate::git::FileStatus::Modified,
            old_content: None,
            new_content: None,
        }
    }

    async fn dive(m: &MockAdapter, kind: DiveKind, web: bool) -> DiveEvent {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session = SessionRef::fresh("mock", ".".into());
        run_dive(
            Arc::new(m.clone()),
            session,
            unit(),
            file(),
            kind,
            web,
            None,
            tx,
        )
        .await;
        rx.recv().await.unwrap()
    }

    #[tokio::test]
    async fn recommend_is_ready_and_may_use_the_web() {
        let m = MockAdapter::new(dive_responder);
        match dive(&m, DiveKind::Recommend, true).await {
            DiveEvent::Ready {
                id,
                kind,
                text,
                sources,
                visual: None,
                behavior: None,
                coverage: None,
            } => {
                assert_eq!((id.as_str(), kind), ("a::f", DiveKind::Recommend));
                assert!(text.starts_with("mock"));
                assert_eq!(sources.len(), 2);
            }
            other => panic!("{other:?}"),
        }
        // Web off stays off.
        dive(&m, DiveKind::Recommend, false).await;
        assert_eq!(*m.web_calls.lock().unwrap(), [true, false]);
    }

    #[tokio::test]
    async fn failure_and_malformed_output_fail() {
        let m = MockAdapter::new(|_, _| anyhow::bail!("boom"));
        assert!(matches!(
            dive(&m, DiveKind::Recommend, false).await,
            DiveEvent::Failed { error, .. } if error.contains("boom")
        ));
        let m = MockAdapter::new(|_, _| Ok(serde_json::json!({"text": 5})));
        assert!(matches!(
            dive(&m, DiveKind::Recommend, false).await,
            DiveEvent::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn output_is_sanitized_and_applied() {
        let m = MockAdapter::new(|_, _| {
            Ok(serde_json::json!({
                "text": "\u{1b}[31mbad\u{1b}[0m",
                "sources": ["https://a.dev", "bad source with://", 7]
            }))
        });
        let ev = dive(&m, DiveKind::Recommend, true).await;
        let mut r = crate::review::dive::tests_review();
        r.units[0].dive_pending.insert(DiveKind::Recommend);
        assert!(r.apply_dive(&ev));
        let res = &r.units[0].dive.results[&DiveKind::Recommend];
        assert_eq!(res.text, "bad");
        assert_eq!(res.sources, ["https://a.dev"]);
        assert!(r.units[0].dive_pending.is_empty());
    }

    fn target(dir: &std::path::Path) -> visual::Target {
        visual::Target {
            regions: vec![validate::Region {
                file: "a.rs".into(),
                range: crate::review::model::Range::new(1, 9),
                role: "this unit, new lines".into(),
            }],
            path: dir.join("visuals/r/a.html"),
            title: "f · a.rs:1".into(),
        }
    }

    fn picture(evidence_line: u32) -> serde_json::Value {
        let ev =
            serde_json::json!([{"file": "a.rs", "start": evidence_line, "end": evidence_line}]);
        serde_json::json!({
            "claim": "f now writes the <cache>.",
            "nodes": [
                {"id": "f", "label": "f", "status": "changed", "evidence": ev},
                {"id": "cache", "label": "cache", "status": "added", "evidence": ev},
            ],
            "edges": [{"from": "f", "to": "cache", "label": "writes", "status": "added", "evidence": ev}]
        })
    }

    async fn ask_visual(m: &MockAdapter, target: Option<visual::Target>) -> DiveEvent {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session = SessionRef::fresh("mock", ".".into());
        run_dive(
            Arc::new(m.clone()),
            session,
            unit(),
            file(),
            DiveKind::Visualize,
            true,
            target,
            tx,
        )
        .await;
        rx.recv().await.unwrap()
    }

    #[tokio::test]
    async fn visualize_saves_the_page_without_web_and_applies() {
        let dir = tempfile::tempdir().unwrap();
        let m = MockAdapter::new(|p, schema| {
            assert!(p.starts_with("DIVE:Visualize"));
            assert!(schema["properties"]["nodes"].is_object());
            Ok(picture(3))
        });
        let ev = ask_visual(&m, Some(target(dir.path()))).await;
        let DiveEvent::Ready {
            text,
            visual: Some(v),
            ..
        } = &ev
        else {
            panic!("{ev:?}")
        };
        assert_eq!(text, "f now writes the <cache>.");
        assert_eq!(v.path, dir.path().join("visuals/r/a.html"));
        let html = std::fs::read_to_string(&v.path).unwrap();
        assert!(html.contains("f now writes the &lt;cache&gt;."), "{html}");
        assert!(html.contains("<h1>f · a.rs:1</h1>"));
        assert_eq!(*m.web_calls.lock().unwrap(), [false], "never the web");

        let mut r = tests_review();
        r.units[0].dive_pending.insert(DiveKind::Visualize);
        assert!(r.apply_dive(&ev));
        let res = &r.units[0].dive.results[&DiveKind::Visualize];
        assert_eq!(res.visual.as_ref().unwrap().edges[0].label, "writes");
        assert!(r.units[0].dive_pending.is_empty());
    }

    #[tokio::test]
    async fn visualize_fails_when_nothing_is_backed_or_nothing_is_known() {
        let dir = tempfile::tempdir().unwrap();
        let m = MockAdapter::new(|_, _| Ok(picture(42)));
        let ev = ask_visual(&m, Some(target(dir.path()))).await;
        assert!(
            matches!(&ev, DiveEvent::Failed { kind: DiveKind::Visualize, error, .. } if error.contains("no connection")),
            "{ev:?}"
        );
        assert!(
            !dir.path().join("visuals").exists(),
            "no page for a failed ask"
        );
        let ev = ask_visual(&m, None).await;
        assert!(matches!(ev, DiveEvent::Failed { .. }), "{ev:?}");
        let m = MockAdapter::new(|_, _| anyhow::bail!("boom"));
        let ev = ask_visual(&m, Some(target(dir.path()))).await;
        assert!(matches!(&ev, DiveEvent::Failed { error, .. } if error.contains("boom")));
    }
}
