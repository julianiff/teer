//! The review list, file by file, in a box per level: needs a close look,
//! worth a look, routine. A file sits in the box of the most attention one
//! of its units needs. Each file's name is a label nobody selects (with how
//! many of its units are decided on the right); its units, the functions
//! and other parts it changed, sit under it, most attention first, each
//! behind its level's icon: `▲` needs a close look, `◆` worth a look, `●`
//! routine. The cursor only ever moves over units. Units with no behavior
//! change follow in their file, muted with why (`○`), and nobody selects
//! them. Two muted boxes close the list: files with nothing but such units,
//! and changed files teer can't show. Each box is a rounded box with a
//! titled top edge, drawn by hand as plain lines so scrolling and click
//! mapping stay line based. A row holds the unit's name; the reasons,
//! summary and code change live in the preview pane. Accepted rows are
//! dimmed, removed names struck through, flagged names in the flagged
//! style.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::app::App;
use crate::review::attention::Level;
use crate::review::model::{Decision, LineRef, Prep, Side, Unit, UnitKind, UnitStatus};
use crate::units::extract::MODULE_SYMBOL;
use crate::util::{sanitize, truncate, truncate_pad, wrap};

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct Rows {
    pub lines: Vec<Line<'static>>,
    /// Which unit, by position in `App::order`, each line is.
    pub owner: Vec<Option<usize>>,
    /// First and one-past-last line of each unit, by position.
    pub spans: Vec<(usize, usize)>,
    /// Line of the name of each unit's file, by position.
    pub heads: Vec<usize>,
    /// Line of the title edge of the box each unit's file sits in, by position.
    pub titles: Vec<usize>,
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

/// Text badge for the decision ("✓ accepted"), empty when pending.
pub fn badge_text(app: &App, u: &Unit) -> Span<'static> {
    let t = &app.theme;
    match u.decision {
        Decision::Accepted => Span::styled("✓ accepted", t.muted()),
        Decision::Investigate => Span::styled("⚑ investigate", t.muted()),
        Decision::Pending => Span::raw(""),
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
    match level {
        Level::Same => t.muted(),
        Level::Routine => t.text(),
        Level::Look => Style::new().fg(t.risk_medium),
        Level::Attention => Style::new().fg(t.risk_high),
    }
}

/// A model as configured, in words: `default` (or nothing) is Claude Code's own.
pub fn model_name(model: &str) -> String {
    let m = model.trim();
    if m.is_empty() || m.eq_ignore_ascii_case("default") {
        "Claude Code's default model".into()
    } else {
        sanitize(m)
    }
}

/// Who wrote the summary and statements of `u`: `agent (haiku)`, or
/// `agent (haiku, sonnet)` once the second model has checked them. Drawn
/// above them; facts from the code carry no label.
pub fn agent_label(app: &App, u: &Unit) -> String {
    let mut models = model_name(&app.cfg.prepare_model);
    if u.checks.second.is_some() {
        models.push_str(&format!(", {}", model_name(&app.cfg.second_model)));
    }
    format!("agent ({models})")
}

/// Runs of cited lines shown before the rest is counted.
const CITE_RUNS: usize = 3;

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
            let cites = cite_text(&c.evidence);
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

/// `N` for a single line, `N-M` for a real range, empty when unknown.
fn line_span(u: &Unit) -> String {
    match u.new_range.or(u.old_range) {
        Some(r) if r.end > r.start => format!("{}-{}", r.start, r.end),
        Some(r) => r.start.to_string(),
        None => String::new(),
    }
}

/// The name a unit goes by in rows: its symbol, `(top level)` for the
/// code outside any function, and a whole file's file name.
pub fn row_name(u: &Unit) -> String {
    match u.kind {
        UnitKind::File => file_name(&u.file),
        _ if u.kind == UnitKind::Module || u.symbol == MODULE_SYMBOL => "(top level)".into(),
        _ => sanitize(&u.symbol),
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
fn icon_style(app: &App, level: Level) -> Style {
    match level {
        Level::Same | Level::Routine => app.theme.muted(),
        _ => level_style(app, level),
    }
}

/// `path` in `w` cells, its folders muted and the file name in `style`.
fn path_spans(app: &App, path: &str, w: usize, style: Style) -> Vec<Span<'static>> {
    let fitted = fit_path(path, w);
    let (dir, name) = match fitted.rfind('/') {
        Some(i) => fitted.split_at(i + 1),
        None => ("", fitted.as_str()),
    };
    let pad = w.saturating_sub(fitted.width());
    vec![
        Span::styled(dir.to_string(), app.theme.muted()),
        Span::styled(name.to_string(), style),
        Span::raw(" ".repeat(pad)),
    ]
}

/// How far a file is: a spinner while preparing, `⚑` when a unit is
/// flagged, and decided units of all (`⠋ ⚑ 2/5`).
fn file_count(app: &App, units: &[&Unit]) -> String {
    let done = units
        .iter()
        .filter(|u| u.decision != Decision::Pending)
        .count();
    let busy = units.iter().any(|u| u.prep == Prep::Pending);
    let flagged = units.iter().any(|u| u.decision == Decision::Investigate);
    format!(
        "{}{}{done}/{}",
        if busy {
            format!("{} ", app.spinner())
        } else {
            String::new()
        },
        if flagged { "⚑ " } else { "" },
        units.len()
    )
}

/// The name of the file whose first unit is at `pos`, and how many of its
/// units are decided.
fn file_spans(app: &App, pos: usize, name_w: usize, count_w: usize) -> Vec<Span<'static>> {
    let t = &app.theme;
    let units: Vec<&Unit> = app
        .file_range(pos)
        .map(|p| &app.review.units[app.order[p]])
        .collect();
    let done = units.iter().all(|u| u.decision != Decision::Pending);
    let style = if done { t.accepted_row() } else { t.text() }.add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(" ")];
    let path = units[0].file.display().to_string();
    spans.extend(path_spans(app, &path, name_w, style));
    if count_w > 0 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{:>count_w$}", truncate(&file_count(app, &units), count_w)),
            t.muted(),
        ));
    }
    spans
}

/// A muted row nobody selects, two cells in: a file with nothing to decide
/// or a unit with no behavior change, then why.
fn muted_row(app: &App, lead: &str, name: &str, why: &str, width: usize) -> Line<'static> {
    let t = &app.theme;
    let mut line = Line::from(vec![
        Span::raw(format!("   {lead}")),
        Span::styled(sanitize(name), t.muted()),
        Span::styled(format!("  {}", sanitize(why)), t.muted()),
    ]);
    clip(&mut line, width);
    let pad = width.saturating_sub(line.width());
    line.spans.push(Span::raw(" ".repeat(pad)));
    line
}

/// Units of `file` with no behavior change, in line order.
fn same_in<'a>(app: &'a App, file: &std::path::Path) -> Vec<&'a Unit> {
    let mut same: Vec<&Unit> = app
        .review
        .units
        .iter()
        .enumerate()
        .filter(|(i, u)| u.file == file && app.level_of(*i).level == Level::Same)
        .map(|(_, u)| u)
        .collect();
    same.sort_by_key(|u| u.display_line());
    same
}

/// Why a unit has no behavior change.
fn same_reason(u: &Unit) -> String {
    u.nbc_reason
        .clone()
        .unwrap_or_else(|| "no behavior change".into())
}

/// Files with no unit to decide, by path: every unit has no behavior change.
fn quiet_files(app: &App) -> Vec<&std::path::Path> {
    let mut files: Vec<&std::path::Path> = Vec::new();
    for (i, u) in app.review.units.iter().enumerate() {
        if app.level_of(i).level == Level::Same
            && !app
                .order
                .iter()
                .any(|&o| app.review.units[o].file == u.file)
            && !files.contains(&u.file.as_path())
        {
            files.push(&u.file);
        }
    }
    files.sort();
    files
}

/// Widest count a file name can show (`⠋ ⚑ done/total`), for sizing the
/// count column; 0 when no file has a unit to decide.
fn widest_count(app: &App) -> usize {
    (0..app.order.len())
        .filter(|&pos| app.starts_file(pos))
        .map(|pos| {
            let n = app.file_range(pos).len();
            format!("⠋ ⚑ {n}/{n}").width()
        })
        .max()
        .unwrap_or(0)
}

/// Widths of a file's name row of `width` cells: the path, and the count
/// column (zero when there is none, or no room). Layout: a blank, the path,
/// 2 spaces, count. The count goes first when the path would fall under 10
/// cells.
fn column_widths(width: usize, widest: usize) -> (usize, usize) {
    if widest == 0 || width < 1 + 10 + 2 + widest {
        return (width.saturating_sub(1), 0);
    }
    (width - 1 - 2 - widest, widest)
}

/// Widest list [`fit_width`] looks at.
const FIT_MAX: usize = 512;

/// Narrowest list in which every file shows its whole path and count, every
/// unit its whole name and every level's box its whole title (why a unit
/// has no behavior change, and the muted boxes' titles, are cut as needed).
pub fn fit_width(app: &App) -> usize {
    let widest = widest_count(app);
    let path = |p: &std::path::Path| sanitize(&p.display().to_string()).width();
    // A file's path, beside its count; any other row's whole width.
    let mut file = 0;
    let mut row = 0;
    for (pos, &i) in app.order.iter().enumerate() {
        let u = &app.review.units[i];
        row = row.max(UNIT_LEAD + indent(app, i).width() + row_name(u).width());
        if app.starts_file(pos) {
            file = file.max(path(&u.file));
        }
    }
    for (i, u) in app.review.units.iter().enumerate() {
        if app.level_of(i).level == Level::Same {
            row = row
                .max(UNIT_LEAD + row_name(u).width())
                .max(3 + path(&u.file));
        }
    }
    for f in &app.review.skipped {
        row = row.max(3 + path(&f.path));
    }
    // A title, its spaces, the corners and a few dashes.
    let title = boxes_of(app)
        .into_iter()
        .map(|(level, files)| box_title(level, files.len()).width() + 7)
        .max()
        .unwrap_or(0);
    // Borders and the blank cell before the right one take 3.
    (4..FIT_MAX)
        .find(|&width| {
            let (n, c) = column_widths(width - 3, widest);
            n >= file && c >= widest && width - 3 >= row && width >= title
        })
        .unwrap_or(FIT_MAX)
}

/// The levels' boxes, most attention first: each level with the positions
/// of the first units of the files in it.
fn boxes_of(app: &App) -> Vec<(Level, Vec<usize>)> {
    let mut boxes: Vec<(Level, Vec<usize>)> = Vec::new();
    for pos in (0..app.order.len()).filter(|&p| app.starts_file(p)) {
        let level = app.file_level(pos);
        match boxes.last_mut() {
            Some((l, files)) if *l == level => files.push(pos),
            _ => boxes.push((level, vec![pos])),
        }
    }
    boxes
}

/// A box's title: `Worth a look · 3 files`.
fn box_title(level: Level, files: usize) -> String {
    format!("{} · {}", level.title(), files_word(files))
}

fn border(app: &App, text: String) -> Span<'static> {
    Span::styled(text, app.theme.muted())
}

/// Top edge of a box: `╭─ Title · N ────╮`.
fn top_edge(app: &App, width: usize, title: &str, style: Style) -> Line<'static> {
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

fn bottom_edge(app: &App, width: usize) -> Line<'static> {
    Line::from(vec![border(
        app,
        format!("╰{}╯", "─".repeat(width.saturating_sub(2))),
    )])
}

/// Cells before a unit's name: the gutter, two blanks, its icon and a blank.
const UNIT_LEAD: usize = 5;

/// What unit `ui`'s name is indented by in a story: `↳ ` under the unit
/// that calls it, two more cells per level down to the third.
fn indent(app: &App, ui: usize) -> String {
    match app.depth.get(ui).copied().unwrap_or(0) {
        0 => String::new(),
        depth => format!("{}↳ ", "  ".repeat(depth.min(3) - 1)),
    }
}

pub fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Fewest list rows at which a blank line separates the boxes, and the
/// files inside them.
pub const PADDING_MIN_ROWS: usize = 14;

/// The rows with blank lines between boxes and files.
pub fn build(app: &App, width: u16) -> Rows {
    build_with(app, width, true)
}

/// The unit row at `pos`, `width` cells: the name takes what the icon and
/// indent leave, the count column of file names included.
fn unit_row(app: &App, pos: usize, width: usize) -> Line<'static> {
    let t = &app.theme;
    let ui = app.order[pos];
    let u = &app.review.units[ui];
    let selected = pos == app.sel;
    let name_style = if u.decision == Decision::Accepted {
        t.accepted_row()
    } else if u.decision == Decision::Investigate {
        t.flagged_row()
    } else if selected {
        t.accent()
    } else {
        Style::new()
    };
    let name_style = if selected {
        name_style.add_modifier(Modifier::BOLD)
    } else {
        name_style
    };
    let name_style = if u.status == UnitStatus::Removed {
        name_style.add_modifier(Modifier::CROSSED_OUT)
    } else {
        name_style
    };
    let level = app.level_of(ui).level;
    let icon = if u.decision == Decision::Accepted {
        t.accepted_row()
    } else {
        icon_style(app, level)
    };
    let indent = indent(app, ui);
    // A pencil at the end says the reviewer wrote a note on it.
    let noted = !u.dive.note.trim().is_empty();
    let mark = if noted { 2 } else { 0 };
    let name_w = width.saturating_sub(UNIT_LEAD + indent.width() + mark);
    let mut spans = vec![
        if selected {
            Span::styled("▌", t.accent())
        } else {
            Span::raw(" ")
        },
        Span::raw("  "),
        Span::styled(format!("{} ", level.icon()), icon),
        Span::styled(indent, t.muted()),
        // The file and lines are in the preview; the row is just the name.
        Span::styled(truncate_pad(&row_name(u), name_w), name_style),
    ];
    if noted {
        spans.push(Span::styled(" ✎", t.accent()));
    }
    Line::from(spans)
}

/// Rows being built: the lines so far, and how a content line of `inner`
/// cells is put between the box's borders.
struct Builder<'a> {
    app: &'a App,
    rows: Rows,
    width: usize,
    /// Content cells of a row: inside the borders and the blank before the
    /// right one, or the whole width when there is no room for a box.
    inner: usize,
    boxes: bool,
    padded: bool,
    /// Where the last file's rows start in the box being drawn.
    last: Option<usize>,
}

impl Builder<'_> {
    fn row(&mut self, mut spans: Vec<Span<'static>>, owner: Option<usize>) {
        let line = if self.boxes {
            spans.insert(0, border(self.app, "│".into()));
            spans.push(Span::raw(" "));
            spans.push(border(self.app, "│".into()));
            Line::from(spans)
        } else {
            Line::from(spans)
        };
        self.rows.push(line, owner);
    }

    /// Close the box being drawn and open one titled `title`, a blank line
    /// between them. Below 4 cells there are no boxes: nothing is drawn.
    fn open_box(&mut self, title: &str, style: Style) -> usize {
        self.close_box();
        if self.padded && !self.rows.lines.is_empty() {
            self.rows.push(Line::raw(" ".repeat(self.width)), None);
        }
        let line = self.rows.lines.len();
        if self.boxes {
            let edge = top_edge(self.app, self.width, title, style);
            self.rows.push(edge, None);
        }
        line
    }

    fn close_box(&mut self) {
        if self.boxes && self.last.is_some() {
            let edge = bottom_edge(self.app, self.width);
            self.rows.push(edge, None);
        }
        self.last = None;
    }

    /// Before a file's rows: a blank line separates a file with more than
    /// its own row from its neighbours in the box; files teer can't show,
    /// one row each, sit together. `one`: this file is one row.
    fn file_gap(&mut self, one: bool) {
        if let Some(start) = self.last
            && self.padded
            && !(one && self.rows.lines.len() - start == 1)
        {
            self.row(vec![Span::raw(" ".repeat(self.inner))], None);
        }
        self.last = Some(self.rows.lines.len());
    }

    /// The muted rows of `file`'s units with no behavior change.
    fn same_rows(&mut self, file: &std::path::Path) {
        for u in same_in(self.app, file) {
            let line = muted_row(self.app, "○ ", &row_name(u), &same_reason(u), self.inner);
            self.row(line.spans, None);
        }
    }
}

pub fn build_with(app: &App, width: u16, padded: bool) -> Rows {
    let t = &app.theme;
    let width = width as usize;
    // Below 4 cells there is no room for a box: bare rows.
    let boxes = width >= 4;
    let inner = if boxes { width - 3 } else { width };
    let mut b = Builder {
        app,
        rows: Rows {
            lines: Vec::new(),
            owner: Vec::new(),
            spans: vec![(0, 0); app.order.len()],
            heads: vec![0; app.order.len()],
            titles: vec![0; app.order.len()],
        },
        width,
        inner,
        boxes,
        padded: padded && boxes,
        last: None,
    };
    let (name_w, count_w) = column_widths(inner, widest_count(app));
    for (level, files) in boxes_of(app) {
        let title = b.open_box(&box_title(level, files.len()), level_style(app, level));
        for &start in &files {
            b.file_gap(false);
            let head = b.rows.lines.len();
            b.row(file_spans(app, start, name_w, count_w), None);
            for pos in app.file_range(start) {
                let line = b.rows.lines.len();
                b.row(unit_row(app, pos, inner).spans, Some(pos));
                b.rows.spans[pos] = (line, line + 1);
                b.rows.heads[pos] = head;
                b.rows.titles[pos] = title;
            }
            b.same_rows(app.file_at(start));
        }
    }
    // Two muted boxes nobody selects: files with nothing to decide, then
    // changed files teer can't show.
    let quiet = quiet_files(app);
    if !quiet.is_empty() {
        let title = format!("{} · {}", Level::Same.title(), files_word(quiet.len()));
        b.open_box(&title, level_style(app, Level::Same));
        for file in quiet {
            b.file_gap(false);
            let mut spans = vec![Span::raw("   ")];
            let path = file.display().to_string();
            spans.extend(path_spans(app, &path, inner.saturating_sub(3), t.muted()));
            b.row(spans, None);
            b.same_rows(file);
        }
    }
    let skipped = &app.review.skipped;
    if !skipped.is_empty() {
        let title = format!("Not shown · {}", files_word(skipped.len()));
        b.open_box(&title, level_style(app, Level::Same));
        for f in skipped {
            b.file_gap(true);
            let path = sanitize(&f.path.display().to_string());
            b.row(muted_row(app, "", &path, &f.why, inner).spans, None);
        }
    }
    b.close_box();
    let mut rows = b.rows;
    for line in &mut rows.lines {
        clip(line, width);
    }
    rows
}

/// `1 file`, `3 files`.
fn files_word(n: usize) -> String {
    format!("{n} file{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, KeyMap};
    use crate::review::model::{Range, Review};
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
            dive_pending: Default::default(),
            checks: Default::default(),
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
        };
        App::new(review, Config::default(), KeyMap::default())
    }

    fn text_of(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn lines_fit_exactly_at_any_width() {
        let app = app();
        for width in [30u16, 60, 80, 120] {
            let rows = build(&app, width);
            assert_eq!(rows.spans.len(), app.order.len());
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
        assert_eq!(model_name("haiku"), "haiku");
        assert_eq!(model_name(" claude-sonnet-5-5 "), "claude-sonnet-5-5");
        for m in ["default", "Default", "", "  "] {
            assert_eq!(model_name(m), "Claude Code's default model", "{m:?}");
        }
        let mut app = app();
        let u = app.review.units[0].clone();
        assert_eq!(agent_label(&app, &u), "agent (haiku)");
        app.cfg.prepare_model = "default".into();
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
    fn the_count_column_fits_the_counts_and_drops_first_when_narrow() {
        // No file with a unit to decide: no column.
        assert_eq!(column_widths(58, 0), (57, 0));
        assert_eq!(column_widths(58, 9), (58 - 3 - 9, 9));
        // No room for a name of 10 next to the count: the count goes.
        assert_eq!(column_widths(21, 9), (20, 0));
        assert_eq!(column_widths(22, 9), (10, 9));
        for inner in 0..120 {
            let (n, f) = column_widths(inner, 9);
            assert!(f == 0 || (f == 9 && n >= 10), "{inner}: {n} {f}");
            assert!(
                1 + n + if f > 0 { 2 + f } else { 0 } <= inner.max(1),
                "{inner}"
            );
        }
    }

    #[test]
    fn rows_are_just_the_icon_and_the_name() {
        let app = app();
        for width in [60u16, 80, 120] {
            let rows = build(&app, width);
            for pos in 0..app.order.len() {
                let t = text_of(&rows.lines[rows.spans[pos].0]);
                assert!(t.starts_with('│') && t.ends_with('│'), "{t:?}");
                let t: String = t.chars().skip(2).collect();
                assert!(t.starts_with("  ◆ a_rather_long"), "{t:?}");
                // Where the unit is, the preview says.
                assert!(!t.contains(":10-20"), "{t:?}");
                assert!(!t.contains("Does something"), "{t:?}");
                for g in ["✓", "⚑", "+", "~", "−"] {
                    assert!(!t.contains(g), "{g} in {t:?}");
                }
            }
        }
    }

    #[test]
    fn each_unit_sits_under_its_files_name_in_its_levels_box() {
        let rows = build(&app(), 80);
        let title = text_of(&rows.lines[0]);
        assert!(title.starts_with("╭─ Worth a look · 3 files ─"), "{title}");
        let first = text_of(&rows.lines[1]);
        assert!(
            first.starts_with("│ src/some/deep/dir/file_with_long_name_0.rs"),
            "{first}"
        );
        assert!(first.ends_with("1/1 │"), "{first}");
        assert_eq!(rows.owner[1], None, "a file's name is not selectable");
        for (pos, &(s, _)) in rows.spans.iter().enumerate() {
            assert_eq!(rows.titles[pos], 0);
            assert_eq!(rows.heads[pos] + 1, s);
            assert!(text_of(&rows.lines[rows.heads[pos]]).contains(&format!("_{pos}.rs")));
        }
    }

    #[test]
    fn every_unit_is_exactly_one_line() {
        let app = app();
        for width in [30u16, 60, 80, 120] {
            let rows = build(&app, width);
            for (pos, &(s, e)) in rows.spans.iter().enumerate() {
                assert_eq!(e - s, 1, "pos {pos} width {width}");
                assert_eq!(rows.owner[s], Some(pos));
                assert_eq!(rows.lines[s].width(), width as usize);
            }
        }
    }

    /// Style of the cell at `x` on the first line of the row at `pos`,
    /// read back from a rendered buffer.
    fn cell_style(app: &App, pos: usize, x: u16, width: u16) -> Style {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::widgets::{Paragraph, Widget};
        let rows = build(app, width);
        let line = rows.lines[rows.spans[pos].0].clone();
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        Paragraph::new(line).render(area, &mut buf);
        buf[(x, 0)].style()
    }

    fn pos_of(app: &App, d: Decision) -> usize {
        app.order
            .iter()
            .position(|&i| app.review.units[i].decision == d)
            .unwrap()
    }

    fn themed_app(theme: crate::config::Theme) -> App {
        let mut app = app();
        app.theme = crate::tui::theme::Theme::new(theme);
        app.cfg.theme = theme;
        app
    }

    // Cells in a unit row: border 0, gutter 1, icon 4, name from 6.
    const NAME_X: u16 = 6;

    #[test]
    fn accepted_rows_are_dimmed_and_pending_rows_are_not() {
        use crate::config::Theme;
        for theme in [Theme::Dark, Theme::Light, Theme::Mono] {
            let mut app = themed_app(theme);
            // Keep the selection off the rows under test.
            app.sel = pos_of(&app, Decision::Investigate);
            let acc = pos_of(&app, Decision::Accepted);
            let pend = pos_of(&app, Decision::Pending);
            let muted = app.theme.accepted_row();
            let x = NAME_X;
            let a = cell_style(&app, acc, x, 120);
            let p = cell_style(&app, pend, x, 120);
            assert_ne!(a, p, "{theme:?}");
            if theme == Theme::Mono {
                assert!(a.add_modifier.contains(Modifier::DIM), "{theme:?}");
                assert!(!p.add_modifier.contains(Modifier::DIM), "{theme:?}");
            } else {
                assert_eq!(a.fg, muted.fg, "{theme:?}");
                assert_ne!(p.fg, muted.fg, "{theme:?}");
            }
        }
    }

    #[test]
    fn flagged_names_have_their_own_style_and_are_not_greyed() {
        use crate::config::Theme;
        for theme in [Theme::Dark, Theme::Mono] {
            let mut app = themed_app(theme);
            app.sel = pos_of(&app, Decision::Pending);
            let inv = cell_style(&app, pos_of(&app, Decision::Investigate), NAME_X, 120);
            let pend = cell_style(&app, pos_of(&app, Decision::Pending), NAME_X, 120);
            assert_ne!(inv, pend, "{theme:?}");
            assert!(!inv.add_modifier.contains(Modifier::DIM), "{theme:?}");
            if theme == Theme::Mono {
                assert!(inv.add_modifier.contains(Modifier::UNDERLINED));
            } else {
                assert_eq!(inv.fg, app.theme.flagged_row().fg, "{theme:?}");
            }
            assert!(inv.add_modifier.contains(Modifier::BOLD), "{theme:?}");
        }
    }

    #[test]
    fn removed_names_are_struck_through() {
        let mut app = app();
        app.review.units[2].status = UnitStatus::Removed;
        app.sel = pos_of(&app, Decision::Accepted);
        let pos = pos_of(&app, Decision::Pending);
        assert!(
            cell_style(&app, pos, NAME_X, 120)
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
        let other = pos_of(&app, Decision::Investigate);
        assert!(
            !cell_style(&app, other, NAME_X, 120)
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
    }

    #[test]
    fn selected_accepted_row_keeps_gutter_and_dimmed_text() {
        use crate::config::Theme;
        for theme in [Theme::Dark, Theme::Mono] {
            let mut app = themed_app(theme);
            app.sel = pos_of(&app, Decision::Accepted);
            let pos = app.sel;
            let rows = build(&app, 120);
            let line = &rows.lines[rows.spans[pos].0];
            assert_eq!(line.spans[1].content, "▌");
            assert_eq!(line.width(), 120);
            let name = cell_style(&app, pos, NAME_X, 120);
            if theme == Theme::Mono {
                assert!(name.add_modifier.contains(Modifier::DIM));
            } else {
                assert_eq!(name.fg, app.theme.accepted_row().fg);
            }
            // The gutter keeps the selection style.
            let gutter = cell_style(&app, pos, 1, 120);
            if theme == Theme::Mono {
                assert!(gutter.add_modifier.contains(Modifier::BOLD));
            } else {
                assert_eq!(gutter.fg, app.theme.accent().fg);
            }
        }
    }
}
