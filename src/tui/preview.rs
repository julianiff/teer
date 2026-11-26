//! The pane beside the queue's tree (below it on a narrow terminal): the
//! changes of the file under the cursor. Each of its listed units in line
//! order, in one text that scrolls: a head line (the level's icon, the
//! unit's name, and `✓` or `⚑` once it is decided), what the agent says it
//! does, then its code change with the agent's statements written into it
//! (see [`super::diff`]), the reviewer's note and the answers to what the
//! reviewer asked about it. A file that is one whole
//! unit has no head line: the tree names it. On a directory the pane is
//! empty.
//!
//! Also what other screens say about a unit (its head, its review, why it
//! has its level), and the scrolling of a text cut to its rows.

use std::ops::Range;
use std::path::Path;

use crate::session::names;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Target};
use super::slim;
use crate::review::model::{Decision, Prep, Risk, Unit, UnitKind, UnitStatus};
use crate::util::{truncate, wrap};

/// Width at which the pane sits beside the list.
const SIDE_WIDTH: u16 = 110;
/// The list never gets fewer rows than this when a bottom pane is drawn.
const MIN_LIST_ROWS: u16 = 3;
/// Bounds of a bottom pane, its top edge included.
const MIN_PANE_ROWS: u16 = 5;
const MAX_PANE_ROWS: u16 = 24;
/// Narrowest list beside the pane; it is never wider than [`SIDE_LIST_MOST`].
const MIN_SIDE_LIST: u16 = 24;
/// Widest list beside the pane, in percent: the code change gets the rest.
const SIDE_LIST_MOST: u32 = 45;
/// Most lines of what the agent says a unit does, over its code.
const SUMMARY_LINES: usize = 2;
/// Cells a unit's text is indented by: under its name, past the bar and
/// the icon.
const UNDER_NAME: usize = 3;

/// Which edge of the list the pane sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Side,
    Bottom,
}

/// What the list needs to show all of itself: its width (every row whole)
/// and its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListSize {
    pub width: u16,
    pub rows: u16,
}

impl ListSize {
    /// A list that takes all the room it is given.
    pub const FULL: ListSize = ListSize {
        width: u16::MAX,
        rows: u16::MAX,
    };

    /// The size of `app`'s queue, as far as a layout of `area` needs it.
    pub fn of(app: &App, area: Rect) -> ListSize {
        if area.width >= SIDE_WIDTH {
            let width = slim::fit_width(app).min(u16::MAX as usize) as u16;
            return ListSize {
                width,
                ..ListSize::FULL
            };
        }
        // Every row of the tree is a line: a list this long takes all it gets.
        if app.rows().len() >= area.height as usize {
            return ListSize::FULL;
        }
        ListSize {
            rows: slim::build_queue(app, area.width).lines.len() as u16,
            ..ListSize::FULL
        }
    }
}

/// Splits `area` into the list and the pane, which is always there unless
/// the area is too small to hold one. At 110 columns or more the pane sits
/// beside the list, and the list is as wide as `list` needs, at least 24
/// columns and at most 45%. Otherwise the pane sits below and takes about
/// 60% of the height (5 to 24 rows), and more when the list leaves more
/// free; the list keeps its rows, at least 3. A pane under 2 rows is not
/// drawn.
pub fn layout_fit(area: Rect, list: ListSize) -> (Rect, Option<(Rect, Pane)>) {
    if area.width == 0 || area.height == 0 {
        return (area, None);
    }
    if area.width >= SIDE_WIDTH {
        let most = (u32::from(area.width) * SIDE_LIST_MOST / 100) as u16;
        let width = list.width.clamp(MIN_SIDE_LIST, most);
        let [list, pane] =
            Layout::horizontal([Constraint::Length(width), Constraint::Min(0)]).areas(area);
        return (list, Some((pane, Pane::Side)));
    }
    let wanted = (area.height * 3 / 5).clamp(MIN_PANE_ROWS, MAX_PANE_ROWS);
    let free = area.height.saturating_sub(list.rows.max(MIN_LIST_ROWS));
    let rows = wanted
        .max(free)
        .min(area.height.saturating_sub(MIN_LIST_ROWS));
    if rows < 2 {
        return (area, None);
    }
    let [list, pane] = Layout::vertical([Constraint::Min(1), Constraint::Length(rows)]).areas(area);
    (list, Some((pane, Pane::Bottom)))
}

/// The queue and the pane for `app` in `area`.
pub fn layout_of(app: &App, area: Rect) -> (Rect, Option<(Rect, Pane)>) {
    layout_fit(area, ListSize::of(app, area))
}

/// The first line, in `w` cells after one blank: the name, a spinner while
/// it is described, and the path in what is left, cut at the front so the
/// file name stays.
pub fn unit_head(app: &App, u: &Unit, w: usize) -> Line<'static> {
    let t = &app.theme;
    let name = truncate(&names::row_name(u), w);
    let mut tail = Vec::new();
    if u.prep == Prep::Pending {
        tail.push(Span::styled(format!(" {}", app.spinner()), t.muted()));
    }
    let tail_w: usize = tail.iter().map(|s| s.content.width()).sum();
    let room = w.saturating_sub(name.width() + tail_w + 2);
    let place = names::place(u);
    let place = if room >= 8 {
        slim::fit_path(&place, room)
    } else {
        String::new()
    };
    let mut head = vec![Span::styled(format!(" {name}"), t.bold())];
    if !place.is_empty() {
        head.push(Span::styled(format!("  {place}"), t.muted()));
    }
    head.extend(tail);
    Line::from(head)
}

/// `text` and then `keep` wrapped to `w` after a two-column `icon`,
/// `indent` cells in, the rest under the text. The icon and the text are
/// styled apart. At most `most` lines; when cut, the last ends in `…` and
/// still ends with `keep`.
fn marked(
    icon: &str,
    text: &str,
    w: usize,
    (icon_style, text_style): (Style, Style),
    indent: usize,
    (most, keep): (usize, &str),
) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthChar;
    let pad = " ".repeat(indent);
    let text_w = w.saturating_sub(indent + 2);
    let mut lines = wrap(&format!("{text}{keep}"), text_w);
    if lines.len() > most && most > 0 {
        lines = wrap(text, text_w);
        lines.truncate(most);
        let last = lines.pop().unwrap_or_default();
        let room = text_w.saturating_sub(keep.width() + 1);
        let mut used = 0;
        let kept: String = last
            .chars()
            .take_while(|c| {
                used += c.width().unwrap_or(0);
                used <= room
            })
            .collect();
        lines.push(format!("{}…{keep}", kept.trim_end()));
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(k, l)| {
            let lead = if k == 0 {
                Span::styled(format!("{pad}{icon} "), icon_style)
            } else {
                Span::raw(format!("{pad}  "))
            };
            Line::from(vec![lead, Span::styled(l, text_style)])
        })
        .collect()
}

/// The heading of what says why a unit has its level.
pub const WHY_HEADING: &str = "Why this level";

/// Why `u` has its level, `indent` cells in: the rules that set the level
/// behind the level's icon, then its written reasons (`Unit::reasons`:
/// expensive to undo, what the check disputes, what the commits or PR
/// description don't mention), each wrapped behind a muted `·`.
pub fn check_lines(app: &App, u: &Unit, w: usize, indent: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines = Vec::new();
    if let Some(a) = app.assessment(&u.id)
        && !a.reasons.is_empty()
    {
        let level = a.level.shown(u.auto_accepted());
        lines.extend(marked(
            level.icon(),
            &a.reasons.join(" · "),
            w,
            (slim::level_style(app, level), t.muted()),
            indent,
            (usize::MAX, ""),
        ));
    }
    for r in &u.reasons {
        lines.extend(marked(
            "·",
            r,
            w,
            (t.muted(), t.text()),
            indent,
            (usize::MAX, ""),
        ));
    }
    lines
}

/// `line` behind the bar that marks what the agent wrote, `indent` cells in.
pub fn barred(app: &App, line: Line<'static>, indent: usize) -> Line<'static> {
    let mut spans = vec![
        Span::raw(" ".repeat(indent)),
        Span::styled("┃ ", app.theme.agent()),
    ];
    spans.extend(line.spans);
    Line::from(spans).style(line.style)
}

/// What the agent wrote about `u`, behind the bar: a label naming the
/// model, the summary and, with `statements`, the numbered statements.
/// Each line takes at most `w` cells after the bar, which sits `indent`
/// cells in. Empty when it said nothing (yet).
pub fn agent_lines(
    app: &App,
    u: &Unit,
    w: usize,
    statements: bool,
    indent: usize,
) -> Vec<Line<'static>> {
    agent_text(app, u, w, statements)
        .into_iter()
        .map(|l| barred(app, l, indent))
        .collect()
}

/// What the agent wrote about `u`, `w` cells wide: a label naming the
/// model, the summary and, with `statements`, the numbered statements.
/// Empty when it said nothing (yet).
fn agent_text(app: &App, u: &Unit, w: usize, statements: bool) -> Vec<Line<'static>> {
    let t = &app.theme;
    let listed = statements && u.prep == Prep::Ready && !u.changes.is_empty();
    if u.summary.is_none() && !listed {
        return Vec::new();
    }
    let mut lines = vec![Line::styled(
        truncate(&names::agent_label(app, u), w),
        t.muted(),
    )];
    if let Some(s) = u.summary.as_deref() {
        lines.extend(wrap(s, w).into_iter().map(Line::raw));
    }
    if listed {
        lines.extend(slim::statement_lines(app, u, w, 0));
    }
    lines
}

/// The review of `u`, `w` cells wide: who wrote it, when the agent did,
/// and its text: what the agent wrote (the summary and, with `statements`,
/// the numbered statements; or a note while it hasn't), then the level
/// and why (see [`check_lines`]).
fn review_text(
    app: &App,
    u: &Unit,
    w: usize,
    statements: bool,
) -> (Option<String>, Vec<Line<'static>>) {
    let mut text = agent_text(app, u, w, statements).into_iter();
    let by = text.next().map(|l| l.to_string());
    let mut text: Vec<_> = text.collect();
    // Not described (yet): the spinner or why not. Why a unit has no
    // behavior change is said with its reasons.
    if u.prep != Prep::Ready && u.summary.is_none() && u.nbc_reason.is_none() {
        text.extend(slim::statement_lines(app, u, w, 0));
    }
    text.extend(check_lines(app, u, w, 0));
    (by, text)
}

/// What `u` is, in groups: its review behind the bar (see
/// [`review_text`]); the reviewer's note; the answers to questions asked
/// about it. Some may be empty.
pub fn unit_groups(app: &App, u: &Unit, w: usize, statements: bool) -> Vec<Vec<Line<'static>>> {
    let (by, text) = review_text(app, u, w.saturating_sub(2), statements);
    let by = by.map(|by| Line::styled(by, app.theme.muted()));
    let said = by
        .into_iter()
        .chain(text)
        .map(|l| barred(app, l, 1))
        .collect();
    let indented = |lines: Vec<Line<'static>>| -> Vec<Line<'static>> {
        lines
            .into_iter()
            .map(|l| {
                let mut spans = vec![Span::raw(" ")];
                spans.extend(l.spans);
                Line::from(spans)
            })
            .collect()
    };
    // The reviewer's note. Under the code box when there is one.
    let card = if statements {
        super::notes::card(app, u, w.saturating_sub(1))
    } else {
        Vec::new()
    };
    // Answers to questions asked about it, so they are seen.
    let asked = super::ask::note_lines(app, &u.id, w.saturating_sub(1));
    vec![said, indented(card), indented(asked)]
}

/// `✎ a change is proposed: 2 hunks · ␣e shows it`, `indent` cells in, cut
/// to `w`: when the agent proposed a change to `u` with hunks not put in
/// yet.
pub fn proposed_line(app: &App, u: &Unit, w: usize, indent: usize) -> Option<Line<'static>> {
    use crate::keymap::Action;
    let open = app.shown_proposal(u)?.open().len();
    if open == 0 {
        return None;
    }
    let s = if open == 1 { "" } else { "s" };
    let text = if app.mode.improves() {
        format!(
            "{open} insert{s} proposed · {} shows them",
            app.keys.label(Action::ViewOpen)
        )
    } else {
        format!(
            "a change is proposed: {open} hunk{s} · {} shows it",
            app.keys.label(Action::ToolFix)
        )
    };
    Some(Line::from(vec![
        Span::styled(format!("{}✎ ", " ".repeat(indent)), app.theme.accent()),
        Span::styled(
            truncate(&text, w.saturating_sub(indent + 2)),
            app.theme.text(),
        ),
    ]))
}

/// `blocks` in `h` rows, from the detail area's scroll offset on. A block
/// shows whole or not at all (a wrapped code row is one block), unless it
/// alone is taller than the rows. While something is cut, the first row
/// says how much is above and the last how much is left and which key
/// scrolls, with `editor` also the editor's key. Hints sit after `indent`
/// cells and take at most `w`. Notes how far the area can scroll.
pub fn scrolled(
    app: &App,
    blocks: Vec<Vec<Line<'static>>>,
    w: usize,
    h: usize,
    indent: usize,
    editor: bool,
) -> Vec<Line<'static>> {
    let s = scroll_rows(app, blocks, w, h, indent, editor, app.detail_offset());
    keep_scroll(app, &s, h);
    s.lines
}

/// Notes how far the detail area, `h` rows, scrolled and can scroll.
pub fn keep_scroll(app: &App, s: &Scroll, h: usize) {
    app.detail_scroll.borrow_mut().1 = s.offset;
    app.detail_max_scroll.set(s.max);
    app.detail_rows.set(h);
}

/// What [`scroll_rows`] made of the blocks.
pub struct Scroll {
    pub lines: Vec<Line<'static>>,
    /// The offset asked for, no further than `max`.
    pub offset: usize,
    /// How far the blocks can scroll.
    pub max: usize,
    /// Lines of the blocks above the first one shown.
    pub above: usize,
    /// Which of `lines` are the blocks': the rest say what is cut.
    pub shown: Range<usize>,
}

/// [`scrolled`] from `offset`, for an area that keeps its own scroll.
pub fn scroll_rows(
    app: &App,
    blocks: Vec<Vec<Line<'static>>>,
    w: usize,
    h: usize,
    indent: usize,
    editor: bool,
    offset: usize,
) -> Scroll {
    use crate::keymap::Action;
    let total: usize = blocks.iter().map(Vec::len).sum();
    let starts: Vec<usize> = blocks
        .iter()
        .scan(0, |at, b| {
            let start = *at;
            *at += b.len();
            Some(start)
        })
        .collect();
    // The row that says what is above, when there is room for it.
    let head = usize::from(h >= 3);
    // Scrolled furthest, the last block is at the bottom: the first start
    // from which the rest fits under that row.
    let max = if total <= h {
        0
    } else {
        starts
            .iter()
            .copied()
            .find(|&s| s > 0 && total - s + head <= h)
            .unwrap_or_else(|| starts.last().copied().unwrap_or(0))
    };
    let offset = offset.min(max);
    // The first block that starts at the offset or after it.
    let first = starts.iter().position(|&s| s >= offset).unwrap_or(0);
    let above = starts.get(first).copied().unwrap_or(0);
    let k = |a| app.keys.label(a);
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let hint = |text: String| {
        Line::styled(
            format!("{}{}", " ".repeat(indent), truncate(&text, w)),
            app.theme.muted(),
        )
    };
    let mut out = Vec::new();
    let mut top = 0;
    if above > 0 && head > 0 {
        out.push(hint(format!(
            "… {above} line{} above · {} scroll up",
            plural(above),
            k(Action::ScrollHalfUp)
        )));
        top = 1;
    }
    let rest = total - above;
    // Keep a row for what is left when not everything fits.
    let budget = if rest + top <= h {
        h - top
    } else {
        h.saturating_sub(top + 1)
    };
    let mut used = 0;
    for b in blocks.into_iter().skip(first) {
        if used + b.len() > budget {
            if used == 0 {
                // A block too tall for the room: as much of it as fits.
                out.extend(b.into_iter().take(budget));
                used = budget;
            }
            break;
        }
        used += b.len();
        out.extend(b);
    }
    let left = rest - used;
    let shown = top..top + used;
    if left > 0 && h > top {
        let mut more = format!(
            "… {left} more line{} · {} scroll",
            plural(left),
            k(Action::ScrollHalfDown)
        );
        if editor {
            more.push_str(&format!(" · {} editor", k(Action::GotoEditor)));
        }
        out.push(hint(more));
    }
    Scroll {
        lines: out,
        offset,
        max,
        above,
        shown,
    }
}

/// The changes of a file as the pane draws them.
#[derive(Default)]
pub struct FileText {
    /// Its rows, each the lines it wraps into (see [`scrolled`]).
    pub blocks: Vec<Vec<Line<'static>>>,
    /// The line each unit starts at, by its position in the queue's order.
    pub starts: Vec<(usize, usize)>,
    /// The lines that are code, and the columns its line numbers and signs
    /// take there.
    pub code: Vec<(Range<usize>, u16)>,
    /// How many lines the blocks hold.
    lines: usize,
}

impl FileText {
    fn push(&mut self, block: Vec<Line<'static>>) {
        self.lines += block.len();
        self.blocks.push(block);
    }
}

/// A unit's head line in the pane, `w` cells: the bar when it is the
/// `selected` one of several, the level's icon, its name (struck through
/// when the change removes it), then `✓` once it is accepted, `⚑` flagged,
/// `⚠` when a second model disputes it, a spinner while it is described.
/// An accepted unit's line is muted.
pub(super) fn head_line(
    app: &App,
    pos: usize,
    u: &Unit,
    selected: bool,
    w: usize,
) -> Line<'static> {
    let t = &app.theme;
    let level = app.shown_level(app.order[pos]);
    let done = u.decision == Decision::Accepted;
    let mut tail = Vec::new();
    match u.decision {
        Decision::Accepted => tail.push(Span::styled(" ✓", t.muted())),
        Decision::Investigate => tail.push(Span::styled(" ⚑", t.badge_investigate())),
        Decision::Pending => {}
    }
    if !app.levels[app.order[pos]].doubts.is_empty() {
        tail.push(Span::styled(" ⚠", t.risk(Risk::Medium)));
    }
    if u.prep == Prep::Pending {
        tail.push(Span::styled(format!(" {}", app.spinner()), t.muted()));
    }
    let tail_w: usize = tail.iter().map(Span::width).sum();
    let name = truncate(&names::row_name(u), w.saturating_sub(UNDER_NAME + tail_w));
    let mut name_style = if done { t.muted() } else { t.bold() };
    if u.status == UnitStatus::Removed {
        name_style = name_style.add_modifier(Modifier::CROSSED_OUT);
    }
    let icon_style = if done {
        t.muted()
    } else {
        slim::icon_style(app, level)
    };
    let mut spans = vec![
        if selected {
            Span::styled("▌", t.accent())
        } else {
            Span::raw(" ")
        },
        Span::styled(format!("{} ", level.icon()), icon_style),
        Span::styled(name, name_style),
    ];
    spans.extend(tail);
    Line::from(spans)
}

/// `line` after `by` blank cells.
fn indented(line: Line<'static>, by: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(by))];
    spans.extend(line.spans);
    Line::from(spans).style(line.style)
}

/// The changes of `file`, `w` cells wide: its listed units in line order,
/// a blank line between them. Of each: its head line (none for a file that
/// is one whole unit, which the tree names), that a change is proposed,
/// what the agent says it does in at most [`SUMMARY_LINES`] lines (or why
/// it has no behavior change), its code change with the statements written
/// into it (listed when its code can't be shown), the reviewer's note and
/// the answers to what the reviewer asked about it. The reasons, the
/// callers and the commits are in the unit view.
pub fn file_text(app: &App, file: &Path, w: usize) -> FileText {
    let t = &app.theme;
    let at = app.file_units(file);
    let several = at.len() > 1;
    let mut text = FileText::default();
    let body_w = w.saturating_sub(UNDER_NAME);
    for pos in at {
        let u = &app.review.units[app.order[pos]];
        if !text.blocks.is_empty() {
            text.push(vec![Line::default()]);
        }
        text.starts.push((pos, text.lines));
        if several || u.kind != UnitKind::File {
            let selected = several && pos == app.sel;
            text.push(vec![head_line(app, pos, u, selected, w)]);
        }
        if let Some(l) = proposed_line(app, u, w, UNDER_NAME) {
            text.push(vec![l]);
        }
        let said = u.summary.as_deref().map(|s| (s, t.text()));
        let said = said.or(u.nbc_reason.as_deref().map(|s| (s, t.muted())));
        if let Some((s, style)) = said {
            let mut lines = wrap(s, body_w);
            if lines.len() > SUMMARY_LINES {
                lines.truncate(SUMMARY_LINES);
                if let Some(last) = lines.last_mut() {
                    *last = format!("{}…", truncate(last, body_w.saturating_sub(1)));
                }
            }
            for l in lines {
                text.push(vec![indented(Line::styled(l, style), UNDER_NAME)]);
            }
        }
        let code = super::diff::code_change_of(app, u, body_w, super::diff::Notes::Inline);
        let first = text.lines;
        for block in code.blocks {
            text.push(block.into_iter().map(|l| indented(l, UNDER_NAME)).collect());
        }
        if text.lines > first {
            text.code.push((first..text.lines, code.gutter));
        } else if u.prep == Prep::Ready {
            // No code to write them into: the statements, listed.
            for l in slim::statement_lines(app, u, body_w, 0) {
                text.push(vec![indented(l, UNDER_NAME)]);
            }
        }
        // What the reviewer wrote and asked: seen where the code is.
        let own = super::notes::card(app, u, body_w)
            .into_iter()
            .chain(super::ask::note_lines(app, &u.id, body_w));
        for l in own {
            text.push(vec![indented(l, UNDER_NAME)]);
        }
    }
    text
}

/// Draw the pane into `area`: the changes of the file under the cursor
/// (see [`file_text`]), scrolled with the detail keys and the wheel. When
/// a jump put the cursor on a unit that isn't its file's lead, that unit
/// is scrolled into view. Nothing on a directory.
pub fn draw_preview(f: &mut Frame, app: &App, area: Rect, pane: Pane) {
    if area.width < 4 || area.height < 2 {
        return;
    }
    // The wheel over the pane scrolls it.
    app.targets.borrow_mut().push((area, Target::Detail));
    super::ui::text_area(app, area, 0);
    // A plain rule shows where the list ends: two columns of text side
    // by side read as one without it.
    let edge = match pane {
        Pane::Side => Borders::LEFT,
        Pane::Bottom => Borders::TOP,
    };
    let block = Block::new().borders(edge).border_style(app.theme.muted());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(u) = app.selected() else {
        app.pane_unit.take();
        return;
    };
    if inner.width < 2 || inner.height == 0 {
        return;
    }
    let w = (inner.width - 1) as usize;
    let h = inner.height as usize;
    let text = file_text(app, &u.file, w);
    // A unit just jumped to starts the pane, unless the file starts with it.
    let mut offset = app.detail_offset();
    if app.pane_unit.replace(u.id.clone()) != u.id && !app.on_lead() {
        let start = text.starts.iter().find(|(pos, _)| *pos == app.sel);
        offset = start.map_or(offset, |&(_, line)| line);
    }
    let s = scroll_rows(app, text.blocks, w, h, 1, false, offset);
    keep_scroll(app, &s, h);
    // Where the code is drawn, a drag leaves its line numbers out.
    let x = (UNDER_NAME as u16).min(inner.width);
    for (lines, gutter) in &text.code {
        let from = lines.start.max(s.above);
        let to = lines.end.min(s.above + s.shown.len());
        if from < to {
            let y = inner.y + (s.shown.start + from - s.above) as u16;
            let code = Rect::new(inner.x + x, y, inner.width - x, (to - from) as u16);
            super::ui::text_area(app, code, *gutter);
        }
    }
    f.render_widget(Paragraph::new(s.lines), inner);
}

/// The first `h` of `lines`; when some are left out, the last row says so.
pub(super) fn cut(app: &App, mut lines: Vec<Line<'static>>, h: usize) -> Vec<Line<'static>> {
    if lines.len() > h && h > 0 {
        lines.truncate(h - 1);
        lines.push(Line::styled(" …", app.theme.muted()));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::Decision;

    fn layout(area: Rect) -> (Rect, Option<(Rect, Pane)>) {
        layout_fit(area, ListSize::FULL)
    }

    fn r(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    #[test]
    fn wide_splits_horizontally() {
        let (l, p) = layout(r(110, 20));
        let (p, pane) = p.unwrap();
        assert_eq!(pane, Pane::Side);
        assert_eq!(l.width + p.width, 110);
        assert_eq!(p.x, l.width);
        // The list takes at most 45%: 49 of 110.
        assert_eq!(p.width, 61);
        assert_eq!(p.height, 20);
        assert_eq!(layout(r(109, 20)).1.map(|x| x.1), Some(Pane::Bottom));
    }

    #[test]
    fn narrow_gets_a_bottom_pane_sized_to_the_height() {
        for (h, want) in [
            (20u16, 12u16),
            (22, 13),
            (28, 16),
            (30, 18),
            (60, 24),
            (12, 7),
        ] {
            let (l, p) = layout(r(100, h));
            let (p, pane) = p.unwrap();
            assert_eq!(pane, Pane::Bottom, "h {h}");
            assert_eq!(p.height, want, "h {h}");
            assert_eq!(l.height + p.height, h);
            assert_eq!(p.y, l.height);
        }
    }

    #[test]
    fn the_list_keeps_three_rows_and_the_pane_shrinks_first() {
        for h in 5u16..=11 {
            let (l, p) = layout(r(100, h));
            let (p, _) = p.unwrap();
            assert!(l.height >= 3, "h {h}");
            assert_eq!(p.height, (h - 3).min((h * 3 / 5).max(5)), "h {h}");
        }
        // Under 2 spare rows there is no pane at all.
        for h in 0u16..=4 {
            assert!(layout(r(100, h)).1.is_none(), "h {h}");
        }
    }

    #[test]
    fn beside_the_list_the_pane_gets_what_the_list_does_not_need() {
        let size = |width| ListSize {
            width,
            ..ListSize::FULL
        };
        for (w, need, want) in [
            (120u16, 20u16, 24u16),
            (120, 30, 30),
            (120, 90, 54),
            (110, 0, 24),
            (200, 70, 70),
        ] {
            let (l, p) = layout_fit(r(w, 30), size(need));
            let (p, pane) = p.unwrap();
            assert_eq!(pane, Pane::Side);
            assert_eq!(l.width, want, "{w} {need}");
            assert_eq!((p.x, p.width), (want, w - want), "{w} {need}");
        }
        // A list of unknown size gets the most it can.
        assert_eq!(layout(r(120, 30)).0.width, 54);
    }

    #[test]
    fn below_a_short_list_the_pane_takes_the_free_rows() {
        let size = |rows| ListSize {
            rows,
            ..ListSize::FULL
        };
        for (h, rows, want) in [
            (28u16, 3u16, 25u16),
            (28, 10, 18),
            // Not less than the usual share of about 60%.
            (28, 13, 16),
            (40, 14, 26),
            // The list keeps 3 rows, on tiny terminals too.
            (40, 2, 37),
            (5, 1, 2),
        ] {
            let (l, p) = layout_fit(r(100, h), size(rows));
            let (p, _) = p.unwrap();
            assert_eq!(p.height, want, "h {h} rows {rows}");
            assert_eq!(l.height + p.height, h);
        }
    }

    #[test]
    fn wide_wins_over_tall() {
        let (_, p) = layout(r(150, 50));
        assert_eq!(p.unwrap().1, Pane::Side);
    }

    #[test]
    fn degenerate_areas_never_panic() {
        for (w, h) in [
            (0, 0),
            (0, 10),
            (10, 0),
            (1, 1),
            (10, 3),
            (110, 1),
            (3, 200),
        ] {
            let (l, p) = layout(r(w, h));
            assert!(l.width <= w && l.height <= h);
            if let Some((p, _)) = p {
                assert!(p.width <= w && p.height <= h);
            }
        }
        assert!(layout(r(0, 0)).1.is_none());
        assert!(layout(r(10, 0)).1.is_none());
    }

    fn app(with_unit: bool) -> App {
        use crate::config::Config;
        use crate::keymap::Keymap;
        use crate::review::model::{Range, Review, Unit, UnitKind, UnitStatus};
        use std::path::PathBuf;
        let units = if with_unit {
            vec![Unit {
                id: "f.rs::my_symbol".into(),
                file: PathBuf::from("src/f.rs"),
                symbol: "my_symbol".into(),
                kind: UnitKind::Function,
                status: UnitStatus::Changed,
                old_range: None,
                new_range: Some(Range::new(1, 5)),
                nested: Vec::new(),
                renamed_from: None,
                fingerprint: String::new(),
                changes: Vec::new(),
                prep: Prep::Unavailable,
                callers: vec!["caller_fn".into()],
                callees: Vec::new(),
                decision: Decision::Accepted,
                summary: Some("Does a thing".into()),
                nbc_reason: None,
                dive: Default::default(),
                accepted_by: None,
                reasons: Vec::new(),
                dive_pending: Default::default(),
                checks: Default::default(),
                security: Vec::new(),
                signature: None,
            }]
        } else {
            Vec::new()
        };
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
            units,
            skipped: Vec::new(),
            graph: Default::default(),
            questions: Vec::new(),
            version: 0,
            history: Default::default(),
            branch: None,
            cursor: None,
            mode: None,
        };
        App::new(review, Config::default(), Keymap::default())
    }

    fn render(app: &App, w: u16, h: u16, pane: Pane) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw_preview(f, app, f.area(), pane)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_pane_shows_the_unit_under_the_cursor_and_what_the_agent_says() {
        let app = app(true);
        for (pane, w, h) in [(Pane::Side, 50, 20), (Pane::Bottom, 100, 8)] {
            let out = render(&app, w, h, pane);
            // The level's icon, the name, and that it is accepted.
            assert!(out.contains("┃ my_symbol ✓"), "{pane:?}:\n{out}");
            assert!(out.contains("   Does a thing"), "{pane:?}:\n{out}");
            // What the unit view is for stays there.
            for word in ["caller_fn", "agent", "src/f.rs"] {
                assert!(!out.contains(word), "{pane:?} {word}:\n{out}");
            }
        }
    }

    #[test]
    fn with_nothing_under_the_cursor_the_pane_is_empty() {
        let app = app(false);
        // Only the rule that ends the list.
        let out = render(&app, 50, 20, Pane::Side);
        assert!(out.lines().all(|l| l.trim_end() == "│"), "{out}");
        let out = render(&app, 100, 8, Pane::Bottom);
        assert_eq!(out.trim(), "─".repeat(100), "{out}");
        render(&app, 3, 1, Pane::Side);
        render(&app, 5, 2, Pane::Bottom);
    }
}
