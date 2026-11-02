//! The queue (v2 Phase 4): what teer opens on. Units by level, then file;
//! the preview of the one under the cursor; the counts.
//!
//! The rows sit in the list's level boxes (see [`super::slim`]), most
//! attention first. Each row is a unit: its level's glyph, its file, its
//! name, `⚠` when a second model disputes it, and at the right edge where
//! it stands: `pending`, `accepted`, or `flagged` and the start of the note
//! that goes with the flag. Listed by file (`zf`), the files are labels
//! above their units and the rows leave the path out. A line under the
//! boxes says how many units were accepted for you. The preview of the unit
//! under the cursor sits beside the list on a wide terminal, else below it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Grouping};
use super::slim::{self, UNIT_LEAD};
use super::{preview, ui};
use crate::keymap::Action;
use crate::review::model::{Decision, Risk, Unit, UnitKind, UnitStatus};
use crate::util::{sanitize, truncate, truncate_pad};

/// The actions the queue handles, for the help, in the order it lists them.
pub const ACTIONS: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::MoveNextPending,
    Action::MovePrevPending,
    Action::MoveNextFile,
    Action::MovePrevFile,
    Action::ScrollHalfDown,
    Action::ScrollHalfUp,
    Action::ScrollPageDown,
    Action::ScrollPageUp,
    Action::DecideAccept,
    Action::DecideFlag,
    Action::DecideAcceptFile,
    Action::DecideFlagFile,
    Action::NoteEdit,
    Action::VisualStart,
    Action::HistoryUndo,
    Action::HistoryRedo,
    Action::HistoryRepeat,
    Action::ViewOpen,
    Action::ViewClose,
    Action::AskOpen,
    Action::ToggleAutoAccepted,
    Action::ToggleByFile,
    Action::HelpOpen,
    Action::FindUnit,
    Action::GotoDefinition,
    Action::GotoCallers,
    Action::GotoEditor,
    Action::ToolTests,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
    Action::ToolHistory,
    Action::ToolNotes,
    Action::ToolSettings,
    Action::FinishOpen,
    Action::ReviewRerun,
    Action::ReviewReset,
];

/// Most characters of a flagged unit's note its row shows.
pub const NOTE_SHOWN: usize = 40;
/// Cells of a unit's name that stay whole before its note gives way.
const NAME_KEEP: usize = 24;
/// Narrowest path column; under it the rows leave the path out.
const PATH_MIN: usize = 6;
/// What a flagged unit's note is put between, after `flagged · `.
const QUOTES: (&str, &str) = ("“", "”");

/// Draw the queue into `area`: the rows, and the preview of the unit under
/// the cursor beside or below them.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let area = ui::banner(f, app, area);
    if app.order.is_empty() {
        ui::draw_empty(f, app, area, slim::Flavor::Queue);
        return;
    }
    let (list, pane) = preview::layout_of(app, area);
    // The blank lines between boxes and files cost rows: only when the list
    // has room for them.
    let padded = list.height as usize >= slim::PADDING_MIN_ROWS;
    let rows = slim::build_queue(app, list.width, padded);
    ui::draw_rows(f, app, list, &rows);
    if let Some((rect, kind)) = pane {
        preview::draw_preview(f, app, rect, kind);
    }
}

/// The path a row shows in its path column.
fn path_of(u: &Unit) -> String {
    sanitize(&u.file.display().to_string())
}

/// The name a row shows; `with_path`, after a path column, where a whole
/// file is named already.
fn name_of(u: &Unit, with_path: bool) -> String {
    if u.kind == UnitKind::File && with_path {
        "whole file".into()
    } else {
        slim::row_name(u)
    }
}

/// What ends a row's name: `⚠` when a second model disputes the unit, `✎`
/// when it has a note its state doesn't show (it isn't flagged).
fn marks(app: &App, ui: usize) -> Vec<(&'static str, Style)> {
    let t = &app.theme;
    let u = &app.review.units[ui];
    let mut marks = Vec::new();
    if !app.level_of(ui).doubts.is_empty() {
        marks.push(("⚠", t.risk(Risk::Medium)));
    }
    if u.decision != Decision::Investigate && !u.dive.note.trim().is_empty() {
        marks.push(("✎", t.accent()));
    }
    marks
}

/// Cells of a row's indent, name and marks (see [`name_of`]).
fn name_width(app: &App, ui: usize, with_path: bool) -> usize {
    let u = &app.review.units[ui];
    let marks: usize = marks(app, ui).iter().map(|m| 1 + m.0.width()).sum();
    slim::indent(app, ui).width() + name_of(u, with_path).width() + marks
}

/// The word for where `u` stands.
fn state_word(u: &Unit) -> &'static str {
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

/// Where `u` stands, in at most `room` cells: `pending`, `accepted`, or
/// `flagged · “the start of its note”`, the note cut first.
fn state_spans(app: &App, u: &Unit, room: usize) -> Vec<Span<'static>> {
    let t = &app.theme;
    let word = state_word(u);
    let style = match u.decision {
        Decision::Pending => t.muted(),
        Decision::Accepted => t.accepted_state(),
        Decision::Investigate => t.flagged_row(),
    };
    let mut spans = vec![Span::styled(truncate(word, room), style)];
    let note = if u.decision == Decision::Investigate {
        note_start(u)
    } else {
        None
    };
    let lead = format!(" · {}", QUOTES.0);
    // The note, cut to what is left; a few cells of it or none.
    let left = room.saturating_sub(word.width() + lead.width() + QUOTES.1.width());
    if let Some(note) = note.filter(|_| left >= 4) {
        let note = if note.width() > left {
            truncate(note.trim_end_matches('…'), left)
        } else {
            note
        };
        spans.push(Span::styled(lead, t.muted()));
        spans.push(Span::styled(note, t.text()));
        spans.push(Span::styled(QUOTES.1, t.muted()));
    }
    spans
}

/// Cells of where `u` stands, written out whole.
fn state_width(app: &App, u: &Unit) -> usize {
    state_spans(app, u, usize::MAX)
        .iter()
        .map(|s| s.content.width())
        .sum()
}

/// The widest state word of the listed units: what every row keeps for its
/// state before the path column takes room.
fn state_reserve(app: &App) -> usize {
    app.order
        .iter()
        .map(|&i| state_word(&app.review.units[i]).width())
        .max()
        .unwrap_or(0)
}

/// Cells of the widest path of the listed units; 0 listed by file, where
/// the files are labels above their units.
fn widest_path(app: &App) -> usize {
    if app.grouping == Grouping::File {
        return 0;
    }
    app.order
        .iter()
        .map(|&i| path_of(&app.review.units[i]).width())
        .max()
        .unwrap_or(0)
}

/// Cells of the widest indent, name and marks of the listed units.
fn widest_name(app: &App, with_path: bool) -> usize {
    app.order
        .iter()
        .map(|&i| name_width(app, i, with_path))
        .max()
        .unwrap_or(0)
}

/// Cells of the path column in rows of `inner` cells: the widest listed
/// path, or when the path and the name don't both fit, what the names
/// leave and at least half. None listed by file, or when a cut path would
/// get under [`PATH_MIN`] cells.
pub(super) fn path_column(app: &App, inner: usize) -> usize {
    let path = widest_path(app);
    let name = widest_name(app, true);
    // The lead, the gap after the path, the gap before the state and the
    // state take the rest.
    let room = inner.saturating_sub(UNIT_LEAD + 2 + 1 + state_reserve(app));
    let w = if path + name <= room {
        path
    } else {
        room.saturating_sub(name).max(room / 2).min(path)
    };
    if w < path && w < PATH_MIN { 0 } else { w }
}

/// Narrowest rows that show every unit whole: its path, name, marks and
/// state with the start of its note (see [`path_column`]).
pub(super) fn rows_width(app: &App) -> usize {
    let path = widest_path(app);
    let with_path = path > 0;
    let name = widest_name(app, with_path);
    let whole = app
        .order
        .iter()
        .map(|&i| name_width(app, i, with_path) + 1 + state_width(app, &app.review.units[i]))
        .max()
        .unwrap_or(0);
    let path = if with_path { path + 2 } else { 0 };
    UNIT_LEAD + path + whole.max(name + 1 + state_reserve(app))
}

/// The row of the unit at `pos`, `width` cells: the bar when it is under
/// the cursor, its level's glyph, its path in a column of `path_w` cells
/// (none when 0), its name and marks, and where it stands at the right
/// edge. When they don't all fit, the note gives way to the first
/// [`NAME_KEEP`] cells of the name, then the name to the state's word.
/// Inside the visual selection the row is marked.
pub(super) fn row(app: &App, pos: usize, width: usize, path_w: usize) -> Line<'static> {
    let t = &app.theme;
    let ui = app.order[pos];
    let u = &app.review.units[ui];
    let selected = pos == app.sel;
    let name_style = match u.decision {
        Decision::Accepted => t.accepted_row(),
        Decision::Investigate => t.flagged_row(),
        Decision::Pending if selected => t.accent(),
        Decision::Pending => Style::new(),
    };
    let mut name_style = if selected {
        name_style.add_modifier(Modifier::BOLD)
    } else {
        name_style
    };
    if u.status == UnitStatus::Removed {
        name_style = name_style.add_modifier(Modifier::CROSSED_OUT);
    }
    let level = app.shown_level(ui);
    let icon = if u.decision == Decision::Accepted {
        t.accepted_row()
    } else {
        slim::icon_style(app, level)
    };
    let mut spans = vec![
        if selected {
            Span::styled("▌", t.accent())
        } else {
            Span::raw(" ")
        },
        Span::raw("  "),
        Span::styled(format!("{} ", level.icon()), icon),
    ];
    if path_w > 0 {
        let path_style = if u.decision == Decision::Accepted {
            t.accepted_row()
        } else {
            t.text()
        };
        spans.extend(slim::path_spans(app, &path_of(u), path_w, path_style));
        spans.push(Span::raw("  "));
    }
    let indent = slim::indent(app, ui);
    let marks = marks(app, ui);
    let mark_w: usize = marks.iter().map(|m| 1 + m.0.width()).sum();
    let with_path = path_w > 0;
    let name = name_of(u, with_path);
    // What the name, its indent and marks, the gap and the state share.
    let avail = width.saturating_sub(spans.iter().map(|s| s.content.width()).sum::<usize>());
    let name_full = indent.width() + name.width() + mark_w;
    let keep = name_full.min(indent.width() + NAME_KEEP + mark_w);
    // The state's word stays whole while the name keeps a few cells.
    let word = state_word(u).width().min(avail.saturating_sub(1 + 4));
    let state_room = avail.saturating_sub(1 + keep).max(word);
    let state = state_spans(app, u, state_room);
    let state_w: usize = state.iter().map(|s| s.content.width()).sum();
    let name_w = avail.saturating_sub(indent.width() + mark_w + 1 + state_w);
    let whole_file = u.kind == UnitKind::File && with_path;
    spans.push(Span::styled(indent, t.muted()));
    spans.push(Span::styled(
        truncate(&name, name_w),
        if whole_file { t.muted() } else { name_style },
    ));
    for (m, style) in marks {
        spans.push(Span::styled(format!(" {m}"), style));
    }
    // The state at the right edge.
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used + state_w))));
    spans.extend(state);
    // Inside the visual selection: the bar in front, the rest behind the
    // selection's colour.
    if slim::in_visual(app, pos) {
        spans[0] = Span::styled("▌", t.accent());
        for s in spans.iter_mut().skip(1) {
            s.style = s.style.patch(t.visual());
        }
    }
    let mut line = Line::from(spans);
    if line.width() > width {
        line = Line::from(truncate_pad(&plain(&line), width));
    }
    line
}

/// A line's text without its styles.
fn plain(l: &Line) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::keymap::Keymap;
    use crate::review::model::{Prep, Range, Review};
    use crate::tui::app::Screen;
    use std::path::PathBuf;

    fn unit(i: usize, file: &str, decision: Decision) -> Unit {
        Unit {
            id: format!("u{i}"),
            file: PathBuf::from(file),
            symbol: format!("symbol_{i}"),
            kind: UnitKind::Function,
            status: UnitStatus::Changed,
            old_range: None,
            new_range: Some(Range::new(1, 4)),
            renamed_from: None,
            fingerprint: format!("fp{i}"),
            changes: Vec::new(),
            prep: Prep::Skipped,
            callers: Vec::new(),
            callees: Vec::new(),
            decision,
            summary: None,
            nbc_reason: None,
            dive: Default::default(),
            accepted_by: None,
            reasons: Vec::new(),
            dive_pending: Default::default(),
            checks: Default::default(),
        }
    }

    fn app(units: Vec<Unit>) -> App {
        let review = Review {
            id: "q".into(),
            repo_root: PathBuf::from("/r"),
            base: "b".into(),
            head: crate::git::WORKTREE.into(),
            head_commit: "h".into(),
            agent_session: None,
            pr: None,
            intent: Default::default(),
            created_at: chrono::Utc::now(),
            files: Vec::new(),
            units,
            skipped: Vec::new(),
            graph: Default::default(),
            questions: Vec::new(),
            version: 0,
            history: Default::default(),
            branch: None,
            cursor: None,
        };
        let mut app = App::new(review, Config::default(), Keymap::default());
        app.screen = Screen::Queue;
        app.grouping = Grouping::Unit;
        app.rebuild_order();
        app
    }

    fn three() -> App {
        let mut flagged = unit(1, "src/billing/routes.rs", Decision::Investigate);
        flagged.dive.note =
            "The retry is gone and nothing replaces it when the call times out.".into();
        app(vec![
            unit(0, "src/billing/charge.rs", Decision::Pending),
            flagged,
            unit(2, "src/ui/cart.rs", Decision::Accepted),
        ])
    }

    fn row_text(app: &App, pos: usize, width: usize) -> String {
        let path_w = path_column(app, width);
        plain(&row(app, pos, width, path_w))
    }

    fn pos_of(app: &App, id: &str) -> usize {
        app.order
            .iter()
            .position(|&i| app.review.units[i].id == id)
            .unwrap()
    }

    #[test]
    fn a_row_is_glyph_path_name_and_state_at_the_edge() {
        let app = three();
        let w = rows_width(&app);
        let pending = row_text(&app, pos_of(&app, "u0"), w);
        assert!(pending.contains("● src/billing/charge.rs"), "{pending}");
        assert!(pending.contains("  symbol_0"), "{pending}");
        assert!(pending.ends_with("pending"), "{pending}");
        let accepted = row_text(&app, pos_of(&app, "u2"), w);
        assert!(accepted.ends_with("accepted"), "{accepted}");
        let flagged = row_text(&app, pos_of(&app, "u1"), w);
        assert!(
            flagged.ends_with("flagged · “The retry is gone and nothing replaces i…”"),
            "{flagged}"
        );
        for pos in 0..3 {
            assert_eq!(row(&app, pos, w, path_column(&app, w)).width(), w);
        }
    }

    #[test]
    fn the_note_shows_its_first_forty_characters_on_one_line() {
        let mut u = unit(0, "a.rs", Decision::Investigate);
        u.dive.note = "line one\nline two".into();
        assert_eq!(note_start(&u).as_deref(), Some("line one line two"));
        u.dive.note = "x".repeat(40);
        assert_eq!(note_start(&u), Some("x".repeat(40)));
        u.dive.note = "x".repeat(41);
        assert_eq!(note_start(&u), Some(format!("{}…", "x".repeat(40))));
        u.dive.note = "  ".into();
        assert_eq!(note_start(&u), None);
    }

    #[test]
    fn narrow_rows_cut_the_note_then_the_path_and_keep_the_state() {
        let app = three();
        let flagged = pos_of(&app, "u1");
        for width in [20usize, 30, 44, 60, 80] {
            let path_w = path_column(&app, width);
            let line = row(&app, flagged, width, path_w);
            assert_eq!(line.width(), width, "{width}");
            let text = plain(&line);
            assert!(text.contains("flagged"), "{width}: {text}");
            assert!(text.contains("sym"), "{width}: {text}");
        }
        // No room for a path column of 6: none.
        assert_eq!(path_column(&app, 24), 0);
        assert!(path_column(&app, 60) >= PATH_MIN);
    }

    #[test]
    fn a_short_path_shows_whole_and_a_whole_file_is_not_named_twice() {
        let mut file = unit(1, "b.rs", Decision::Pending);
        file.kind = UnitKind::File;
        let app = app(vec![unit(0, "a.rs", Decision::Pending), file]);
        let w = rows_width(&app);
        assert_eq!(path_column(&app, w), 4);
        let text = row_text(&app, pos_of(&app, "u0"), w);
        assert!(text.contains("● a.rs  symbol_0"), "{text}");
        let text = row_text(&app, pos_of(&app, "u1"), w);
        assert!(text.contains("● b.rs  whole file"), "{text}");
    }

    #[test]
    fn listed_by_file_the_rows_leave_the_path_out() {
        let mut app = three();
        app.grouping = Grouping::File;
        app.rebuild_order();
        assert_eq!(path_column(&app, 120), 0);
        let text = row_text(&app, pos_of(&app, "u0"), 60);
        assert!(!text.contains("charge.rs"), "{text}");
        assert!(text.contains("symbol_0"), "{text}");
    }

    #[test]
    fn a_disputed_unit_shows_the_warning_and_a_note_on_an_unflagged_one_the_pencil() {
        let mut app = three();
        let i = app.order[pos_of(&app, "u0")];
        app.levels[i].doubts = vec!["the retry moved".into()];
        app.review.units[i].dive.note = "check".into();
        let w = rows_width(&app);
        let text = row_text(&app, pos_of(&app, "u0"), w);
        assert!(text.contains("symbol_0 ⚠ ✎"), "{text}");
        // A flagged unit's note is in its state.
        let text = row_text(&app, pos_of(&app, "u1"), w);
        assert!(!text.contains('✎'), "{text}");
    }

    #[test]
    fn the_queue_help_lists_each_action_once_and_no_v1_ones() {
        for (i, a) in ACTIONS.iter().enumerate() {
            assert!(!ACTIONS[i + 1..].contains(a), "{} twice", a.id());
            assert!(!a.id().starts_with("v1."), "{}", a.id());
        }
        for a in [
            Action::DecideAccept,
            Action::DecideFlag,
            Action::ToggleAutoAccepted,
            Action::ToggleByFile,
            Action::FinishOpen,
            Action::ToolHistory,
        ] {
            assert!(ACTIONS.contains(&a), "{}", a.id());
        }
        assert!(!ACTIONS.contains(&Action::FinishYank));
        assert!(!ACTIONS.contains(&Action::MoveNextHunk));
    }

    /// `app` drawn `w` x `h`, one string per row.
    fn render(app: &App, w: u16, h: u16) -> Vec<String> {
        use ratatui::{Terminal, backend::TestBackend};
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| ui::draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn the_queue_draws_rows_preview_and_bar_at_any_size() {
        let mut app = three();
        app.review.intent.summary = Some("Refunds, retried.".into());
        let s = render(&app, 160, 30);
        assert_eq!(s[0].trim_end(), " your changes");
        assert_eq!(s[1].trim_end(), " Refunds, retried.");
        let row = s.iter().find(|l| l.contains("│▌")).unwrap();
        assert!(row.contains("● src/billing/charge.rs  symbol_0"), "{row}");
        assert!(row.contains("pending │"), "{row}");
        let bar = s.last().unwrap();
        assert!(
            bar.starts_with(" 1 pending · 1 flagged · 1 accepted"),
            "{bar}"
        );
        // Each row a click target, the preview the wheel's.
        let targets = app.targets.borrow().clone();
        let units = targets
            .iter()
            .filter(|(_, t)| matches!(t, crate::tui::app::Target::Unit(_)))
            .count();
        assert_eq!(units, 3);
        assert!(
            targets
                .iter()
                .any(|(_, t)| *t == crate::tui::app::Target::Detail)
        );
        for (w, h) in [
            (0, 0),
            (1, 1),
            (10, 3),
            (30, 8),
            (80, 24),
            (109, 40),
            (110, 40),
        ] {
            let s = render(&app, w, h);
            assert_eq!(s.len(), h as usize);
            for l in &s {
                assert_eq!(l.width(), w as usize, "{w}x{h}: {l}");
            }
        }
    }
}
