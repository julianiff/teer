//! The words a unit goes by on every screen: its name, where it is, where
//! it stands, the lines its statements cite and who wrote them.

use std::path::Path;

use super::app::App;
use crate::config::Role;
use crate::review::model::{Decision, LineRef, Side, Unit, UnitKind};
use crate::units::extract::MODULE_SYMBOL;
use crate::util::sanitize;

/// The name a unit goes by in rows: its symbol, `(top level)` for the
/// code outside any function, and a whole file's file name.
pub fn row_name(u: &Unit) -> String {
    match u.kind {
        UnitKind::File => file_name(&u.file),
        _ if u.kind == UnitKind::Module || u.symbol == MODULE_SYMBOL => "(top level)".into(),
        _ => sanitize(&u.symbol),
    }
}

/// The last part of `path`.
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Who wrote the summary and statements of `u`: `agent (haiku)`, or
/// `agent (haiku, sonnet)` once the second model has checked them. Drawn
/// above them; facts from the code carry no label.
pub fn agent_label(app: &App, u: &Unit) -> String {
    let mut models = app.cfg.model_label(Role::Fast);
    if u.checks.second.is_some() {
        models.push_str(&format!(", {}", app.cfg.model_label(Role::Second)));
    }
    format!("agent ({models})")
}

/// Runs of cited lines shown before the rest is counted.
pub const CITE_RUNS: usize = 3;

/// The lines a statement cites, compactly: `L168-172` for lines of the new
/// file, `old L192` for lines of the old one, consecutive lines merged;
/// past three runs, how many more (`+2 more`).
pub fn cite_text(evidence: &[LineRef]) -> String {
    let mut runs = Vec::new();
    for (side, prefix) in [(Side::New, "L"), (Side::Old, "old L")] {
        let mut lines: Vec<u32> = evidence
            .iter()
            .filter(|e| e.side == side)
            .map(|e| e.line)
            .collect();
        lines.sort_unstable();
        lines.dedup();
        let mut i = 0;
        while i < lines.len() {
            let start = lines[i];
            while i + 1 < lines.len() && lines[i + 1] == lines[i] + 1 {
                i += 1;
            }
            runs.push(match lines[i] {
                end if end > start => format!("{prefix}{start}-{end}"),
                _ => format!("{prefix}{start}"),
            });
            i += 1;
        }
    }
    if runs.len() > CITE_RUNS {
        let more = runs.len() - CITE_RUNS;
        runs.truncate(CITE_RUNS);
        runs.push(format!("+{more} more"));
    }
    runs.join(", ")
}

/// `N` for a single line, `N-M` for a real range, empty when unknown.
fn line_span(u: &Unit) -> String {
    match u.new_range.or(u.old_range) {
        Some(r) if r.end > r.start => format!("{}-{}", r.start, r.end),
        Some(r) => r.start.to_string(),
        None => String::new(),
    }
}

/// Where a unit is, next to its [`row_name`]: `path:lines` (just the path
/// when no range is known), or for a whole file (named already) the folder
/// it is in and that it is the whole file.
/// Cut at the front, it keeps the end.
pub fn place(u: &Unit) -> String {
    match u.kind {
        UnitKind::File => match u.file.parent().map(|p| p.display().to_string()) {
            Some(p) if !p.is_empty() => format!("{p}/ · {WHOLE_FILE}"),
            _ => WHOLE_FILE.into(),
        },
        _ => match line_span(u) {
            s if s.is_empty() => u.file.display().to_string(),
            s => format!("{}:{s}", u.file.display()),
        },
    }
}

/// What [`place`] says for a whole file.
const WHOLE_FILE: &str = "whole file";

/// Most characters of a flagged unit's note its row shows.
pub const NOTE_SHOWN: usize = 40;

/// The word for where `u` stands.
pub fn state_word(u: &Unit) -> &'static str {
    match u.decision {
        Decision::Pending => "pending",
        Decision::Accepted => "accepted",
        Decision::Investigate => "flagged",
    }
}

/// The first [`NOTE_SHOWN`] characters of the note on `u`, on one line,
/// `…` after them when there is more; `None` without a note.
pub fn note_start(u: &Unit) -> Option<String> {
    let note = sanitize(&u.dive.note);
    let note = note.split_whitespace().collect::<Vec<_>>().join(" ");
    if note.is_empty() {
        return None;
    }
    let mut start: String = note.chars().take(NOTE_SHOWN).collect();
    if note.chars().count() > NOTE_SHOWN {
        start.push('…');
    }
    Some(start)
}
