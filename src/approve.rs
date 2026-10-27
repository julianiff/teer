//! Posting a review of a pull request on GitHub: approving it from the "all
//! reviewed" popup, or requesting changes or commenting from the notes.
//!
//! The user's notes on units become inline review comments (when their line
//! is part of the PR diff) or lines of the review summary. The review is
//! posted with `gh api` (JSON on stdin, never in argv), pinned to the head
//! commit that was reviewed, and only after the remote still reports that
//! same head. Nothing here runs a shell.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use serde_json::json;

use crate::gh::{self, Output as GhOut};
use crate::git::ChangedFile;
use crate::pr;
use crate::review::model::{PrInfo, Review, Side, Unit};
use crate::util::{DiffOp, diff_lines, sanitize};

/// First line of every approval summary (editable by the user).
pub const DEFAULT_INTRO: &str = "Reviewed with teer.";
/// Longest summary, in characters.
pub const BODY_MAX_CHARS: usize = 4000;
/// Most lines of notes listed in a summary.
pub const BODY_MAX_LINES: usize = 40;
/// Longest single inline comment, in characters.
pub const INLINE_MAX_CHARS: usize = 2000;
/// Most inline comments in one review; the rest go into the summary.
pub const INLINE_MAX: usize = 50;

/// The summary's first line for `review`: [`DEFAULT_INTRO`], and the files
/// teer couldn't show, so the approval doesn't claim more than was seen.
pub fn intro_for(review: &Review) -> String {
    let name = |f: &crate::git::SkippedFile| format!("{} ({})", f.path.display(), f.why);
    match review.not_shown(name) {
        Some(note) => clamp_text(&format!("{DEFAULT_INTRO} {note}."), BODY_MAX_CHARS),
        None => DEFAULT_INTRO.to_string(),
    }
}

/// The programs teer starts for this feature. Overridable so tests never
/// touch a real `gh`, browser or editor.
#[derive(Debug, Clone)]
pub struct Tools {
    pub gh: OsString,
    pub opener: OsString,
    pub editor: OsString,
    /// Claude Code, which the notes can be handed to.
    pub claude: OsString,
}

impl Default for Tools {
    fn default() -> Self {
        Tools {
            gh: crate::gh::program(),
            opener: if cfg!(target_os = "macos") {
                "open".into()
            } else {
                "xdg-open".into()
            },
            editor: "nvim".into(),
            claude: crate::agent::commands::CLAUDE_BIN.into(),
        }
    }
}

/// What a posted review says about the pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    RequestChanges,
    Comment,
}

impl Verdict {
    /// GitHub's name for it.
    pub fn event(self) -> &'static str {
        match self {
            Verdict::Approve => "APPROVE",
            Verdict::RequestChanges => "REQUEST_CHANGES",
            Verdict::Comment => "COMMENT",
        }
    }

    /// `Approve PR #7`, `Request changes on PR #7`, `Comment on PR #7`.
    pub fn ask(self, n: u32) -> String {
        match self {
            Verdict::Approve => format!("Approve PR #{n}"),
            Verdict::RequestChanges => format!("Request changes on PR #{n}"),
            Verdict::Comment => format!("Comment on PR #{n}"),
        }
    }

    /// `Approving PR #7`, while it is posted.
    pub fn doing(self, n: u32) -> String {
        match self {
            Verdict::Approve => format!("Approving PR #{n}"),
            Verdict::RequestChanges => format!("Requesting changes on PR #{n}"),
            Verdict::Comment => format!("Commenting on PR #{n}"),
        }
    }

    /// `PR #7 approved`, after it was posted.
    pub fn done(self, n: u32) -> String {
        match self {
            Verdict::Approve => format!("PR #{n} approved"),
            Verdict::RequestChanges => format!("Changes requested on PR #{n}"),
            Verdict::Comment => format!("Review comment posted on PR #{n}"),
        }
    }

    /// The label of the key that switches to the next verdict.
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Approve => "approve",
            Verdict::RequestChanges => "request changes",
            Verdict::Comment => "comment",
        }
    }
}

// ------------------------------------------------------------------- text

/// Strip control characters (keeping newlines), drop `\r`, turn tabs into
/// spaces, and cap the length. Used on everything that is posted.
pub fn clamp_text(s: &str, max_chars: usize) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| *c != '\r')
        .map(|c| match c {
            '\t' => ' ',
            '\n' => '\n',
            c if c.is_control() || c == '\u{2028}' || c == '\u{2029}' => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_end();
    cleaned
        .chars()
        .take(max_chars)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Make text from gh or git safe to show in the status line: one line, no
/// control characters, no URL credentials, no token-looking words, capped.
pub fn tidy_output(s: &str) -> String {
    let words: Vec<String> = s
        .split_whitespace()
        .map(|w| {
            let bare = w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
            let token = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"]
                .iter()
                .any(|p| bare.starts_with(p));
            if token {
                "[redacted]".to_string()
            } else if w.contains("://") {
                pr::display_url(w)
            } else {
                sanitize(w)
            }
        })
        .collect();
    let line = words.join(" ");
    if line.chars().count() > 300 {
        let cut: String = line.chars().take(299).collect();
        format!("{cut}…")
    } else {
        line
    }
}

// ----------------------------------------------------------- line mapping

/// Where a note on `unit` can be attached as an inline comment: the first
/// added line inside the unit's new range (RIGHT side of the diff), else the
/// first removed line inside its old range (LEFT). `None` when the unit has
/// no changed line in the diff, the diff is too big to compute here, or the
/// file was renamed and the line is on the old side.
///
/// The diff is taken between the stored old and new contents of the file,
/// which is the PR diff (merge base .. head). It can differ from git's own
/// hunks in how it aligns moved lines; GitHub's answer for such a line is
/// handled by the caller (see [`approve`]).
pub fn inline_target(file: &ChangedFile, unit: &Unit) -> Option<(u32, Side)> {
    let old: Vec<&str> = file
        .old_content
        .as_deref()
        .map(|s| s.lines().collect())
        .unwrap_or_default();
    let new: Vec<&str> = file
        .new_content
        .as_deref()
        .map(|s| s.lines().collect())
        .unwrap_or_default();
    let (n, m) = (old.len(), new.len());
    let mut pre = 0;
    while pre < n.min(m) && old[pre] == new[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < n - pre && suf < m - pre && old[n - 1 - suf] == new[m - 1 - suf] {
        suf += 1;
    }
    let (om, nm) = (&old[pre..n - suf], &new[pre..m - suf]);
    if om.len().saturating_mul(nm.len()) > 4_000_000 {
        return None;
    }
    let ops = diff_lines(om, nm);
    if let Some(r) = unit.new_range
        && let Some(line) = ops
            .iter()
            .filter(|(op, _, _)| *op == DiffOp::Insert)
            .map(|(_, _, j)| (pre + j + 1) as u32)
            .find(|l| r.contains(*l))
    {
        return Some((line, Side::New));
    }
    if file.old_path.is_some() {
        return None;
    }
    let r = unit.old_range?;
    ops.iter()
        .filter(|(op, _, _)| *op == DiffOp::Delete)
        .map(|(_, i, _)| (pre + i + 1) as u32)
        .find(|l| r.contains(*l))
        .map(|l| (l, Side::Old))
}

// ------------------------------------------------------------------ plan

/// One unit's note, in list order, with where it can go.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteEntry {
    pub symbol: String,
    pub file: String,
    pub line: u32,
    pub note: String,
    /// Inline target; `None` means the note can only go into the summary.
    pub target: Option<(u32, Side)>,
}

/// An inline review comment.
#[derive(Debug, Clone, PartialEq)]
pub struct Inline {
    pub path: String,
    pub line: u32,
    pub side: Side,
    pub body: String,
    pub symbol: String,
}

/// What would be posted.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The review summary: the intro plus the notes that are not inline.
    pub body: String,
    pub inline: Vec<Inline>,
}

fn safe_path(p: &Path) -> Option<String> {
    let s = p.to_str()?;
    let ok = !s.is_empty()
        && !p.is_absolute()
        && p.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
    ok.then(|| s.replace('\\', "/"))
}

/// Every unit with a note (whatever its decision), in the given order.
pub fn collect_notes(review: &Review, order: &[usize]) -> Vec<NoteEntry> {
    // With a guessed base the diff is not the PR's: never inline.
    let guessed = review.pr.as_ref().is_some_and(|p| p.base_assumed.is_some());
    order
        .iter()
        .filter_map(|&i| review.units.get(i))
        .filter_map(|u| {
            let note = clamp_text(&u.dive.note.replace('\n', " "), INLINE_MAX_CHARS);
            let note = note.trim().to_string();
            if note.is_empty() {
                return None;
            }
            let path = safe_path(&u.file);
            let target = match (&path, guessed) {
                (Some(_), false) => review.file_of(u).and_then(|f| inline_target(f, u)),
                _ => None,
            };
            Some(NoteEntry {
                symbol: sanitize(&u.symbol),
                file: path.unwrap_or_else(|| sanitize(&u.file.to_string_lossy())),
                line: u.display_line(),
                note,
                target,
            })
        })
        .collect()
}

fn note_line(e: &NoteEntry) -> String {
    format!("- {} ({}:{}): {}", e.symbol, e.file, e.line, e.note)
}

fn compose_body(intro: &str, listed: &[&NoteEntry]) -> String {
    let intro = clamp_text(intro, BODY_MAX_CHARS);
    let intro = if intro.trim().is_empty() {
        DEFAULT_INTRO.to_string()
    } else {
        intro
    };
    if listed.is_empty() {
        return intro;
    }
    let room = BODY_MAX_LINES
        .saturating_sub(intro.lines().count() + 2)
        .max(1);
    let mut out = intro;
    out.push('\n');
    for e in listed.iter().take(room) {
        out.push('\n');
        out.push_str(&note_line(e));
    }
    if listed.len() > room {
        out.push_str(&format!("\n- … and {} more notes", listed.len() - room));
    }
    clamp_text(&out, BODY_MAX_CHARS)
}

/// The review to post: with `inline_on`, notes that have a target become
/// inline comments (at most [`INLINE_MAX`]); everything else is listed in the
/// summary.
pub fn build_plan(notes: &[NoteEntry], intro: &str, inline_on: bool) -> Plan {
    let mut inline = Vec::new();
    let mut listed = Vec::new();
    for e in notes {
        match e.target {
            Some((line, side)) if inline_on && inline.len() < INLINE_MAX => inline.push(Inline {
                path: e.file.clone(),
                line,
                side,
                body: e.note.clone(),
                symbol: e.symbol.clone(),
            }),
            _ => listed.push(e),
        }
    }
    Plan {
        body: compose_body(intro, &listed),
        inline,
    }
}

/// The JSON for `POST /repos/{owner}/{repo}/pulls/{n}/reviews`.
pub fn review_json(commit: &str, verdict: Verdict, body: &str, inline: &[Inline]) -> String {
    let mut v = json!({"commit_id": commit, "event": verdict.event(), "body": body});
    if !inline.is_empty() {
        v["comments"] = inline
            .iter()
            .map(|c| {
                json!({
                    "path": c.path,
                    "line": c.line,
                    "side": if c.side == Side::New { "RIGHT" } else { "LEFT" },
                    "body": c.body,
                })
            })
            .collect();
    }
    v.to_string()
}

/// The PR's page, rebuilt from validated parts.
pub fn pr_page_url(p: &PrInfo) -> String {
    let host = match p.port {
        Some(port) => format!("{}:{port}", p.host),
        None => p.host.clone(),
    };
    format!("https://{host}/{}/{}/pull/{}", p.owner, p.repo, p.number)
}

// ---------------------------------------------------------------- editing

/// How an external edit of the summary ended.
#[derive(Debug, Clone, PartialEq)]
pub enum EditOutcome {
    Edited(String),
    /// The editor ran but asked to discard (non-zero exit).
    Cancelled,
    /// The editor could not be started or the file could not be prepared.
    Unavailable(String),
}

/// Run `editor` on a private temp file named `name` (in its own directory
/// inside teer's data dir) holding `text`; the file's raw content afterwards,
/// or `None` when the editor exited non-zero. The caller owns the terminal
/// handover. The directory and file are deleted on return.
fn run_editor(editor: &OsStr, text: &str, name: &str) -> Result<Option<String>, String> {
    let dir = crate::review::store::data_dir().join("tmp");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{e}"))?;
    let scratch = tempfile::Builder::new()
        .prefix("edit-")
        .tempdir_in(&dir)
        .map_err(|e| format!("{e}"))?;
    let path = scratch.path().join(name);
    std::fs::write(&path, text.as_bytes())
        .map_err(|_| "couldn't write the temp file".to_string())?;
    let status = std::process::Command::new(editor)
        .arg(&path)
        .status()
        .map_err(|e| format!("{e}"))?;
    if !status.success() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("{e}"))?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    // `scratch` drops here and the temp directory is deleted.
}

/// Edit `text` in `editor` on a private temp file inside teer's data dir,
/// then read it back (sanitised and capped) and delete the file. The caller
/// owns the terminal handover.
pub fn edit_in_editor(editor: &OsStr, text: &str) -> EditOutcome {
    match run_editor(editor, text, "approve.md") {
        Err(why) => EditOutcome::Unavailable(why),
        Ok(None) => EditOutcome::Cancelled,
        Ok(Some(t)) => EditOutcome::Edited(clamp_text(&t, BODY_MAX_CHARS)),
    }
}

/// Like [`edit_in_editor`] for a commit message: the file is named
/// `COMMIT_EDITMSG` (so editors pick git's syntax) and the result is
/// normalised as a commit message.
pub fn edit_message_in_editor(editor: &OsStr, text: &str) -> EditOutcome {
    match run_editor(editor, text, "COMMIT_EDITMSG") {
        Err(why) => EditOutcome::Unavailable(why),
        Ok(None) => EditOutcome::Cancelled,
        Ok(Some(t)) => EditOutcome::Edited(crate::commit::normalize_message(&t)),
    }
}

// ---------------------------------------------------------------- sending

async fn run_gh(tools: &Tools, cwd: &Path, args: &[String], stdin: &str) -> Result<GhOut, String> {
    gh::run(&tools.gh, cwd, args, Some(stdin))
        .await
        .map_err(|e| e.to_string())
}

fn gh_args(p: &PrInfo) -> Vec<String> {
    let mut a: Vec<String> = ["api", "--method", "POST"].map(String::from).to_vec();
    if p.host != "github.com" || p.port.is_some() {
        a.push("--hostname".into());
        a.push(match p.port {
            Some(port) => format!("{}:{port}", p.host),
            None => p.host.clone(),
        });
    }
    a.push(format!(
        "repos/{}/{}/pulls/{}/reviews",
        p.owner, p.repo, p.number
    ));
    a.extend(["--input", "-"].map(String::from));
    a
}

/// GitHub refused the inline comments (a line outside the diff): 422 and a
/// message about lines, paths or the diff. Other 422s (such as approving your
/// own PR) are not retried.
fn rejected_comment_lines(out: &GhOut) -> bool {
    let all = format!("{} {}", out.stderr, out.stdout).to_ascii_lowercase();
    (all.contains("422") || all.contains("unprocessable") || all.contains("validation failed"))
        && [
            "line",
            "diff",
            "path",
            "position",
            "review_thread",
            "comments",
        ]
        .iter()
        .any(|k| all.contains(k))
}

fn failure_text(out: &GhOut) -> String {
    let t = tidy_output(if out.stderr.trim().is_empty() {
        &out.stdout
    } else {
        &out.stderr
    });
    if t.is_empty() {
        format!(
            "exit status {}",
            out.code.map_or("unknown".into(), |c| c.to_string())
        )
    } else {
        t
    }
}

/// Start `tools.opener` (open / xdg-open) on a URL or a file, detached: its
/// output is discarded and it is reaped in the background. Needs the tokio
/// runtime. Whether it could be started.
pub fn open_detached(tools: &Tools, target: &OsStr) -> bool {
    use std::process::Stdio;
    match tokio::process::Command::new(&tools.opener)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
            true
        }
        Err(_) => false,
    }
}

async fn fallback(tools: &Tools, p: &PrInfo, why: &str) -> String {
    if open_detached(tools, OsStr::new(&pr_page_url(p))) {
        format!("gh failed: {why}; opened the PR page instead")
    } else {
        format!("gh failed: {why}; couldn't open the PR page either")
    }
}

/// Post a review with `verdict` on the PR. `Ok` carries the status line to
/// show; `Err` the message for a failure (already including the browser
/// fallback where it applies).
///
/// 1. every value from `pr` is validated again;
/// 2. the remote's head must still be the reviewed one, else nothing is sent;
/// 3. `gh api` posts the review (JSON on stdin), pinned to the reviewed head;
/// 4. if GitHub rejects the inline comments, once more with a summary only;
/// 5. any other failure opens the PR page and says so.
pub async fn post(
    tools: &Tools,
    pr_info: &PrInfo,
    verdict: Verdict,
    body: String,
    inline: Vec<Inline>,
    flat_body: String,
) -> Result<String, String> {
    let n = pr_info.number;
    let (Some(remote), Some(head), Some(repo)) = (
        pr_info.remote.clone(),
        pr_info.head_sha.clone(),
        pr_info.repo_dir.clone(),
    ) else {
        return Err(format!(
            "Can't post to PR #{n}: teer doesn't know its remote or head."
        ));
    };
    if !pr::info_is_valid(pr_info) || !pr::is_full_sha(&head) {
        return Err(format!("Can't post to PR #{n}: invalid pull request data."));
    }
    // Safety guard: never judge commits that were not seen.
    let (r2, remote2) = (repo.clone(), remote.clone());
    let now = tokio::task::spawn_blocking(move || pr::remote_pr_head(&r2, &remote2, n))
        .await
        .map_err(|e| format!("{e}"))
        .and_then(|r| r);
    match now {
        Err(why) => {
            return Err(format!(
                "Could not verify the PR head, nothing was sent: {}",
                tidy_output(&why)
            ));
        }
        Ok(now) if !now.eq_ignore_ascii_case(&head) => {
            return Err(format!(
                "PR #{n} has new commits since you reviewed it (reviewed {}, now {}). Run teer pr again.",
                &head[..7],
                &now[..7.min(now.len())]
            ));
        }
        Ok(_) => {}
    }

    let args = gh_args(pr_info);
    let first = run_gh(
        tools,
        &repo,
        &args,
        &review_json(&head, verdict, &body, &inline),
    )
    .await;
    let out = match first {
        Err(why) => return Err(fallback(tools, pr_info, &why).await),
        Ok(o) => o,
    };
    let done = verdict.done(n);
    if out.ok {
        return Ok(format!("{done} on GitHub."));
    }
    if !inline.is_empty() && rejected_comment_lines(&out) {
        let flat = review_json(&head, verdict, &flat_body, &[]);
        let second = run_gh(tools, &repo, &args, &flat).await;
        return match second {
            Ok(o) if o.ok => Ok(format!(
                "{done} on GitHub; inline comments were rejected by GitHub; posted with a summary comment instead."
            )),
            Ok(o) => Err(fallback(tools, pr_info, &failure_text(&o)).await),
            Err(why) => Err(fallback(tools, pr_info, &why).await),
        };
    }
    Err(fallback(tools, pr_info, &failure_text(&out)).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::FileStatus;
    use crate::review::model::{Decision, Prep, Range, UnitKind, UnitStatus};
    use std::path::PathBuf;

    fn file(old: Option<&str>, new: Option<&str>) -> ChangedFile {
        ChangedFile {
            path: PathBuf::from("a.rs"),
            old_path: None,
            status: FileStatus::Modified,
            old_content: old.map(String::from),
            new_content: new.map(String::from),
        }
    }

    fn unit(status: UnitStatus, old: Option<(u32, u32)>, new: Option<(u32, u32)>) -> Unit {
        Unit {
            id: "a.rs::f".into(),
            file: PathBuf::from("a.rs"),
            symbol: "f".into(),
            kind: UnitKind::Function,
            status,
            old_range: old.map(|r| Range::new(r.0, r.1)),
            new_range: new.map(|r| Range::new(r.0, r.1)),
            renamed_from: None,
            fingerprint: String::new(),
            changes: Vec::new(),
            prep: Prep::Skipped,
            callers: Vec::new(),
            callees: Vec::new(),
            decision: Decision::Accepted,
            summary: None,
            nbc_reason: None,
            dive: Default::default(),
            dive_pending: Default::default(),
            checks: Default::default(),
        }
    }

    #[test]
    fn added_unit_maps_to_its_first_line() {
        let f = file(Some("x\n"), Some("x\nfn f() {\n  1\n}\n"));
        let u = unit(UnitStatus::Added, None, Some((2, 4)));
        assert_eq!(inline_target(&f, &u), Some((2, Side::New)));
    }

    #[test]
    fn modified_unit_maps_to_the_first_changed_line() {
        let f = file(
            Some("fn f() {\n  1\n  2\n}\n"),
            Some("fn f() {\n  1\n  3\n}\n"),
        );
        let u = unit(UnitStatus::Changed, Some((1, 4)), Some((1, 4)));
        assert_eq!(inline_target(&f, &u), Some((3, Side::New)));
    }

    #[test]
    fn removed_unit_maps_to_the_old_side() {
        let f = file(Some("a\nfn f() {\n}\nb\n"), Some("a\nb\n"));
        let u = unit(UnitStatus::Removed, Some((2, 3)), None);
        assert_eq!(inline_target(&f, &u), Some((2, Side::Old)));
    }

    #[test]
    fn deletion_only_change_inside_a_changed_unit_goes_left() {
        let f = file(Some("fn f() {\n  1\n  2\n}\n"), Some("fn f() {\n  1\n}\n"));
        let u = unit(UnitStatus::Changed, Some((1, 4)), Some((1, 3)));
        assert_eq!(inline_target(&f, &u), Some((3, Side::Old)));
    }

    #[test]
    fn several_hunks_pick_the_hunk_inside_the_unit() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\nfn f() {\n  1\n}\n";
        let new = "A\nb\nc\nd\ne\nf\ng\nh\nfn f() {\n  2\n}\n";
        let f = file(Some(old), Some(new));
        // The first hunk (line 1) is outside the unit.
        let u = unit(UnitStatus::Changed, Some((9, 11)), Some((9, 11)));
        assert_eq!(inline_target(&f, &u), Some((10, Side::New)));
        let top = unit(UnitStatus::Changed, Some((1, 2)), Some((1, 2)));
        assert_eq!(inline_target(&f, &top), Some((1, Side::New)));
    }

    #[test]
    fn unit_without_a_changed_line_has_no_target() {
        let f = file(Some("a\nb\nc\n"), Some("a\nB\nc\n"));
        let u = unit(UnitStatus::Changed, Some((3, 3)), Some((3, 3)));
        assert_eq!(inline_target(&f, &u), None);
        let same = file(Some("a\n"), Some("a\n"));
        assert_eq!(
            inline_target(
                &same,
                &unit(UnitStatus::Changed, Some((1, 1)), Some((1, 1)))
            ),
            None
        );
    }

    #[test]
    fn crlf_lines_are_compared_without_the_carriage_return() {
        let f = file(Some("a\r\nb\r\n"), Some("a\r\nB\r\n"));
        let u = unit(UnitStatus::Changed, Some((1, 2)), Some((1, 2)));
        assert_eq!(inline_target(&f, &u), Some((2, Side::New)));
        // Only the line endings differ: lines compare equal, so no target (the
        // note goes into the summary).
        let g = file(Some("a\nb\n"), Some("a\r\nb\r\n"));
        assert_eq!(inline_target(&g, &u), None);
    }

    #[test]
    fn renamed_files_map_the_new_side_but_not_the_old_one() {
        let mut f = file(Some("a\nfn f() {\n}\n"), Some("a\nfn g() {\n}\n"));
        f.old_path = Some(PathBuf::from("old.rs"));
        let u = unit(UnitStatus::Changed, Some((2, 3)), Some((2, 3)));
        assert_eq!(inline_target(&f, &u), Some((2, Side::New)));
        let mut f = file(Some("a\nfn f() {\n}\n"), Some("a\n"));
        f.old_path = Some(PathBuf::from("old.rs"));
        let u = unit(UnitStatus::Removed, Some((2, 3)), None);
        assert_eq!(inline_target(&f, &u), None);
    }

    #[test]
    fn json_escapes_quotes_newlines_and_unicode() {
        let inl = Inline {
            path: "a.rs".into(),
            line: 3,
            side: Side::New,
            body: "say \"hi\"\nnew line \\ ✓ 日本".into(),
            symbol: "f".into(),
        };
        let j = review_json(
            "abc",
            Verdict::Approve,
            "sum \"x\"\nline",
            std::slice::from_ref(&inl),
        );
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["body"], "sum \"x\"\nline");
        assert_eq!(v["comments"][0]["body"], inl.body);
        assert_eq!(v["comments"][0]["side"], "RIGHT");
        assert_eq!(v["event"], "APPROVE");
        assert!(
            review_json("abc", Verdict::RequestChanges, "b", &[])
                .find("comments")
                .is_none()
        );
    }

    #[test]
    fn tidy_output_hides_credentials_and_tokens() {
        let t =
            tidy_output("fail https://me:pw@github.com/a/b token ghp_abcdef123 \u{1b}[31mred\n");
        assert!(!t.contains("me:pw") && !t.contains("ghp_") && !t.contains('\u{1b}'));
        assert!(t.contains("https://github.com/a/b"));
        assert!(tidy_output(&"x ".repeat(500)).chars().count() <= 300);
    }

    #[test]
    fn plan_lists_notes_without_a_target_and_caps_inline() {
        let mk = |i: u32, t: bool| NoteEntry {
            symbol: format!("s{i}"),
            file: "a.rs".into(),
            line: i,
            note: format!("n{i}"),
            target: t.then_some((i, Side::New)),
        };
        let notes: Vec<_> = (1..=60).map(|i| mk(i, i != 2)).collect();
        let p = build_plan(&notes, DEFAULT_INTRO, true);
        assert_eq!(p.inline.len(), INLINE_MAX);
        assert!(p.body.starts_with(DEFAULT_INTRO));
        assert!(p.body.contains("- s2 (a.rs:2): n2"));
        assert!(p.body.contains("- s52 (a.rs:52): n52"));
        assert!(!p.body.contains("s3 ("));
        let off = build_plan(&notes, DEFAULT_INTRO, false);
        assert!(off.inline.is_empty());
        assert!(off.body.lines().count() <= BODY_MAX_LINES);
        assert!(off.body.contains("more notes"));
        assert_eq!(build_plan(&[], "", true).body, DEFAULT_INTRO);
    }
}
