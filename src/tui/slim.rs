//! The queue's rows: the tree of the changed files (see
//! [`crate::session::tree`]), a row each, indented under its directory. In
//! front of a file, the mark of the most attention one of its units needs:
//! a vertical line `┃`, red for a close look, yellow for worth a look and
//! grey for routine; `○` accepted for you, muted. After its name only what there
//! is to act on or know: `⚑` flagged, `⚠` disputed by the second model, `↻`
//! changed by the last re-run, a spinner while it is described. A file with
//! nothing left to do is muted.
//! A directory is behind `▾`, or `▸` while it is folded, then in the colour
//! of the most attention under it. Files teer can't show are muted rows the
//! cursor skips. A line under the tree names the key that shows the units
//! accepted for you.
//!
//! Also the pieces other screens draw units with: names, places, levels'
//! styles, statements and the models that wrote them.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::app::App;
use crate::config::Role;
use crate::keymap::Action;
use crate::review::attention::Level;
use crate::review::model::{Prep, Risk, Unit, UnitStatus};
use crate::session::tree::{RowKind, TreeRow};
use crate::util::{sanitize, truncate, truncate_pad, wrap};

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A list as lines, and which item each line is: the queue's tree rows,
/// improve mode's inserts.
pub struct Rows {
    pub lines: Vec<Line<'static>>,
    /// Which item, by its index, each line is.
    pub owner: Vec<Option<usize>>,
    /// First and one-past-last line of each item, by index.
    pub spans: Vec<(usize, usize)>,
    /// Line of the head each item is kept in view with, by index: its own
    /// line where it has none.
    pub heads: Vec<usize>,
    /// Line of the title edge of the box each item sits in, by index.
    pub titles: Vec<usize>,
    /// One past the last line to keep in view below each item, by index:
    /// its own line, or for the last item of a box, the box's bottom edge
    /// and, with blank lines, the one after it.
    pub ends: Vec<usize>,
}

impl Rows {
    fn push(&mut self, line: Line<'static>, owner: Option<usize>) {
        self.lines.push(line);
        self.owner.push(owner);
    }
}

impl App {
    pub fn spinner(&self) -> &'static str {
        SPINNER[(self.tick % 10) as usize]
    }
}

/// Cut a line to at most `width` cells, dropping whatever sticks out.
fn clip(line: &mut Line<'static>, width: usize) {
    use unicode_width::UnicodeWidthChar;
    if line.width() <= width {
        return;
    }
    let mut used = 0;
    let mut keep = 0;
    for span in line.spans.iter_mut() {
        if used >= width {
            break;
        }
        keep += 1;
        let mut text = String::new();
        for c in span.content.chars() {
            let cw = c.width().unwrap_or(0);
            if used + cw > width {
                break;
            }
            used += cw;
            text.push(c);
        }
        if text != span.content {
            span.content = text.into();
        }
    }
    line.spans.truncate(keep);
}

fn status_word(u: &Unit) -> &'static str {
    match u.status {
        UnitStatus::Added => "added",
        UnitStatus::Removed => "removed",
        UnitStatus::Changed => "changed",
        UnitStatus::NoBehaviorChange => "no behavior change",
    }
}

/// The note shown in place of change statements when there are none.
pub fn prep_note(app: &App, u: &Unit) -> Option<String> {
    match u.prep {
        Prep::Ready | Prep::Skipped => None,
        Prep::Pending => Some(format!("{} preparing…", app.spinner())),
        Prep::Unavailable => Some(format!("{}, details unavailable", status_word(u))),
        Prep::NotRequested if app.large && app.can_ask() => {
            Some("details are prepared when you open this file".into())
        }
        Prep::NotRequested => Some(status_word(u).to_string()),
    }
}

pub fn level_style(app: &App, level: Level) -> Style {
    let t = &app.theme;
    let style = match level {
        Level::Same => return t.muted(),
        Level::Routine => t.muted(),
        Level::Look => Style::new().fg(t.risk_medium),
        Level::Attention => Style::new().fg(t.risk_high),
    };
    // The three share one mark (a vertical line): without colour its
    // weight tells them apart.
    match level {
        Level::Attention if t.mono() => style.add_modifier(Modifier::BOLD | Modifier::REVERSED),
        Level::Look if t.mono() => style.add_modifier(Modifier::BOLD),
        _ => style,
    }
}

/// The model for `role` in words: `haiku`, or `Claude Code's default
/// model` when the CLI picks.
pub fn model_name(app: &App, role: Role) -> String {
    app.cfg.model_label(role)
}

/// The agent's statements, each ending with the lines it cites (muted).
pub fn statement_lines(app: &App, u: &Unit, width: usize, indent: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    if u.prep == Prep::Ready {
        let mut out = Vec::new();
        // "+ text": the marker, then the text.
        let text_w = width.saturating_sub(indent + 2);
        for c in &u.changes {
            let mut lines: Vec<Line<'static>> = Vec::new();
            let mut last_w = 0;
            for (k, l) in wrap(&c.statement, text_w).into_iter().enumerate() {
                let lead = if k == 0 {
                    vec![
                        Span::styled(c.marker.symbol().to_string(), t.risk(c.risk)),
                        Span::raw(" "),
                    ]
                } else {
                    vec![Span::raw("  ")]
                };
                let mut spans = vec![Span::raw(" ".repeat(indent))];
                spans.extend(lead);
                last_w = l.width();
                spans.push(Span::raw(l));
                lines.push(Line::from(spans));
            }
            // The cited lines end the last line, or get one of their own.
            let cites = crate::session::names::cite_text(&c.evidence);
            if let Some(last) = lines.last_mut().filter(|_| !cites.is_empty()) {
                if last_w + 2 + cites.width() <= text_w {
                    last.spans
                        .push(Span::styled(format!("  {cites}"), t.muted()));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(indent + 2)),
                        Span::styled(truncate(&cites, text_w), t.muted()),
                    ]));
                }
            }
            out.extend(lines);
        }
        return out;
    }
    match prep_note(app, u) {
        Some(note) => vec![Line::from(vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(truncate(&note, width.saturating_sub(indent)), t.muted()),
        ])],
        None => Vec::new(),
    }
}

/// `path` in at most `w` cells, cut at the front so the file name stays.
pub fn fit_path(path: &str, w: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let path = sanitize(path);
    if path.width() <= w {
        return path;
    }
    if w == 0 {
        return String::new();
    }
    let mut tail = Vec::new();
    let mut used = 1;
    for c in path.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w {
            break;
        }
        used += cw;
        tail.push(c);
    }
    std::iter::once('…').chain(tail.into_iter().rev()).collect()
}

/// The style of a level's icon in a row: the routine ones step back.
pub(super) fn icon_style(app: &App, level: Level) -> Style {
    match level {
        Level::Same | Level::Routine => app.theme.muted(),
        _ => level_style(app, level),
    }
}

/// The line under the tree when units were accepted for you: the key that
/// shows or hides them, and how many. `None` when there are none.
pub fn auto_line(app: &App) -> Option<String> {
    let n = app.auto_accepted();
    if n == 0 {
        return None;
    }
    let does = if app.show_auto { "hides" } else { "shows" };
    let key = app.keys.label(Action::ToggleAutoAccepted);
    Some(format!("{key} {does} {n} accepted for you"))
}

/// Narrowest queue in which every row of the tree shows whole and the
/// line about units accepted for you fits on one line.
pub fn fit_width(app: &App) -> usize {
    let rows = app.rows();
    let row = rows.iter().map(|r| row_width(app, r)).max().unwrap_or(0);
    let auto = auto_line(app).map_or(0, |text| LEAD + text.width());
    row.max(auto) + 1
}

fn border(app: &App, text: String) -> Span<'static> {
    Span::styled(text, app.theme.muted())
}

/// Top edge of a box: `╭─ Title · N ────╮`.
pub(super) fn top_edge(app: &App, width: usize, title: &str, style: Style) -> Line<'static> {
    let inner = width.saturating_sub(2);
    let lead = usize::from(inner >= 3);
    let t = truncate(&format!(" {title} "), inner.saturating_sub(lead));
    let dashes = inner.saturating_sub(lead + t.width());
    Line::from(vec![
        border(app, "╭".into()),
        border(app, "─".repeat(lead)),
        Span::styled(t, style.add_modifier(Modifier::BOLD)),
        border(app, "─".repeat(dashes)),
        border(app, "╮".into()),
    ])
}

pub(super) fn bottom_edge(app: &App, width: usize) -> Line<'static> {
    Line::from(vec![border(
        app,
        format!("╰{}╯", "─".repeat(width.saturating_sub(2))),
    )])
}

/// Fewest list rows at which a blank line separates the boxes of improve
/// mode's list.
pub const PADDING_MIN_ROWS: usize = 14;

/// Cells of a row before its name at depth 0: the cursor's bar, the mark,
/// a gap.
const LEAD: usize = 3;
/// Cells a row is indented by for each directory it is under.
const INDENT: usize = 2;

/// What follows a file's name, each after a blank: `⚑` when one of its
/// units is flagged, `⚠` when a second model disputes one, `↻` when the
/// last re-run changed one, a spinner while one is described.
fn file_marks(app: &App, row: &TreeRow) -> Vec<Span<'static>> {
    let t = &app.theme;
    let mut marks = Vec::new();
    let Some(f) = app.file_facts(&row.path) else {
        return marks;
    };
    if f.flagged {
        marks.push(Span::styled(" ⚑", t.badge_investigate()));
    }
    if f.disputed {
        marks.push(Span::styled(" ⚠", t.risk(Risk::Medium)));
    }
    if f.changed {
        marks.push(Span::styled(" ↻", t.accent()));
    }
    if f.preparing {
        marks.push(Span::styled(format!(" {}", app.spinner()), t.muted()));
    }
    marks
}

/// Why teer can't show the file of `row`, after two blanks.
fn skipped_why(app: &App, row: &TreeRow) -> Option<String> {
    let why = app.skipped_why(&row.path)?;
    Some(format!("  {}", sanitize(why)))
}

/// The cells `row` takes, written out whole.
fn row_width(app: &App, row: &TreeRow) -> usize {
    let tail = match row.kind {
        RowKind::Skipped => skipped_why(app, row).map_or(0, |why| why.width()),
        RowKind::File => file_marks(app, row).iter().map(Span::width).sum(),
        RowKind::Dir => 0,
    };
    LEAD + INDENT * row.depth + sanitize(&row.name).width() + tail
}

/// Row `i` of the tree, `width` cells: the bar when it is under the
/// cursor, its indent, its mark, its name, and what follows it.
///
/// A file's mark is its level's icon, in the level's colour when it needs
/// a look and muted when it is routine; its name is muted too once nothing
/// is left to do on it. A directory's mark is `▾`, or `▸` while it is
/// folded, then in the colour of the most attention under it. A file teer
/// can't show is muted, with why when there is room for it. A name too
/// long is cut (a directory's chain at the front); the marks after it
/// stay. Inside the visual selection the row is marked.
fn tree_row(app: &App, i: usize, row: &TreeRow, selected: bool, width: usize) -> Line<'static> {
    let t = &app.theme;
    let quiet = t.muted();
    let (mark, mark_style, name_style, tail) = match row.kind {
        RowKind::Dir => {
            let done = app.dir_done(&row.path);
            let level = app.dir_level(&row.path).filter(|_| row.folded && !done);
            (
                if row.folded { "▸" } else { "▾" },
                level.map_or(quiet, |l| icon_style(app, l)),
                if done { quiet } else { t.text() },
                Vec::new(),
            )
        }
        RowKind::File => {
            let facts = app.file_facts(&row.path);
            let level = facts.map_or(Level::Routine, |f| f.level);
            let done = facts.is_some_and(|f| f.done);
            (
                level.icon(),
                if done { quiet } else { icon_style(app, level) },
                if done || level == Level::Same {
                    quiet
                } else {
                    t.text()
                },
                file_marks(app, row),
            )
        }
        RowKind::Skipped => (
            " ",
            quiet,
            quiet,
            skipped_why(app, row)
                .map(|why| Span::styled(why, quiet))
                .into_iter()
                .collect(),
        ),
    };
    let lead = LEAD + INDENT * row.depth;
    let mut tail_w: usize = tail.iter().map(Span::width).sum();
    let mut name = sanitize(&row.name);
    let mut tail = tail;
    // Why a file can't be shown gives way to its name.
    if row.kind == RowKind::Skipped && lead + name.width() + tail_w > width {
        tail.clear();
        tail_w = 0;
    }
    // A directory's chain is cut at the front, so its last part stays.
    let room = width.saturating_sub(lead + tail_w);
    name = match row.kind {
        RowKind::Dir => fit_path(&name, room),
        _ => truncate(&name, room),
    };
    let name_style = if selected {
        name_style.add_modifier(Modifier::BOLD)
    } else {
        name_style
    };
    let mut spans = vec![
        if selected {
            Span::styled("▌", t.accent())
        } else {
            Span::raw(" ")
        },
        Span::raw(" ".repeat(INDENT * row.depth)),
        Span::styled(format!("{mark} "), mark_style),
        Span::styled(name, name_style),
    ];
    spans.extend(tail);
    let mut line = Line::from(spans);
    clip(&mut line, width);
    let pad = width.saturating_sub(line.width());
    line.spans.push(Span::raw(" ".repeat(pad)));
    // The cursor's row has the selection's background; a row of the
    // visual selection its own, and the bar.
    let behind = if app.visual_rows().is_some_and(|rows| rows.contains(&i)) {
        line.spans[0] = Span::styled("▌", t.accent());
        Some(t.visual())
    } else {
        selected.then(|| t.selected_entry())
    };
    if let Some(bg) = behind {
        for s in line.spans.iter_mut().skip(1) {
            s.style = s.style.patch(bg);
        }
    }
    line
}

/// The queue's rows, `width` cells wide: the tree, then after a blank
/// line the line about the units accepted for you.
pub fn build_queue(app: &App, width: u16) -> Rows {
    let t = &app.theme;
    let width = width as usize;
    let tree = app.rows();
    let at = app.cursor_row();
    let mut rows = Rows {
        lines: Vec::new(),
        owner: Vec::new(),
        spans: Vec::new(),
        heads: Vec::new(),
        titles: Vec::new(),
        ends: Vec::new(),
    };
    for (i, row) in tree.iter().enumerate() {
        let line = rows.lines.len();
        rows.push(
            tree_row(app, i, row, at == Some(i), width),
            (row.kind != RowKind::Skipped).then_some(i),
        );
        rows.spans.push((line, line + 1));
        rows.heads.push(line);
        rows.titles.push(line);
        rows.ends.push(line + 1);
    }
    // Wrapped rather than cut: it names the key.
    if let Some(text) = auto_line(app) {
        if !rows.lines.is_empty() {
            rows.push(Line::raw(" ".repeat(width)), None);
        }
        let lead = " ".repeat(LEAD);
        for l in wrap(&text, width.saturating_sub(LEAD)) {
            let l = truncate_pad(&format!("{lead}{l}"), width);
            rows.push(Line::styled(l, t.muted()), None);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::keymap::Keymap;
    use crate::review::model::{Decision, LineRef, Range, Review, Side, UnitKind};
    use crate::session::names::{agent_label, cite_text};
    use std::path::PathBuf;

    fn unit(i: usize, decision: Decision) -> Unit {
        Unit {
            id: format!("f{i}.rs::a_rather_long_function_name_number_{i}"),
            file: PathBuf::from(format!("src/some/deep/dir/file_with_long_name_{i}.rs")),
            symbol: format!("a_rather_long_function_name_number_{i}"),
            kind: UnitKind::Function,
            status: UnitStatus::Changed,
            old_range: None,
            new_range: Some(Range::new(10, 20)),
            nested: Vec::new(),
            renamed_from: None,
            fingerprint: String::new(),
            changes: Vec::new(),
            prep: Prep::Unavailable,
            callers: Vec::new(),
            callees: Vec::new(),
            decision,
            summary: Some("Does something fairly involved with a long description text".into()),
            nbc_reason: None,
            dive: Default::default(),
            accepted_by: None,
            reasons: Vec::new(),
            dive_pending: Default::default(),
            checks: Default::default(),
            security: Vec::new(),
            signature: None,
        }
    }

    fn app() -> App {
        let decisions = [Decision::Accepted, Decision::Investigate, Decision::Pending];
        let review = Review {
            id: "x".into(),
            repo_root: PathBuf::from("/r"),
            base: "b".into(),
            head: "h".into(),
            head_commit: "h".into(),
            agent_session: None,
            pr: None,
            intent: Default::default(),
            created_at: chrono::Utc::now(),
            files: Vec::new(),
            units: decisions
                .iter()
                .enumerate()
                .map(|(i, d)| unit(i, *d))
                .collect(),
            skipped: Vec::new(),
            graph: Default::default(),
            questions: Vec::new(),
            version: 0,
            history: Default::default(),
            branch: None,
            cursor: None,
            mode: None,
        };
        let mut app = App::new(review, Config::default(), Keymap::default());
        app.rebuild_order();
        app
    }

    fn build(app: &App, width: u16) -> Rows {
        build_queue(app, width)
    }

    fn text_of(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.to_string()).collect()
    }

    /// The rows as text, without the blanks at their ends.
    fn texts(app: &App, width: u16) -> Vec<String> {
        let rows = build(app, width);
        rows.lines
            .iter()
            .map(|l| text_of(l).trim_end().to_string())
            .collect()
    }

    const DIR: &str = "src/some/deep/dir";

    #[test]
    fn lines_fit_exactly_at_any_width() {
        let app = app();
        for width in [30u16, 60, 80, 120] {
            let rows = build(&app, width);
            // The directory and its three files.
            assert_eq!(rows.spans.len(), 4);
            for l in &rows.lines {
                assert_eq!(l.width(), width as usize, "width {width}: {l:?}");
            }
        }
    }

    #[test]
    fn rows_never_exceed_width_at_tiny_widths() {
        let mut app = app();
        app.review.units[0].symbol = "bad\nname\twith\0controls\r".into();
        app.review.units[0].summary = Some("line one\nline two\u{85}\u{1b}[31m".into());
        app.review.units[0].file = PathBuf::from("src/we\nird\u{1b}name.rs");
        app.review.skipped.push(crate::git::SkippedFile {
            path: PathBuf::from("src/lo\tgo.png"),
            status: crate::git::FileStatus::Modified,
            why: "bin\nary".into(),
        });
        app.rebuild_order();
        for width in 0u16..=40 {
            let rows = build(&app, width);
            for l in &rows.lines {
                assert!(l.width() <= width as usize, "{width}: {l:?}");
                for s in &l.spans {
                    assert!(!s.content.chars().any(char::is_control), "{width}: {s:?}");
                }
            }
        }
    }

    fn cites(refs: &[(Side, u32)]) -> Vec<LineRef> {
        refs.iter()
            .map(|&(side, line)| LineRef { side, line })
            .collect()
    }

    #[test]
    fn cited_lines_merge_into_ranges_new_side_first() {
        use Side::{New, Old};
        assert_eq!(cite_text(&cites(&[(New, 168)])), "L168");
        assert_eq!(
            cite_text(&cites(&[
                (Old, 193),
                (New, 170),
                (New, 168),
                (New, 169),
                (New, 172),
                (Old, 192),
                (New, 168),
            ])),
            "L168-170, L172, old L192-193"
        );
        assert_eq!(cite_text(&cites(&[(Old, 5)])), "old L5");
        assert_eq!(cite_text(&[]), "");
        // Scattered lines: the first three runs, then how many more.
        assert_eq!(
            cite_text(&cites(&[(New, 1), (New, 3), (New, 5), (New, 7), (Old, 2)])),
            "L1, L3, L5, +2 more"
        );
    }

    #[test]
    fn models_are_named_and_default_says_whose() {
        let mut app = app();
        assert_eq!(model_name(&app, Role::Fast), "haiku");
        assert_eq!(model_name(&app, Role::Deep), "Claude Code's default model");
        for m in ["default", "Default", "", "  "] {
            app.cfg.prepare_model = Some(m.into());
            assert_eq!(
                model_name(&app, Role::Fast),
                "Claude Code's default model",
                "{m:?}"
            );
        }
        app.cfg.provider = Some(crate::config::Provider::Codex);
        assert_eq!(model_name(&app, Role::Deep), "Codex's default model");
        app.cfg.provider = Some(crate::config::Provider::Api);
        app.cfg.second_model = Some(" claude-sonnet-5-5 ".into());
        assert_eq!(model_name(&app, Role::Deep), "claude-sonnet-5-5");
        app.cfg.provider = None;
        app.cfg.second_model = None;
        app.cfg.prepare_model = None;
        let u = app.review.units[0].clone();
        assert_eq!(agent_label(&app, &u), "agent (haiku)");
        app.cfg.prepare_model = Some("default".into());
        let mut u = u;
        u.checks.second = Some(crate::review::model::SecondOpinion {
            risk: crate::review::model::Risk::Low,
            verdicts: Vec::new(),
            missed: Vec::new(),
        });
        assert_eq!(
            agent_label(&app, &u),
            "agent (Claude Code's default model, sonnet)"
        );
    }

    #[test]
    fn statements_end_with_the_lines_they_cite() {
        use crate::review::model::{Change, Marker, Risk};
        use Side::{New, Old};
        let mut app = app();
        let u = &mut app.review.units[0];
        u.prep = Prep::Ready;
        u.changes = vec![
            Change {
                marker: Marker::Added,
                statement: "Rejects unknown currencies.".into(),
                evidence: cites(&[(New, 12), (New, 13)]),
                risk: Risk::High,
            },
            Change {
                marker: Marker::Removed,
                statement: "No longer sends the receipt email to the customer after paying.".into(),
                evidence: cites(&[(Old, 15)]),
                risk: Risk::Low,
            },
        ];
        let u = app.review.units[0].clone();
        let lines: Vec<String> = statement_lines(&app, &u, 41, 1)
            .iter()
            .map(text_of)
            .collect();
        assert_eq!(
            lines,
            [
                " + Rejects unknown currencies.  L12-13",
                " − No longer sends the receipt email to",
                "   the customer after paying.  old L15",
            ]
        );
        assert!(lines.iter().all(|l| l.width() < 41), "{lines:?}");
    }

    #[test]
    fn a_row_is_a_mark_a_name_and_only_what_there_is_to_act_on() {
        let app = app();
        assert_eq!(
            texts(&app, 80),
            [
                format!(" ▾ {DIR}"),
                "▌  ┃ file_with_long_name_0.rs".into(),
                "   ┃ file_with_long_name_1.rs ⚑".into(),
                "   ┃ file_with_long_name_2.rs".into(),
            ]
        );
        let rows = build(&app, 80);
        for (i, &(s, e)) in rows.spans.iter().enumerate() {
            assert_eq!((e - s, rows.owner[s]), (1, Some(i)));
        }
    }

    #[test]
    fn a_name_too_long_is_cut_and_its_marks_stay() {
        let app = app();
        let rows = texts(&app, 24);
        assert_eq!(rows[2], "   ┃ file_with_long_n… ⚑");
    }

    #[test]
    fn a_file_teer_cant_show_says_why_while_there_is_room() {
        let mut app = app();
        app.review.skipped.push(crate::git::SkippedFile {
            path: PathBuf::from(format!("{DIR}/logo.png")),
            status: crate::git::FileStatus::Modified,
            why: "binary".into(),
        });
        let rows = build(&app, 80);
        let at = rows.lines.len() - 1;
        assert_eq!(text_of(&rows.lines[at]).trim_end(), "     logo.png  binary");
        // The cursor skips it.
        assert_eq!(rows.owner[at], None);
        assert_eq!(texts(&app, 16)[at], "     logo.png");
        // The list is as wide as it takes to say it.
        assert!(fit_width(&app) > "     logo.png  binary".len());
    }

    #[test]
    fn the_units_accepted_for_you_are_one_line_that_names_its_key() {
        let mut app = app();
        assert_eq!(auto_line(&app), None);
        app.review.units[2].decision = Decision::Accepted;
        app.review.units[2].accepted_by = Some(crate::review::model::AcceptedBy::Auto);
        assert_eq!(
            auto_line(&app).as_deref(),
            Some("za shows 1 accepted for you")
        );
        let rows = texts(&app, 80);
        let n = rows.len();
        assert_eq!(rows[n - 2], "");
        assert_eq!(rows[n - 1], "   za shows 1 accepted for you");
        app.show_auto = true;
        assert_eq!(
            auto_line(&app).as_deref(),
            Some("za hides 1 accepted for you")
        );
    }

    /// Style of the cell at `x` on tree row `i`, read back from a rendered
    /// buffer.
    fn cell_style(app: &App, i: usize, x: u16, width: u16) -> Style {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::widgets::{Paragraph, Widget};
        let rows = build(app, width);
        let line = rows.lines[rows.spans[i].0].clone();
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        Paragraph::new(line).render(area, &mut buf);
        buf[(x, 0)].style()
    }

    /// The tree row of the file whose unit is decided `d`.
    fn row_of(app: &App, d: Decision) -> usize {
        let u = app.review.units.iter().find(|u| u.decision == d).unwrap();
        app.rows().iter().position(|r| r.path == u.file).unwrap()
    }

    fn themed_app(theme: crate::config::Theme) -> App {
        let mut app = app();
        app.theme = crate::tui::theme::Theme::new(theme);
        app.cfg.theme = theme;
        app
    }

    // Cells of a file's row under one directory: the bar 0, the mark 3,
    // the name from 5.
    const MARK_X: u16 = 3;
    const NAME_X: u16 = 5;

    #[test]
    fn a_file_with_nothing_left_to_do_is_dimmed_and_the_others_are_not() {
        use crate::config::Theme;
        for theme in [Theme::Dark, Theme::Light, Theme::Mono] {
            let app = themed_app(theme);
            let muted = app.theme.muted();
            let dimmed = |s: Style| {
                if theme == Theme::Mono {
                    s.add_modifier.contains(Modifier::DIM)
                } else {
                    s.fg == muted.fg
                }
            };
            let done = row_of(&app, Decision::Accepted);
            assert!(dimmed(cell_style(&app, done, NAME_X, 120)), "{theme:?}");
            assert!(dimmed(cell_style(&app, done, MARK_X, 120)), "{theme:?}");
            for d in [Decision::Pending, Decision::Investigate] {
                let row = row_of(&app, d);
                assert!(!dimmed(cell_style(&app, row, NAME_X, 120)), "{theme:?}");
            }
        }
    }

    #[test]
    fn the_mark_has_the_colour_of_the_level_and_a_routine_one_is_calm() {
        let mut app = app();
        let pending = app
            .review
            .units
            .iter()
            .position(|u| u.decision == Decision::Pending);
        let row = row_of(&app, Decision::Pending);
        for (level, icon) in [
            (Level::Attention, "┃"),
            (Level::Look, "┃"),
            (Level::Routine, "┃"),
        ] {
            app.levels[pending.unwrap()].level = level;
            assert!(texts(&app, 80)[row].starts_with(&format!("   {icon} ")));
            let mark = cell_style(&app, row, MARK_X, 80);
            let want = match level {
                Level::Routine => app.theme.muted().fg,
                _ => level_style(&app, level).fg,
            };
            assert_eq!(mark.fg, want, "{level:?}");
        }
        assert_ne!(app.theme.muted().fg, level_style(&app, Level::Look).fg);
    }

    #[test]
    fn a_folded_directory_has_the_colour_of_the_most_attention_under_it() {
        let mut app = app();
        let pending = app
            .review
            .units
            .iter()
            .position(|u| u.decision == Decision::Pending);
        app.levels[pending.unwrap()].level = Level::Attention;
        // Open: its files say it.
        assert_eq!(cell_style(&app, 0, 1, 80).fg, app.theme.muted().fg);
        app.folded.insert(PathBuf::from(DIR));
        app.dir = Some(PathBuf::from(DIR));
        assert_eq!(texts(&app, 80), [format!("▌▸ {DIR}")]);
        assert_eq!(cell_style(&app, 0, 1, 80).fg, Some(app.theme.risk_high));
        // With nothing left to do under it, it is calm.
        for u in &mut app.review.units {
            u.decision = Decision::Accepted;
        }
        assert_eq!(cell_style(&app, 0, 1, 80).fg, app.theme.muted().fg);
        assert_eq!(cell_style(&app, 0, 3, 80).fg, app.theme.muted().fg);
    }

    #[test]
    fn a_flagged_file_has_the_flag_after_its_name() {
        use crate::config::Theme;
        for theme in [Theme::Dark, Theme::Mono] {
            let app = themed_app(theme);
            let row = row_of(&app, Decision::Investigate);
            let flag = NAME_X + "file_with_long_name_1.rs ".len() as u16;
            let (got, want) = (
                cell_style(&app, row, flag, 120),
                app.theme.badge_investigate(),
            );
            assert_eq!(got.add_modifier, want.add_modifier, "{theme:?}");
            if theme == Theme::Dark {
                assert_eq!(got.fg, want.fg);
            }
        }
    }

    #[test]
    fn a_file_the_last_rerun_changed_says_so_after_its_name() {
        let mut app = app();
        let id = app.review.units[2].id.clone();
        app.changed.insert(id);
        assert_eq!(texts(&app, 80)[3], "   ┃ file_with_long_name_2.rs ↻");
    }

    #[test]
    fn the_cursors_row_has_the_bar_a_bold_name_and_the_selections_background() {
        use crate::config::Theme;
        for theme in [Theme::Dark, Theme::Mono] {
            let mut app = themed_app(theme);
            let row = row_of(&app, Decision::Accepted);
            app.select_row(row);
            let rows = build(&app, 120);
            let line = &rows.lines[rows.spans[row].0];
            assert_eq!(line.spans[0].content, "▌");
            assert_eq!(line.width(), 120);
            let name = cell_style(&app, row, NAME_X, 120);
            assert!(name.add_modifier.contains(Modifier::BOLD));
            // To the row's end, on the blanks too.
            use ratatui::style::Color::Reset;
            let bg = |i, x| cell_style(&app, i, x, 120).bg.unwrap_or(Reset);
            for x in [NAME_X, 119] {
                assert_eq!(bg(row, x), app.theme.selected_entry().bg.unwrap_or(Reset));
            }
            // The other rows have neither.
            let other = row_of(&app, Decision::Pending);
            let rows = build(&app, 120);
            assert_eq!(rows.lines[rows.spans[other].0].spans[0].content, " ");
            assert_eq!(bg(other, NAME_X), Reset);
        }
    }

    #[test]
    fn visual_rows_run_from_the_anchor_to_the_cursor() {
        use crate::config::Theme;
        for theme in [Theme::Dark, Theme::Mono] {
            let mut app = themed_app(theme);
            let marked = |app: &App, i: usize| {
                let s = cell_style(app, i, NAME_X, 120);
                if theme == Theme::Mono {
                    s.add_modifier.contains(Modifier::REVERSED)
                } else {
                    s.bg.is_some() && s.bg == app.theme.visual().bg
                }
            };
            let bar = |app: &App, i: usize| {
                let rows = build(app, 120);
                rows.lines[rows.spans[i].0].spans[0].content.to_string()
            };
            for (anchor, at) in [(1, 2), (2, 1)] {
                app.visual = Some(app.rows()[anchor].id());
                app.select_row(at);
                assert!(marked(&app, 1) && marked(&app, 2), "{theme:?}");
                assert_eq!((bar(&app, 1), bar(&app, 2)), ("▌".into(), "▌".into()));
                assert_eq!(bar(&app, 3), " ");
                if theme == Theme::Mono {
                    assert!(!marked(&app, 3));
                }
            }
        }
    }
}
