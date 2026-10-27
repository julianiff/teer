//! Committing the reviewed changes of a plain `teer` review from the "all
//! reviewed" popup.
//!
//! Only the files of the review are committed: `git add -A -- <paths>` and
//! `git commit -F <file> --only -- <paths>`, so staged changes and working-tree
//! changes of other files are left exactly as they were. Unlike the read-only
//! helpers in [`crate::git`] (which switch hooks and filters off because they
//! read untrusted content), this runs the user's own plain `git`: their hooks,
//! signing and identity apply. The message goes through a private temp file,
//! never argv. Before anything is staged, [`check`] refuses when what is on
//! disk is no longer what was reviewed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::agent::AgentAdapter;
use crate::approve::tidy_output;
use crate::git::{self, ChangedFile, FileStatus};
use crate::review::model::{Decision, Review, SessionRef, UnitStatus};
use crate::review::prompts;
use crate::util::{sanitize, sanitize_agent_text};

/// Longest subject line, in characters.
pub const SUBJECT_MAX: usize = 72;
/// Longest whole message, in characters.
pub const MESSAGE_MAX: usize = 4000;
/// Most bullets in the template message.
pub const BULLETS_MAX: usize = 8;
/// How long the agent may take to draft a message.
pub const DRAFT_TIMEOUT: Duration = Duration::from_secs(30);

// ------------------------------------------------------------------ text

/// Clean a commit message: strip ANSI sequences and control characters, cap
/// the subject (first non-blank line) at [`SUBJECT_MAX`] characters, drop
/// trailing whitespace and repeated blank lines, and shape it as
/// `subject` or `subject\n\nbody`, at most [`MESSAGE_MAX`] characters.
/// Empty when there is no text at all.
pub fn normalize_message(raw: &str) -> String {
    let clean = sanitize_agent_text(raw).replace('\t', "    ");
    let mut lines = clean
        .lines()
        .map(str::trim_end)
        .skip_while(|l| l.trim().is_empty());
    let Some(subject) = lines.next() else {
        return String::new();
    };
    let subject: String = subject.trim().chars().take(SUBJECT_MAX).collect();
    let subject = subject.trim_end().to_string();
    let mut body: Vec<&str> = Vec::new();
    for l in lines {
        if l.trim().is_empty() && body.last().is_none_or(|p| p.trim().is_empty()) {
            continue;
        }
        body.push(l);
    }
    while body.last().is_some_and(|l| l.trim().is_empty()) {
        body.pop();
    }
    let mut out = subject;
    if !body.is_empty() {
        out.push_str("\n\n");
        out.push_str(&body.join("\n"));
    }
    out.chars()
        .take(MESSAGE_MAX)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The first line of a message.
pub fn subject_of(message: &str) -> &str {
    message.lines().next().unwrap_or("").trim()
}

/// The message teer starts from: `Update <files>` and a bullet per unit
/// summary (at most [`BULLETS_MAX`]).
pub fn template_message(paths: &[String], summaries: &[String]) -> String {
    let n = paths.len();
    let names: Vec<String> = paths.iter().map(|p| sanitize(p)).collect();
    let mut subject = match n {
        0 => "Update changes".to_string(),
        1..=3 => format!("Update {}", names.join(", ")),
        _ => format!("Update {}, {} and {} more files", names[0], names[1], n - 2),
    };
    if subject.chars().count() > SUBJECT_MAX {
        subject = if n == 1 {
            format!("Update {}", names[0])
        } else {
            format!("Update {} and {} more files", names[0], n - 1)
        };
    }
    if subject.chars().count() > SUBJECT_MAX {
        subject = format!("Update {n} file{}", if n == 1 { "" } else { "s" });
    }
    let mut seen = HashSet::new();
    let bullets: Vec<String> = summaries
        .iter()
        .map(|s| sanitize(s).trim().chars().take(100).collect::<String>())
        .filter(|s| !s.is_empty() && seen.insert(s.clone()))
        .take(BULLETS_MAX)
        .map(|s| format!("- {s}"))
        .collect();
    let mut m = subject;
    if !bullets.is_empty() {
        m.push_str("\n\n");
        m.push_str(&bullets.join("\n"));
    }
    normalize_message(&m)
}

// ----------------------------------------------------------- the review

/// The branch HEAD is on.
#[derive(Debug, Clone, PartialEq)]
pub enum Branch {
    Named(String),
    Detached,
    /// git could not say.
    Unknown,
}

pub fn current_branch(repo: &Path) -> Branch {
    let out = git::command(repo)
        .args(["symbolic-ref", "-q", "--short", "HEAD"])
        .stdin(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let name = sanitize(String::from_utf8_lossy(&o.stdout).trim());
            if name.trim().is_empty() {
                Branch::Unknown
            } else {
                Branch::Named(name)
            }
        }
        Ok(o) if o.status.code() == Some(1) => Branch::Detached,
        _ => Branch::Unknown,
    }
}

fn safe_rel(p: &Path) -> bool {
    !p.as_os_str().is_empty()
        && !p.is_absolute()
        && p.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Every path the commit covers: each file of the review, and both paths of a
/// rename. `Err` names a path that is not a plain relative path.
pub fn reviewed_paths(review: &Review) -> Result<Vec<PathBuf>, String> {
    let mut out: Vec<PathBuf> = Vec::new();
    for f in &review.files {
        for p in std::iter::once(&f.path).chain(f.old_path.as_ref()) {
            if !safe_rel(p) {
                return Err(format!(
                    "Won't commit the path '{}'.",
                    sanitize(&p.to_string_lossy())
                ));
            }
            if !out.contains(p) {
                out.push(p.clone());
            }
        }
    }
    Ok(out)
}

/// One row per reviewed file: `+` added, `~` modified or renamed, `−` deleted.
pub fn file_rows(review: &Review) -> Vec<(char, String)> {
    review
        .files
        .iter()
        .map(|f| {
            let path = sanitize(&f.path.to_string_lossy());
            match f.status {
                FileStatus::Added => ('+', path),
                FileStatus::Deleted => ('−', path),
                FileStatus::Modified => ('~', path),
                FileStatus::Renamed => (
                    '~',
                    match &f.old_path {
                        Some(o) => format!("{} → {path}", sanitize(&o.to_string_lossy())),
                        None => path,
                    },
                ),
            }
        })
        .collect()
}

/// Display names of the reviewed files, for the template and the prompt.
pub fn file_names(review: &Review) -> Vec<String> {
    review
        .files
        .iter()
        .map(|f| sanitize(&f.path.to_string_lossy()))
        .collect()
}

/// Summaries of the accepted units with a behavior change, in list order.
pub fn accepted_summaries(review: &Review, order: &[usize]) -> Vec<String> {
    order
        .iter()
        .filter_map(|&i| review.units.get(i))
        .filter(|u| u.status != UnitStatus::NoBehaviorChange && u.decision == Decision::Accepted)
        .filter_map(|u| u.summary.clone())
        .collect()
}

/// What the commit-message prompt is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftInput {
    pub repo: PathBuf,
    /// One line per reviewed file, e.g. `modified src/a.rs`.
    pub files: Vec<String>,
    /// One line per accepted unit: where, what, and the agent's statements.
    pub units: Vec<String>,
}

pub fn draft_input(review: &Review, order: &[usize]) -> DraftInput {
    let files = review
        .files
        .iter()
        .map(|f: &ChangedFile| {
            let word = match f.status {
                FileStatus::Added => "added",
                FileStatus::Deleted => "deleted",
                FileStatus::Modified => "modified",
                FileStatus::Renamed => "renamed",
            };
            match (&f.old_path, f.status) {
                (Some(o), FileStatus::Renamed) => {
                    format!("{word} {} -> {}", o.display(), f.path.display())
                }
                _ => format!("{word} {}", f.path.display()),
            }
        })
        .collect();
    let units = order
        .iter()
        .filter_map(|&i| review.units.get(i))
        .filter(|u| u.status != UnitStatus::NoBehaviorChange && u.decision == Decision::Accepted)
        .map(|u| {
            let mut s = format!("{} {} ({})", u.file.display(), u.symbol, u.status_word());
            if let Some(sum) = &u.summary {
                s.push_str(&format!(": {sum}"));
            }
            for c in &u.changes {
                s.push_str(&format!("; {}", c.statement));
            }
            s
        })
        .collect();
    DraftInput {
        repo: review.repo_root.clone(),
        files,
        units,
    }
}

// ------------------------------------------------------------ git helpers

fn plain(repo: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(repo).env("GIT_LITERAL_PATHSPECS", "1");
    c
}

/// Paths git reports as changed (porcelain `-z`), each entry's paths together
/// (a rename lists both). Restricted to `only` when given.
fn status_entries(repo: &Path, only: Option<&[PathBuf]>) -> Option<Vec<Vec<PathBuf>>> {
    let mut c = git::command(repo);
    c.env("GIT_LITERAL_PATHSPECS", "1")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .stdin(Stdio::null());
    if let Some(p) = only {
        c.arg("--").args(p);
    }
    let out = c.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let mut fields = out
        .stdout
        .split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned());
    let mut entries = Vec::new();
    while let Some(f) = fields.next() {
        let (Some(xy), Some(path)) = (f.get(..2), f.get(3..)) else {
            continue;
        };
        let mut paths = vec![PathBuf::from(path)];
        if (xy.contains('R') || xy.contains('C'))
            && let Some(orig) = fields.next()
        {
            paths.push(PathBuf::from(orig));
        }
        entries.push(paths);
    }
    Some(entries)
}

/// How many changed paths in the working tree are not part of the review.
pub fn other_changed(repo: &Path, reviewed: &[PathBuf]) -> usize {
    let set: HashSet<&PathBuf> = reviewed.iter().collect();
    status_entries(repo, None).map_or(0, |entries| {
        entries
            .iter()
            .filter(|paths| paths.iter().any(|p| !set.contains(p)))
            .count()
    })
}

/// The last few commit subjects, sanitised, as style examples.
pub fn recent_subjects(repo: &Path) -> Vec<String> {
    let Ok(out) = git::command(repo)
        .args([
            "log",
            "-n",
            "5",
            "--no-color",
            "--no-ext-diff",
            "--format=%s",
        ])
        .stdin(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| sanitize(l).trim().chars().take(120).collect::<String>())
        .filter(|l| !l.is_empty())
        .take(5)
        .collect()
}

fn in_progress(repo: &Path) -> Option<&'static str> {
    let out = git::command(repo)
        .args(["rev-parse", "--git-dir"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let dir = if dir.is_absolute() {
        dir
    } else {
        repo.join(dir)
    };
    [
        ("MERGE_HEAD", "merge"),
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
    ]
    .into_iter()
    .find(|(f, _)| dir.join(f).exists())
    .map(|(_, what)| what)
}

// ---------------------------------------------------------------- guards

/// Why a commit was refused before anything was staged.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// A unit is flagged or still pending.
    NotReviewed,
    BadPath(String),
    InProgress(&'static str),
    /// HEAD is not the commit the review was made on.
    HeadMoved {
        reviewed: String,
        now: String,
    },
    /// A reviewed file is not what the review holds.
    Changed(String),
    Nothing,
}

impl Refusal {
    /// The status line; `rerun` is the label of the re-run key.
    pub fn text(&self, rerun: &str) -> String {
        match self {
            Refusal::NotReviewed => "Not everything is accepted yet; nothing was committed.".into(),
            Refusal::BadPath(m) => m.clone(),
            Refusal::InProgress(what) => format!(
                "A {what} is in progress; finish or abort it before committing. Nothing was committed."
            ),
            Refusal::HeadMoved { reviewed, now } => format!(
                "HEAD moved since you reviewed (reviewed {reviewed}, now {now}). Press {rerun} to re-run the review."
            ),
            Refusal::Changed(file) => format!(
                "Files changed since you reviewed them ({file}). Press {rerun} to re-run the review."
            ),
            Refusal::Nothing => "Nothing to commit: the reviewed files have no changes.".into(),
        }
    }
}

fn short(s: &str) -> String {
    s.chars().take(7).collect()
}

/// Everything that must hold before staging anything. On success: the paths
/// to commit. No git command here changes the repository.
pub fn check(review: &Review) -> Result<Vec<PathBuf>, Refusal> {
    let counts_open = review
        .units
        .iter()
        .filter(|u| u.status != UnitStatus::NoBehaviorChange)
        .any(|u| u.decision != Decision::Accepted);
    if counts_open {
        return Err(Refusal::NotReviewed);
    }
    let repo = &review.repo_root;
    let paths = reviewed_paths(review).map_err(Refusal::BadPath)?;
    if let Some(what) = in_progress(repo) {
        return Err(Refusal::InProgress(what));
    }
    // HEAD must still be the commit the review was made on.
    let now = git::head_commit(repo);
    let ok = match &now {
        Some(h) => *h == review.head_commit,
        None => review.head_commit == git::EMPTY_TREE,
    };
    if !ok {
        return Err(Refusal::HeadMoved {
            reviewed: short(&review.head_commit),
            now: now.map_or("none".into(), |h| short(&h)),
        });
    }
    // Every file must still be what was reviewed.
    for f in &review.files {
        let full = repo.join(&f.path);
        let same = match f.status {
            FileStatus::Deleted => matches!(
                std::fs::symlink_metadata(&full),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound
            ),
            _ => f.new_content.as_deref().is_some_and(|want| {
                git::read_text_file(repo, &f.path, git::MAX_WORKTREE_FILE_BYTES)
                    .is_some_and(|have| have.as_bytes() == want.as_bytes())
            }),
        };
        let old_gone = f.status != FileStatus::Renamed
            || f.old_path.as_ref().is_none_or(|o| {
                matches!(
                    std::fs::symlink_metadata(repo.join(o)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound
                )
            });
        if !same || !old_gone {
            return Err(Refusal::Changed(sanitize(&f.path.to_string_lossy())));
        }
    }
    // Something must be left to commit in those files.
    match status_entries(repo, Some(&paths)) {
        Some(e) if e.is_empty() => Err(Refusal::Nothing),
        _ => Ok(paths),
    }
}

// ---------------------------------------------------------------- commit

/// How git's output is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Io {
    /// The terminal belongs to git (hooks, signing and pinentry may print or
    /// ask). The caller has released the TUI.
    Inherit,
    /// stdin closed, output captured (tests).
    Capture,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Committed {
        short: String,
    },
    /// git refused or a hook failed. `output` is what git printed (empty when
    /// the terminal was inherited).
    Failed {
        code: Option<i32>,
        output: String,
    },
    /// Not even tried.
    Refused(String),
}

fn add_failed(o: &std::process::Output) -> Outcome {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    Outcome::Failed {
        code: o.status.code(),
        output: format!("git add failed: {}", text.trim()),
    }
}

/// Stage `paths` and commit exactly them with `message`. Runs the user's own
/// `git` (hooks, signing and identity apply); never `--no-verify`, `--amend`
/// or a signing flag. Call only after [`check`].
pub fn run_commit(repo: &Path, paths: &[PathBuf], message: &str, io: Io) -> Outcome {
    use std::io::Write;
    let message = normalize_message(message);
    if message.is_empty() {
        return Outcome::Refused("The commit message is empty.".into());
    }
    if paths.is_empty() || !paths.iter().all(|p| safe_rel(p)) {
        return Outcome::Refused("Nothing to commit.".into());
    }
    let dir = crate::review::store::data_dir().join("tmp");
    let file = std::fs::create_dir_all(&dir).and_then(|_| {
        let mut f = tempfile::Builder::new()
            .prefix("commit-msg-")
            .tempfile_in(&dir)?;
        f.write_all(message.as_bytes())?;
        f.write_all(b"\n")?;
        f.flush()?;
        Ok(f)
    });
    let file = match file {
        Ok(f) => f,
        Err(e) => return Outcome::Refused(format!("Couldn't write the message file: {e}")),
    };

    // Stage what is on disk (new files must be known to git; this also
    // covers modifications). A path that is gone (a deletion, the old name of
    // a rename) is not staged: `git add` would refuse a path that is in
    // neither the index nor the working tree, and `commit --only` records the
    // removal from HEAD by itself.
    let present: Vec<&PathBuf> = paths
        .iter()
        .filter(|p| std::fs::symlink_metadata(repo.join(p)).is_ok())
        .collect();
    let add = if present.is_empty() {
        None
    } else {
        Some(
            plain(repo)
                .args(["add", "-A", "--"])
                .args(present)
                .stdin(Stdio::null())
                .output(),
        )
    };
    if let Some(res) = add {
        match res {
            Err(e) => return Outcome::Refused(format!("Couldn't run git: {e}")),
            Ok(o) if !o.status.success() => {
                let res = add_failed(&o);
                if let (Io::Inherit, Outcome::Failed { output, .. }) = (io, &res) {
                    eprintln!("{output}");
                }
                return res;
            }
            Ok(_) => {}
        }
    }

    let mut c = plain(repo);
    c.arg("commit")
        .arg("-F")
        .arg(file.path())
        .args(["--only", "--"])
        .args(paths);
    let result = match io {
        Io::Inherit => c
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map(|s| (s, String::new())),
        Io::Capture => c
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map(|o| {
                (
                    o.status,
                    format!(
                        "{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    ),
                )
            }),
    };
    drop(file);
    match result {
        Err(e) => Outcome::Refused(format!("Couldn't run git: {e}")),
        Ok((status, output)) if !status.success() => Outcome::Failed {
            code: status.code(),
            output,
        },
        Ok(_) => {
            let short = git::command(repo)
                .args(["rev-parse", "--short", "HEAD"])
                .stdin(Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| sanitize(String::from_utf8_lossy(&o.stdout).trim()))
                .unwrap_or_default();
            Outcome::Committed { short }
        }
    }
}

/// A short, safe line about a failed commit for the status bar.
pub fn failure_status(code: Option<i32>, output: &str) -> String {
    let last = output
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(tidy_output)
        .unwrap_or_default();
    let exit = code.map_or("killed by a signal".to_string(), |c| {
        format!("exit status {c}")
    });
    if last.is_empty() {
        format!(
            "Commit failed ({exit}). Nothing was committed; fix the problem and press y to retry."
        )
    } else {
        format!("Commit failed ({exit}): {last}")
    }
}

// ----------------------------------------------------------------- draft

/// Read `{subject, body}` from the agent into a normalised message.
pub fn parse_draft(v: &Value) -> Result<String, String> {
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
    let subject = sanitize_agent_text(get("subject")).replace('\n', " ");
    let subject = subject.trim().to_string();
    if subject.is_empty() {
        return Err("the agent returned no subject".into());
    }
    let body = get("body");
    let msg = normalize_message(&format!("{subject}\n\n{body}"));
    if msg.is_empty() {
        Err("the agent returned an empty message".into())
    } else {
        Ok(msg)
    }
}

/// Ask the agent for a commit message (read-only tools, no web), giving up
/// after `timeout`.
pub async fn draft_message(
    adapter: Arc<dyn AgentAdapter>,
    session: SessionRef,
    input: DraftInput,
    timeout: Duration,
) -> Result<String, String> {
    let repo = input.repo.clone();
    let recent = tokio::task::spawn_blocking(move || recent_subjects(&repo))
        .await
        .unwrap_or_default();
    let prompt = prompts::commit_message_prompt(&input.files, &input.units, &recent);
    let schema = prompts::commit_message_schema();
    match tokio::time::timeout(timeout, adapter.ask_dive(&session, &prompt, &schema, false)).await {
        Err(_) => Err(format!("timed out after {}s", timeout.as_secs())),
        Ok(Err(e)) => Err(tidy_output(&format!("{e:#}"))),
        Ok(Ok(v)) => parse_draft(&v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_shapes_subject_and_body() {
        assert_eq!(normalize_message(""), "");
        assert_eq!(normalize_message("  \n\n"), "");
        assert_eq!(normalize_message("Fix it\n"), "Fix it");
        assert_eq!(
            normalize_message("\n\nFix it  \n\n\n\n- a\n\n\n- b   \n\n"),
            "Fix it\n\n- a\n\n- b"
        );
        assert_eq!(
            normalize_message("Fix\nbody directly"),
            "Fix\n\nbody directly"
        );
        let long = "x".repeat(100);
        assert_eq!(normalize_message(&long).chars().count(), SUBJECT_MAX);
        let huge = format!("s\n\n{}", "y".repeat(9000));
        assert!(normalize_message(&huge).chars().count() <= MESSAGE_MAX);
        assert_eq!(
            normalize_message("a\u{1b}[31mb\u{7}c\r\nd\u{1b}]0;title\u{7}e"),
            "abc\n\nde"
        );
    }

    #[test]
    fn template_names_files_and_bullets() {
        let p = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(template_message(&p(&["a.rs"]), &[]), "Update a.rs");
        assert_eq!(
            template_message(
                &p(&["a.rs", "b.rs"]),
                &p(&["Does x.", "Does x.", "Does y."])
            ),
            "Update a.rs, b.rs\n\n- Does x.\n- Does y."
        );
        assert_eq!(
            template_message(&p(&["a", "b", "c", "d", "e"]), &[]),
            "Update a, b and 3 more files"
        );
        let long = "d/".repeat(60) + "f.rs";
        let m = template_message(std::slice::from_ref(&long), &[]);
        assert!(m.chars().count() <= SUBJECT_MAX, "{m}");
        let many: Vec<String> = (0..30).map(|i| format!("s{i}")).collect();
        assert_eq!(
            template_message(&p(&["a"]), &many).lines().count(),
            2 + BULLETS_MAX
        );
    }

    #[test]
    fn draft_is_parsed_and_cleaned() {
        let v = serde_json::json!({"subject": "Add \u{1b}[1mthing", "body": "- one\n- two"});
        assert_eq!(parse_draft(&v).unwrap(), "Add thing\n\n- one\n- two");
        assert!(parse_draft(&serde_json::json!({"subject": "  ", "body": "x"})).is_err());
        assert_eq!(
            parse_draft(&serde_json::json!({"subject": "S", "body": ""})).unwrap(),
            "S"
        );
    }

    #[test]
    fn failure_status_is_one_short_line() {
        let s = failure_status(Some(1), "hook said\nlint failed \u{1b}[31mbadly\n\n");
        assert!(
            s.contains("exit status 1") && s.contains("lint failed"),
            "{s}"
        );
        assert!(!s.contains('\u{1b}') && !s.contains('\n'));
        assert!(failure_status(None, "").contains("signal"));
    }
}
