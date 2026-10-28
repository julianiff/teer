//! Live preview pane for the selected unit in the Slim list.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Target};
use super::slim;
use crate::review::attention::Level;
use crate::review::model::{Prep, Unit};
use crate::util::{truncate, wrap};

/// Width at which the preview sits beside the list.
const SIDE_WIDTH: u16 = 110;
/// The list never gets fewer rows than this when a bottom pane is drawn.
const MIN_LIST_ROWS: u16 = 3;
/// Bounds of a bottom pane, border included.
const MIN_PANE_ROWS: u16 = 5;
const MAX_PANE_ROWS: u16 = 12;
/// Upper bound of a bottom pane that shows a code change.
const MAX_CODE_PANE_ROWS: u16 = 24;
/// The fewest rows the code change's box gets under the unit: its borders
/// and three rows of code.
pub const MIN_BOX_ROWS: usize = 5;
/// Narrowest list beside the pane; it is never wider than [`SIDE_LIST_MOST`].
const MIN_SIDE_LIST: u16 = 24;
/// Widest list beside the pane, in percent: the code change gets the rest.
const SIDE_LIST_MOST: u32 = 35;

/// Which edge of the list the preview sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Side,
    Bottom,
}

/// What the list needs to show all of itself: its width (whole paths and
/// names, file counts, whole box titles) and its rows, without and with the
/// blank rows between boxes and files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListSize {
    pub width: u16,
    pub rows: u16,
    pub padded_rows: u16,
}

impl ListSize {
    /// A list that takes all the room it is given.
    pub const FULL: ListSize = ListSize {
        width: u16::MAX,
        rows: u16::MAX,
        padded_rows: u16::MAX,
    };

    /// The size of `app`'s list, as far as a layout of `area` needs it.
    pub fn of(app: &App, area: Rect) -> ListSize {
        if area.width >= SIDE_WIDTH {
            let width = slim::fit_width(app).min(u16::MAX as usize) as u16;
            return ListSize {
                width,
                ..ListSize::FULL
            };
        }
        // Every unit is a line: a list this long takes all it gets.
        if app.order.len() >= area.height as usize {
            return ListSize::FULL;
        }
        let count = |padded| slim::build_with(app, area.width, padded).lines.len() as u16;
        let rows = count(false);
        let padded_rows = if rows as usize >= slim::PADDING_MIN_ROWS {
            count(true)
        } else {
            rows
        };
        ListSize {
            rows,
            padded_rows,
            ..ListSize::FULL
        }
    }
}

/// Splits `area` into the list and the preview pane, which is always there
/// unless the area is too small to hold one. At 110 columns or more the pane
/// sits beside the list, and the list is as wide as `list` needs, at least
/// 24 columns and at most 35%. Otherwise the pane sits below and takes about
/// 40% of the height (5 to 12 rows), or with `code` (the selected unit shows
/// its code change) about 60%, up to 24 rows, so the diff is visible below
/// the statements. It takes more when the list leaves more free; the list
/// keeps its rows (with the blank rows between boxes and files once it has
/// 14) and at
/// least 3. A pane under 2 rows is not drawn.
pub fn layout_fit(area: Rect, code: bool, list: ListSize) -> (Rect, Option<(Rect, Pane)>) {
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
    let wanted = if code {
        (area.height * 3 / 5).clamp(MIN_PANE_ROWS, MAX_CODE_PANE_ROWS)
    } else {
        (area.height * 2 / 5).clamp(MIN_PANE_ROWS, MAX_PANE_ROWS)
    };
    let need = if (list.rows as usize) < slim::PADDING_MIN_ROWS {
        list.rows
    } else {
        list.padded_rows
    };
    let free = area.height.saturating_sub(need.max(MIN_LIST_ROWS));
    let rows = wanted
        .max(free)
        .min(area.height.saturating_sub(MIN_LIST_ROWS));
    if rows < 2 {
        return (area, None);
    }
    let [list, pane] = Layout::vertical([Constraint::Min(1), Constraint::Length(rows)]).areas(area);
    (list, Some((pane, Pane::Bottom)))
}

/// The list and the pane for `app` in `area`, as the list view draws them.
pub fn layout_of(app: &App, area: Rect) -> (Rect, Option<(Rect, Pane)>) {
    let code = app.selected().is_some_and(|u| super::diff::shows(app, u));
    layout_fit(area, code, ListSize::of(app, area))
}

/// The change itself as lines: its name, why it matters and its commits
/// under it, then a blank row before each other group of [`unit_groups`].
/// With `statements` its statements are listed too; the list preview and
/// the swipe card write them into the code change instead (see
/// [`super::diff`]).
pub fn unit_lines(app: &App, u: &Unit, w: usize, statements: bool) -> Vec<Line<'static>> {
    let name = truncate(&slim::row_name(u), w);
    let mut lines = vec![Line::styled(format!(" {name}"), app.theme.bold())];
    let mut groups = unit_groups(app, u, w, statements).into_iter();
    lines.extend(groups.next().unwrap_or_default());
    for g in groups.filter(|g| !g.is_empty()) {
        lines.push(Line::default());
        lines.extend(g);
    }
    lines
}

/// The first line, in `w` cells after one blank: the name, a spinner while
/// it is described, and the path in what is left, cut at the front so the
/// file name stays.
pub fn unit_head(app: &App, u: &Unit, w: usize) -> Line<'static> {
    let t = &app.theme;
    let name = truncate(&slim::row_name(u), w);
    let mut tail = Vec::new();
    if u.prep == Prep::Pending {
        tail.push(Span::styled(format!(" {}", app.spinner()), t.muted()));
    }
    let tail_w: usize = tail.iter().map(|s| s.content.width()).sum();
    let room = w.saturating_sub(name.width() + tail_w + 2);
    let place = slim::place(u);
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

/// `lines` from [`marked`] with `label`, where their text starts, in
/// `style`.
fn labelled(mut lines: Vec<Line<'static>>, label: &str, style: Style) -> Vec<Line<'static>> {
    if let Some(first) = lines.first_mut()
        && let Some(text) = first.spans.get(1)
        && let Some(rest) = text.content.strip_prefix(label)
    {
        let rest = Span::styled(rest.to_string(), text.style);
        first.spans.truncate(1);
        first.spans.push(Span::styled(label.to_string(), style));
        first.spans.push(rest);
    }
    lines
}

/// Lines for the signals beside the reasons, `indent` cells in: expensive
/// to undo (`$`, with `tagged` `(agent)` when the agent says so rather than
/// the path), a second opinion that disagrees (`⚠`, its model, the first
/// point in at most two lines and how many more; the Disagreement tab has
/// them all) and a change the commit messages don't mention (`≠`, with
/// `tagged` `(agent)`).
pub fn check_lines(
    app: &App,
    u: &Unit,
    w: usize,
    indent: usize,
    tagged: bool,
) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines = Vec::new();
    let Some(a) = app.assessment(&u.id) else {
        return lines;
    };
    // Each says what it is in the icon's colour, then what was found.
    let mut check = |icon: &str, label: String, text: &str, style: Style, keep: (usize, &str)| {
        let all = marked(
            icon,
            &format!("{label} {text}"),
            w,
            (style, t.text()),
            indent,
            keep,
        );
        lines.extend(labelled(all, &label, style));
    };
    if let Some(why) = &a.cost {
        let by = if tagged && u.checks.cost.is_some() {
            " (agent)"
        } else {
            ""
        };
        let label = format!("Expensive to undo{by}:");
        check("$", label, why, t.needs_fix(), (usize::MAX, ""));
    }
    if let Some(first) = a.doubts.first() {
        let more = match a.doubts.len() {
            1 => String::new(),
            n => format!(" (+{} more)", n - 1),
        };
        let label = format!(
            "Second opinion ({}):",
            slim::model_name(app, crate::config::Role::Second)
        );
        let style = t.risk(crate::review::model::Risk::Medium);
        check("⚠", label, first, style, (2, &more));
    }
    if let Some(gap) = &u.checks.unmentioned {
        let by = if tagged { " (agent)" } else { "" };
        let label = format!("Not in the commit messages{by}:");
        check("≠", label, &gap.text, t.accent(), (usize::MAX, ""));
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
    let t = &app.theme;
    let listed = statements && u.prep == Prep::Ready && !u.changes.is_empty();
    if u.summary.is_none() && !listed {
        return Vec::new();
    }
    let mut lines = vec![Line::styled(
        truncate(&slim::agent_label(app, u), w),
        t.muted(),
    )];
    if let Some(s) = u.summary.as_deref() {
        lines.extend(wrap(s, w).into_iter().map(Line::raw));
    }
    if listed {
        lines.extend(slim::statement_lines(app, u, w, 0));
    }
    lines.into_iter().map(|l| barred(app, l, indent)).collect()
}

/// [`unit_groups`] one after the other.
pub fn unit_explain(app: &App, u: &Unit, w: usize, statements: bool) -> Vec<Line<'static>> {
    unit_groups(app, u, w, statements).concat()
}

/// What `u` is, in groups: why it matters and its commits; what the agent
/// said (see [`agent_lines`]), or a note while it hasn't; the checks; the
/// reviewer's note; the answers to questions asked about it. Some may be
/// empty.
pub fn unit_groups(app: &App, u: &Unit, w: usize, statements: bool) -> Vec<Vec<Line<'static>>> {
    let level = app
        .assessment(&u.id)
        .map(|a| a.level)
        .unwrap_or(Level::Routine);
    let style = slim::level_style(app, level);
    // Why it matters comes first: the list no longer shows it.
    let mut about = Vec::new();
    if let Some(a) = app.assessment(&u.id).filter(|a| !a.reasons.is_empty()) {
        about.extend(marked(
            level.icon(),
            &a.reasons.join(" · "),
            w,
            (style, app.theme.muted()),
            1,
            (usize::MAX, ""),
        ));
    }
    about.extend(commits_line(app, u, w, 1));
    let mut said = agent_lines(app, u, w.saturating_sub(2), statements, 1);
    if u.summary.is_none()
        && let Some(r) = u.nbc_reason.as_deref()
    {
        for l in wrap(r, w) {
            said.push(Line::raw(format!(" {l}")));
        }
    }
    // Not described (yet): the spinner or why not.
    if u.prep != Prep::Ready && u.summary.is_none() && u.nbc_reason.is_none() {
        said.extend(slim::statement_lines(app, u, w, 1));
    }
    let checks = check_lines(app, u, w, 1, true);
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
    // The reviewer's note. Under the code box when there is one; the
    // Flagged screen has it in its head.
    let card = if statements && app.screen != super::app::Screen::Flagged {
        super::notes::card(app, u, w.saturating_sub(1))
    } else {
        Vec::new()
    };
    // Answers to questions asked about it, so they are seen. On the Flagged
    // screen they are in its chat.
    let asked = if app.screen == super::app::Screen::Flagged {
        Vec::new()
    } else {
        super::ask::note_lines(app, &u.id, w.saturating_sub(1))
    };
    vec![about, said, checks, indented(card), indented(asked)]
}

/// `commits: c86f6fc Batch invalidate. · 1a2b3c4 Renames DTO.`: the commits
/// that changed the unit, oldest first, `indent` cells in, cut to `w`.
pub fn commits_line(app: &App, u: &Unit, w: usize, indent: usize) -> Option<Line<'static>> {
    let commits = app.review.commits_of(u);
    if commits.is_empty() {
        return None;
    }
    let list: Vec<String> = commits
        .iter()
        .map(|c| format!("{} {}", c.short, c.subject))
        .collect();
    Some(Line::styled(
        format!(
            "{}{}",
            " ".repeat(indent),
            truncate(
                &format!("commits: {}", list.join(" · ")),
                w.saturating_sub(indent)
            )
        ),
        app.theme.muted(),
    ))
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
    /// Rows left under the last one shown.
    pub below: usize,
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
    scroll_in(app, blocks, w, (h, h), indent, editor, offset)
}

/// [`scroll_rows`] for a box that leaves its bottom edge off while rows are
/// left below: what is left in `h - 1` rows when it fits there, which
/// leaves a row for the edge; else `h` rows, the last saying what is left.
pub fn scroll_open(
    app: &App,
    blocks: Vec<Vec<Line<'static>>>,
    w: usize,
    h: usize,
    editor: bool,
    offset: usize,
) -> Scroll {
    scroll_in(app, blocks, w, (h.saturating_sub(1), h), 0, editor, offset)
}

/// [`scroll_rows`] in `fit` rows when what is left fits there, else in `h`.
fn scroll_in(
    app: &App,
    blocks: Vec<Vec<Line<'static>>>,
    w: usize,
    (fit, h): (usize, usize),
    indent: usize,
    editor: bool,
    offset: usize,
) -> Scroll {
    use crate::config::Action;
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
    let head = usize::from(fit >= 3);
    // Scrolled furthest, the last block is at the bottom: the first start
    // from which the rest fits under that row.
    let max = if total <= fit {
        0
    } else {
        starts
            .iter()
            .copied()
            .find(|&s| s > 0 && total - s + head <= fit)
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
            k(Action::PreviewUp)
        )));
        top = 1;
    }
    let rest = total - above;
    // Keep a row for what is left when not everything fits.
    let budget = if rest + top <= fit {
        fit - top
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
    if left > 0 && h > top {
        let mut more = format!(
            "… {left} more line{} · {} scroll",
            plural(left),
            k(Action::PreviewDown)
        );
        if editor {
            more.push_str(&format!(" · {} editor", k(Action::Edit)));
        }
        out.push(hint(more));
    }
    Scroll {
        lines: out,
        offset,
        max,
        below: left,
    }
}

pub fn draw_preview(f: &mut Frame, app: &App, area: Rect, pane: Pane) {
    if area.width < 4 || area.height < 2 {
        return;
    }
    // The wheel over the pane scrolls it.
    app.targets.borrow_mut().push((area, Target::Detail));
    super::ui::text_area(app, area, 0);
    let t = &app.theme;
    // Beside the list a blank column is enough; below it, a plain rule
    // shows where the list ends.
    let block = match pane {
        Pane::Side => Block::new().padding(Padding::left(1)),
        Pane::Bottom => Block::new().borders(Borders::TOP).border_style(t.muted()),
    };
    let unit = app.selected();
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 2 || inner.height == 0 {
        return;
    }
    let w = inner.width.saturating_sub(2) as usize;
    let h = inner.height as usize;
    let one = |lines: Vec<Line<'static>>| -> Vec<Vec<Line<'static>>> {
        lines.into_iter().map(|l| vec![l]).collect()
    };
    let Some(u) = unit else {
        let lines = vec![Line::styled(" Nothing selected", t.muted())];
        f.render_widget(
            Paragraph::new(scrolled(app, one(lines), w, h, 1, false)),
            inner,
        );
        return;
    };
    // What there is to say on top, the code change in its own box below,
    // taking the rest and scrolling on its own, with the statements in it.
    let boxed = super::diff::shows(app, u) && h > MIN_BOX_ROWS;
    let head = unit_lines(app, u, w, !boxed);
    if boxed {
        let air = usize::from(h >= head.len() + 1 + MIN_BOX_ROWS);
        let head_h = head.len().min(h - MIN_BOX_ROWS);
        let [top, _, code] = Layout::vertical([
            Constraint::Length(head_h as u16),
            Constraint::Length(air as u16),
            Constraint::Min(0),
        ])
        .areas(inner);
        let code = Rect::new(
            code.x + 1,
            code.y,
            code.width.saturating_sub(2),
            code.height,
        );
        if super::notes::draw_box(f, app, code, u, super::diff::Notes::Inline) {
            f.render_widget(Paragraph::new(cut(app, head, head_h)), top);
            return;
        }
    }
    let head = unit_lines(app, u, w, true);
    f.render_widget(
        Paragraph::new(scrolled(app, one(head), w, h, 1, false)),
        inner,
    );
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
        layout_for(area, false)
    }

    fn layout_for(area: Rect, code: bool) -> (Rect, Option<(Rect, Pane)>) {
        layout_fit(area, code, ListSize::FULL)
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
        // The list takes at most 35%: 38 of 110.
        assert_eq!(p.width, 72);
        assert_eq!(p.height, 20);
        assert_eq!(layout(r(109, 20)).1.map(|x| x.1), Some(Pane::Bottom));
    }

    #[test]
    fn narrow_gets_a_bottom_pane_sized_to_the_height() {
        for (h, want) in [
            (20u16, 8u16),
            (22, 8),
            (28, 11),
            (30, 12),
            (60, 12),
            (12, 5),
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
            assert_eq!(p.height, (h - 3).min(5), "h {h}");
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
            (120, 90, 42),
            (110, 0, 24),
            (200, 70, 70),
        ] {
            let (l, p) = layout_fit(r(w, 30), true, size(need));
            let (p, pane) = p.unwrap();
            assert_eq!(pane, Pane::Side);
            assert_eq!(l.width, want, "{w} {need}");
            assert_eq!((p.x, p.width), (want, w - want), "{w} {need}");
        }
        // A list of unknown size gets the most it can.
        assert_eq!(layout_for(r(120, 30), true).0.width, 42);
    }

    #[test]
    fn below_a_short_list_the_pane_takes_the_free_rows() {
        let size = |rows, padded_rows| ListSize {
            rows,
            padded_rows,
            ..ListSize::FULL
        };
        for (h, rows, padded, want) in [
            // One unit: the box edges and its row.
            (28u16, 3u16, 3u16, 25u16),
            (28, 10, 12, 18),
            // Not less than the usual share of about 60%.
            (28, 13, 15, 16),
            // From 14 rows on the list has blank rows between its boxes.
            (40, 14, 15, 25),
            // The list keeps 3 rows, on tiny terminals too.
            (40, 2, 2, 37),
            (5, 1, 1, 2),
        ] {
            let (l, p) = layout_fit(r(100, h), true, size(rows, padded));
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
        use crate::config::{Config, KeyMap};
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
                dive_pending: Default::default(),
                checks: Default::default(),
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
        };
        App::new(review, Config::default(), KeyMap::default())
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
    fn renders_selected_unit_in_both_panes() {
        let app = app(true);
        for (pane, w, h) in [(Pane::Side, 50, 20), (Pane::Bottom, 100, 8)] {
            let out = render(&app, w, h, pane);
            assert!(out.contains("my_symbol"), "{pane:?}:\n{out}");
        }
    }

    #[test]
    fn renders_empty_state_and_tiny_areas() {
        let app = app(false);
        for (pane, w, h) in [(Pane::Side, 50, 20), (Pane::Bottom, 100, 8)] {
            assert!(render(&app, w, h, pane).contains("Nothing selected"));
        }
        render(&app, 3, 1, Pane::Side);
        render(&app, 5, 2, Pane::Bottom);
    }
}
