//! Behavior: a flagged unit run before and after the change on the same
//! inputs. The agent, read-only as always, writes a small probe program and
//! predicts for each case whether its output changes. teer runs the probe
//! in a sandbox (see `crate::sandbox`) on its own checkout of the base, then
//! again with the change applied, and compares what the two runs printed.
//! The table comes from those runs, never from the agent's word, and a case
//! that comes out other than the agent predicted is shown as a surprise.
//!
//! The probe is written to the checkout teer keeps for this (`pr::probe_tree`),
//! never to the user's working tree. A probe that doesn't run goes back to
//! the agent with what it printed, up to [`ATTEMPTS`] times.
//!
//! Each case names the change statement it tests, and for those cases the
//! prediction is what the statement implies: a surprise there means the text
//! the reviewer reads is wrong. Once a probe works, the project's own tests
//! the agent picked run twice more: with the change, and with only this unit
//! put back the way it was. Passing both ways means no test notices it.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use super::dive::DiveEvent;
use super::model::{
    Behavior, DiveKind, Expect, Outcome, ProbeCase, SessionRef, TestCheck, Unit, UnitStatus,
};
use super::prompts::{self, MAX_CASES, MAX_PROBE_BYTES, MAX_PROBE_FILES, PROBE_MARK, ProbeSetting};
use super::validate::one_line;
use crate::agent::AgentAdapter;
use crate::git::{ChangedFile, FileStatus};
use crate::pr::ProbeTree;
use crate::sandbox::{self, Jail, RunOutput};

/// Probes the agent may write before teer gives up.
const ATTEMPTS: usize = 3;

/// Installed dependency folders linked from the user's checkout into the
/// probe checkout (read only in the sandbox), when they exist there.
const DEP_DIRS: &[&str] = &["node_modules", "vendor", ".venv", "venv"];

/// Programs named in the prompt when they are on PATH, with how to get a version line.
const PROGRAMS: &[&str] = &["python3", "node", "php", "cargo", "deno", "bun"];

/// Longest output kept per case, in columns.
const MAX_OUTCOME: usize = 300;

/// What a Behavior run needs, gathered from the review.
#[derive(Debug, Clone)]
pub struct Job {
    /// The user's repository: the probe checkout is added from it, and
    /// installed dependency folders are linked from it.
    pub repo: PathBuf,
    /// The commit the change is compared against.
    pub base: String,
    /// Every changed file: applied on top of `base` for the run after.
    pub files: Vec<ChangedFile>,
    pub review_id: String,
    pub unit: Unit,
    pub file: ChangedFile,
    /// Per run.
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeFile {
    pub path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CasePlan {
    pub name: String,
    pub input: String,
    pub expect: Expect,
    pub why: String,
    /// The statement it tests (1-based), 0 for none.
    pub claim: usize,
}

/// A checked agent answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub files: Vec<ProbeFile>,
    pub command: Vec<String>,
    pub cases: Vec<CasePlan>,
    /// The project's tests for the unit; empty when the agent found none.
    pub tests: Vec<String>,
    /// Which tests, or why there are none.
    pub tests_why: String,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    #[serde(default)]
    path: String,
    #[serde(default)]
    content: String,
}

#[derive(Debug, Deserialize)]
struct RawCase {
    #[serde(default)]
    name: String,
    #[serde(default)]
    input: String,
    #[serde(default)]
    expect: String,
    #[serde(default)]
    why: String,
    #[serde(default)]
    claim: i64,
}

#[derive(Debug, Default, Deserialize)]
struct RawTests {
    #[serde(default)]
    command: Vec<String>,
    #[serde(default)]
    why: String,
}

#[derive(Debug, Deserialize)]
struct RawPlan {
    #[serde(default)]
    files: Vec<RawFile>,
    #[serde(default)]
    command: Vec<String>,
    #[serde(default)]
    cases: Vec<RawCase>,
    #[serde(default)]
    tests: Option<RawTests>,
}

/// A program and its arguments that may be run: not empty, no option for a
/// program, at most `max` arguments, no NUL, none very long.
fn check_command(command: &[String], max: usize) -> Result<(), String> {
    match command.first() {
        None => return Err("no command".into()),
        Some(p) if p.trim().is_empty() || p.starts_with('-') => {
            return Err(format!("'{p}' is not a program"));
        }
        _ => {}
    }
    if command.len() > max || command.iter().any(|a| a.contains('\0') || a.len() > 300) {
        return Err("the command is too long".into());
    }
    Ok(())
}

/// A relative path made only of plain names, none of them `.git`.
fn plain_path(raw: &str) -> Option<PathBuf> {
    let p = Path::new(raw.trim());
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(part) if part != ".git" => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Check an agent's probe: new files with plain relative paths that no
/// changed file uses, a command, and cases with unique names. Cases with an
/// unknown prediction are dropped, and a claim number past the `claims`
/// statements becomes 0; anything else wrong refuses the whole plan. A test
/// command that can't be run is dropped, never the probe.
fn validate_plan(raw: &Value, changed: &[ChangedFile], claims: usize) -> Result<Plan, String> {
    let raw: RawPlan =
        serde_json::from_value(raw.clone()).map_err(|e| format!("not a probe: {e}"))?;
    if raw.files.is_empty() || raw.files.len() > MAX_PROBE_FILES {
        return Err(format!("a probe has 1 to {MAX_PROBE_FILES} files"));
    }
    let taken: Vec<&Path> = changed
        .iter()
        .flat_map(|f| std::iter::once(f.path.as_path()).chain(f.old_path.as_deref()))
        .collect();
    let mut files: Vec<ProbeFile> = Vec::new();
    for f in raw.files {
        let path = plain_path(&f.path)
            .ok_or_else(|| format!("'{}' is not a plain relative path", f.path))?;
        if taken.contains(&path.as_path()) {
            return Err(format!("{} is a file of the change", path.display()));
        }
        if files.iter().any(|x| x.path == path) {
            return Err(format!("{} is listed twice", path.display()));
        }
        if f.content.len() > MAX_PROBE_BYTES {
            return Err(format!("{} is too large", path.display()));
        }
        files.push(ProbeFile {
            path,
            content: f.content,
        });
    }
    let command = raw.command;
    check_command(&command, 12)?;
    let mut cases: Vec<CasePlan> = Vec::new();
    for c in raw.cases {
        let name = one_line(&c.name, 40);
        let expect = match c.expect.trim() {
            "same" => Expect::Same,
            "different" => Expect::Different,
            _ => continue,
        };
        if name.is_empty() || cases.iter().any(|x| x.name == name) {
            continue;
        }
        let claim = usize::try_from(c.claim)
            .ok()
            .filter(|&n| n <= claims)
            .unwrap_or(0);
        cases.push(CasePlan {
            name,
            input: one_line(&c.input, 160),
            expect,
            why: one_line(&c.why, 160),
            claim,
        });
        if cases.len() == MAX_CASES {
            break;
        }
    }
    if cases.is_empty() {
        return Err("no usable case".into());
    }
    let raw_tests = raw.tests.unwrap_or_default();
    let tests_why = one_line(&raw_tests.why, 160);
    let tests = if raw_tests.command.is_empty() {
        Vec::new()
    } else {
        match check_command(&raw_tests.command, 16) {
            Ok(()) => raw_tests.command,
            Err(why) => {
                crate::log::write(&format!("test command dropped: {why}"));
                Vec::new()
            }
        }
    };
    Ok(Plan {
        files,
        command,
        cases,
        tests,
        tests_why,
    })
}

fn outcome_text(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    crate::util::truncate(&crate::util::sanitize_agent_text(&s), MAX_OUTCOME)
}

/// The cases a run printed, by name: `TEER_CASE {"case", "output" | "error"}`
/// lines. The first line for a name counts; anything else is ignored.
fn parse_run(stdout: &str) -> HashMap<String, Outcome> {
    let mark = PROBE_MARK.trim_end();
    let mut out = HashMap::new();
    for line in stdout.lines() {
        let Some(rest) = line.trim_start().strip_prefix(mark) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(rest.trim()) else {
            continue;
        };
        let Some(name) = v.get("case").and_then(|c| c.as_str()) else {
            continue;
        };
        let name = one_line(name, 40);
        if out.contains_key(&name) {
            continue;
        }
        let outcome = match (v.get("error").filter(|e| !e.is_null()), v.get("output")) {
            (Some(e), _) => Outcome::Error(outcome_text(e)),
            (None, Some(o)) => Outcome::Value(crate::util::truncate(
                &crate::util::sanitize_agent_text(&o.to_string()),
                MAX_OUTCOME,
            )),
            (None, None) => continue,
        };
        out.insert(name, outcome);
    }
    out
}

/// The table: each planned case with what the two runs printed for it.
fn build_cases(plan: &Plan, before: &str, after: &str) -> Vec<ProbeCase> {
    let (mut b, mut a) = (parse_run(before), parse_run(after));
    plan.cases
        .iter()
        .map(|c| ProbeCase {
            name: c.name.clone(),
            input: c.input.clone(),
            expect: c.expect,
            why: c.why.clone(),
            before: b.remove(&c.name).unwrap_or(Outcome::Missing),
            after: a.remove(&c.name).unwrap_or(Outcome::Missing),
            claim: c.claim,
        })
        .collect()
}

/// Did the probe work? Each run the unit exists in must print its cases, and
/// at least one case must produce a value: a probe that only errors is broken.
pub fn judge(status: UnitStatus, cases: &[ProbeCase]) -> Result<(), String> {
    let printed = |side: fn(&ProbeCase) -> &Outcome| {
        cases.iter().any(|c| !matches!(side(c), Outcome::Missing))
    };
    if status != UnitStatus::Added && !printed(|c| &c.before) {
        return Err("the run before the change printed no case".into());
    }
    if status != UnitStatus::Removed && !printed(|c| &c.after) {
        return Err("the run after the change printed no case".into());
    }
    let value = cases
        .iter()
        .any(|c| matches!(c.before, Outcome::Value(_)) || matches!(c.after, Outcome::Value(_)));
    if !value {
        return Err("every case failed in both runs, so the probe itself is broken".into());
    }
    Ok(())
}

fn tail(s: &str, lines: usize) -> String {
    let all: Vec<&str> = s.lines().collect();
    let from = all.len().saturating_sub(lines);
    let text = all[from..].join("\n");
    crate::util::sanitize_agent_text(&text)
}

/// What the agent is told when its probe didn't work.
fn retry_text(plan: &Plan, runs: &[(&str, &RunOutput)], why: &str) -> String {
    let mut p = format!("Problem: {why}\ncommand: {:?}\n", plan.command);
    for f in &plan.files {
        let mut content = f.content.clone();
        if content.len() > 6000 {
            let mut cut = 6000;
            while !content.is_char_boundary(cut) {
                cut -= 1;
            }
            content.truncate(cut);
            content.push_str("\n... (cut)");
        }
        p.push_str(&format!("\nfile {}:\n{}\n", f.path.display(), content));
    }
    for (name, r) in runs {
        p.push_str(&format!(
            "\nrun {name} ({}):\nstdout (last lines):\n{}\nstderr (last lines):\n{}\n",
            r.status_text(),
            tail(&r.stdout, 30),
            tail(&r.stderr, 40)
        ));
    }
    p
}

// ---------------------------------------------------------------- the checkout

/// `root/rel` with every folder on the way a real folder inside `root`
/// (created when `create`), never a symlink. `None` when a folder is missing
/// and `create` is false.
fn inside(root: &Path, rel: &Path, create: bool) -> Result<Option<PathBuf>, String> {
    let parts: Vec<Component> = rel.components().collect();
    let mut cur = root.to_path_buf();
    for (k, c) in parts.iter().enumerate() {
        let Component::Normal(part) = c else {
            return Err(format!("'{}' is not a plain path", rel.display()));
        };
        if *part == ".git" {
            return Err(format!("'{}' is inside .git", rel.display()));
        }
        cur.push(part);
        if k + 1 == parts.len() {
            break;
        }
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(format!("{} is not a folder", cur.display())),
            Err(_) if create => std::fs::create_dir(&cur).map_err(|e| e.to_string())?,
            Err(_) => return Ok(None),
        }
    }
    Ok(Some(cur))
}

/// Write `content` at `root/rel` as a new regular file. An existing file or
/// link there is removed first unless `new_only`, which refuses instead.
fn write_file(root: &Path, rel: &Path, content: &str, new_only: bool) -> Result<(), String> {
    use std::io::Write;
    let p = inside(root, rel, true)?.ok_or("no path")?;
    if let Ok(m) = std::fs::symlink_metadata(&p) {
        if new_only {
            return Err(format!("{} already exists", rel.display()));
        }
        if m.is_dir() {
            return Err(format!("{} is a folder", rel.display()));
        }
        std::fs::remove_file(&p).map_err(|e| e.to_string())?;
    }
    // create_new never follows a link that appeared in between.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
        .map_err(|e| format!("{}: {e}", rel.display()))?;
    f.write_all(content.as_bytes()).map_err(|e| e.to_string())
}

fn remove_file(root: &Path, rel: &Path) -> Result<(), String> {
    if let Some(p) = inside(root, rel, false)?
        && let Ok(m) = std::fs::symlink_metadata(&p)
        && !m.is_dir()
    {
        std::fs::remove_file(&p).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Put the change on top of the base checkout: removed and renamed-away
/// files go, every other changed file gets its new content.
fn apply_change(root: &Path, files: &[ChangedFile]) -> Result<(), String> {
    for f in files {
        if f.status == FileStatus::Deleted {
            remove_file(root, &f.path)?;
        }
        if let Some(old) = &f.old_path {
            remove_file(root, old)?;
        }
    }
    for f in files {
        if let Some(c) = &f.new_content {
            write_file(root, &f.path, c, false)?;
        }
    }
    Ok(())
}

/// The installed dependency folders of the user's checkout: real folders only.
fn dep_sources(repo: &Path) -> Vec<(&'static str, PathBuf)> {
    DEP_DIRS
        .iter()
        .filter_map(|name| {
            let src = repo.join(name);
            std::fs::symlink_metadata(&src)
                .is_ok_and(|m| m.is_dir())
                .then(|| src.canonicalize().ok().map(|r| (*name, r)))
                .flatten()
        })
        .collect()
}

/// Copy `from` to `to`: folders and files, links made again as links.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let kind = e.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        } else if kind.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// A Composer `vendor` for the checkout: a real folder with every package
/// linked, and `composer/`, `bin/` and `autoload.php` copied. Composer finds
/// the project's own classes relative to where its autoloader really lives,
/// so a linked `vendor` would load them from the user's checkout (which the
/// sandbox then refuses to read) instead of the code under test.
fn shadow_vendor(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let name = e.file_name();
        let (from, to) = (e.path(), dst.join(&name));
        if name == "composer" || name == "bin" {
            copy_dir(&from, &to)?;
        } else if name == "autoload.php" {
            std::fs::copy(&from, &to)?;
        } else {
            std::os::unix::fs::symlink(&from, &to)?;
        }
    }
    Ok(())
}

/// Link the dependency folders into the checkout where it has none of its
/// own; a Composer `vendor` gets [`shadow_vendor`].
fn link_deps(root: &Path, deps: &[(&str, PathBuf)]) {
    for (name, src) in deps {
        let dst = root.join(name);
        if std::fs::symlink_metadata(&dst).is_ok() {
            continue;
        }
        let made = if *name == "vendor" && src.join("composer").is_dir() {
            shadow_vendor(src, &dst)
        } else {
            std::os::unix::fs::symlink(src, &dst)
        };
        if let Err(e) = made {
            crate::log::write(&format!(
                "couldn't link {name} into the probe checkout: {e}"
            ));
        }
    }
}

/// `new` (a file's content) with the unit's lines put back as `old` had
/// them; an added unit's lines are taken out. `None` for a removed unit or
/// lines that don't fit.
fn undo_unit(new: &str, old: Option<&str>, unit: &Unit) -> Option<String> {
    let r = unit.new_range?;
    let lines: Vec<&str> = new.split_inclusive('\n').collect();
    let (start, end) = (r.start as usize - 1, r.end as usize);
    if r.start == 0 || end > lines.len() || start > end {
        return None;
    }
    let mut back: String = match (old, unit.old_range) {
        (Some(o), Some(or)) => o
            .split_inclusive('\n')
            .skip(or.start as usize - 1)
            .take(or.len() as usize)
            .collect(),
        _ => String::new(),
    };
    if !back.is_empty() && !back.ends_with('\n') && end < lines.len() {
        back.push('\n');
    }
    Some(lines[..start].concat() + &back + &lines[end..].concat())
}

enum RunError {
    /// The plan can't be carried out (a probe file already exists, ...): the agent may try again.
    Plan(String),
    /// teer couldn't run anything.
    Setup(String),
}

/// Which code a run sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// The base.
    Before,
    /// The base with the change applied.
    After,
    /// The change applied, but this unit put back the way it was.
    Undone,
}

/// One side: the checkout reset to the base, dependencies linked, the change
/// applied for `After` and `Undone` (the unit put back for `Undone`), then
/// `files` written. Blocking.
fn prepare_side(
    tree: &ProbeTree,
    job: &Job,
    files: &[ProbeFile],
    deps: &[(&str, PathBuf)],
    side: Side,
) -> Result<(), RunError> {
    tree.reset(&job.base)
        .map_err(|e| RunError::Setup(format!("couldn't reset the probe checkout: {e:#}")))?;
    link_deps(&tree.dir, deps);
    if side != Side::Before {
        apply_change(&tree.dir, &job.files)
            .map_err(|e| RunError::Setup(format!("couldn't apply the change: {e}")))?;
    }
    if side == Side::Undone {
        let undone = job
            .file
            .new_content
            .as_deref()
            .and_then(|new| undo_unit(new, job.file.old_content.as_deref(), &job.unit))
            .ok_or_else(|| RunError::Setup("couldn't put the unit back".into()))?;
        write_file(&tree.dir, &job.unit.file, &undone, false).map_err(RunError::Setup)?;
    }
    for f in files {
        write_file(&tree.dir, &f.path, &f.content, true).map_err(RunError::Plan)?;
    }
    Ok(())
}

/// Run `command` on `side`, with `files` written first.
async fn run_side(
    tree: &Arc<ProbeTree>,
    job: &Arc<Job>,
    files: Vec<ProbeFile>,
    command: &[String],
    deps: &Arc<Vec<(&'static str, PathBuf)>>,
    side: Side,
) -> Result<RunOutput, RunError> {
    let (t, j, d) = (tree.clone(), job.clone(), deps.clone());
    tokio::task::spawn_blocking(move || prepare_side(&t, &j, &files, &d, side))
        .await
        .map_err(|e| RunError::Setup(e.to_string()))??;
    let jail = Jail {
        dir: tree.dir.clone(),
        cache: tree.cache.clone(),
        read: deps.iter().map(|(_, p)| p.clone()).collect(),
    };
    sandbox::run(&jail, command, job.timeout)
        .await
        .map_err(|e| RunError::Setup(format!("{e:#}")))
}

// ------------------------------------------------------------------ receipts

/// `<data>/probes/<review>/<unit>/`: the probe and what each run printed.
fn receipt_dir(review_id: &str, unit_id: &str) -> PathBuf {
    let page = super::visual::page_name(unit_id);
    super::store::data_dir()
        .join("probes")
        .join(super::visual::safe_name(review_id, 64))
        .join(page.trim_end_matches(".html"))
}

fn save_run(dir: &Path, name: &str, r: &RunOutput) {
    let _ = std::fs::write(dir.join(format!("{name}.stdout")), &r.stdout);
    let status = format!("({})\n{}", r.status_text(), r.stderr);
    let _ = std::fs::write(dir.join(format!("{name}.stderr")), status);
}

fn save_receipt(dir: &Path, plan: &Plan, before: &RunOutput, after: &RunOutput) {
    let _ = std::fs::remove_dir_all(dir);
    let probe = dir.join("probe");
    let write = |rel: &Path, text: &str| {
        if let Ok(Some(p)) = inside(&probe, rel, true) {
            let _ = std::fs::write(p, text);
        }
    };
    if std::fs::create_dir_all(&probe).is_err() {
        return;
    }
    for f in &plan.files {
        write(&f.path, &f.content);
    }
    let _ = std::fs::write(dir.join("command.txt"), plan.command.join(" ") + "\n");
    save_run(dir, "before", before);
    save_run(dir, "after", after);
}

// ------------------------------------------------------------------- the run

/// `prog` in an absolute folder on PATH: a relative entry (`.`,
/// `node_modules/.bin`) could name a program from the repository, which
/// must never run outside the sandbox.
fn which(prog: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(prog))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// "python3 (Python 3.13.1)" for each of [`PROGRAMS`] on PATH. Asked once.
fn programs() -> Vec<String> {
    static FOUND: OnceLock<Vec<String>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            PROGRAMS
                .iter()
                .filter_map(|p| {
                    let out = std::process::Command::new(which(p)?)
                        .arg("--version")
                        .stdin(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .output()
                        .ok()
                        .filter(|o| o.status.success())?;
                    let text = String::from_utf8_lossy(&out.stdout);
                    let first = one_line(text.lines().next().unwrap_or(""), 40);
                    Some(if first.is_empty() {
                        p.to_string()
                    } else {
                        format!("{p} ({first})")
                    })
                })
                .collect()
        })
        .clone()
}

/// One Behavior run at a time per teer process; another process is kept out
/// by the probe checkout's lock.
static RUNS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Tells the tab what is happening.
struct Progress {
    id: String,
    tx: UnboundedSender<DiveEvent>,
}

impl Progress {
    fn say(&self, text: impl Into<String>) {
        let _ = self.tx.send(DiveEvent::Progress {
            id: self.id.clone(),
            kind: DiveKind::Behavior,
            text: text.into(),
        });
    }
}

/// The project's tests with the change, then with only this unit put back.
async fn check_tests(
    tree: &Arc<ProbeTree>,
    job: &Arc<Job>,
    plan: &Plan,
    deps: &Arc<Vec<(&'static str, PathBuf)>>,
    dir: &Path,
    progress: &Progress,
) -> TestCheck {
    if plan.tests.is_empty() {
        let why = if plan.tests_why.is_empty() {
            "the agent found no test that exercises this unit".to_string()
        } else {
            plan.tests_why.clone()
        };
        return TestCheck::NoTests(why);
    }
    if job.unit.status == UnitStatus::Removed {
        return TestCheck::NotRun("a removed unit has nothing to put back".into());
    }
    progress.say("running the project's tests with the change…");
    let with = match run_side(tree, job, Vec::new(), &plan.tests, deps, Side::After).await {
        Ok(r) => r,
        Err(RunError::Setup(e) | RunError::Plan(e)) => return TestCheck::NotRun(e),
    };
    save_run(dir, "tests-with-change", &with);
    if with.timed_out || with.code != Some(0) {
        return TestCheck::FailWithChange(with.status_text());
    }
    progress.say("running the tests again with this unit put back…");
    let undone = match run_side(tree, job, Vec::new(), &plan.tests, deps, Side::Undone).await {
        Ok(r) => r,
        Err(RunError::Setup(e) | RunError::Plan(e)) => return TestCheck::NotRun(e),
    };
    save_run(dir, "tests-unit-undone", &undone);
    match (undone.timed_out, undone.code) {
        (true, _) => TestCheck::NotRun("timed out with the unit put back".into()),
        (false, Some(0)) => TestCheck::Unguarded,
        _ => TestCheck::Guarded,
    }
}

async fn behave(
    adapter: Arc<dyn AgentAdapter>,
    session: &SessionRef,
    job: Arc<Job>,
    progress: &Progress,
) -> Result<Behavior, String> {
    if !sandbox::available() {
        return Err(sandbox::UNAVAILABLE.into());
    }
    if RUNS.try_lock().is_err() {
        progress.say("waiting for another Behavior run to finish…");
    }
    let _one = RUNS.lock().await;
    progress.say("preparing the probe checkout…");
    let (repo, base) = (job.repo.clone(), job.base.clone());
    let tree = tokio::task::spawn_blocking(move || crate::pr::probe_tree(&repo, &base))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
    let tree = Arc::new(tree);
    let repo = job.repo.clone();
    let deps = Arc::new(dep_sources(&repo));
    let setting = ProbeSetting {
        programs: tokio::task::spawn_blocking(programs)
            .await
            .unwrap_or_default(),
        deps: deps.iter().map(|(n, _)| n.to_string()).collect(),
        timeout_secs: job.timeout.as_secs(),
    };
    let claims: Vec<String> = job
        .unit
        .changes
        .iter()
        .map(|c| c.statement.clone())
        .collect();
    let mut retry: Option<String> = None;
    let mut last = String::new();
    for attempt in 1..=ATTEMPTS {
        let of = if attempt == 1 {
            String::new()
        } else {
            format!(" (try {attempt} of {ATTEMPTS})")
        };
        progress.say(format!("the agent writes a probe{of}…"));
        let prompt = prompts::behavior_prompt(&job.unit, &job.file, &setting, retry.as_deref());
        let raw = adapter
            .ask_probe(session, &prompt, &prompts::behavior_schema())
            .await
            .map_err(|e| format!("{e:#}"))?;
        let plan = match validate_plan(&raw, &job.files, claims.len()) {
            Ok(p) => Arc::new(p),
            Err(why) => {
                crate::log::write(&format!("probe {attempt} for {}: {why}", job.unit.id));
                retry = Some(format!("Problem: your answer was refused: {why}"));
                last = why;
                continue;
            }
        };
        let mut runs = Vec::new();
        let mut refused = None;
        for side in [Side::Before, Side::After] {
            let word = if side == Side::Before {
                "before"
            } else {
                "after"
            };
            progress.say(format!(
                "running `{}` {word} the change…",
                plan.command.join(" ")
            ));
            match run_side(&tree, &job, plan.files.clone(), &plan.command, &deps, side).await {
                Ok(r) => runs.push(r),
                Err(RunError::Setup(e)) => return Err(e),
                Err(RunError::Plan(why)) => {
                    refused = Some(why);
                    break;
                }
            }
        }
        if let Some(why) = refused {
            crate::log::write(&format!("probe {attempt} for {}: {why}", job.unit.id));
            retry = Some(format!("Problem: {why}"));
            last = why;
            continue;
        }
        let after = runs.pop().unwrap_or_default();
        let before = runs.pop().unwrap_or_default();
        let dir = receipt_dir(&job.review_id, &job.unit.id);
        save_receipt(&dir, &plan, &before, &after);
        let cases = build_cases(&plan, &before.stdout, &after.stdout);
        match judge(job.unit.status, &cases) {
            Ok(()) => {
                let tests = check_tests(&tree, &job, &plan, &deps, &dir, progress).await;
                return Ok(Behavior {
                    cases,
                    command: plan.command.clone(),
                    dir,
                    claims,
                    tests: Some(tests),
                    test_command: plan.tests.clone(),
                });
            }
            Err(why) => {
                crate::log::write(&format!("probe {attempt} for {}: {why}", job.unit.id));
                retry = Some(retry_text(
                    &plan,
                    &[("before the change", &before), ("after the change", &after)],
                    &why,
                ));
                last = why;
            }
        }
    }
    Err(format!("no working probe after {ATTEMPTS} tries: {last}"))
}

/// Ask for a probe of `job.unit`, run it before and after the change, and
/// send the table (or why there is none). Progress goes out as it happens.
pub async fn run(
    adapter: Arc<dyn AgentAdapter>,
    session: SessionRef,
    job: Job,
    tx: UnboundedSender<DiveEvent>,
) {
    let id = job.unit.id.clone();
    let progress = Progress {
        id: id.clone(),
        tx: tx.clone(),
    };
    let ev = match behave(adapter, &session, Arc::new(job), &progress).await {
        Ok(b) => DiveEvent::Ready {
            id,
            kind: DiveKind::Behavior,
            text: b.summary(),
            sources: Vec::new(),
            visual: None,
            behavior: Some(Box::new(b)),
            coverage: None,
        },
        Err(error) => DiveEvent::Failed {
            id,
            kind: DiveKind::Behavior,
            error,
        },
    };
    let _ = tx.send(ev);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::ClaimCheck;
    use serde_json::json;

    fn changed(path: &str) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            old_path: None,
            status: FileStatus::Modified,
            old_content: Some("old".into()),
            new_content: Some("new".into()),
        }
    }

    fn plan_json() -> Value {
        json!({
            "files": [{"path": "teer_probe.py", "content": "print(1)"}],
            "command": ["python3", "teer_probe.py"],
            "cases": [
                {"name": "neg", "input": "-5", "expect": "different", "why": "now clamps"},
                {"name": "pos", "input": "3", "expect": "same", "why": "untouched"},
                {"name": "pos", "input": "dup", "expect": "same", "why": "dup"},
                {"name": "odd", "input": "x", "expect": "maybe", "why": "?"}
            ]
        })
    }

    #[test]
    fn a_good_plan_passes_and_bad_cases_are_dropped() {
        let p = validate_plan(&plan_json(), &[changed("src/a.py")], 2).unwrap();
        assert_eq!(p.files[0].path, PathBuf::from("teer_probe.py"));
        assert_eq!(p.command, ["python3", "teer_probe.py"]);
        let names: Vec<&str> = p.cases.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["neg", "pos"]);
        assert_eq!(p.cases[0].expect, Expect::Different);
    }

    #[test]
    fn probe_files_must_be_plain_new_paths() {
        for bad in [
            "../x.py",
            "/tmp/x.py",
            ".git/hooks/pre-commit",
            "a/.git/x",
            "src/a.py",
            "",
        ] {
            let mut v = plan_json();
            v["files"][0]["path"] = json!(bad);
            assert!(
                validate_plan(&v, &[changed("src/a.py")], 2).is_err(),
                "{bad} was accepted"
            );
        }
        let mut v = plan_json();
        v["command"] = json!(["-rf"]);
        assert!(validate_plan(&v, &[], 2).is_err());
        let mut v = plan_json();
        v["cases"] = json!([]);
        assert!(validate_plan(&v, &[], 2).is_err());
    }

    #[test]
    fn runs_are_read_from_marked_lines_only() {
        let out = "noise\nTEER_CASE {\"case\": \"neg\", \"output\": -5.0}\n  TEER_CASE {\"case\": \"pos\", \"error\": \"ValueError: no\"}\nTEER_CASE {\"case\": \"neg\", \"output\": 1}\nTEER_CASE not json\nTEER_CASE {\"case\": \"s\", \"error\": \"\\u001b[31mred\"}\nTEER_CASE {\"case\": \"t\", \"output\": \"1\"}\n";
        let r = parse_run(out);
        assert_eq!(r["neg"], Outcome::Value("-5.0".into()));
        assert_eq!(r["pos"], Outcome::Error("ValueError: no".into()));
        // Terminal escapes from the code under review never reach the screen.
        assert_eq!(r["s"], Outcome::Error("red".into()));
        // A string stays a JSON string, so "1" and 1 differ.
        assert_eq!(r["t"], Outcome::Value("\"1\"".into()));
        assert_eq!(r.len(), 4);
    }

    fn plan() -> Plan {
        validate_plan(&plan_json(), &[], 2).unwrap()
    }

    #[test]
    fn cases_compare_the_runs_and_flag_surprises() {
        let before = "TEER_CASE {\"case\": \"neg\", \"output\": -5}\nTEER_CASE {\"case\": \"pos\", \"output\": 3}\n";
        let after = "TEER_CASE {\"case\": \"neg\", \"output\": 0}\nTEER_CASE {\"case\": \"pos\", \"output\": 4}\n";
        let cases = build_cases(&plan(), before, after);
        assert!(cases[0].changed() && !cases[0].surprise());
        assert!(cases[1].changed() && cases[1].surprise(), "predicted same");
        assert_eq!(judge(UnitStatus::Changed, &cases), Ok(()));
        let b = Behavior {
            cases,
            command: Vec::new(),
            dir: PathBuf::new(),
            claims: Vec::new(),
            tests: None,
            test_command: Vec::new(),
        };
        assert_eq!(
            b.summary(),
            "2 cases ran before and after the change: all behave differently, 1 not as the agent predicted."
        );
    }

    #[test]
    fn cases_name_the_statement_they_test() {
        let mut v = plan_json();
        v["cases"][0]["claim"] = json!(1);
        v["cases"][1]["claim"] = json!(7);
        v["tests"] =
            json!({"command": ["python3", "-m", "pytest", "-q"], "why": "tests/test_a.py"});
        let p = validate_plan(&v, &[], 2).unwrap();
        assert_eq!(p.cases[0].claim, 1);
        assert_eq!(p.cases[1].claim, 0, "there is no statement 7");
        assert_eq!(p.tests, ["python3", "-m", "pytest", "-q"]);
        let before = "TEER_CASE {\"case\": \"neg\", \"output\": -5}\nTEER_CASE {\"case\": \"pos\", \"output\": 3}\n";
        let after = "TEER_CASE {\"case\": \"neg\", \"output\": -5}\nTEER_CASE {\"case\": \"pos\", \"output\": 3}\n";
        let b = Behavior {
            cases: build_cases(&p, before, after),
            command: Vec::new(),
            dir: PathBuf::new(),
            claims: vec![
                "Negative amounts now clamp to 0.".into(),
                "Logs the total.".into(),
            ],
            tests: None,
            test_command: Vec::new(),
        };
        // Statement 1 implies "neg" changes; it didn't. Nothing tests statement 2.
        assert_eq!(
            b.claim_checks(),
            [
                (
                    "Negative amounts now clamp to 0.".to_string(),
                    ClaimCheck::Fails { wrong: 1, of: 1 }
                ),
                ("Logs the total.".to_string(), ClaimCheck::Untested),
            ]
        );
        // A test command that can't run is dropped, the probe kept.
        v["tests"] = json!({"command": ["-x"], "why": ""});
        assert!(validate_plan(&v, &[], 2).unwrap().tests.is_empty());
    }

    fn unit_at(old: Option<(u32, u32)>, new: Option<(u32, u32)>) -> Unit {
        let r = |p: Option<(u32, u32)>| p.map(|(a, b)| json!({"start": a, "end": b}));
        serde_json::from_value(json!({
            "id": "a::f", "file": "a.py", "symbol": "f", "kind": "Function",
            "status": "Changed", "old_range": r(old), "new_range": r(new),
            "fingerprint": "x", "changes": [], "callers": [], "callees": [],
            "decision": "Pending"
        }))
        .unwrap()
    }

    #[test]
    fn a_unit_is_put_back_the_way_it_was() {
        let old = "a\nf1\nf2\nz\n";
        let new = "a\nF1\nF2\nF3\nz\n";
        let u = unit_at(Some((2, 3)), Some((2, 4)));
        assert_eq!(undo_unit(new, Some(old), &u).unwrap(), old);
        // An added unit is taken out.
        let added = unit_at(None, Some((2, 4)));
        assert_eq!(undo_unit(new, None, &added).unwrap(), "a\nz\n");
        // At the end of a file without a final newline.
        let u = unit_at(Some((2, 2)), Some((2, 2)));
        assert_eq!(undo_unit("a\nNEW", Some("a\nold"), &u).unwrap(), "a\nold");
        // A removed unit, or lines past the end: nothing to do.
        assert_eq!(
            undo_unit(new, Some(old), &unit_at(Some((2, 3)), None)),
            None
        );
        assert_eq!(
            undo_unit(new, Some(old), &unit_at(Some((2, 3)), Some((4, 9)))),
            None
        );
    }

    #[test]
    fn a_composer_vendor_loads_the_projects_classes_from_the_checkout() {
        let user = tempfile::tempdir().unwrap();
        let vendor = user.path().join("vendor");
        std::fs::create_dir_all(vendor.join("composer")).unwrap();
        std::fs::create_dir_all(vendor.join("symfony/console")).unwrap();
        std::fs::create_dir_all(vendor.join("bin")).unwrap();
        std::fs::write(vendor.join("autoload.php"), "<?php // a").unwrap();
        std::fs::write(vendor.join("composer/autoload_static.php"), "<?php // s").unwrap();
        std::os::unix::fs::symlink("../symfony/console/bin/console", vendor.join("bin/console"))
            .unwrap();
        let tree = tempfile::tempdir().unwrap();
        link_deps(tree.path(), &[("vendor", vendor.clone())]);
        let v = tree.path().join("vendor");
        let meta = |p: &Path| std::fs::symlink_metadata(p).unwrap();
        assert!(meta(&v).is_dir(), "a real folder, not a link");
        assert!(meta(&v.join("composer")).is_dir());
        assert!(meta(&v.join("composer/autoload_static.php")).is_file());
        assert!(meta(&v.join("autoload.php")).is_file());
        assert!(meta(&v.join("symfony")).file_type().is_symlink());
        assert_eq!(
            std::fs::read_link(v.join("bin/console")).unwrap(),
            Path::new("../symfony/console/bin/console")
        );
        // Anything else is linked as before.
        std::fs::create_dir_all(user.path().join("node_modules")).unwrap();
        link_deps(
            tree.path(),
            &[("node_modules", user.path().join("node_modules"))],
        );
        assert!(
            meta(&tree.path().join("node_modules"))
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn a_probe_that_prints_nothing_or_only_errors_is_broken() {
        let ok = "TEER_CASE {\"case\": \"neg\", \"output\": 1}\n";
        let err = "TEER_CASE {\"case\": \"neg\", \"error\": \"ImportError: x\"}\nTEER_CASE {\"case\": \"pos\", \"error\": \"ImportError: x\"}\n";
        let cases = build_cases(&plan(), "", ok);
        assert!(judge(UnitStatus::Changed, &cases).is_err());
        // A new unit has nothing to run before the change.
        assert_eq!(judge(UnitStatus::Added, &cases), Ok(()));
        let cases = build_cases(&plan(), err, err);
        assert!(
            judge(UnitStatus::Changed, &cases)
                .unwrap_err()
                .contains("broken")
        );
    }

    #[test]
    fn writes_never_go_through_links_or_out_of_the_checkout() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
        assert!(write_file(root.path(), Path::new("link/x.py"), "x", false).is_err());
        assert!(!outside.path().join("x.py").exists());
        // A link in the place of the file itself is replaced, not followed.
        let target = outside.path().join("t");
        std::fs::write(&target, "keep").unwrap();
        std::os::unix::fs::symlink(&target, root.path().join("f.py")).unwrap();
        assert!(write_file(root.path(), Path::new("f.py"), "new", true).is_err());
        write_file(root.path(), Path::new("f.py"), "new", false).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        assert_eq!(
            std::fs::read_to_string(root.path().join("f.py")).unwrap(),
            "new"
        );
        write_file(root.path(), Path::new("deep/er/g.py"), "g", true).unwrap();
        assert!(root.path().join("deep/er/g.py").is_file());
    }

    #[test]
    fn the_change_is_applied_on_top_of_the_base() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("gone.py"), "x").unwrap();
        std::fs::write(root.path().join("old_name.py"), "x").unwrap();
        let files = vec![
            ChangedFile {
                path: "gone.py".into(),
                old_path: None,
                status: FileStatus::Deleted,
                old_content: Some("x".into()),
                new_content: None,
            },
            ChangedFile {
                path: "pkg/new_name.py".into(),
                old_path: Some("old_name.py".into()),
                status: FileStatus::Renamed,
                old_content: Some("x".into()),
                new_content: Some("y".into()),
            },
        ];
        apply_change(root.path(), &files).unwrap();
        assert!(!root.path().join("gone.py").exists());
        assert!(!root.path().join("old_name.py").exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join("pkg/new_name.py")).unwrap(),
            "y"
        );
    }
}
