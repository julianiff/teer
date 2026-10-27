//! What the author says a change is for: the commits between base and head,
//! with the lines each one touched so units can be matched to them, and for
//! a pull request its title and description (fetched in `pr`). Also how a
//! commit subject reads: its conventional-commit type and the tickets it
//! names. All of it is written by the change's author, so it is untrusted:
//! sanitised when read, and only ever shown or quoted as data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::model::{Commit, CommitFile, DiffSize, Intent, Lines, Range, Review, Unit};
use crate::util::{sanitize, sanitize_agent_text};

/// Most commits read; a longer history keeps its newest.
const MAX_COMMITS: usize = 200;
/// Longest commit body kept, in characters.
const MAX_BODY: usize = 2000;
/// Longest PR description kept, in characters.
pub const MAX_PR_BODY: usize = 4000;
/// Most of the PR description a describe prompt quotes, in characters.
const PROMPT_BODY: usize = 1500;

/// The commits of `base..head` and its size. Empty when git fails (logged).
pub fn read(repo: &Path, base: &str, head: &str) -> Intent {
    let commits = match crate::git::log_patches(repo, base, head, MAX_COMMITS) {
        Ok(raw) => parse_log(&raw),
        Err(e) => {
            crate::log::write(&format!("reading the commits failed: {e:#}"));
            Vec::new()
        }
    };
    let size =
        crate::git::diff_size(repo, base, head).map(|(additions, deletions, files)| DiffSize {
            additions,
            deletions,
            files,
        });
    Intent {
        commits,
        size,
        ..Default::default()
    }
}

/// One line, no escape sequences or control characters, at most `max` characters.
pub fn clean_line(s: &str, max: usize) -> String {
    let s = sanitize(&sanitize_agent_text(s));
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    cap(&s, max)
}

/// Several lines (ANSI and control characters stripped), at most `max` characters.
pub fn clean_text(s: &str, max: usize) -> String {
    cap(&sanitize_agent_text(s), max)
}

fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A hunk header's old and new `(start, count)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hunk {
    old: (u32, u32),
    new: (u32, u32),
}

/// One file of one commit's patch. A path is `None` on the side where the
/// file does not exist (added or deleted).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FileDiff {
    old: Option<PathBuf>,
    new: Option<PathBuf>,
    hunks: Vec<Hunk>,
}

/// Commits from [`crate::git::log_patches`] output, oldest first, with the
/// lines each touched mapped to the base and the head. Stops at the first
/// record that doesn't look like a commit.
pub fn parse_log(raw: &[u8]) -> Vec<Commit> {
    let text = String::from_utf8_lossy(raw);
    let mut fields = text.split('\0');
    fields.next();
    let mut commits = Vec::new();
    let mut diffs = Vec::new();
    while let Some(sha) = fields.next() {
        let rest: Vec<&str> = fields.by_ref().take(7).collect();
        let [short, parents, author, date, subject, body, patch] = rest[..] else {
            break;
        };
        if !matches!(sha.len(), 40 | 64) || !is_hex(sha) || !is_hex(short) {
            crate::log::write("reading the commits: unexpected git log output, stopped");
            break;
        }
        commits.push(Commit {
            sha: sha.to_string(),
            short: short.chars().take(12).collect(),
            author: clean_line(author, 80),
            date: clean_line(date, 10),
            subject: clean_line(subject, 200),
            body: clean_text(body, MAX_BODY),
            merge: parents.split_whitespace().count() > 1,
            files: Vec::new(),
        });
        diffs.push(parse_patch(patch));
    }
    for (k, files) in diffs.iter().enumerate() {
        commits[k].files = files
            .iter()
            .filter_map(|f| {
                let path = f.new.clone().or_else(|| f.old.clone())?;
                let own_new = f
                    .hunks
                    .iter()
                    .map(|h| point_or_range(h.new))
                    .collect::<Vec<_>>();
                let own_old = f
                    .hunks
                    .iter()
                    .map(|h| point_or_range(h.old))
                    .collect::<Vec<_>>();
                Some(CommitFile {
                    path,
                    head: f
                        .new
                        .as_ref()
                        .and_then(|p| follow(&diffs[k + 1..], p, own_new, true)),
                    base: f
                        .old
                        .as_ref()
                        .and_then(|p| follow(&diffs[..k], p, own_old, false)),
                })
            })
            .collect();
    }
    commits
}

/// Lines `start..start+count`; for a count of 0 (lines only added or only
/// removed on the other side) the gap after line `start`, an empty range
/// `start+1..start` that only counts as inside a unit with lines on both sides.
fn point_or_range((start, count): (u32, u32)) -> Range {
    if count == 0 {
        Range::new(start + 1, start)
    } else {
        Range::new(start, start + count - 1)
    }
}

/// Carry `ranges` of `path` through `commits`: forward (to the head) through
/// the later commits, or backward (to the base) through the earlier ones,
/// following renames. `None` when the file stops existing on the way.
fn follow(
    commits: &[Vec<FileDiff>],
    path: &Path,
    ranges: Vec<Range>,
    forward: bool,
) -> Option<Lines> {
    let mut path = path.to_path_buf();
    let mut ranges = ranges;
    let step = |files: &Vec<FileDiff>, path: &mut PathBuf, ranges: &mut Vec<Range>| -> bool {
        let found = files.iter().find(|f| {
            let from = if forward { &f.old } else { &f.new };
            from.as_deref() == Some(path.as_path())
        });
        let Some(f) = found else {
            return true;
        };
        let to = if forward { &f.new } else { &f.old };
        let Some(to) = to else {
            return false;
        };
        for r in ranges.iter_mut() {
            *r = if r.is_empty() {
                // A gap moves with the line before it.
                match r.end {
                    0 => *r,
                    before => {
                        let b = map_line(before, &f.hunks, forward);
                        Range::new(b + 1, b)
                    }
                }
            } else {
                let start = map_line(r.start, &f.hunks, forward);
                let end = map_line(r.end, &f.hunks, forward);
                Range::new(start, end.max(start))
            };
        }
        *path = to.clone();
        true
    };
    if forward {
        for files in commits {
            if !step(files, &mut path, &mut ranges) {
                return None;
            }
        }
    } else {
        for files in commits.iter().rev() {
            if !step(files, &mut path, &mut ranges) {
                return None;
            }
        }
    }
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut merged: Vec<Range> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(m) if !m.is_empty() && !r.is_empty() && r.start <= m.end + 1 => {
                m.end = m.end.max(r.end)
            }
            Some(m) if *m == r => {}
            _ => merged.push(r),
        }
    }
    Some(Lines {
        path,
        ranges: merged,
    })
}

/// Where `line` of one side of a commit's file ends up on the other side
/// (`forward`: old to new). A line inside a replaced block keeps its offset
/// in the block, up to the replacement's last line (git often puts a changed
/// line and a deleted function next to it in one block); a block replaced by
/// nothing leaves the line before the spot.
fn map_line(line: u32, hunks: &[Hunk], forward: bool) -> u32 {
    let mut delta: i64 = 0;
    for h in hunks {
        let ((a, ac), (b, bc)) = if forward {
            (h.old, h.new)
        } else {
            (h.new, h.old)
        };
        if ac == 0 {
            // Lines inserted after line `a`.
            if line <= a {
                break;
            }
        } else {
            if line < a {
                break;
            }
            if line < a + ac {
                return match bc {
                    0 => b.max(1),
                    _ => b + (line - a).min(bc - 1),
                };
            }
        }
        delta += bc as i64 - ac as i64;
    }
    (line as i64 + delta).max(1) as u32
}

/// The files and hunk headers of one commit's patch.
fn parse_patch(patch: &str) -> Vec<FileDiff> {
    #[derive(Default)]
    struct Building {
        header: Option<PathBuf>,
        old: Option<PathBuf>,
        new: Option<PathBuf>,
        added: bool,
        deleted: bool,
        hunks: Vec<Hunk>,
    }
    let finish = |b: Building| FileDiff {
        old: if b.added {
            None
        } else {
            b.old.or_else(|| b.header.clone())
        },
        new: if b.deleted { None } else { b.new.or(b.header) },
        hunks: b.hunks,
    };
    let mut out = Vec::new();
    let mut cur: Option<Building> = None;
    // Lines of the current hunk still to come: removed, added.
    let mut left = (0u32, 0u32);
    for line in patch.lines() {
        if left != (0, 0) {
            match line.as_bytes().first() {
                Some(b'-') => {
                    left.0 = left.0.saturating_sub(1);
                    continue;
                }
                Some(b'+') => {
                    left.1 = left.1.saturating_sub(1);
                    continue;
                }
                Some(b'\\') => continue,
                _ => left = (0, 0),
            }
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(b) = cur.take() {
                out.push(finish(b));
            }
            cur = Some(Building {
                header: same_path_header(rest),
                ..Default::default()
            });
            continue;
        }
        let Some(b) = cur.as_mut() else {
            continue;
        };
        if let Some(p) = line.strip_prefix("rename from ") {
            b.old = Some(PathBuf::from(unquote(p)));
        } else if let Some(p) = line.strip_prefix("rename to ") {
            b.new = Some(PathBuf::from(unquote(p)));
        } else if line.starts_with("new file mode") {
            b.added = true;
        } else if line.starts_with("deleted file mode") {
            b.deleted = true;
        } else if let Some(p) = line.strip_prefix("--- ") {
            match side_path(p, "a/") {
                Some(p) => b.old = Some(p),
                None => b.added = true,
            }
        } else if let Some(p) = line.strip_prefix("+++ ") {
            match side_path(p, "b/") {
                Some(p) => b.new = Some(p),
                None => b.deleted = true,
            }
        } else if let Some(h) = parse_hunk(line) {
            left = (h.old.1, h.new.1);
            b.hunks.push(h);
        }
    }
    if let Some(b) = cur.take() {
        out.push(finish(b));
    }
    out
}

/// `a/x b/x` names one path; anything else (a rename, quoting) is left to
/// the lines that follow.
fn same_path_header(rest: &str) -> Option<PathBuf> {
    let n = rest.len();
    if n < 7 || !(n - 5).is_multiple_of(2) || !rest.starts_with("a/") {
        return None;
    }
    let len = (n - 5) / 2;
    let (a, b) = (rest.get(2..2 + len)?, rest.get(2 + len..)?);
    (b.strip_prefix(" b/") == Some(a)).then(|| PathBuf::from(a))
}

/// The path of a `---` / `+++` line, without its `a/` or `b/`; `None` for `/dev/null`.
fn side_path(p: &str, prefix: &str) -> Option<PathBuf> {
    let p = unquote(p.trim_end_matches('\t'));
    if p == "/dev/null" {
        return None;
    }
    Some(PathBuf::from(p.strip_prefix(prefix).unwrap_or(&p)))
}

/// Undo git's C-style quoting of a path (`"a/caf\303\251.py"`).
fn unquote(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return s.to_string();
    };
    let mut bytes = Vec::new();
    let mut it = inner.bytes().peekable();
    while let Some(c) = it.next() {
        if c != b'\\' {
            bytes.push(c);
            continue;
        }
        match it.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(d @ b'0'..=b'7') => {
                let mut v = (d - b'0') as u32;
                for _ in 0..2 {
                    match it.peek() {
                        Some(&e @ b'0'..=b'7') => {
                            v = v * 8 + (e - b'0') as u32;
                            it.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(v as u8);
            }
            Some(o) => bytes.push(o),
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `@@ -a[,b] +c[,d] @@`.
fn parse_hunk(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    let num = |s: &str| -> Option<(u32, u32)> {
        match s.split_once(',') {
            Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    Some(Hunk {
        old: num(old)?,
        new: num(new)?,
    })
}

// ------------------------------------------------------------- matching

/// `a` (a commit's lines, or a gap) touches the lines `b`. A gap after line
/// `g` only does when lines `g` and `g+1` are both in `b`.
fn overlaps(a: Range, b: Range) -> bool {
    a.start <= b.end && b.start <= a.end
}

/// Whether `commit` changed lines of `unit`: its lines at the head, or for a
/// removed unit its lines at the base, where its file was `base_path`.
pub fn touches(commit: &Commit, unit: &Unit, base_path: &Path) -> bool {
    let hit = |lines: &Option<Lines>, path: &Path, r: Range| {
        lines
            .as_ref()
            .is_some_and(|l| l.path == path && l.ranges.iter().any(|x| overlaps(*x, r)))
    };
    commit
        .files
        .iter()
        .any(|f| match (unit.new_range, unit.old_range) {
            (Some(r), _) => hit(&f.head, &unit.file, r),
            (None, Some(r)) => hit(&f.base, base_path, r),
            (None, None) => false,
        })
}

impl Review {
    /// The commits that changed lines of `unit`, oldest first.
    pub fn commits_of(&self, unit: &Unit) -> Vec<&Commit> {
        self.intent
            .commits
            .iter()
            .filter(|c| self.changed_by(unit, c))
            .collect()
    }

    /// Whether `commit` changed lines of `unit`.
    pub fn changed_by(&self, unit: &Unit, commit: &Commit) -> bool {
        let base_path = self
            .file_of(unit)
            .and_then(|f| f.old_path.as_deref())
            .unwrap_or(&unit.file);
        touches(commit, unit, base_path)
    }

    /// What the describe prompt is told about the author's intent.
    pub fn brief(&self) -> Brief {
        let mut commits = HashMap::new();
        for u in self.units.iter().filter(|u| u.needs_agent()) {
            let subjects: Vec<String> = self
                .commits_of(u)
                .into_iter()
                .filter(|c| !tag(c).minor)
                .map(|c| c.subject.clone())
                .collect();
            if !subjects.is_empty() {
                commits.insert(u.id.clone(), subjects);
            }
        }
        Brief {
            pr: self
                .intent
                .pr
                .as_ref()
                .map(|m| (clean_line(&m.title, 200), clean_text(&m.body, PROMPT_BODY))),
            commits,
            untrusted: self.pr.is_some(),
        }
    }
}

/// The agent's summary of the whole change, cleaned: one paragraph, at most
/// 300 characters.
pub fn clean_summary(answer: &serde_json::Value) -> Option<String> {
    let s = clean_line(answer["summary"].as_str()?, 300);
    (!s.is_empty()).then_some(s)
}

/// Whether to ask for the summary of the whole change now: there is
/// something the author said (commits or a PR), no summary yet, and every
/// unit that changes behavior has been described.
pub fn wants_summary(review: &Review) -> bool {
    use super::model::Prep;
    let i = &review.intent;
    let units: Vec<&Unit> = review.units.iter().filter(|u| u.needs_agent()).collect();
    i.summary.is_none()
        && (!i.commits.is_empty() || i.pr.is_some())
        && units.iter().any(|u| u.prep == Prep::Ready)
        && units
            .iter()
            .all(|u| matches!(u.prep, Prep::Ready | Prep::Unavailable))
}

/// What a describe prompt is told about the author's intent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Brief {
    /// The pull request's title and description (capped).
    pub pr: Option<(String, String)>,
    /// The subjects of the commits that changed each unit, by unit id
    /// (fixups and merges left out).
    pub commits: HashMap<String, Vec<String>>,
    /// It comes from a pull request: untrusted data.
    pub untrusted: bool,
}

impl Brief {
    /// The author said something about this unit: a commit that changed it,
    /// or the pull request's description.
    pub fn covers(&self, unit_id: &str) -> bool {
        self.pr.is_some() || self.commits.contains_key(unit_id)
    }
}

// ----------------------------------------------------------- subjects

/// Conventional-commit types teer recognises.
const TYPES: [&str; 11] = [
    "feat", "fix", "refactor", "chore", "docs", "test", "perf", "build", "ci", "style", "revert",
];

/// How a commit subject reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// The conventional-commit type, or `fixup`, `squash`, `amend`, `merge`.
    pub label: Option<String>,
    /// The subject without the prefix that gave the label (a scope stays, as `scope: …`).
    pub subject: String,
    /// A fixup, squash or merge commit: not part of the story.
    pub minor: bool,
    /// `type!:`, a breaking change.
    pub breaking: bool,
}

/// The label and subject of `c`.
pub fn tag(c: &Commit) -> Tag {
    let s = c.subject.trim();
    let tagged = |label: &str, subject: &str, minor: bool, breaking: bool| Tag {
        label: Some(label.to_string()),
        subject: subject.trim().to_string(),
        minor,
        breaking,
    };
    for (prefix, label) in [
        ("fixup! ", "fixup"),
        ("squash! ", "squash"),
        ("amend! ", "amend"),
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return tagged(label, rest, true, false);
        }
    }
    if c.merge || s.starts_with("Merge ") {
        return tagged("merge", s, true, false);
    }
    if s.starts_with("Revert \"") {
        return tagged("revert", s, false, false);
    }
    if let Some((head, rest)) = s.split_once(": ") {
        let (head, breaking) = match head.strip_suffix('!') {
            Some(h) => (h, true),
            None => (head, false),
        };
        let (ty, scope) = match head.split_once('(') {
            Some((t, sc)) => (t, sc.strip_suffix(')')),
            None => (head, Some("")),
        };
        let ty = ty.to_ascii_lowercase();
        if let Some(scope) = scope
            && TYPES.contains(&ty.as_str())
            && !scope.contains(['(', ')', ' '])
        {
            let subject = if scope.is_empty() {
                rest.to_string()
            } else {
                format!("{scope}: {rest}")
            };
            return tagged(&ty, &subject, false, breaking);
        }
    }
    Tag {
        label: None,
        subject: s.to_string(),
        minor: false,
        breaking: false,
    }
}

/// Byte ranges of ticket references in `text`: `ABC-123` (a capital letter,
/// capitals or digits, a dash and digits; `UTF-8` and the like aside) and `#123`.
pub fn tickets(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let word_end = |k: usize| k == b.len() || !b[k].is_ascii_alphanumeric();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let starts = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        if starts && b[i] == b'#' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && word_end(j) {
                out.push((i, j));
                i = j;
                continue;
            }
        }
        if starts && b[i].is_ascii_uppercase() {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_uppercase() || b[j].is_ascii_digit()) {
                j += 1;
            }
            let key = &text[i..j];
            if j - i >= 2 && j < b.len() && b[j] == b'-' && !["UTF", "SHA", "ISO"].contains(&key) {
                let mut k = j + 1;
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                if k > j + 1 && word_end(k) {
                    out.push((i, k));
                    i = k;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// What the header says about the change: a title (the PR's, or the only
/// commit's subject, or `N commits`), an author, and its size.
pub fn headline(review: &Review) -> (Option<String>, Option<String>, Option<DiffSize>) {
    let i = &review.intent;
    if let Some(m) = &i.pr {
        let author = (!m.author.is_empty()).then(|| format!("@{}", m.author));
        return (Some(m.title.clone()), author, Some(m.size));
    }
    let title = match i.commits.len() {
        0 => None,
        1 => Some(i.commits[0].subject.clone()),
        n => Some(format!("{n} commits")),
    };
    let mut authors: Vec<&str> = i.commits.iter().map(|c| c.author.as_str()).collect();
    authors.sort();
    authors.dedup();
    let author = match authors[..] {
        [one] if !one.is_empty() => Some(one.to_string()),
        _ => None,
    };
    (title, author, i.size)
}

/// `+145 −44 · 13 files`.
pub fn size_text(s: DiffSize) -> String {
    format!(
        "+{} −{} · {} file{}",
        s.additions,
        s.deletions,
        s.files,
        if s.files == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(old: (u32, u32), new: (u32, u32)) -> Hunk {
        Hunk { old, new }
    }

    #[test]
    fn lines_move_through_hunks_both_ways() {
        // 3 lines inserted after line 5; lines 10-11 replaced by one line.
        let hunks = [h((5, 0), (6, 3)), h((10, 2), (13, 1))];
        assert_eq!(map_line(5, &hunks, true), 5);
        assert_eq!(map_line(6, &hunks, true), 9);
        assert_eq!(map_line(10, &hunks, true), 13);
        assert_eq!(map_line(11, &hunks, true), 13);
        assert_eq!(map_line(12, &hunks, true), 14);
        // And back: the inserted lines sit after old line 5.
        assert_eq!(map_line(7, &hunks, false), 5);
        assert_eq!(map_line(9, &hunks, false), 6);
        assert_eq!(map_line(13, &hunks, false), 10);
        assert_eq!(map_line(14, &hunks, false), 12);
        // A deleted block leaves the line before it.
        assert_eq!(map_line(4, &[h((3, 2), (2, 0))], true), 2);
    }

    #[test]
    fn a_gap_touches_a_unit_only_from_inside() {
        let unit = Range::new(9, 10);
        // Lines added right after the unit's last line: not this unit.
        assert!(!overlaps(point_or_range((10, 0)), unit));
        // Lines removed between two of its lines: this unit.
        assert!(overlaps(point_or_range((9, 0)), unit));
        assert!(overlaps(point_or_range((10, 2)), unit));
        assert!(!overlaps(point_or_range((11, 2)), unit));
    }

    #[test]
    fn parses_hunks_and_paths() {
        assert_eq!(parse_hunk("@@ -2 +2 @@ a"), Some(h((2, 1), (2, 1))));
        assert_eq!(parse_hunk("@@ -3,0 +4,2 @@"), Some(h((3, 0), (4, 2))));
        assert_eq!(parse_hunk("@@ bogus"), None);
        assert_eq!(
            same_path_header("a/src/x.py b/src/x.py"),
            Some(PathBuf::from("src/x.py"))
        );
        assert_eq!(same_path_header("a/x.py b/y.py"), None);
        assert_eq!(unquote("\"a/caf\\303\\251 x.py\""), "a/café x.py");
        assert_eq!(side_path("/dev/null", "a/"), None);
    }

    #[test]
    fn a_removed_line_starting_with_dashes_is_not_a_header() {
        let patch = "diff --git a/q.sql b/q.sql\n--- a/q.sql\n+++ b/q.sql\n@@ -2 +2 @@\n--- old comment\n++++ new\n";
        let files = parse_patch(patch);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].old, Some(PathBuf::from("q.sql")));
        assert_eq!(files[0].new, Some(PathBuf::from("q.sql")));
        assert_eq!(files[0].hunks, [h((2, 1), (2, 1))]);
    }

    fn commit(subject: &str) -> Commit {
        Commit {
            subject: subject.into(),
            ..Default::default()
        }
    }

    #[test]
    fn subjects_get_their_type() {
        let t = tag(&commit("feat(api)!: add batching"));
        assert_eq!(t.label.as_deref(), Some("feat"));
        assert_eq!(t.subject, "api: add batching");
        assert!(t.breaking && !t.minor);
        assert_eq!(tag(&commit("Fix: typo")).label.as_deref(), Some("fix"));
        assert_eq!(
            tag(&commit("fixup! Batch withdraw.")).label.as_deref(),
            Some("fixup")
        );
        assert!(tag(&commit("squash! x")).minor);
        assert!(tag(&commit("Merge branch 'main' into x")).minor);
        let plain = tag(&commit("Batch withdraw."));
        assert_eq!(
            (plain.label, plain.subject.as_str()),
            (None, "Batch withdraw.")
        );
        assert_eq!(tag(&commit("Note: not a type")).label, None);
        assert_eq!(tag(&commit("feat(a b): no")).label, None);
    }

    #[test]
    fn tickets_are_found() {
        let s = "[MD-732] Fix ABC-123 and #45, not UTF-8 or abc-1 or x#9";
        let found: Vec<&str> = tickets(s).into_iter().map(|(a, b)| &s[a..b]).collect();
        assert_eq!(found, ["MD-732", "ABC-123", "#45"]);
        assert!(tickets("éé #7").len() == 1);
    }
}
