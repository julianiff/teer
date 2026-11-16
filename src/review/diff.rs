//! A unit's code change as diff rows: the lines it changed, with
//! [`CONTEXT`] unchanged lines around them, longer unchanged runs folded
//! into one gap row. Built from the old and new contents stored in the
//! review, so no git runs. Line numbers are those of the new file; removed
//! lines know theirs in the old file. Around the lines statements cite,
//! [`rows_near`] keeps [`NOTE_CONTEXT`] lines instead. A unit nested in
//! the shown one (a class's method, a module's function) is its own unit:
//! its lines are folded to the one that names it. Each row may carry the
//! syntax class of its chars (see [`crate::syntax`]). Front ends draw the
//! rows; `crate::notes` quotes them.

use std::borrow::Cow;

use crate::review::attention::Level;
use crate::review::model::{LineRef, Range, Review, Side, Unit};
use crate::syntax::{Cache, Class};
use crate::util::{DiffOp, diff_lines};

/// Units at this level or above show their code change: every unit, now
/// that units without a behavior change are listed too (accepted for you,
/// shown with `za`).
pub const DIFF_LEVEL: Level = Level::Same;
/// Unchanged lines kept before and after each change.
pub const CONTEXT: usize = 2;
/// Lines kept before and after each line a statement cites, when the
/// statements are drawn: enough to read what the code does there.
pub const NOTE_CONTEXT: usize = 2;
const TAB: &str = "    ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Kind {
    Context,
    Removed,
    Added,
    /// Lines left out between two shown runs.
    Gap,
    /// A nested unit's lines, folded to the line that names it (see
    /// [`Unit::folds`]): that unit shows them. A fold with only a new line
    /// came with the change, one with only an old line went.
    Fold,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: Kind,
    /// Line in the new file; `None` for removed lines, which it no longer has.
    pub line: Option<u32>,
    /// Line in the old file; `None` for added lines, which it never had.
    pub old: Option<u32>,
    pub text: String,
    /// The syntax class of each char of `text`; empty when not coloured.
    pub classes: Vec<Option<Class>>,
}

/// One line of a unit's side as the diff compares it: a line of its own,
/// or a nested unit folded to one item.
struct Item<'a> {
    /// Its line in that side's file; a fold's first line.
    line: u32,
    /// What the diff compares: the text, or for a fold a key no code has,
    /// so that the old and the new fold of one symbol match.
    key: Cow<'a, str>,
    /// What is shown: the line, or the line that names the folded unit.
    text: &'a str,
    fold: bool,
}

/// One side of a unit: its own lines in `r`, each nested unit (see
/// [`Unit::folds`]) as one item where its lines were.
fn items<'a>(content: Option<&'a str>, r: Option<Range>, folds: &[(Range, &str)]) -> Vec<Item<'a>> {
    let (Some(content), Some(r)) = (content, r) else {
        return Vec::new();
    };
    if r.start == 0 {
        return Vec::new();
    }
    let lines: Vec<&str> = content.lines().collect();
    let mut out = Vec::new();
    let mut n = r.start;
    while n <= r.end {
        let Some(&text) = lines.get(n as usize - 1) else {
            break;
        };
        match folds.iter().find(|(f, _)| f.start == n) {
            Some((f, symbol)) => {
                out.push(Item {
                    line: n,
                    key: Cow::Owned(format!("\0{symbol}")),
                    text: signature(&lines, *f),
                    fold: true,
                });
                n = f.end.max(n) + 1;
            }
            None => {
                out.push(Item {
                    line: n,
                    key: Cow::Borrowed(text),
                    text,
                    fold: false,
                });
                n += 1;
            }
        }
    }
    out
}

/// The line that names a folded unit: the first of its lines that isn't
/// blank, an attribute, a decorator or a comment.
pub(crate) fn signature<'a>(lines: &[&'a str], r: Range) -> &'a str {
    let start = (r.start.max(1) as usize - 1).min(lines.len());
    let end = (r.end as usize).clamp(start, lines.len());
    let names = |l: &str| {
        let t = l.trim_start();
        !(t.is_empty() || ["@", "#", "//", "/*", "*"].iter().any(|p| t.starts_with(p)))
    };
    lines[start..end]
        .iter()
        .copied()
        .find(|l| names(l))
        .or_else(|| lines.get(start).copied())
        .unwrap_or("")
}

/// The unit's change as diff rows, indentation shared by every shown line
/// removed. Empty when the file is unknown or nothing inside the unit changed.
pub fn unit_rows(review: &Review, u: &Unit) -> Vec<Row> {
    rows_near(review, u, &[], None).0
}

/// The row a comment is written under: the last one it cites, else the
/// one nearest its first cited line (a cited line may sit in a gap).
pub fn anchor(evidence: &[LineRef], rows: &[Row]) -> Option<usize> {
    let cites = |r: &Row| {
        evidence.iter().any(|e| match e.side {
            Side::New => r.line == Some(e.line),
            Side::Old => r.old == Some(e.line),
        })
    };
    if let Some(k) = rows.iter().rposition(cites) {
        return Some(k);
    }
    let first = evidence.first()?;
    rows.iter()
        .enumerate()
        .filter_map(|(k, r)| {
            let n = match first.side {
                Side::New => r.line,
                Side::Old => r.old,
            }?;
            Some((n.abs_diff(first.line), k))
        })
        .min()
        .map(|(_, k)| k)
}

/// [`unit_rows`] around the lines in `cites`, with how many lines the whole
/// unit adds and removes. When some of `cites` lie inside the unit, only
/// they are kept, with [`NOTE_CONTEXT`] lines around each; otherwise every
/// change is kept, with [`CONTEXT`] lines around it. A gap row stands for
/// what is left out between shown rows, and before the first or after the
/// last only when that holds a change; one line alone is shown instead.
/// With `syntax`, each row knows the syntax class of its chars.
pub fn rows_near(
    review: &Review,
    u: &Unit,
    cites: &[LineRef],
    syntax: Option<&Cache>,
) -> (Vec<Row>, usize, usize) {
    rows_around(review, u, cites, false, syntax)
}

/// [`rows_near`]; with `every`, every change is kept as well as the lines
/// in `cites`, each with [`CONTEXT`] lines around it.
pub fn rows_around(
    review: &Review,
    u: &Unit,
    cites: &[LineRef],
    every: bool,
    syntax: Option<&Cache>,
) -> (Vec<Row>, usize, usize) {
    let Some(f) = review.file_of(u) else {
        return (Vec::new(), 0, 0);
    };
    let old = items(f.old_content.as_deref(), u.old_range, &u.folds(Side::Old));
    let new = items(f.new_content.as_deref(), u.new_range, &u.folds(Side::New));
    let old_keys: Vec<&str> = old.iter().map(|i| i.key.as_ref()).collect();
    let new_keys: Vec<&str> = new.iter().map(|i| i.key.as_ref()).collect();
    let ops = diff_lines(&old_keys, &new_keys);
    // A fold that came or went is a change to show, not a line to count.
    let fold = |k: usize| {
        let (op, i, j) = ops[k];
        if op == DiffOp::Delete {
            old[i].fold
        } else {
            new[j].fold
        }
    };
    let changed: Vec<usize> = (0..ops.len())
        .filter(|&k| ops[k].0 != DiffOp::Equal)
        .collect();
    if changed.is_empty() {
        return (Vec::new(), 0, 0);
    }
    let count = |op: DiffOp| {
        (0..ops.len())
            .filter(|&k| ops[k].0 == op && !fold(k))
            .count()
    };
    let (added, removed) = (count(DiffOp::Insert), count(DiffOp::Delete));
    // Cited as in [`anchor`]: an added line by its new number, a removed
    // one by its old, an unchanged one by either.
    let cited: Vec<usize> = (0..ops.len())
        .filter(|&k| {
            let (op, i, j) = ops[k];
            cites.iter().any(|e| match e.side {
                Side::New => op != DiffOp::Delete && new[j].line == e.line,
                Side::Old => op != DiffOp::Insert && old[i].line == e.line,
            })
        })
        .collect();
    let (around, context) = if every {
        (changed.into_iter().chain(cited).collect(), CONTEXT)
    } else if cited.is_empty() {
        (changed, CONTEXT)
    } else {
        (cited, NOTE_CONTEXT)
    };
    let mut kept: Vec<bool> = (0..ops.len())
        .map(|k| around.iter().any(|&c| k + context >= c && k <= c + context))
        .collect();
    // Folding one line saves nothing: show it, where a gap would stand.
    for k in 0..ops.len() {
        let edge = k == 0 || k + 1 == ops.len();
        let between = (k == 0 || kept[k - 1]) && kept.get(k + 1).is_none_or(|&b| b);
        if !kept[k] && between && (!edge || ops[k].0 != DiffOp::Equal) {
            kept[k] = true;
        }
    }
    let gap = || Row {
        kind: Kind::Gap,
        line: None,
        old: None,
        text: String::new(),
        classes: Vec::new(),
    };
    // Each side's classes, by line of its file.
    let paint = |new: bool| {
        let content = if new { &f.new_content } else { &f.old_content };
        let path = if new {
            &f.path
        } else {
            f.old_path.as_ref().unwrap_or(&f.path)
        };
        match (syntax, content) {
            (Some(cache), Some(c)) => cache.get(path, new, c),
            _ => Default::default(),
        }
    };
    let (new_classes, old_classes) = (paint(true), paint(false));
    let mut rows = Vec::new();
    let (mut hidden, mut changed) = (0, 0);
    for (k, &(op, i, j)) in ops.iter().enumerate() {
        if !kept[k] {
            hidden += 1;
            changed += usize::from(op != DiffOp::Equal);
            continue;
        }
        if hidden > 0 && (!rows.is_empty() || changed > 0) {
            rows.push(gap());
        }
        (hidden, changed) = (0, 0);
        let (kind, line, old_line, text) = match op {
            DiffOp::Equal => (
                Kind::Context,
                Some(new[j].line),
                Some(old[i].line),
                new[j].text,
            ),
            DiffOp::Insert => (Kind::Added, Some(new[j].line), None, new[j].text),
            DiffOp::Delete => (Kind::Removed, None, Some(old[i].line), old[i].text),
        };
        let kind = if fold(k) { Kind::Fold } else { kind };
        // The new file's colours where the row has a new line; a fold is
        // drawn quiet, without them.
        let classes = match (kind, line, old_line) {
            (Kind::Fold, _, _) => None,
            (_, Some(n), _) => new_classes.get(n as usize - 1),
            (_, None, Some(n)) => old_classes.get(n as usize - 1),
            _ => None,
        };
        let (text, classes) = expand_tabs(text, classes.map_or(&[][..], Vec::as_slice));
        rows.push(Row {
            kind,
            line,
            old: old_line,
            text,
            classes,
        });
    }
    if changed > 0 {
        rows.push(gap());
    }
    dedent(&mut rows);
    (rows, added, removed)
}

/// The new file's line at the unit's first change, where `gf` opens a whole
/// file: the line added there, else the next (or last) line the new file has.
pub fn first_change(review: &Review, u: &Unit) -> Option<u32> {
    let rows = unit_rows(review, u);
    let k = rows
        .iter()
        .position(|r| matches!(r.kind, Kind::Added | Kind::Removed))?;
    rows[k..]
        .iter()
        .find_map(|r| r.line)
        .or_else(|| rows[..k].iter().rev().find_map(|r| r.line))
}

/// `text` with each tab as [`TAB`], and its `classes` (one per char, or
/// none) to match; a tab has no class.
pub fn expand_tabs(text: &str, classes: &[Option<Class>]) -> (String, Vec<Option<Class>>) {
    if classes.len() != text.chars().count() {
        return (text.replace('\t', TAB), Vec::new());
    }
    let mut out = (
        String::with_capacity(text.len()),
        Vec::with_capacity(classes.len()),
    );
    for (c, &class) in text.chars().zip(classes) {
        if c == '\t' {
            out.0.push_str(TAB);
            out.1.extend([None; TAB.len()]);
        } else {
            out.0.push(c);
            out.1.push(class);
        }
    }
    out
}

/// Drop the leading spaces every non-blank shown line has in common.
fn dedent(rows: &mut [Row]) {
    let common = rows
        .iter()
        .filter(|r| r.kind != Kind::Gap && !r.text.trim().is_empty())
        .map(|r| r.text.len() - r.text.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    for r in rows.iter_mut().filter(|r| r.kind != Kind::Gap) {
        let cut = common.min(r.text.len() - r.text.trim_start_matches(' ').len());
        r.text.drain(..cut);
        r.classes.drain(..cut.min(r.classes.len()));
    }
}
