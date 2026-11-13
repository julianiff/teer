//! Evidence validation: the most important correctness rule in teer.
//! A change statement is only shown if every line it cites lies inside the
//! unit's old or new range. Anything else is dropped and logged. A Visualize
//! picture follows the same rule, with the unit's callers and callees added
//! to the code it may cite.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use super::model::{
    Answer, Change, Cite, Cost, Coverage, CoveringTest, FoundTest, Gap, LineRef, Marker, Range,
    Related, Risk, SecondOpinion, Shown, Side, Unit, Unmentioned, Verdict,
};
use crate::git::ChangedFile;
use crate::security::{Class, Suspect};

const MAX_CHANGES: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct Dropped {
    pub unit_id: String,
    pub statement: String,
    pub reason: String,
}

/// Log what validation dropped, as `dropped <from> <unit>: "…" (why)`, e.g.
/// with `from` = "from the picture of".
pub fn log_dropped(from: &str, dropped: &[Dropped]) {
    for d in dropped {
        crate::log::write(&format!(
            "dropped {from} {}: \"{}\" ({})",
            d.unit_id, d.statement, d.reason
        ));
    }
}

#[derive(Debug, Deserialize)]
struct RawLine {
    side: String,
    line: i64,
}

#[derive(Debug, Deserialize)]
struct RawChange {
    #[serde(default)]
    marker: String,
    #[serde(default)]
    statement: String,
    #[serde(default)]
    risk: String,
    #[serde(default)]
    evidence: Vec<RawLine>,
}

/// Every cited line must lie inside the unit's range on its side.
fn check_evidence(unit: &Unit, lines: &[RawLine]) -> Result<Vec<LineRef>, String> {
    if lines.is_empty() {
        return Err("no evidence".into());
    }
    let mut evidence = Vec::new();
    for e in lines {
        let side = match e.side.trim().to_ascii_lowercase().as_str() {
            "old" => Side::Old,
            "new" => Side::New,
            other => return Err(format!("unknown evidence side '{other}'")),
        };
        let range = if side == Side::Old {
            unit.old_range
        } else {
            unit.new_range
        };
        if !(e.line > 0 && range.is_some_and(|r| r.contains(e.line as u32))) {
            return Err(format!(
                "evidence {} line {} outside unit range {}",
                e.side,
                e.line,
                range
                    .map(|r| format!("{}-{}", r.start, r.end))
                    .unwrap_or_else(|| "none".into())
            ));
        }
        evidence.push(LineRef {
            side,
            line: e.line as u32,
        });
    }
    evidence.sort_by_key(|e| (e.side == Side::New, e.line));
    evidence.dedup();
    Ok(evidence)
}

/// The unit summary: one sentence, trimmed. It makes no claim about
/// specific lines, so it needs no evidence; it is only shown for routine units.
pub fn validate_summary(raw: &Value) -> Option<String> {
    let s = raw
        .as_str()?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!s.is_empty()).then(|| crate::util::truncate(&s, 100))
}

fn parse_marker(s: &str) -> Option<Marker> {
    match s.trim().to_ascii_lowercase().as_str() {
        "added" | "+" => Some(Marker::Added),
        "changed" | "~" => Some(Marker::Changed),
        "removed" | "-" | "−" => Some(Marker::Removed),
        _ => None,
    }
}

fn parse_risk(s: &str) -> Risk {
    match s.trim().to_ascii_lowercase().as_str() {
        "high" => Risk::High,
        "medium" => Risk::Medium,
        _ => Risk::Low,
    }
}

/// Validate the raw `changes` array an agent returned for `unit`.
pub fn validate_changes(unit: &Unit, raw: &Value) -> (Vec<Change>, Vec<Dropped>) {
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    let items = raw.as_array().cloned().unwrap_or_default();
    for item in items {
        let statement_hint = item
            .get("statement")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        let drop = |reason: String, dropped: &mut Vec<Dropped>| {
            dropped.push(Dropped {
                unit_id: unit.id.clone(),
                statement: statement_hint.clone(),
                reason,
            });
        };
        let rc: RawChange = match serde_json::from_value(item.clone()) {
            Ok(rc) => rc,
            Err(e) => {
                drop(format!("malformed change: {e}"), &mut dropped);
                continue;
            }
        };
        let statement = rc
            .statement
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if statement.is_empty() {
            drop("empty statement".into(), &mut dropped);
            continue;
        }
        let Some(marker) = parse_marker(&rc.marker) else {
            drop(format!("unknown marker '{}'", rc.marker), &mut dropped);
            continue;
        };
        let evidence = match check_evidence(unit, &rc.evidence) {
            Ok(ev) => ev,
            Err(reason) => {
                drop(reason, &mut dropped);
                continue;
            }
        };
        if kept.len() >= MAX_CHANGES {
            drop("more than 3 changes".into(), &mut dropped);
            continue;
        }
        kept.push(Change {
            marker,
            statement,
            evidence,
            risk: parse_risk(&rc.risk),
        });
    }
    (kept, dropped)
}

#[derive(Debug, Deserialize)]
struct RawUndo {
    #[serde(default)]
    expensive: bool,
    #[serde(default)]
    why: String,
    #[serde(default)]
    evidence: Vec<RawLine>,
}

/// The agent's `undo` answer for `unit`: a [`Cost`] when it says undoing is
/// expensive and cites lines of the unit that show it. Without valid lines
/// the claim is dropped like any other.
pub fn validate_cost(unit: &Unit, raw: &Value) -> (Option<Cost>, Option<Dropped>) {
    let Ok(u) = serde_json::from_value::<RawUndo>(raw.clone()) else {
        return (None, None);
    };
    if !u.expensive {
        return (None, None);
    }
    let why = one_line(&u.why, 100);
    let drop = |reason: String| Dropped {
        unit_id: unit.id.clone(),
        statement: format!("expensive to undo: {why}"),
        reason,
    };
    if why.is_empty() {
        return (None, Some(drop("no reason given".into())));
    }
    match check_evidence(unit, &u.evidence) {
        Ok(evidence) => (Some(Cost { why, evidence }), None),
        Err(reason) => (None, Some(drop(reason))),
    }
}

#[derive(Debug, Deserialize)]
struct RawSuspect {
    #[serde(default)]
    class: String,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    evidence: Vec<RawLine>,
    #[serde(default)]
    checks: Vec<String>,
}

/// The questions an answer offers to ask next: one line each, the blank
/// and the repeated left out, at most `max`.
pub(crate) fn questions(raw: &[String], max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for q in raw {
        let q = one_line(q, super::prompts::MAX_QUESTION);
        if !q.is_empty() && !out.contains(&q) && out.len() < max {
            out.push(q);
        }
    }
    out
}

/// The agent's `risks` answer for `unit`: the possible security risks it
/// names, each kept only with a class teer knows and lines of the unit
/// that show it, like a statement. At most [`super::prompts::MAX_SUSPECTS`].
pub fn validate_suspects(unit: &Unit, raw: &Value) -> (Vec<Suspect>, Vec<Dropped>) {
    let raws: Vec<RawSuspect> = serde_json::from_value(raw.clone()).unwrap_or_default();
    let (mut kept, mut dropped) = (Vec::new(), Vec::new());
    for r in raws.into_iter().take(super::prompts::MAX_SUSPECTS) {
        let text = one_line(&r.text, 140);
        if text.is_empty() {
            continue;
        }
        let drop = |reason: String| Dropped {
            unit_id: unit.id.clone(),
            statement: format!("possible risk: {text}"),
            reason,
        };
        let Some(class) = Class::from_id(r.class.trim()) else {
            dropped.push(drop(format!("unknown class '{}'", r.class)));
            continue;
        };
        match check_evidence(unit, &r.evidence) {
            Ok(evidence) => kept.push(Suspect {
                class,
                severity: parse_risk(&r.severity),
                text,
                evidence,
                checks: questions(&r.checks, super::prompts::MAX_CHECKS),
            }),
            Err(reason) => dropped.push(drop(reason)),
        }
    }
    (kept, dropped)
}

#[derive(Debug, Deserialize)]
struct RawUnmentioned {
    #[serde(default)]
    text: String,
    #[serde(default)]
    evidence: Vec<RawLine>,
}

/// The agent's `unmentioned` answer for `unit`: a change the commit messages
/// and the PR description leave out, kept only with lines of the unit that
/// show it, like a statement. Absent or empty: nothing to say.
pub fn validate_unmentioned(unit: &Unit, raw: &Value) -> (Option<Unmentioned>, Option<Dropped>) {
    let Ok(u) = serde_json::from_value::<RawUnmentioned>(raw.clone()) else {
        return (None, None);
    };
    let text = one_line(&u.text, 100);
    if text.is_empty() {
        return (None, None);
    }
    match check_evidence(unit, &u.evidence) {
        Ok(evidence) => (Some(Unmentioned { text, evidence }), None),
        Err(reason) => (
            None,
            Some(Dropped {
                unit_id: unit.id.clone(),
                statement: format!("not in the commit messages: {text}"),
                reason,
            }),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct RawVerdict {
    #[serde(default)]
    n: i64,
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    why: String,
    #[serde(default)]
    evidence: Vec<RawLine>,
}

/// A second opinion on `unit`, whose statements are `unit.changes`. A
/// verdict needs a statement number that exists, `agree` or `dispute`, a
/// reason and valid evidence; missed changes follow the statement rules.
/// Anything else is dropped (and is then no dispute).
pub fn validate_second(unit: &Unit, raw: &Value) -> (SecondOpinion, Vec<Dropped>) {
    let mut dropped = Vec::new();
    let mut verdicts: Vec<Verdict> = Vec::new();
    let items = raw["verdicts"].as_array().cloned().unwrap_or_default();
    for item in items {
        let Ok(v) = serde_json::from_value::<RawVerdict>(item) else {
            continue;
        };
        let why = one_line(&v.why, 120);
        let Some(change) = usize::try_from(v.n)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| unit.changes.get(i))
        else {
            dropped.push(Dropped {
                unit_id: unit.id.clone(),
                statement: format!("second opinion: {why}"),
                reason: format!("no statement {}", v.n),
            });
            continue;
        };
        let agree = match v.verdict.trim() {
            "agree" => true,
            "dispute" => false,
            _ => continue,
        };
        if verdicts.iter().any(|x| x.statement == change.statement) {
            continue;
        }
        match check_evidence(unit, &v.evidence) {
            Ok(evidence) if !why.is_empty() => verdicts.push(Verdict {
                statement: change.statement.clone(),
                agree,
                why,
                evidence,
            }),
            Ok(_) => {}
            Err(reason) => dropped.push(Dropped {
                unit_id: unit.id.clone(),
                statement: format!("second opinion on \"{}\": {why}", change.statement),
                reason,
            }),
        }
    }
    let (mut missed, more) = validate_changes(unit, &raw["missed"]);
    missed.truncate(2);
    dropped.extend(more);
    let opinion = SecondOpinion {
        risk: parse_risk(raw["risk"].as_str().unwrap_or("")),
        verdicts,
        missed,
    };
    (opinion, dropped)
}

/// Most covering tests and gaps a Tests answer keeps.
pub const MAX_COVERING: usize = 8;
pub const MAX_GAPS: usize = 5;

#[derive(Debug, Deserialize)]
struct RawTest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    file: String,
    #[serde(default)]
    start: i64,
    #[serde(default)]
    end: i64,
    #[serde(default)]
    checks: String,
}

#[derive(Debug, Deserialize)]
struct RawGap {
    #[serde(default)]
    case: String,
    #[serde(default)]
    danger: String,
    #[serde(default)]
    risk: String,
    #[serde(default)]
    evidence: Vec<RawCite>,
}

/// Names a test of `unit` would mention: the parts of its symbol, its
/// file's name, and its callers' names. Lowercase.
fn names_of(unit: &Unit) -> Vec<String> {
    let mut names: Vec<String> = unit
        .symbol
        .split("::")
        .filter(|p| *p != crate::units::extract::MODULE_SYMBOL && p.len() > 2)
        .map(|p| p.split('#').next().unwrap_or(p).to_lowercase())
        .collect();
    if let Some(stem) = unit.file.file_stem() {
        names.push(stem.to_string_lossy().to_lowercase());
    }
    for c in &unit.callers {
        let short = c.rsplit("::").next().unwrap_or(c);
        if short.len() > 2 && short != crate::units::extract::MODULE_SYMBOL {
            names.push(short.to_lowercase());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Check a covering test: a plain file of the repository under `root`,
/// lines that exist, and lines that mention the unit, its class, its file or
/// a caller by name. A test that never names what it supposedly tests is no
/// evidence that it covers it.
fn check_test(unit: &Unit, t: &RawTest, root: &Path) -> Result<CoveringTest, String> {
    let file = repo_path(&t.file, root)
        .ok_or_else(|| format!("'{}' is outside the repository", t.file))?;
    let text = crate::git::read_text_file(root, &file, 4 * 1024 * 1024)
        .ok_or_else(|| format!("{} can't be read", file.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    if t.start < 1 || t.end < t.start || t.end as usize > lines.len() {
        return Err(format!(
            "{}:{}-{} doesn't exist",
            file.display(),
            t.start,
            t.end
        ));
    }
    let body = lines[t.start as usize - 1..t.end as usize]
        .join("\n")
        .to_lowercase();
    if !names_of(unit).iter().any(|n| body.contains(n.as_str())) {
        return Err(format!(
            "{}:{}-{} never names the unit or a caller",
            file.display(),
            t.start,
            t.end
        ));
    }
    let name = one_line(&t.name, 80);
    if name.is_empty() {
        return Err("no test name".into());
    }
    Ok(CoveringTest {
        name,
        cite: Cite {
            file,
            start: t.start as u32,
            end: t.end as u32,
        },
        checks: one_line(&t.checks, 140),
    })
}

/// Validate a Tests answer about `unit`. A covering test from `found` (teer's
/// own search) is kept under the name and lines teer found; any other is
/// checked by [`check_test`]. A gap must cite lines of the unit, its callers
/// or callees (`regions`). What fails is dropped.
pub fn validate_coverage(
    unit: &Unit,
    raw: &Value,
    regions: &[Region],
    root: &Path,
    found: &[FoundTest],
) -> (Coverage, Vec<Dropped>) {
    let mut dropped = Vec::new();
    let drop = |what: String, reason: String| Dropped {
        unit_id: unit.id.clone(),
        statement: what,
        reason,
    };
    let mut covered: Vec<CoveringTest> = Vec::new();
    for item in raw["covered"].as_array().cloned().unwrap_or_default() {
        let Ok(t) = serde_json::from_value::<RawTest>(item) else {
            continue;
        };
        let known = found.iter().find(|f| {
            Path::new(&t.file) == f.cite.file
                && t.start <= f.cite.end as i64
                && t.end >= f.cite.start as i64
        });
        let checked = match known {
            Some(f) => Ok(CoveringTest {
                name: f.name.clone(),
                cite: f.cite.clone(),
                checks: one_line(&t.checks, 140),
            }),
            None => check_test(unit, &t, root),
        };
        match checked {
            Ok(c) if !covered.iter().any(|x| x.cite == c.cite) => covered.push(c),
            Ok(_) => {}
            Err(reason) => dropped.push(drop(format!("covering test {}", t.name), reason)),
        }
    }
    covered.truncate(MAX_COVERING);
    let mut gaps: Vec<Gap> = Vec::new();
    for item in raw["gaps"].as_array().cloned().unwrap_or_default() {
        let Ok(g) = serde_json::from_value::<RawGap>(item) else {
            continue;
        };
        let case = one_line(&g.case, 100);
        if case.is_empty() {
            continue;
        }
        match check_cites(&g.evidence, regions, root) {
            Ok(evidence) => gaps.push(Gap {
                case,
                danger: one_line(&g.danger, 160),
                risk: parse_risk(&g.risk),
                evidence,
            }),
            Err(reason) => dropped.push(drop(format!("uncovered case: {case}"), reason)),
        }
    }
    gaps.sort_by_key(|g| std::cmp::Reverse(g.risk));
    gaps.truncate(MAX_GAPS);
    (
        Coverage {
            search: None,
            judged: true,
            covered,
            gaps,
        },
        dropped,
    )
}

/// Code a picture may cite: the unit itself, a caller, or something it calls.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    /// Relative to the repository root.
    pub file: PathBuf,
    pub range: Range,
    /// What it is, for the prompt: "this unit, new lines", "caller billing::charge".
    pub role: String,
}

/// The unit's own lines: its new range, and its old range (under the old path
/// of a renamed file).
pub fn unit_regions(unit: &Unit, file: &ChangedFile) -> Vec<Region> {
    let mut out = Vec::new();
    if let Some(r) = unit.new_range {
        out.push(Region {
            file: unit.file.clone(),
            range: r,
            role: "this unit, new lines".into(),
        });
    }
    if let Some(r) = unit.old_range {
        out.push(Region {
            file: file.old_path.clone().unwrap_or_else(|| unit.file.clone()),
            range: r,
            role: "this unit, old lines".into(),
        });
    }
    out
}

#[derive(Debug, Deserialize)]
struct RawCite {
    file: String,
    start: i64,
    end: i64,
}

/// Agent text on one line: cleaned, whitespace collapsed, capped at `max` columns.
pub(crate) fn one_line(s: &str, max: usize) -> String {
    let s = crate::util::sanitize_agent_text(s);
    crate::util::truncate(&s.split_whitespace().collect::<Vec<_>>().join(" "), max)
}

/// Validate an Ask answer. Each region it shows must be lines that exist in a
/// plain file of the repository under `root`; teer reads them itself, from
/// the reviewed version when the file is one of `files` (the change), and
/// marks the lines the change adds or edits. A region longer than
/// [`MAX_SHOWN_LINES`](super::prompts::MAX_SHOWN_LINES) is cut. Related
/// units must be `units` of the change; units holding shown code of the
/// change are added. Fails when the answer has neither text nor code.
pub fn validate_answer(
    raw: &Value,
    root: &Path,
    files: &[ChangedFile],
    units: &[Unit],
) -> (Result<Answer, String>, Vec<Dropped>) {
    use super::prompts::{MAX_RELATED, MAX_SHOWN, MAX_SHOWN_LINES};
    let mut dropped = Vec::new();
    let mut drop = |what: String, reason: &str| {
        dropped.push(Dropped {
            unit_id: "the question".into(),
            statement: what,
            reason: reason.into(),
        });
    };
    let text = crate::util::sanitize_agent_text(raw["text"].as_str().unwrap_or(""))
        .trim()
        .to_string();
    let mut shown: Vec<Shown> = Vec::new();
    for item in raw["show"].as_array().into_iter().flatten().take(MAX_SHOWN) {
        let name = one_line(item["file"].as_str().unwrap_or(""), 200);
        let (start, end) = (item["start"].as_i64(), item["end"].as_i64());
        let what = format!("{name}:{}-{}", start.unwrap_or(0), end.unwrap_or(0));
        let Some(file) = repo_path(&name, root) else {
            drop(what, "outside the repository");
            continue;
        };
        let (Some(start), Some(end)) = (start, end) else {
            drop(what, "no lines");
            continue;
        };
        if start < 1 || end < start || start > u32::MAX as i64 {
            drop(what, "bad lines");
            continue;
        }
        let changed_file = files.iter().find(|f| f.path == file);
        let text = match changed_file {
            Some(f) => f.new_content.clone().or_else(|| f.old_content.clone()),
            None => crate::git::read_text_file(root, &file, 4 * 1024 * 1024),
        };
        let Some(text) = text else {
            drop(what, "can't be read");
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        let start = start as u32;
        if start as usize > lines.len() {
            drop(what, "no such lines");
            continue;
        }
        let end = (end.min(u32::MAX as i64) as u32)
            .min(lines.len() as u32)
            .min(start + MAX_SHOWN_LINES - 1);
        let cite = Cite { file, start, end };
        if shown.iter().any(|s| s.cite == cite) {
            continue;
        }
        let changed = changed_file
            .map(added_lines)
            .unwrap_or_default()
            .into_iter()
            .filter(|n| (start..=end).contains(n))
            .collect();
        shown.push(Shown {
            code: lines[start as usize - 1..end as usize]
                .iter()
                .map(|l| l.to_string())
                .collect(),
            why: one_line(item["why"].as_str().unwrap_or(""), 100),
            in_change: changed_file.is_some(),
            changed,
            cite,
        });
    }
    let mut related: Vec<Related> = Vec::new();
    for item in raw["related"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_RELATED)
    {
        let id = item["id"].as_str().unwrap_or("").trim();
        if !units.iter().any(|u| u.id == id) {
            drop(one_line(id, 200), "not a unit of this change");
            continue;
        }
        if !related.iter().any(|r| r.id == id) {
            related.push(Related {
                id: id.to_string(),
                why: one_line(item["why"].as_str().unwrap_or(""), 100),
            });
        }
    }
    // Code of the change on show: its units are related whatever the agent
    // said. Not `(top level)`, whose range is the whole file.
    for s in shown.iter().filter(|s| s.in_change) {
        for u in units
            .iter()
            .filter(|u| u.file == s.cite.file && u.kind != super::model::UnitKind::Module)
        {
            let overlaps = u
                .new_range
                .is_some_and(|r| r.start <= s.cite.end && s.cite.start <= r.end);
            if overlaps && related.len() < MAX_RELATED && !related.iter().any(|r| r.id == u.id) {
                related.push(Related {
                    id: u.id.clone(),
                    why: "its code is shown below".into(),
                });
            }
        }
    }
    if text.is_empty() && shown.is_empty() {
        return (Err("the agent returned no answer".into()), dropped);
    }
    (
        Ok(Answer {
            text,
            shown,
            related,
            next: questions(
                &serde_json::from_value::<Vec<String>>(raw["next"].clone()).unwrap_or_default(),
                super::prompts::MAX_NEXT,
            ),
        }),
        dropped,
    )
}

/// Lines (1-based) of the new version of `f` that the change adds or edits;
/// every line of an added file.
fn added_lines(f: &ChangedFile) -> Vec<u32> {
    let Some(new) = f.new_content.as_deref() else {
        return Vec::new();
    };
    let new: Vec<&str> = new.lines().collect();
    let old: Vec<&str> = f.old_content.as_deref().unwrap_or("").lines().collect();
    crate::util::diff_lines(&old, &new)
        .into_iter()
        .filter(|(op, _, _)| *op == crate::util::DiffOp::Insert)
        .map(|(_, _, j)| j as u32 + 1)
        .collect()
}

/// A cited path relative to the repository: `./` and the absolute prefix
/// `root` are removed; `..` and other absolute paths are refused.
pub(crate) fn repo_path(raw: &str, root: &Path) -> Option<PathBuf> {
    let p = Path::new(raw.trim());
    let p = if p.is_absolute() {
        p.strip_prefix(root).ok()?
    } else {
        p
    };
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Every cited range must lie inside one region of the same file.
fn check_cites(raw: &[RawCite], regions: &[Region], root: &Path) -> Result<Vec<Cite>, String> {
    if raw.is_empty() {
        return Err("no evidence".into());
    }
    let mut out: Vec<Cite> = Vec::new();
    for c in raw {
        let Some(file) = repo_path(&c.file, root) else {
            return Err(format!(
                "evidence in '{}' is outside the repository",
                c.file
            ));
        };
        if c.start < 1 || c.end < c.start || c.end > u32::MAX as i64 {
            return Err(format!("bad evidence lines {}-{}", c.start, c.end));
        }
        let (start, end) = (c.start as u32, c.end as u32);
        let inside = regions
            .iter()
            .any(|r| r.file == file && r.range.contains(start) && r.range.contains(end));
        if !inside {
            return Err(format!(
                "evidence {}:{start}-{end} outside the unit, its callers and callees",
                file.display()
            ));
        }
        let cite = Cite { file, start, end };
        if !out.contains(&cite) {
            out.push(cite);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{Decision, Prep, UnitKind, UnitStatus};
    use serde_json::json;

    fn unit() -> Unit {
        Unit {
            id: "a.py::f".into(),
            file: "a.py".into(),
            symbol: "f".into(),
            kind: UnitKind::Function,
            status: UnitStatus::Changed,
            old_range: Some(Range::new(10, 20)),
            new_range: Some(Range::new(12, 25)),
            renamed_from: None,
            fingerprint: String::new(),
            changes: vec![],
            prep: Prep::Pending,
            callers: vec![],
            callees: vec![],
            decision: Decision::Pending,
            summary: None,
            nbc_reason: None,
            dive: Default::default(),
            accepted_by: None,
            reasons: Vec::new(),
            dive_pending: Default::default(),
            checks: Default::default(),
            security: Vec::new(),
        }
    }

    #[test]
    fn a_possible_risk_needs_a_known_class_and_lines_of_the_unit() {
        let raw = json!([
            { "class": "authorization", "severity": "high", "text": "A caller can pass any id.",
              "evidence": [{ "side": "new", "line": 14 }],
              "checks": ["Which callers check the owner?", " ", "Which callers check the owner?",
                         "Is there a voter?", "A third one is not read."] },
            { "class": "magic", "severity": "high", "text": "Unknown class.",
              "evidence": [{ "side": "new", "line": 14 }] },
            { "class": "injection", "severity": "low", "text": "Lines of other code.",
              "evidence": [{ "side": "new", "line": 99 }] },
            { "class": "secret", "severity": "low", "text": "  ", "evidence": [] },
        ]);
        let (kept, dropped) = validate_suspects(&unit(), &raw);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].class, Class::Authorization);
        assert_eq!(kept[0].severity, Risk::High);
        assert_eq!(kept[0].evidence[0].line, 14);
        // The checks that follow it up: the blank and the repeated left out.
        assert_eq!(
            kept[0].checks,
            ["Which callers check the owner?", "Is there a voter?"]
        );
        // The fourth says nothing; only three are read anyway.
        assert_eq!(dropped.len(), 2);
        assert!(dropped[0].statement.starts_with("possible risk: "));
        // An answer without the field (an older agent) is no risk.
        assert_eq!(validate_suspects(&unit(), &Value::Null).0, []);
    }

    #[test]
    fn an_expensive_undo_needs_a_reason_and_lines_of_the_unit() {
        let u = unit();
        let ok = json!({"expensive": true, "why": "Deletes  the\nworkflows.", "evidence": [{"side": "new", "line": 14}]});
        let (cost, dropped) = validate_cost(&u, &ok);
        assert_eq!(cost.unwrap().why, "Deletes the workflows.");
        assert!(dropped.is_none());
        let far = json!({"expensive": true, "why": "x", "evidence": [{"side": "new", "line": 99}]});
        let (cost, dropped) = validate_cost(&u, &far);
        assert!(cost.is_none() && dropped.is_some());
        let no_why =
            json!({"expensive": true, "why": " ", "evidence": [{"side": "new", "line": 14}]});
        assert!(validate_cost(&u, &no_why).0.is_none());
        let cheap = json!({"expensive": false, "why": "", "evidence": []});
        assert_eq!(validate_cost(&u, &cheap), (None, None));
        assert_eq!(validate_cost(&u, &Value::Null), (None, None));
    }

    #[test]
    fn a_second_opinion_judges_only_statements_that_exist_with_lines() {
        let mut u = unit();
        let (changes, _) = validate_changes(
            &u,
            &json!([
                {"marker": "changed", "statement": "Retries twice.", "risk": "low", "evidence": [{"side": "new", "line": 14}]},
                {"marker": "added", "statement": "Logs it.", "risk": "low", "evidence": [{"side": "new", "line": 15}]}
            ]),
        );
        u.changes = changes;
        let raw = json!({
            "risk": "high",
            "verdicts": [
                {"n": 1, "verdict": "dispute", "why": "It retries forever.", "evidence": [{"side": "new", "line": 16}]},
                {"n": 1, "verdict": "agree", "why": "dup", "evidence": [{"side": "new", "line": 16}]},
                {"n": 2, "verdict": "dispute", "why": "No lines.", "evidence": [{"side": "new", "line": 99}]},
                {"n": 7, "verdict": "dispute", "why": "No such statement.", "evidence": [{"side": "new", "line": 14}]}
            ],
            "missed": [
                {"marker": "added", "statement": "Sends an email.", "risk": "medium", "evidence": [{"side": "new", "line": 20}]}
            ]
        });
        let (o, dropped) = validate_second(&u, &raw);
        assert_eq!(o.risk, Risk::High);
        assert_eq!(o.verdicts.len(), 1, "{o:?}");
        assert_eq!(o.verdicts[0].statement, "Retries twice.");
        assert_eq!(dropped.len(), 2);
        assert_eq!(
            o.disagreements(&u.changes),
            [
                "disputes \"Retries twice.\": It retries forever.",
                "missed: Sends an email.",
                "rates it high risk, not low"
            ]
        );
    }

    #[test]
    fn keeps_valid_and_drops_out_of_range() {
        let raw = json!([
            {"marker": "added", "statement": "Rejects unknown currencies.", "risk": "medium", "evidence": [{"side": "new", "line": 14}]},
            {"marker": "changed", "statement": "Retries up to 3x.", "risk": "high", "evidence": [{"side": "new", "line": 30}]},
            {"marker": "removed", "statement": "No receipt.", "risk": "low", "evidence": [{"side": "old", "line": 11}, {"side": "new", "line": 9}]},
            {"marker": "removed", "statement": "Old side line.", "risk": "low", "evidence": [{"side": "old", "line": 20}]},
            {"marker": "added", "statement": "No proof.", "risk": "low", "evidence": []}
        ]);
        let (kept, dropped) = validate_changes(&unit(), &raw);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].statement, "Rejects unknown currencies.");
        assert_eq!(kept[1].marker, Marker::Removed);
        assert_eq!(dropped.len(), 3);
        assert!(dropped[0].reason.contains("outside"));
    }

    #[test]
    fn added_unit_cannot_cite_old_lines() {
        let mut u = unit();
        u.old_range = None;
        let raw = json!([{"marker": "added", "statement": "x", "risk": "low", "evidence": [{"side": "old", "line": 12}]}]);
        let (kept, dropped) = validate_changes(&u, &raw);
        assert!(kept.is_empty());
        assert_eq!(dropped.len(), 1);
    }

    #[test]
    fn caps_at_three() {
        let one = json!({"marker": "changed", "statement": "s", "risk": "low", "evidence": [{"side": "new", "line": 13}]});
        let raw = json!([one, one, one, one]);
        let (kept, dropped) = validate_changes(&unit(), &raw);
        assert_eq!(kept.len(), 3);
        assert_eq!(dropped.len(), 1);
    }

    #[test]
    fn unit_regions_use_the_old_path_of_a_renamed_file() {
        let f = ChangedFile {
            path: "a.py".into(),
            old_path: Some("old.py".into()),
            status: crate::git::FileStatus::Renamed,
            old_content: None,
            new_content: None,
        };
        let r = unit_regions(&unit(), &f);
        assert_eq!(r.len(), 2);
        assert_eq!(
            (r[0].file.to_str(), r[0].range),
            (Some("a.py"), Range::new(12, 25))
        );
        assert_eq!(
            (r[1].file.to_str(), r[1].range),
            (Some("old.py"), Range::new(10, 20))
        );
    }
}
