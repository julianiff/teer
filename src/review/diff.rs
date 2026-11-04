//! A unit's code change as diff rows: the lines it changed, with
//! [`CONTEXT`] unchanged lines around them, longer unchanged runs folded
//! into one gap row. Built from the old and new contents stored in the
//! review, so no git runs. Line numbers are those of the new file; removed
//! lines know theirs in the old file. Around the lines statements cite,
//! [`rows_near`] keeps [`NOTE_CONTEXT`] lines instead. Each row may carry
//! the syntax class of its chars (see [`crate::syntax`]). Front ends draw
//! the rows; `crate::notes` quotes them.

use crate::review::attention::Level;
use crate::review::model::{Change, LineRef, Range, Review, Side, Unit};
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

fn lines_in(content: Option<&str>, r: Option<Range>) -> Vec<&str> {
    match (content, r) {
        (Some(c), Some(r)) if r.start > 0 => c
            .lines()
            .skip(r.start as usize - 1)
            .take(r.len() as usize)
            .collect(),
        _ => Vec::new(),
    }
}

/// The unit's change as diff rows, indentation shared by every shown line
/// removed. Empty when the file is unknown or nothing inside the unit changed.
pub fn unit_rows(review: &Review, u: &Unit) -> Vec<Row> {
    rows_near(review, u, &[], None).0
}

/// The row a statement is written under: the last one it cites, else the
/// one nearest its first cited line (a cited line may sit in a gap).
pub fn anchor(c: &Change, rows: &[Row]) -> Option<usize> {
    let cites = |r: &Row| {
        c.evidence.iter().any(|e| match e.side {
            Side::New => r.line == Some(e.line),
            Side::Old => r.old == Some(e.line),
        })
    };
    if let Some(k) = rows.iter().rposition(cites) {
        return Some(k);
    }
    let first = c.evidence.first()?;
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
    let old = lines_in(f.old_content.as_deref(), u.old_range);
    let new = lines_in(f.new_content.as_deref(), u.new_range);
    let ops = diff_lines(&old, &new);
    let changed: Vec<usize> = (0..ops.len())
        .filter(|&k| ops[k].0 != DiffOp::Equal)
        .collect();
    if changed.is_empty() {
        return (Vec::new(), 0, 0);
    }
    let count = |op: DiffOp| ops.iter().filter(|o| o.0 == op).count();
    let (added, removed) = (count(DiffOp::Insert), count(DiffOp::Delete));
    let n0 = u.new_range.map_or(1, |r| r.start);
    let o0 = u.old_range.map_or(1, |r| r.start);
    // Cited as in [`anchor`]: an added line by its new number, a removed
    // one by its old, an unchanged one by either.
    let cited: Vec<usize> = (0..ops.len())
        .filter(|&k| {
            let (op, i, j) = ops[k];
            cites.iter().any(|e| match e.side {
                Side::New => op != DiffOp::Delete && n0 + j as u32 == e.line,
                Side::Old => op != DiffOp::Insert && o0 + i as u32 == e.line,
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
                Some(n0 + j as u32),
                Some(o0 + i as u32),
                new[j],
            ),
            DiffOp::Insert => (Kind::Added, Some(n0 + j as u32), None, new[j]),
            DiffOp::Delete => (Kind::Removed, None, Some(o0 + i as u32), old[i]),
        };
        // The new file's colours where the row has a new line.
        let classes = match (line, old_line) {
            (Some(n), _) => new_classes.get(n as usize - 1),
            (None, Some(n)) => old_classes.get(n as usize - 1),
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
