//! Improve mode's list on the terminal (see [`crate::session::improve`]):
//! a box per kind of insert (fills a gap, makes it stronger, polish) in
//! the level colours, a row per insert: `+`, its file, its unit, why it is
//! there, and at the right edge where it stands (proposed, put in,
//! suggested, left out). Under the boxes, how many units have nothing to
//! add and how many the agent still looks at. Beside the list (below it on
//! a narrow terminal), the insert under the cursor: its unit, what the
//! agent said of the unit's inserts and the second model's word, and the
//! hunk in a box as the unit view draws it (why, lines), with the keys on
//! its edge. With no insert yet, a line says what the agent is doing.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Target};
use super::preview::{self, ListSize, Pane};
use super::slim::{self, Rows};
use super::ui;
use super::unit::{box_bottom, box_row, box_top, hunk_box};
use crate::fix::{HunkState, InsertKind};
use crate::keymap::Action;
use crate::review::attention::Level;
use crate::review::model::{DiveKind, Risk};
use crate::session::improve::{Insert, inserts_of};
use crate::session::{fix, names, tools};
use crate::util::{sanitize, truncate, truncate_pad, wrap};

/// Cells of a row before its path: the bar's column, a gap, `+`, a gap.
const LEAD: usize = 5;
/// Widest the path column gets, as a share of the row.
const PATH_SHARE: usize = 3;
/// Cells of a unit's name that stay whole before the why gives way.
const NAME_KEEP: usize = 20;

/// The level whose colours a kind's box takes: the most needed first.
fn level_of(kind: InsertKind) -> Level {
    match kind {
        InsertKind::Gap => Level::Attention,
        InsertKind::Stronger => Level::Look,
        InsertKind::Polish => Level::Routine,
    }
}

/// Draw the list into `area`, and the insert under the cursor beside or
/// below it.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    if app.order.is_empty() {
        ui::draw_empty(f, app, area);
        return;
    }
    let inserts = app.inserts();
    if inserts.is_empty() {
        draw_none(f, app, area);
        return;
    }
    let (list, pane) = preview::layout_fit(area, true, ListSize::FULL);
    let padded = list.height as usize >= slim::PADDING_MIN_ROWS;
    let rows = build_rows(app, &inserts, list.width as usize, padded);
    let sel = app.isel.min(inserts.len() - 1);
    ui::draw_rows(f, app, list, &rows, sel, Target::Insert);
    if let Some((rect, pane)) = pane {
        draw_insert(f, app, rect, pane, inserts[sel]);
    }
}

/// With no insert yet: what the agent is doing, or why there is none.
fn draw_none(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let k = |a: Action| app.keys.label(a);
    let icon = crate::mode::Mode::Improve.icon();
    let (done, total) = app.improve_progress();
    let text = if !app.can_ask() {
        "No agent: improve mode needs one to write the inserts. `teer setup` picks one.".to_string()
    } else if app.improve_pending() > 0 {
        format!(
            "{} the agent looks for what to add · {done} of {total} units",
            app.spinner()
        )
    } else if total > 0 && done == total {
        format!(
            "Nothing to add: the agent found no insert for {total} unit{}. {} changes the mode.",
            if total == 1 { "" } else { "s" },
            k(Action::ToolMode)
        )
    } else {
        "The agent describes the units first; then it looks for what to add.".to_string()
    };
    let w = (area.width as usize).saturating_sub(4);
    let mut lines = vec![Line::default()];
    for (n, l) in wrap(&text, w.max(8)).into_iter().enumerate() {
        let lead = if n == 0 {
            format!(" {icon} ")
        } else {
            "    ".to_string()
        };
        lines.push(Line::styled(format!("{lead}{l}"), t.muted()));
    }
    ui::text_area(app, area, 0);
    f.render_widget(Paragraph::new(lines), area);
}

// ---- The rows ----------------------------------------------------------

/// The list's rows: a box per kind with its inserts, then the lines under
/// them. `owner` is the insert's place in `inserts`.
pub(super) fn build_rows(app: &App, inserts: &[Insert], width: usize, padded: bool) -> Rows {
    let t = &app.theme;
    let n = inserts.len();
    let mut rows = Rows {
        lines: Vec::new(),
        owner: Vec::new(),
        spans: vec![(0, 0); n],
        heads: vec![0; n],
        titles: vec![0; n],
        ends: vec![0; n],
    };
    let boxes = width >= 4;
    let inner = if boxes { width - 2 } else { width };
    let (path_w, name_w) = columns(app, inserts, inner);
    let mut k = 0;
    let mut first_box = true;
    for kind in InsertKind::EVERY {
        let in_box: Vec<usize> = (k..n).take_while(|&j| inserts[j].kind == kind).collect();
        if in_box.is_empty() {
            continue;
        }
        if padded && !first_box {
            rows.lines.push(Line::raw(" ".repeat(width)));
            rows.owner.push(None);
        }
        first_box = false;
        let title = rows.lines.len();
        let count = in_box.len();
        let name = format!(
            "{} · {count} insert{}",
            kind.title(),
            if count == 1 { "" } else { "s" }
        );
        if boxes {
            rows.lines.push(slim::top_edge(
                app,
                width,
                &name,
                slim::level_style(app, level_of(kind)),
            ));
            rows.owner.push(None);
        }
        for &j in &in_box {
            let line = rows.lines.len();
            let selected = j == app.isel.min(n - 1);
            rows.lines
                .push(row(app, inserts[j], inner, path_w, name_w, selected, boxes));
            rows.owner.push(Some(j));
            rows.spans[j] = (line, line + 1);
            rows.heads[j] = line;
            rows.titles[j] = title;
            rows.ends[j] = line + 1;
        }
        if boxes {
            rows.lines.push(slim::bottom_edge(app, width));
            rows.owner.push(None);
            // The last insert of a box keeps the box's edge in view.
            if let Some(&last) = in_box.last() {
                rows.ends[last] = rows.lines.len() + usize::from(padded);
            }
        }
        k += count;
    }
    // Under the boxes: the units with nothing to add, and the ones the
    // agent still looks at.
    let mut said = Vec::new();
    let none = app.nothing_to_add();
    if none > 0 {
        said.push(format!(
            "{none} unit{} ha{} nothing to add",
            if none == 1 { "" } else { "s" },
            if none == 1 { "s" } else { "ve" }
        ));
    }
    let (done, total) = app.improve_progress();
    if app.improve_pending() > 0 {
        said.push(format!(
            "{} the agent looks at {} more",
            app.spinner(),
            total.saturating_sub(done)
        ));
    }
    if !said.is_empty() {
        if padded {
            rows.lines.push(Line::raw(" ".repeat(width)));
            rows.owner.push(None);
        }
        let text = format!("{} {}", Level::Same.icon(), said.join(" · "));
        for l in wrap(&text, width.saturating_sub(1)) {
            rows.lines.push(Line::styled(
                truncate_pad(&format!(" {l}"), width),
                t.muted(),
            ));
            rows.owner.push(None);
        }
    }
    if padded && !rows.lines.is_empty() {
        rows.lines.push(Line::raw(" ".repeat(width)));
        rows.owner.push(None);
    }
    rows
}

/// The path and name columns' widths in a row `inner` cells wide: the
/// widest of each, the path at most a third of the row, the name as far
/// as the why keeps a few words.
fn columns(app: &App, inserts: &[Insert], inner: usize) -> (usize, usize) {
    let units = inserts.iter().map(|i| &app.review.units[i.unit]);
    let path_w = units
        .clone()
        .map(|u| sanitize(&u.file.display().to_string()).width())
        .max()
        .unwrap_or(0)
        .min(inner / PATH_SHARE);
    let name_w = units
        .map(|u| names::row_name(u).width())
        .max()
        .unwrap_or(0)
        .min(inner.saturating_sub(LEAD + path_w + 2 + 12).max(NAME_KEEP));
    (path_w, name_w)
}

/// Insert `i`'s row: the bar, `+`, its file, its unit, why, and its state
/// at the right edge; the selection's background behind it all.
fn row(
    app: &App,
    i: Insert,
    inner: usize,
    path_w: usize,
    name_w: usize,
    selected: bool,
    boxes: bool,
) -> Line<'static> {
    let t = &app.theme;
    let u = &app.review.units[i.unit];
    let bg = if selected {
        t.selected_entry()
    } else {
        Style::default()
    };
    let (state, state_style) = match app.insert_hunk(i).map(|h| h.state) {
        Some(HunkState::Applied) => ("put in", t.ok()),
        Some(HunkState::Suggested) => ("suggested", t.accent()),
        Some(HunkState::LeftOut) => ("left out", t.muted()),
        _ => ("proposed", t.text()),
    };
    let why = app
        .insert_hunk(i)
        .map(|h| sanitize(&h.why))
        .unwrap_or_default();
    let path = slim::fit_path(&sanitize(&u.file.display().to_string()), path_w);
    let name = truncate(&names::row_name(u), name_w);
    let mut spans = vec![
        Span::styled(if selected { "▌" } else { " " }, t.accent().patch(bg)),
        Span::styled(" ", bg),
        Span::styled("+", t.marker(crate::review::model::Marker::Added).patch(bg)),
        Span::styled(" ", bg),
        Span::styled(format!("{path:<path_w$}"), t.muted().patch(bg)),
        Span::styled("  ", bg),
        Span::styled(format!("{name:<name_w$}"), t.bold().patch(bg)),
        Span::styled("  ", bg),
    ];
    let used: usize = spans.iter().map(Span::width).sum();
    let tail = state.width() + 1;
    let room = inner.saturating_sub(used + tail + 1);
    let why = truncate(&why, room);
    let pad = inner.saturating_sub(used + why.width() + tail);
    spans.push(Span::styled(why, t.text().patch(bg)));
    spans.push(Span::styled(" ".repeat(pad), bg));
    spans.push(Span::styled(state, state_style.patch(bg)));
    spans.push(Span::styled(" ", bg));
    if boxes {
        let mut line = vec![Span::styled("│", t.muted())];
        line.extend(spans);
        line.push(Span::styled("│", t.muted()));
        Line::from(line)
    } else {
        Line::from(spans)
    }
}

// ---- The insert under the cursor ----------------------------------------

/// Beside or below the list: the insert's unit, what the agent said of
/// the unit's inserts with the second model's word, and the hunk in its
/// box (its why, its lines) with the keys on its edge.
fn draw_insert(f: &mut Frame, app: &App, area: Rect, pane: Pane, i: Insert) {
    if area.width < 4 || area.height < 2 {
        return;
    }
    app.targets.borrow_mut().push((area, Target::Detail));
    ui::text_area(app, area, 0);
    let t = &app.theme;
    let block = match pane {
        Pane::Side => Block::new().padding(Padding::left(1)),
        Pane::Bottom => Block::new().borders(Borders::TOP).border_style(t.muted()),
    };
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 2 || inner.height == 0 {
        return;
    }
    let w = inner.width.saturating_sub(2) as usize;
    let h = inner.height as usize;
    let u = &app.review.units[i.unit];
    let Some(p) = inserts_of(u) else {
        return;
    };
    let Some(hunk) = p.hunks.get(i.hunk) else {
        return;
    };
    // The unit, and what the insert does for it.
    let level = app.shown_level(i.unit);
    let head = vec![
        Span::styled(format!(" {} ", level.icon()), slim::level_style(app, level)),
        Span::styled(
            truncate(&names::row_name(u), w.saturating_sub(16)),
            t.bold(),
        ),
        Span::styled(format!(" · {}", i.kind.name()), t.muted()),
    ];
    let mut blocks: Vec<Vec<Line<'static>>> = vec![vec![Line::from(head)], vec![Line::default()]];
    // What the agent said of the unit's inserts, and the checks, in a box
    // titled as the queue's review box is; the why is on the hunk's box.
    let border = t.text();
    let by = tools::answer_label(app, u, DiveKind::Improve);
    let bw = w.saturating_sub(1);
    let mut card = vec![box_top(
        bw,
        vec![Span::styled(" inserts ", t.bold())],
        vec![Span::styled(format!(" {by} "), t.muted())],
        border,
    )];
    for l in wrap(&sanitize(&p.summary), bw.saturating_sub(4)) {
        card.push(box_row(Line::styled(l, t.text()), bw, border));
    }
    match &p.second {
        Some(s) if s.agree => {
            card.push(box_row(Line::styled("✓ check agrees", t.ok()), bw, border))
        }
        Some(s) => {
            for l in wrap(
                &format!("⚠ disputed: {}", sanitize(&s.why)),
                bw.saturating_sub(4),
            ) {
                card.push(box_row(Line::styled(l, t.risk(Risk::Medium)), bw, border));
            }
        }
        None => {}
    }
    if !hunk.parses {
        card.push(box_row(
            Line::styled("⚠ doesn't parse", t.risk(Risk::Medium)),
            bw,
            border,
        ));
    }
    card.push(box_bottom(bw, border));
    blocks.push(card);
    blocks.push(vec![Line::default()]);
    // The hunk as the unit view draws it, current: the keys on its edge.
    blocks.extend(hunk_box(app, p, i.hunk, bw, true));
    f.render_widget(
        Paragraph::new(preview::scrolled(app, blocks, w, h, 1, false)),
        inner,
    );
}

/// The bottom bar's counts on the list: `2 proposed · 1 put in · 1 left
/// out`, the states with none left out; with no insert proposed, that.
pub(super) fn count_spans(app: &App) -> (Vec<Span<'static>>, Option<usize>) {
    let t = &app.theme;
    let (proposed, put_in, suggested, left_out) = app.insert_counts();
    let sep = || Span::styled(" · ", t.muted());
    let mut spans = Vec::new();
    if proposed > 0 {
        spans.push(Span::styled(format!("{proposed} proposed"), t.bold()));
    } else if put_in + suggested + left_out == 0 {
        spans.push(Span::styled("No insert yet", t.muted()));
    } else {
        spans.push(Span::styled("No insert left", t.bold()));
    }
    for (n, what, style) in [
        (put_in, "put in", t.ok()),
        (suggested, "suggested", t.accent()),
        (left_out, "left out", t.muted()),
    ] {
        if n > 0 {
            spans.push(sep());
            spans.push(Span::styled(format!("{n} {what}"), style));
        }
    }
    (spans, None)
}

/// The word of a hunk's state the list and the bar use.
#[allow(dead_code)]
fn state_word(s: HunkState) -> &'static str {
    fix::state_word(s)
}
