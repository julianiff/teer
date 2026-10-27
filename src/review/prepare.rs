//! Preparing a review: ask the agent about each changed unit, a few
//! requests at a time, validating every claim as it arrives. With a second
//! opinion on, each batch that got statements then goes to the second model,
//! which checks them (see `SecondOpinion`).

use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;

use super::intent::Brief;
use super::model::{Change, Cost, SecondOpinion, SessionRef, Unit, Unmentioned};
use super::prompts;
use super::validate::{
    Dropped, log_dropped, validate_changes, validate_cost, validate_second, validate_summary,
    validate_unmentioned,
};
use crate::agent::AgentAdapter;
use crate::git::ChangedFile;

// Events are short-lived and moved once; boxing would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum PrepareEvent {
    Ready {
        id: String,
        changes: Vec<Change>,
        summary: Option<String>,
        /// Undoing it would be costly, with the lines that show it.
        cost: Option<Cost>,
        /// A change the commit messages and the PR description don't mention.
        unmentioned: Option<Unmentioned>,
        dropped: Vec<Dropped>,
        /// A second opinion on these statements follows.
        checking: bool,
    },
    Failed {
        id: String,
        error: String,
    },
    /// The second model's check of a unit described earlier, or why there is none.
    Second {
        id: String,
        opinion: Result<SecondOpinion, String>,
    },
}

/// Units up to this many lines share a request with others.
const SMALL_UNIT: u32 = 40;
const BATCH_LINES: u32 = 160;
const BATCH_UNITS: usize = 6;

fn unit_lines(u: &Unit) -> u32 {
    u.old_range
        .map(|r| r.len())
        .unwrap_or(0)
        .max(u.new_range.map(|r| r.len()).unwrap_or(0))
}

/// Group units into requests: large units alone, small ones together
/// (same file first, in source order).
pub fn batches(units: &[(Unit, ChangedFile)]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..units.len()).collect();
    order.sort_by_key(|&i| (units[i].0.file.clone(), units[i].0.display_line()));
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut cur_lines = 0;
    for i in order {
        let lines = unit_lines(&units[i].0);
        if lines > SMALL_UNIT {
            out.push(vec![i]);
            continue;
        }
        if !cur.is_empty() && (cur_lines + lines > BATCH_LINES || cur.len() >= BATCH_UNITS) {
            out.push(std::mem::take(&mut cur));
            cur_lines = 0;
        }
        cur.push(i);
        cur_lines += lines;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

async fn run_batch(
    adapter: Arc<dyn AgentAdapter>,
    session: SessionRef,
    units: Vec<(Unit, ChangedFile)>,
    second: bool,
    brief: Arc<Brief>,
    tx: UnboundedSender<PrepareEvent>,
) {
    // Nobody listens any more (teer quit): ask nothing new.
    if tx.is_closed() {
        return;
    }
    let refs: Vec<(&Unit, &ChangedFile)> = units.iter().map(|(u, f)| (u, f)).collect();
    let prompt = prompts::units_prompt(&refs, &brief);
    let intent = prompts::has_intent(&refs, &brief);
    let answer = adapter
        .ask_prepare(&session, &prompt, &prompts::units_schema(intent))
        .await;
    let answer = match answer {
        Ok(a) => a,
        Err(e) => {
            for (u, _) in &units {
                let _ = tx.send(PrepareEvent::Failed {
                    id: u.id.clone(),
                    error: format!("{e:#}"),
                });
            }
            return;
        }
    };
    let items = answer["units"].as_array().cloned().unwrap_or_default();
    let mut described: Vec<(Unit, ChangedFile)> = Vec::new();
    for (u, f) in &units {
        let Some(item) = items
            .iter()
            .find(|i| i["id"].as_str() == Some(u.id.as_str()))
        else {
            let _ = tx.send(PrepareEvent::Failed {
                id: u.id.clone(),
                error: "agent returned nothing for this unit".into(),
            });
            continue;
        };
        let (changes, mut dropped) = validate_changes(u, &item["changes"]);
        let (cost, cost_dropped) = validate_cost(u, &item["undo"]);
        dropped.extend(cost_dropped);
        // Only where the author said something about the unit.
        let (unmentioned, gap_dropped) = if intent && brief.covers(&u.id) {
            validate_unmentioned(u, &item["unmentioned"])
        } else {
            (None, None)
        };
        dropped.extend(gap_dropped);
        log_dropped("claim for", &dropped);
        let summary = validate_summary(&item["summary"]);
        let checking = second && !changes.is_empty();
        if checking {
            let mut u = u.clone();
            u.changes = changes.clone();
            u.summary = summary.clone();
            described.push((u, f.clone()));
        }
        let _ = tx.send(PrepareEvent::Ready {
            id: u.id.clone(),
            changes,
            summary,
            cost,
            unmentioned,
            dropped,
            checking,
        });
    }
    if !described.is_empty() {
        second_opinions(adapter, session, described, tx).await;
    }
}

/// Ask the second model about `units` (described already: their `changes`
/// are the statements it checks) in one request, one event per unit.
pub async fn second_opinions(
    adapter: Arc<dyn AgentAdapter>,
    session: SessionRef,
    units: Vec<(Unit, ChangedFile)>,
    tx: UnboundedSender<PrepareEvent>,
) {
    let refs: Vec<(&Unit, &ChangedFile)> = units.iter().map(|(u, f)| (u, f)).collect();
    let prompt = prompts::second_prompt(&refs);
    let answer = adapter
        .ask_second(&session, &prompt, &prompts::second_schema())
        .await;
    let items = match answer {
        Ok(a) => a["units"].as_array().cloned().unwrap_or_default(),
        Err(e) => {
            for (u, _) in &units {
                let _ = tx.send(PrepareEvent::Second {
                    id: u.id.clone(),
                    opinion: Err(format!("{e:#}")),
                });
            }
            return;
        }
    };
    for (u, _) in &units {
        let opinion = match items
            .iter()
            .find(|i| i["id"].as_str() == Some(u.id.as_str()))
        {
            Some(item) => {
                let (opinion, dropped) = validate_second(u, item);
                log_dropped("claim for", &dropped);
                Ok(opinion)
            }
            None => Err("the second model returned nothing for this unit".into()),
        };
        let _ = tx.send(PrepareEvent::Second {
            id: u.id.clone(),
            opinion,
        });
    }
}

/// Prepare `units`, sending one event per unit as soon as its batch is
/// done, and with `second`, one more per described unit once the second
/// model has checked it. `brief` is what the author said the change is for
/// (`Review::brief`): the agent may then name a change they don't mention.
pub async fn prepare(
    adapter: Arc<dyn AgentAdapter>,
    session: SessionRef,
    units: Vec<(Unit, ChangedFile)>,
    concurrency: usize,
    second: bool,
    brief: Arc<Brief>,
    tx: UnboundedSender<PrepareEvent>,
) {
    let groups = batches(&units);
    let jobs = groups.into_iter().map(|g| {
        let batch: Vec<(Unit, ChangedFile)> = g.iter().map(|&i| units[i].clone()).collect();
        run_batch(
            adapter.clone(),
            session.clone(),
            batch,
            second,
            brief.clone(),
            tx.clone(),
        )
    });
    futures::stream::iter(jobs)
        .buffer_unordered(concurrency.max(1))
        .for_each(|()| async {})
        .await;
}
