//! Posting a review of a pull request on GitHub from the `:w` sheet (see
//! `tui::finish`): approving it, requesting changes, or commenting.
//!
//! The user's notes on units become inline review comments (when their line
//! is part of the PR diff) or lines of the review summary. A flagged unit's
//! standing security findings go under its inline note, and the summary
//! gets a "Security" section with the findings and the facts that need a
//! close look of every flagged unit. A hunk of a proposed change the
//! reviewer suggested (`p` on a pull request) becomes a GitHub suggestion
//! on its lines when they are in the PR diff, merged into the unit's note
//! when that lands on them; otherwise it goes into the summary's "Proposed
//! changes" as a diff. The review is posted with `gh api` (JSON on stdin,
//! never in argv), pinned to the head commit that was reviewed, and only
//! after the remote still reports that same head. Nothing here runs a
//! shell.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::Path;

use serde_json::json;

use crate::editor::Editor;
use crate::fix::{HunkState, ProposedHunk};
use crate::gh::{self, Output as GhOut};
use crate::git::ChangedFile;
use crate::pr;
use crate::review::model::{Decision, PrInfo, Range, Review, Side, Unit};
use crate::security::Weight;
use crate::security::order::{
    cite_text, class_name, fact_place, facts_from, one_line, path_text, standing,
};
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
/// Most lines of the summary's Security section.
pub const SECURITY_MAX_LINES: usize = 15;
/// Longest "Proposed changes" section, in characters, on top of the
/// summary's [`BODY_MAX_CHARS`]: the code isn't cut to fit the notes.
pub const PROPOSED_MAX_CHARS: usize = 4000;
/// Longest suggestion comment, in characters; a longer one goes into the
/// summary as a diff.
pub const SUGGESTION_MAX_CHARS: usize = 8000;
/// The unchanged lines a diff hunk shows around a change, as git and
/// GitHub show them.
pub const DIFF_CONTEXT: usize = 3;

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
    pub editor: Editor,
    /// Claude Code and Codex, which the notes can be handed to.
    pub claude: OsString,
    pub codex: OsString,
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
            editor: Editor::default(),
            claude: crate::agent::commands::CLAUDE_BIN.into(),
            codex: crate::agent::commands::CODEX_BIN.into(),
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

/// Code to post: [`clamp_text`]'s cleaning without its cap, keeping tabs
/// and leading or trailing whitespace, since a suggestion replaces lines
/// with exactly this text.
pub fn clean_code(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '\r')
        .map(|c| match c {
            '\t' | '\n' => c,
            c if c.is_control() || c == '\u{2028}' || c == '\u{2029}' => ' ',
            c => c,
        })
        .collect()
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

/// The lines of `file`'s new side the PR diff shows, hunk by hunk, as
/// [`inline_target`]'s diff aligns them: the added lines and the
/// [`DIFF_CONTEXT`] unchanged lines around each change, a hunk running on
/// while the gap to the next change is no more than twice that, as git
/// cuts them. A comment, one line or several, can only go on lines of one
/// hunk. Empty when the file is gone or the diff is too big to compute
/// here.
pub fn right_hunks(file: &ChangedFile) -> Vec<Range> {
    use DiffOp::{Delete, Equal};
    let old: Vec<&str> = file
        .old_content
        .as_deref()
        .map(|s| s.lines().collect())
        .unwrap_or_default();
    let Some(new) = file.new_content.as_deref() else {
        return Vec::new();
    };
    let new: Vec<&str> = new.lines().collect();
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
        return Vec::new();
    }
    // Every line of the file as (op, its 0-based line on the new side).
    let ops: Vec<(DiffOp, usize)> = (0..pre)
        .map(|j| (Equal, j))
        .chain(
            diff_lines(om, nm)
                .into_iter()
                .map(|(op, _, j)| (op, pre + j)),
        )
        .chain((m - suf..m).map(|j| (Equal, j)))
        .collect();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (k, _) in ops.iter().enumerate().filter(|(_, (op, _))| *op != Equal) {
        let (a, b) = (
            k.saturating_sub(DIFF_CONTEXT),
            (k + DIFF_CONTEXT).min(ops.len() - 1),
        );
        match spans.last_mut() {
            Some(last) if a <= last.1 + 1 => last.1 = b,
            _ => spans.push((a, b)),
        }
    }
    spans
        .into_iter()
        .filter_map(|(a, b)| {
            let mut lines = ops[a..=b]
                .iter()
                .filter(|(op, _)| *op != Delete)
                .map(|(_, j)| *j as u32 + 1);
            let first = lines.next()?;
            Some(Range::new(first, lines.next_back().unwrap_or(first)))
        })
        .collect()
}

/// Lines `lines` (1-based, inclusive) of `content`, the last one's ending
/// included. `None` when `content` doesn't have them.
fn whole_lines(content: &str, lines: Range) -> Option<&str> {
    if lines.start == 0 || lines.is_empty() {
        return None;
    }
    let mut starts = vec![0];
    starts.extend(
        content
            .bytes()
            .enumerate()
            .filter(|(_, b)| *b == b'\n')
            .map(|(i, _)| i + 1),
    );
    let count = if content.ends_with('\n') {
        starts.len() - 1
    } else {
        starts.len()
    };
    if lines.end as usize > count {
        return None;
    }
    let from = starts[lines.start as usize - 1];
    let to = starts
        .get(lines.end as usize)
        .copied()
        .unwrap_or(content.len());
    Some(&content[from..to])
}

/// `s` without one line ending at its end.
fn without_ending(s: &str) -> &str {
    let s = s.strip_suffix('\n').unwrap_or(s);
    s.strip_suffix('\r').unwrap_or(s)
}

/// Lines `lines` (1-based, inclusive) of `content`, whole, with `old`
/// replaced by `new` in them: what a suggestion on those lines puts there,
/// even when `old` starts or ends inside a line. Without the last line's
/// ending. `None` when the lines aren't in `content` or don't hold `old`
/// exactly once.
pub fn replaced_lines(content: &str, lines: Range, old: &str, new: &str) -> Option<String> {
    let whole = whole_lines(content, lines)?;
    if old.is_empty() || whole.matches(old).count() != 1 {
        return None;
    }
    let out = whole.replacen(old, new, 1);
    Some(without_ending(&out).to_string())
}

/// Hunk `h` as diff lines of whole lines, read from `content` (the file as
/// reviewed) when it has them; else of its own text.
fn whole_diff(content: Option<&str>, h: &ProposedHunk) -> Vec<String> {
    let whole = content.and_then(|c| {
        let old = without_ending(whole_lines(c, h.lines)?);
        let new = replaced_lines(c, h.lines, &h.old, &h.new)?;
        Some((old.to_string(), new))
    });
    let lines = match whole {
        Some((old, new)) => ProposedHunk {
            old,
            new,
            ..h.clone()
        }
        .diff_lines(),
        None => h.diff_lines(),
    };
    lines.iter().map(|l| clean_code(l)).collect()
}

// ------------------------------------------------------------------ plan

/// One unit's note, in list order, with where it can go.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteEntry {
    /// The unit's id: a suggestion of its own can join its comment.
    pub id: String,
    pub symbol: String,
    pub file: String,
    pub line: u32,
    pub note: String,
    /// Inline target; `None` means the note can only go into the summary.
    pub target: Option<(u32, Side)>,
    /// A flagged unit's standing security findings, a Markdown list item
    /// each, for under its inline note (see [`finding_lines`]).
    pub security: Vec<String>,
}

/// A suggested hunk of a unit's proposed change ([`HunkState::Suggested`]),
/// in list order, with where it can go.
#[derive(Debug, Clone, PartialEq)]
pub struct SuggestionEntry {
    /// The unit's id, to join its note's comment.
    pub id: String,
    pub symbol: String,
    /// The hunk's file, which can be another than the unit's.
    pub file: String,
    /// The lines of the head it replaces.
    pub lines: Range,
    /// Why this hunk, on one line.
    pub why: String,
    /// The hunk as diff lines, for the summary: of whole lines when the
    /// file is in the review.
    pub diff: Vec<String>,
    /// The comment's body when it can go inline: the why, then the
    /// suggestion. `None` when its lines aren't all in one hunk of the PR
    /// diff, or the replacement can't be built.
    pub inline: Option<String>,
}

/// An inline review comment.
#[derive(Debug, Clone, PartialEq)]
pub struct Inline {
    pub path: String,
    /// The comment's line, the last of its range when it has one.
    pub line: u32,
    /// The first line, for a comment on several lines.
    pub start_line: Option<u32>,
    pub side: Side,
    pub body: String,
    pub symbol: String,
    /// It suggests a change: the body holds a suggestion block.
    pub suggestion: bool,
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
                id: u.id.clone(),
                symbol: sanitize(&u.symbol),
                file: path.unwrap_or_else(|| sanitize(&u.file.to_string_lossy())),
                line: u.display_line(),
                note,
                target,
                security: finding_lines(u),
            })
        })
        .collect()
}

/// Every suggested hunk of the units' proposed changes, in `order` (then
/// the units it doesn't list), with its suggestion when it can go inline
/// ([`suggestion_body`]). Proposed hunks never suggested aren't posted.
pub fn collect_suggestions(review: &Review, order: &[usize]) -> Vec<SuggestionEntry> {
    // With a guessed base the diff is not the PR's: never inline.
    let guessed = review.pr.as_ref().is_some_and(|p| p.base_assumed.is_some());
    let listed: HashSet<usize> = order.iter().copied().collect();
    let units = order
        .iter()
        .copied()
        .chain((0..review.units.len()).filter(|i| !listed.contains(i)))
        .filter_map(|i| review.units.get(i));
    // Each file's diff hunks, worked out once.
    let mut hunks: HashMap<&Path, Vec<Range>> = HashMap::new();
    let mut out = Vec::new();
    for u in units {
        let Some(p) = crate::notes::proposal(u) else {
            continue;
        };
        for h in p.hunks.iter().filter(|h| h.state == HunkState::Suggested) {
            let path = safe_path(&h.file);
            let file = review.files.iter().find(|f| f.path == h.file);
            let inline = match (&path, file, guessed) {
                (Some(_), Some(f), false) => {
                    let shown = hunks
                        .entry(f.path.as_path())
                        .or_insert_with(|| right_hunks(f));
                    suggestion_body(f, shown, h)
                }
                _ => None,
            };
            out.push(SuggestionEntry {
                id: u.id.clone(),
                symbol: sanitize(&u.symbol),
                file: path.unwrap_or_else(|| sanitize(&h.file.to_string_lossy())),
                lines: h.lines,
                why: clamp_text(&one_line(&h.why), INLINE_MAX_CHARS),
                diff: whole_diff(file.and_then(|f| f.new_content.as_deref()), h),
                inline,
            });
        }
    }
    out
}

/// A suggested hunk's comment: its why, then a suggestion block holding
/// the whole lines it covers in `file`'s new side, its old text replaced
/// by the new. `None` when those lines aren't all in one of `shown` (the
/// file's diff hunks, [`right_hunks`]), the lines don't hold the old text
/// once, or the comment would be longer than [`SUGGESTION_MAX_CHARS`].
fn suggestion_body(file: &ChangedFile, shown: &[Range], h: &ProposedHunk) -> Option<String> {
    let r = h.lines;
    if !shown
        .iter()
        .any(|s| s.contains(r.start) && s.contains(r.end))
    {
        return None;
    }
    let text = replaced_lines(file.new_content.as_deref()?, r, &h.old, &h.new)?;
    let text = clean_code(&text);
    let fence = crate::notes::fence(&[&text]);
    let block = if text.is_empty() {
        format!("{fence}suggestion\n{fence}")
    } else {
        format!("{fence}suggestion\n{text}\n{fence}")
    };
    let why = clamp_text(&one_line(&h.why), INLINE_MAX_CHARS);
    let body = if why.is_empty() {
        block
    } else {
        format!("{why}\n\n{block}")
    };
    (body.chars().count() <= SUGGESTION_MAX_CHARS).then_some(body)
}

/// `text` in backticks, its own backticks made quotes.
fn code(text: &str) -> String {
    format!("`{}`", text.replace('`', "'"))
}

/// A flagged unit's standing security findings, most severe first, one
/// Markdown list item each: severity and class, the title, the path from
/// source to sink, what is missing. None when it isn't flagged.
pub fn finding_lines(u: &Unit) -> Vec<String> {
    if u.decision != Decision::Investigate {
        return Vec::new();
    }
    standing(u)
        .into_iter()
        .map(|f| {
            format!(
                "- **{} · {}**: {} Path: {}. Missing: {}",
                f.severity.word(),
                class_name(f.class),
                one_line(&f.title),
                code(&path_text(f)),
                one_line(&f.missing),
            )
        })
        .collect()
}

/// The summary's "Security" section: for every flagged unit, in `order`
/// (then those it doesn't list), its standing findings (severity, title,
/// where they do harm) and its facts that need a close look (class, text,
/// where). At most [`SECURITY_MAX_LINES`] lines. `None` when there is
/// nothing to say.
pub fn security_section(review: &Review, order: &[usize]) -> Option<String> {
    let listed: HashSet<usize> = order.iter().copied().collect();
    let flagged = order
        .iter()
        .copied()
        .chain((0..review.units.len()).filter(|i| !listed.contains(i)))
        .filter_map(|i| review.units.get(i))
        .filter(|u| u.decision == Decision::Investigate);
    let mut lines = Vec::new();
    for u in flagged {
        for f in standing(u) {
            lines.push(format!(
                "- **{}** · {}: {} ({})",
                f.severity.word(),
                class_name(f.class),
                one_line(&f.title),
                code(&cite_text(&f.sink)),
            ));
        }
        for f in facts_from(u, Weight::Attention) {
            lines.push(format!(
                "- **found by teer** · {}: {} ({})",
                class_name(f.class),
                one_line(&f.text),
                code(&fact_place(u, f)),
            ));
        }
    }
    if lines.is_empty() {
        return None;
    }
    if lines.len() > SECURITY_MAX_LINES {
        let more = lines.len() - (SECURITY_MAX_LINES - 1);
        lines.truncate(SECURITY_MAX_LINES - 1);
        lines.push(format!("- … and {more} more"));
    }
    Some(format!("### Security\n\n{}", lines.join("\n")))
}

/// The review to post for `review` listed in `order`: [`build_plan`] of its
/// notes ([`collect_notes`]) and suggested hunks ([`collect_suggestions`]),
/// the summary's [`security_section`] when there is one, then its
/// [`proposed_section`]. The notes give way to the security section when
/// the summary is too long for both; the proposed changes come on top.
pub fn plan(review: &Review, order: &[usize], intro: &str, inline_on: bool) -> Plan {
    let notes = collect_notes(review, order);
    let suggestions = collect_suggestions(review, order);
    let (mut plan, rest) = place(&notes, &suggestions, intro, inline_on);
    if let Some(section) = security_section(review, order) {
        let section = clamp_text(&section, BODY_MAX_CHARS / 2);
        let room = BODY_MAX_CHARS.saturating_sub(section.chars().count() + 2);
        let body = clamp_text(&plan.body, room);
        plan.body = format!("{body}\n\n{section}");
    }
    if let Some(section) = proposed_section(&rest) {
        plan.body = format!("{}\n\n{section}", plan.body);
    }
    plan
}

/// The summary's "Proposed changes": each suggested hunk that isn't
/// inline, its unit, place and why, then its diff. As many as fit in
/// [`PROPOSED_MAX_CHARS`], whole, then how many more there are. `None`
/// without any.
pub fn proposed_section(entries: &[&SuggestionEntry]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut out = String::from("### Proposed changes");
    let mut shown = 0;
    for e in entries {
        let place = if e.lines.end > e.lines.start {
            format!("{}:{}-{}", e.file, e.lines.start, e.lines.end)
        } else {
            format!("{}:{}", e.file, e.lines.start)
        };
        let why = if e.why.is_empty() {
            String::new()
        } else {
            format!(": {}", e.why)
        };
        let fence = crate::notes::fence(&e.diff);
        let block = format!(
            "\n\n- {} ({}){why}\n\n{fence}diff\n{}\n{fence}",
            e.symbol,
            code(&place),
            e.diff.join("\n")
        );
        // Room kept for the line saying how many more.
        if out.chars().count() + block.chars().count() > PROPOSED_MAX_CHARS - 60 {
            break;
        }
        out.push_str(&block);
        shown += 1;
    }
    if shown < entries.len() {
        let more = entries.len() - shown;
        let what = if shown == 0 {
            format!("{more} too long to show here")
        } else {
            format!("and {more} more")
        };
        out.push_str(&format!("\n\n- … {what}"));
    }
    Some(out)
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

/// An inline comment's body: the note, then the unit's security findings
/// under a "Security" line.
fn inline_body(e: &NoteEntry) -> String {
    if e.security.is_empty() {
        return e.note.clone();
    }
    let findings = e.security.join("\n");
    clamp_text(
        &format!("{}\n\n**Security** (teer)\n{findings}", e.note),
        INLINE_MAX_CHARS,
    )
}

/// The review to post: with `inline_on`, notes that have a target become
/// inline comments (at most [`INLINE_MAX`]), a flagged unit's security
/// findings under its note; everything else is listed in the summary.
pub fn build_plan(notes: &[NoteEntry], intro: &str, inline_on: bool) -> Plan {
    place(notes, &[], intro, inline_on).0
}

/// [`build_plan`] with suggested hunks: with `inline_on`, the notes go
/// first, then each suggestion that can go inline joins its unit's note
/// comment when that lands on its lines, or becomes a comment of its own
/// while there is room ([`INLINE_MAX`] counts both). The suggestions left
/// come back, for the summary's [`proposed_section`].
fn place<'a>(
    notes: &[NoteEntry],
    suggestions: &'a [SuggestionEntry],
    intro: &str,
    inline_on: bool,
) -> (Plan, Vec<&'a SuggestionEntry>) {
    let mut inline = Vec::new();
    // The unit of each inline comment.
    let mut of = Vec::new();
    let mut listed = Vec::new();
    for e in notes {
        match e.target {
            Some((line, side)) if inline_on && inline.len() < INLINE_MAX => {
                inline.push(Inline {
                    path: e.file.clone(),
                    line,
                    start_line: None,
                    side,
                    body: inline_body(e),
                    symbol: e.symbol.clone(),
                    suggestion: false,
                });
                of.push(e.id.as_str());
            }
            _ => listed.push(e),
        }
    }
    let mut rest = Vec::new();
    for s in suggestions {
        let Some(body) = s.inline.as_ref().filter(|_| inline_on) else {
            rest.push(s);
            continue;
        };
        let (start, end) = (s.lines.start, s.lines.end);
        let start_line = (start < end).then_some(start);
        let note = inline.iter_mut().zip(&of).find(|(c, id)| {
            **id == s.id
                && !c.suggestion
                && c.side == Side::New
                && c.path == s.file
                && s.lines.contains(c.line)
        });
        if let Some((c, _)) = note {
            c.body = format!("{}\n\n{body}", c.body);
            c.line = end;
            c.start_line = start_line;
            c.suggestion = true;
        } else if inline.len() < INLINE_MAX {
            inline.push(Inline {
                path: s.file.clone(),
                line: end,
                start_line,
                side: Side::New,
                body: body.clone(),
                symbol: s.symbol.clone(),
                suggestion: true,
            });
            of.push(s.id.as_str());
        } else {
            rest.push(s);
        }
    }
    let plan = Plan {
        body: compose_body(intro, &listed),
        inline,
    };
    (plan, rest)
}

/// The JSON for `POST /repos/{owner}/{repo}/pulls/{n}/reviews`. A comment
/// on several lines gets `start_line`, on the same side.
pub fn review_json(commit: &str, verdict: Verdict, body: &str, inline: &[Inline]) -> String {
    let mut v = json!({"commit_id": commit, "event": verdict.event(), "body": body});
    if !inline.is_empty() {
        v["comments"] = inline
            .iter()
            .map(|c| {
                let side = if c.side == Side::New { "RIGHT" } else { "LEFT" };
                let mut j = json!({
                    "path": c.path,
                    "line": c.line,
                    "side": side,
                    "body": c.body,
                });
                if let Some(start) = c.start_line {
                    j["start_line"] = json!(start);
                    j["start_side"] = json!(side);
                }
                j
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
fn run_editor(editor: &Editor, text: &str, name: &str) -> Result<Option<String>, String> {
    let dir = crate::review::store::data_dir().join("tmp");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{e}"))?;
    let scratch = tempfile::Builder::new()
        .prefix("edit-")
        .tempdir_in(&dir)
        .map_err(|e| format!("{e}"))?;
    let path = scratch.path().join(name);
    std::fs::write(&path, text.as_bytes())
        .map_err(|_| "couldn't write the temp file".to_string())?;
    let status = editor
        .command(&path, 1)
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
pub fn edit_in_editor(editor: &Editor, text: &str) -> EditOutcome {
    match run_editor(editor, text, "approve.md") {
        Err(why) => EditOutcome::Unavailable(why),
        Ok(None) => EditOutcome::Cancelled,
        Ok(Some(t)) => EditOutcome::Edited(clamp_text(&t, BODY_MAX_CHARS)),
    }
}

/// Text written somewhere else than the editor (a window's own text
/// field), made what [`edit_in_editor`] or, `for_commit`,
/// [`edit_message_in_editor`] makes of what the editor saved.
pub fn edited(text: &str, for_commit: bool) -> EditOutcome {
    EditOutcome::Edited(if for_commit {
        crate::commit::normalize_message(text)
    } else {
        clamp_text(text, BODY_MAX_CHARS)
    })
}

/// Like [`edit_in_editor`] for a commit message: the file is named
/// `COMMIT_EDITMSG` (so editors pick git's syntax) and the result is
/// normalised as a commit message.
pub fn edit_message_in_editor(editor: &Editor, text: &str) -> EditOutcome {
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
            accepted_by: None,
            reasons: Vec::new(),
            dive_pending: Default::default(),
            checks: Default::default(),
            security: Vec::new(),
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
            start_line: None,
            side: Side::New,
            body: "say \"hi\"\nnew line \\ ✓ 日本".into(),
            symbol: "f".into(),
            suggestion: false,
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
            id: format!("u{i}"),
            symbol: format!("s{i}"),
            file: "a.rs".into(),
            line: i,
            note: format!("n{i}"),
            target: t.then_some((i, Side::New)),
            security: Vec::new(),
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

    #[test]
    fn the_diff_shows_three_lines_around_each_change_and_joins_close_ones() {
        let old: Vec<String> = (1..=30).map(|i| format!("l{i}")).collect();
        let mut new = old.clone();
        new[9] = "L10".into();
        // Six unchanged lines between: one hunk.
        new[16] = "L17".into();
        // Seven: a hunk of its own.
        new[24] = "L25".into();
        let f = file(Some(&old.join("\n")), Some(&new.join("\n")));
        assert_eq!(right_hunks(&f), [Range::new(7, 20), Range::new(22, 28)]);
        // A line added at the top: its context below it only.
        let f = file(Some("a\nb\nc\nd\ne\n"), Some("x\na\nb\nc\nd\ne\n"));
        assert_eq!(right_hunks(&f), [Range::new(1, 4)]);
        // A file removed has no new side.
        assert!(right_hunks(&file(Some("a\n"), None)).is_empty());
    }

    #[test]
    fn replaced_lines_are_whole_lines_even_for_text_inside_a_line() {
        let src = "fn f() {\n\tlet x = a + b;\n\tx\n}\n";
        assert_eq!(
            replaced_lines(src, Range::new(2, 2), "a + b", "a - b").as_deref(),
            Some("\tlet x = a - b;")
        );
        assert_eq!(
            replaced_lines(src, Range::new(2, 3), "b;\n\tx", "b;\n\tx + 1").as_deref(),
            Some("\tlet x = a + b;\n\tx + 1")
        );
        // The whole line, ending and all, removed: an empty suggestion.
        assert_eq!(
            replaced_lines(src, Range::new(3, 3), "\tx\n", "").as_deref(),
            Some("")
        );
        // Not in those lines, twice in them, or past the end.
        assert_eq!(replaced_lines(src, Range::new(1, 1), "a + b", "c"), None);
        assert_eq!(replaced_lines("a a\n", Range::new(1, 1), "a", "b"), None);
        assert_eq!(replaced_lines(src, Range::new(4, 5), "}", ""), None);
    }
}
