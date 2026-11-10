//! The unit view (v2 Phase 5): any unit, with why it has its level, what
//! changed and the second model; the agent's advice when asked.
//!
//! One text scrolls under a title row: "Why this level" and "What changed"
//! (the summary, then the diff in its box with each statement under the
//! lines it cites; `]c` `[c` jump between hunks).
//!
//! Beside it tiles open, tmux style. "Advice" is open as the unit opens,
//! when there is an agent, and says which key asks the agent what to do
//! about the unit (`<space>a`, or `enter` on it): nothing is asked before.
//! Under the advice it shows the "Second model" when that disputes the
//! description; with the tile closed, the text shows it at its end.
//! The rest are added to the ones open: `K` asks about the unit
//! (the conversation is kept with it), `<space>t` `<space>r` `<space>v`
//! `<space>x` show the tests (teer's search; the tile then asks whether
//! the agent reads them), old against new, the diagram and the security
//! findings (teer's facts first; `<space>p` proves them), and `gd` shows
//! what the unit calls outside the review, read-only. Under them the keys
//! tile says what can be done here. The text takes a tall column on the
//! left (on top when narrow), the tiles the rest. `tab` and `shift+tab`
//! move the focus, `zz` zooms the tile in focus, `h` closes it. Deciding
//! here moves on to the next pending unit and stays in the view; `h` on
//! the text goes back to the queue.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::answers;
use super::app::{App, Target};
use super::{ask, diff, preview, slim};
use crate::keymap::Action;
use crate::review::model::{AcceptedBy, Decision, DiveKind, Prep, Unit};
use crate::session::help;
use crate::session::tools::{self, TOOLS, tool_key};
use crate::session::unit::{Panel, TileScroll};
use crate::util::{truncate, wrap};

/// Blanks between two items of the keys tile.
const KEY_GAP: usize = 2;

/// From this width the text sits beside the tiles, else above them.
const SIDE_BY_SIDE: u16 = 90;
/// Two tiles share a row once each gets this many columns.
const TILE_W: u16 = 48;
/// Blank rows above and below the advice tile's text while it only asks
/// or waits.
const ADVICE_AIR: u16 = 1;

// ---- Drawing -----------------------------------------------------------

/// Draw the unit view into `area`: the title row, the text, and the tiles
/// open beside it.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    if area.width < 4 || area.height == 0 {
        return;
    }
    let Some(u) = app.unit_on_show() else {
        let back = app.keys.label(Action::ViewBack);
        f.render_widget(
            Paragraph::new(Line::styled(
                format!(" Nothing to show. {back} goes back."),
                t.muted(),
            )),
            area,
        );
        return;
    };
    let [title, _, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(area.height >= 12)),
        Constraint::Min(0),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(title_line(app, u, title.width as usize)),
        title,
    );
    let tiles = &app.unit.tiles;
    let focus = app.unit.panel.filter(|p| tiles.contains(p));
    // Zoomed, or no room for more than one: what has the focus alone.
    let alone = app.unit.zoom || body.width < 40 || body.height < 8;
    if alone {
        match focus {
            Some(p) => draw_tile(f, app, u, p, body, true),
            None => draw_text(f, app, u, body, true),
        }
        return;
    }
    let keys_h = |w: u16, room: u16| keys_height(app, w).min(room / 2);
    let (text, cells, keys) = if tiles.is_empty() {
        // The keys tile under the text.
        let [text, keys] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(keys_h(body.width, body.height)),
        ])
        .areas(body);
        (text, Vec::new(), keys)
    } else {
        let small = |p: Panel, w: u16| small_height(app, u, p, w);
        tile_areas(tiles, body, keys_h, small)
    };
    draw_text(f, app, u, text, focus.is_none());
    for (p, r) in cells {
        draw_tile(f, app, u, p, r, focus == Some(p));
    }
    draw_keys(f, app, keys);
}

/// Where the text, each tile and the keys tile go in `area`. The text
/// takes a tall column on the left, [`SIDE_BY_SIDE`] wide or more (on top
/// otherwise): half the room beside one column of tiles (above one tile),
/// two fifths beside two (above more).
/// The tiles fill the rest in the order they opened, two to a row where
/// each gets [`TILE_W`] columns; the ask tile has a row of its own, twice
/// as tall as the others. A tile `small` gives rows for (at the column's
/// width) has a row of its own that tall; the room it leaves goes to the
/// others, or stays blank. The keys tile is under them, as tall as
/// `keys_h` says for the width and the room.
fn tile_areas(
    tiles: &[Panel],
    area: Rect,
    keys_h: impl Fn(u16, u16) -> u16,
    small: impl Fn(Panel, u16) -> Option<u16>,
) -> (Rect, Vec<(Panel, Rect)>, Rect) {
    let side = area.width >= SIDE_BY_SIDE;
    let two = side && tiles.len() > 1 && area.width * 3 / 5 >= 2 * TILE_W;
    let split = if two || (!side && tiles.len() > 1) {
        [Constraint::Ratio(2, 5), Constraint::Ratio(3, 5)]
    } else {
        [Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)]
    };
    let [text, rest] = if side {
        Layout::horizontal(split).areas(area)
    } else {
        Layout::vertical(split).areas(area)
    };
    let [rest, keys] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(keys_h(rest.width, rest.height)),
    ])
    .areas(rest);
    let small = |p: Panel| small(p, rest.width);
    let alone = |p: Panel| p == Panel::Ask || small(p).is_some();
    let mut rows: Vec<Vec<Panel>> = Vec::new();
    for &p in tiles {
        match rows.last_mut() {
            Some(row) if two && row.len() == 1 && !alone(p) && !alone(row[0]) => row.push(p),
            _ => rows.push(vec![p]),
        }
    }
    let heights = rows.iter().map(|r| match small(r[0]) {
        Some(h) => Constraint::Length(h),
        None => Constraint::Fill(if r[0] == Panel::Ask { 2 } else { 1 }),
    });
    let row_areas = Layout::vertical(heights).split(rest);
    let mut out = Vec::new();
    for (row, r) in rows.iter().zip(row_areas.iter()) {
        let cells =
            Layout::horizontal(vec![Constraint::Ratio(1, row.len() as u32); row.len()]).split(*r);
        out.extend(row.iter().copied().zip(cells.iter().copied()));
    }
    (text, out, keys)
}

/// The title row: the level's icon, the name and where it is, and on the
/// right whether it is pending, accepted or flagged (with the note's start).
fn title_line(app: &App, u: &Unit, w: usize) -> Line<'static> {
    let t = &app.theme;
    let (state, style) = match u.decision {
        Decision::Pending => ("pending".to_string(), t.muted()),
        Decision::Accepted if u.accepted_by == Some(AcceptedBy::Auto) => {
            ("✓ accepted for you".to_string(), t.ok())
        }
        Decision::Accepted => ("✓ accepted".to_string(), t.ok()),
        Decision::Investigate => {
            let note = u.dive.note.trim();
            let text = if note.is_empty() {
                "⚑ flagged".to_string()
            } else {
                format!("⚑ flagged: {}", truncate(&crate::util::sanitize(note), 40))
            };
            (text, t.badge_investigate())
        }
    };
    let state = format!(" {state} ");
    let level = app
        .assessment(&u.id)
        .map(|a| a.level.shown(u.auto_accepted()));
    let mut spans = vec![Span::raw(" ")];
    if let Some(l) = level {
        spans.push(Span::styled(
            l.icon().to_string(),
            slim::level_style(app, l),
        ));
    }
    let head = preview::unit_head(app, u, w.saturating_sub(state.width() + 3));
    spans.extend(head.spans);
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    spans.push(Span::raw(
        " ".repeat(w.saturating_sub(used + state.width())),
    ));
    spans.push(Span::styled(state, style));
    Line::from(spans)
}

/// A section's heading, one cell in.
fn heading(app: &App, text: &str) -> Line<'static> {
    Line::styled(format!(" {text}"), app.theme.bold())
}

/// The heading of what the agent wrote: the heading, then who wrote it,
/// cut to fit `w` cells.
fn heading_by(app: &App, text: &str, by: String, w: usize) -> Line<'static> {
    let room = w.saturating_sub(text.width() + 3);
    Line::from(vec![
        Span::styled(format!(" {text}"), app.theme.bold()),
        Span::styled(format!("  {}", truncate(&by, room)), app.theme.muted()),
    ])
}

/// What the second model says counts as a dispute: it disagrees with the
/// description somewhere.
fn disputed(app: &App, u: &Unit) -> bool {
    app.assessment(&u.id).is_some_and(|a| !a.doubts.is_empty())
}

/// The unit's text `w` cells wide as blocks a scroll keeps whole, and the
/// line each hunk of its diff starts at.
fn text_blocks(app: &App, u: &Unit, w: usize) -> (Vec<Vec<Line<'static>>>, Vec<usize>) {
    let t = &app.theme;
    let mut blocks: Vec<Vec<Line<'static>>> = Vec::new();
    // Text scrolls a line at a time.
    let add = |blocks: &mut Vec<Vec<Line<'static>>>, lines: Vec<Line<'static>>| {
        blocks.extend(lines.into_iter().map(|l| vec![l]));
    };
    // One cell in, as the text is.
    let indented = |l: Line<'static>| {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(l.spans);
        Line::from(spans).style(l.style)
    };

    // Why this level: the rules that set it and the written reasons.
    let mut why = preview::check_lines(app, u, w, 1, true);
    if why.is_empty()
        && let Some(a) = app.assessment(&u.id)
    {
        let level = a.level.shown(u.auto_accepted());
        why.push(heading(app, preview::WHY_HEADING));
        why.push(Line::from(vec![
            Span::styled(format!(" {} ", level.icon()), slim::level_style(app, level)),
            Span::styled(level.title().to_string(), t.muted()),
        ]));
    }
    why.extend(preview::commits_line(app, u, w, 1));
    add(&mut blocks, why);

    // What changed: the summary, then the diff in its box (one cell in,
    // borders and a cell of padding on each side) with the statements in it.
    let code = diff::code_change_of(app, u, w.saturating_sub(4), diff::Notes::Full);
    add(
        &mut blocks,
        vec![Line::default(), heading(app, "What changed")],
    );
    let mut said = preview::agent_lines(app, u, w.saturating_sub(2), code.blocks.is_empty(), 1);
    if u.prep != Prep::Ready && u.summary.is_none() && u.nbc_reason.is_none() {
        said.extend(slim::statement_lines(app, u, w, 1));
    }
    add(&mut blocks, said);
    let mut hunks = Vec::new();
    if !code.blocks.is_empty() {
        let top = code_edge(app, w, Some(diff::counts(app, code.added, code.removed)));
        add(&mut blocks, vec![Line::default(), top]);
        let mut at: usize = blocks.iter().map(Vec::len).sum();
        let mut starts = Vec::with_capacity(code.blocks.len());
        for b in &code.blocks {
            starts.push(at);
            at += b.len();
        }
        hunks = code
            .hunks
            .iter()
            .filter_map(|&h| starts.get(h).copied())
            .collect();
        blocks.extend(
            code.blocks
                .into_iter()
                .map(|b| b.into_iter().map(|l| code_row(app, l, w)).collect()),
        );
        add(&mut blocks, vec![code_edge(app, w, None)]);
    }
    let card = super::notes::card(app, u, w.saturating_sub(1));
    if !card.is_empty() {
        add(&mut blocks, vec![Line::default()]);
        add(&mut blocks, card.into_iter().map(indented).collect());
    }

    // The second model goes in the advice tile; with it closed, here.
    if !app.unit.tiles.contains(&Panel::Recommend) {
        add(&mut blocks, second_model_lines(app, u, w));
    }
    (blocks, hunks)
}

/// The second model `w` cells wide, after a blank line, when it disputes
/// the description (and there is an agent); else nothing.
fn second_model_lines(app: &App, u: &Unit, w: usize) -> Vec<Line<'static>> {
    if !second_model_shown(app, u) {
        return Vec::new();
    }
    let by = crate::session::tools::answer_label(app, u, DiveKind::Disagree);
    let mut lines = vec![Line::default(), heading_by(app, "Second model", by, w)];
    lines.extend(answers::disagree_lines(app, u, w));
    lines
}

/// Whether the second model shows: there is an agent and it disputes the
/// description.
fn second_model_shown(app: &App, u: &Unit) -> bool {
    app.can_ask() && disputed(app, u)
}

/// Whether the advice tile only asks or waits: no advice yet, and no
/// second model to show in it. Then it is small, its text in the middle.
fn advice_waits(app: &App, u: &Unit) -> bool {
    !u.dive.results.contains_key(&DiveKind::Recommend) && !second_model_shown(app, u)
}

/// What the advice tile says while it [waits](advice_waits), `w` cells
/// wide: the question (or that the agent is asked), each line centred.
fn advice_wait_lines(app: &App, u: &Unit, w: usize) -> Vec<Line<'static>> {
    answers::section_body(app, u, DiveKind::Recommend, w)
        .into_iter()
        .map(|mut l| {
            // Without the indent the other tiles' text has.
            if let Some(s) = l.spans.first_mut()
                && let Some(rest) = s.content.strip_prefix(' ')
            {
                s.content = rest.to_string().into();
            }
            l.centered()
        })
        .collect()
}

/// Rows tile `p` needs `w` columns wide, borders included, when it is
/// small: the advice tile while it [waits](advice_waits). None for the
/// others.
fn small_height(app: &App, u: &Unit, p: Panel, w: u16) -> Option<u16> {
    if p != Panel::Recommend || !advice_waits(app, u) {
        return None;
    }
    let lines = advice_wait_lines(app, u, (w as usize).saturating_sub(4)).len() as u16;
    Some(lines + 2 * ADVICE_AIR + 2)
}

/// The code box's edge `w` cells wide, one cell in: the top one titled
/// `code change` with `counts` on the right, as the queue's box is (see
/// [`diff::draw_box`]), or with none the bottom one.
pub(super) fn code_edge(app: &App, w: usize, counts: Option<Vec<Span<'static>>>) -> Line<'static> {
    let border = app.theme.text();
    match counts {
        Some(counts) => {
            let title = vec![Span::styled(diff::TITLE, app.theme.bold())];
            box_top(w, title, counts, border)
        }
        None => box_bottom(w, border),
    }
}

/// Row `l` of the code change inside its box `w` cells wide, one cell in:
/// a border and a cell of padding on each side.
pub(super) fn code_row(app: &App, l: Line<'static>, w: usize) -> Line<'static> {
    box_row(l, w, app.theme.text())
}

/// A box's top edge `w` cells wide, one cell in, drawn in `border`:
/// `title` on the left and `right` on the right.
pub(super) fn box_top(
    w: usize,
    title: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    border: Style,
) -> Line<'static> {
    let used = 2
        + title.iter().map(Span::width).sum::<usize>()
        + right.iter().map(Span::width).sum::<usize>();
    let mut spans = vec![Span::raw(" "), Span::styled("╭", border)];
    spans.extend(title);
    spans.push(Span::styled("─".repeat(w.saturating_sub(used)), border));
    spans.extend(right);
    spans.push(Span::styled("╮", border));
    Line::from(spans)
}

/// A box's bottom edge `w` cells wide, one cell in, drawn in `border`.
pub(super) fn box_bottom(w: usize, border: Style) -> Line<'static> {
    Line::from(vec![
        Span::raw(" "),
        Span::styled(format!("╰{}╯", "─".repeat(w.saturating_sub(2))), border),
    ])
}

/// Row `l` inside a box `w` cells wide, one cell in, drawn in `border`: a
/// border and a cell of padding on each side.
pub(super) fn box_row(l: Line<'static>, w: usize, border: Style) -> Line<'static> {
    let room = w.saturating_sub(4);
    let used = l.width();
    let base = l.style;
    let mut spans = vec![Span::raw(" "), Span::styled("│ ", border)];
    spans.extend(l.spans.into_iter().map(|s| {
        let style = base.patch(s.style);
        s.style(style)
    }));
    spans.push(Span::raw(" ".repeat(room.saturating_sub(used))));
    spans.push(Span::styled(" │", border));
    Line::from(spans)
}

/// The unit's text in `area`, scrolled where it was left. With `focus`
/// the scroll keys and the footer go by it.
fn draw_text(f: &mut Frame, app: &App, u: &Unit, area: Rect, focus: bool) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    app.targets.borrow_mut().push((area, Target::Detail));
    super::ui::text_area(app, area, 0);
    let w = (area.width as usize).saturating_sub(2);
    let h = area.height as usize;
    let (blocks, hunks) = text_blocks(app, u, w);
    *app.unit.hunks.borrow_mut() = hunks;
    let s = preview::scroll_rows(app, blocks, w, h, 1, true, app.detail_offset());
    app.detail_scroll.borrow_mut().1 = s.offset;
    app.unit.text_max.set(s.max);
    app.unit.text_rows.set(h);
    if focus {
        preview::keep_scroll(app, &s, h);
    }
    f.render_widget(Paragraph::new(s.lines), area);
}

/// Tile `p` in `area`: a rounded border (in the accent with the `focus`),
/// its name on top, and with the focus its keys at the bottom.
fn draw_tile(f: &mut Frame, app: &App, u: &Unit, p: Panel, area: Rect, focus: bool) {
    let t = &app.theme;
    if area.width < 12 || area.height < 3 {
        return;
    }
    app.targets.borrow_mut().push((area, Target::Panel(p)));
    let k = |a: Action| app.keys.label(a);
    let next = format!("{} next", k(Action::MoveNextTile));
    let (title, keys) = match p {
        Panel::Recommend => {
            let mut title = vec![
                Span::styled(format!(" {}", k(Action::ToolAdvice)), t.accent()),
                Span::styled(format!(" {} ", answers::ADVICE), t.bold()),
            ];
            if tools::pending(u, DiveKind::Recommend) {
                title.push(Span::styled(format!("{} ", app.spinner()), t.muted()));
            }
            let mut keys = format!(" {next} · {} close ", k(Action::ViewBack));
            if tools::may_advise(app, u) {
                keys = format!(" {} ask ·{keys}", k(Action::ViewOpen));
            }
            (Line::from(title), keys)
        }
        Panel::Ask => {
            let mut title = vec![
                Span::styled(format!(" {}", k(Action::AskOpen)), t.accent()),
                Span::styled(" Ask about it ", t.bold()),
            ];
            if app
                .chat_of(&u.id)
                .iter()
                .any(|n| app.ask.pending.contains(n))
            {
                title.push(Span::styled(format!("{} ", app.spinner()), t.muted()));
            }
            let keys = if app.ask.link.is_some() {
                format!(
                    " {} {} links · {} open · {} type · {} done ",
                    k(Action::MoveDown),
                    k(Action::MoveUp),
                    k(Action::ViewOpen),
                    k(Action::NoteEdit),
                    k(Action::ViewBack)
                )
            } else if app.unit.typing {
                format!(
                    " {} ask · {} links · {} again · {} done ",
                    super::ui::fixed::ENTER,
                    super::ui::fixed::TAB,
                    super::ui::fixed::RETRY,
                    super::ui::fixed::ESC
                )
            } else {
                format!(
                    " {} type · {next} · {} close ",
                    k(Action::AskOpen),
                    k(Action::ViewBack)
                )
            };
            (Line::from(title), keys)
        }
        Panel::Tool(kind) => {
            let (name, _) = tools::light(kind);
            let mut title = vec![
                Span::styled(format!(" {}", tool_key(app, kind)), t.accent()),
                Span::styled(format!(" {name} "), t.bold()),
            ];
            if tools::pending(u, kind) || (kind == DiveKind::Security && answers::security_busy(u))
            {
                title.push(Span::styled(format!("{} ", app.spinner()), t.muted()));
            }
            let mut keys = format!(" {next} · {} close ", k(Action::ViewBack));
            if kind == DiveKind::Visualize {
                keys = format!(" {} browser ·{keys}", k(Action::GotoBrowser));
            }
            if kind == DiveKind::Security && answers::may_prove(u) {
                keys = format!(" {} prove ·{keys}", k(Action::ToolProve));
            }
            if kind == DiveKind::Tests && tools::may_read_tests(app, u) {
                let none = (u.dive.results.get(&kind))
                    .and_then(|r| r.coverage.as_ref())
                    .and_then(|c| c.search.as_ref())
                    .is_some_and(|s| s.total == 0);
                let what = if none {
                    "agent looks"
                } else {
                    "agent reads them"
                };
                keys = format!(" {} {what} ·{keys}", k(Action::ViewOpen));
            }
            (Line::from(title), keys)
        }
        Panel::Outside => (
            Line::from(vec![
                Span::styled(format!(" {}", k(Action::GotoDefinition)), t.accent()),
                Span::styled(" What it calls outside the review ", t.bold()),
            ]),
            format!(" read-only · {next} · {} close ", k(Action::ViewBack)),
        ),
    };
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(if focus { t.accent() } else { t.muted() })
        .title(title);
    let keys_w = if focus { keys.width() } else { 0 };
    if focus {
        block = block.title_bottom(Line::styled(keys, t.muted()).right_aligned());
    }
    // Who gave the advice, or the findings.
    let by = match p {
        Panel::Recommend if u.dive.results.contains_key(&DiveKind::Recommend) => {
            Some(DiveKind::Recommend)
        }
        Panel::Tool(DiveKind::Security) if answers::audit_of(u).is_some() => {
            Some(DiveKind::Security)
        }
        _ => None,
    };
    if let Some(kind) = by {
        let by = crate::session::tools::answer_label(app, u, kind);
        let room = (area.width as usize).saturating_sub(keys_w + 6);
        block = block.title_bottom(
            Line::styled(format!(" {} ", truncate(&by, room)), t.muted()).left_aligned(),
        );
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    if inner.width < 4 || inner.height == 0 {
        return;
    }
    super::ui::text_area(app, inner, 0);
    match p {
        Panel::Recommend => {
            let w = inner.width as usize;
            if advice_waits(app, u) {
                // The question in the middle, nothing to scroll.
                let lines = advice_wait_lines(app, u, w);
                let h = (lines.len() as u16).min(inner.height);
                let top = (inner.height - h) / 2;
                let at = Rect::new(inner.x, inner.y + top, inner.width, h);
                f.render_widget(Paragraph::new(lines), at);
                app.unit.scroll.borrow_mut().remove(&p);
                if focus {
                    app.detail_max_scroll.set(0);
                    app.detail_rows.set(inner.height as usize);
                }
            } else {
                let mut lines = answers::section_body(app, u, DiveKind::Recommend, w);
                lines.extend(second_model_lines(app, u, w));
                draw_scrolled(f, app, p, lines, inner, focus);
            }
            // A click on the question asks.
            if tools::may_advise(app, u)
                && let Some(r) = super::select::find(f.buffer_mut(), inner, answers::ASK_ADVICE)
            {
                let row = Rect::new(inner.x, r.y, inner.width, 1);
                let to = Target::Action(Action::ToolAdvice);
                app.targets.borrow_mut().push((row, to));
            }
        }
        Panel::Ask => draw_ask(f, app, u, inner, focus),
        Panel::Tool(DiveKind::Visualize)
            if answers::draw_picture(f, app, u, inner, inner.width as usize) =>
        {
            if focus {
                app.detail_max_scroll.set(0);
                app.detail_rows.set(inner.height as usize);
            }
        }
        Panel::Tool(kind) => {
            let w = inner.width as usize;
            let lines = answers::section_body(app, u, kind, w);
            draw_scrolled(f, app, p, lines, inner, focus);
            // A click on the question has the agent read the tests.
            if kind == DiveKind::Tests
                && tools::may_read_tests(app, u)
                && let Some(r) = [answers::READ_TESTS, answers::LOOK_TESTS]
                    .iter()
                    .find_map(|q| super::select::find(f.buffer_mut(), inner, q))
            {
                let row = Rect::new(inner.x, r.y, inner.width, 1);
                app.targets.borrow_mut().push((row, Target::ReadTests));
            }
        }
        Panel::Outside => {
            let w = inner.width as usize;
            let mut lines = Vec::new();
            for (k, s) in app.unit.outside.iter().enumerate() {
                if k > 0 {
                    lines.push(Line::default());
                }
                lines.extend(ask::code_box(app, s, false, w));
            }
            draw_scrolled(f, app, p, lines, inner, focus);
        }
    }
}

/// `lines` in `area`, from tile `p`'s scroll on; with the `focus` the
/// scroll keys go by it.
fn draw_scrolled(
    f: &mut Frame,
    app: &App,
    p: Panel,
    lines: Vec<Line<'static>>,
    area: Rect,
    focus: bool,
) {
    let w = area.width as usize;
    let h = area.height as usize;
    let blocks = lines.into_iter().map(|l| vec![l]).collect();
    let at = app.unit.tile_offset(p);
    let s = preview::scroll_rows(app, blocks, w, h, 0, false, at);
    app.unit.scroll.borrow_mut().insert(
        p,
        TileScroll {
            offset: s.offset,
            max: s.max,
            rows: h,
        },
    );
    if focus {
        app.detail_max_scroll.set(s.max);
        app.detail_rows.set(h);
    }
    f.render_widget(Paragraph::new(s.lines), area);
}

/// Where an item of the keys tile is: its line, column and width, and the
/// action a click on it runs.
struct KeyAt {
    line: usize,
    x: usize,
    w: usize,
    action: Action,
}

/// The keys tile's lines `w` cells wide: each group's name, then its
/// items (`key what`) wrapped after it; and where each item is (line,
/// column, width) with the action a click on it runs. Without an agent
/// the items that need one are left out, and so is an item whose key was
/// unbound.
fn keys_lines(app: &App, w: usize) -> (Vec<Line<'static>>, Vec<KeyAt>) {
    let t = &app.theme;
    let label_w = help::KEY_GROUPS
        .iter()
        .map(|g| g.0.width())
        .max()
        .unwrap_or(0)
        + 2;
    // Too narrow for the names in a column of their own: none.
    let label_w = if w >= label_w + 24 { label_w } else { 0 };
    let mut lines = Vec::new();
    let mut at = Vec::new();
    for (name, items) in help::KEY_GROUPS {
        let mut spans: Vec<Span<'static>> = Vec::new();
        if label_w > 0 {
            spans.push(Span::styled(format!("{name:<label_w$}"), t.muted()));
        }
        let mut x = label_w;
        let mut first = true;
        for (actions, what) in *items {
            if !app.can_ask() && actions.iter().any(|a| help::AGENT_KEYS.contains(a)) {
                continue;
            }
            let keys: Vec<String> = actions.iter().map(|&a| app.keys.label(a)).collect();
            if keys.iter().any(String::is_empty) {
                continue;
            }
            let keys = keys.join(" ");
            let item_w = keys.width() + 1 + what.width();
            let sep = if first { 0 } else { KEY_GAP };
            if !first && x + sep + item_w > w {
                lines.push(Line::from(std::mem::take(&mut spans)));
                spans.push(Span::raw(" ".repeat(label_w)));
                x = label_w;
                first = true;
            }
            if !first {
                spans.push(Span::raw(" ".repeat(KEY_GAP)));
                x += KEY_GAP;
            }
            at.push(KeyAt {
                line: lines.len(),
                x,
                w: item_w,
                action: actions[0],
            });
            spans.push(Span::styled(keys, t.accent()));
            spans.push(Span::raw(format!(" {what}")));
            x += item_w;
            first = false;
        }
        if !first {
            lines.push(Line::from(spans));
        }
    }
    (lines, at)
}

/// Rows the keys tile takes `w` columns wide, borders included.
fn keys_height(app: &App, w: u16) -> u16 {
    let inner = (w as usize).saturating_sub(4);
    let rows = keys_lines(app, inner).0.len() + 2;
    rows.min(u16::MAX as usize) as u16
}

/// The keys tile in `area`: what can be done in the unit view, by group,
/// each item a click away. It never takes the focus; cut short, `?` lists
/// every key.
fn draw_keys(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    if area.width < 12 || area.height < 3 {
        return;
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.muted())
        .title(Span::styled(" Keys ", t.bold()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    if inner.width < 4 || inner.height == 0 {
        return;
    }
    super::ui::text_area(app, inner, 0);
    let (lines, at) = keys_lines(app, inner.width as usize);
    let shown = (inner.height as usize).min(lines.len());
    let mut targets = app.targets.borrow_mut();
    for k in at.into_iter().filter(|k| k.line < shown) {
        let w = (k.w as u16).min(inner.width.saturating_sub(k.x as u16));
        let r = Rect::new(inner.x + k.x as u16, inner.y + k.line as u16, w, 1);
        targets.push((r, Target::Action(k.action)));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// The ask tile: the conversation about `u` (or, before the first
/// question, what to ask and the tools that bring more light), a rule, and
/// the question being typed. With the `focus` the scroll keys go by it.
fn draw_ask(f: &mut Frame, app: &App, u: &Unit, area: Rect, focus: bool) {
    let t = &app.theme;
    let w = area.width as usize;
    let [talk, rule, input] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let chat = app.chat_of(&u.id);
    if chat.is_empty() {
        let lines = preview::cut(app, empty_ask(app, u, w), talk.height as usize);
        f.render_widget(Paragraph::new(lines), talk);
        // A click on a tool opens it.
        for (kind, action, name, _) in TOOLS {
            let shown = format!("{}  {name}", tool_key(app, kind));
            if let Some(r) = super::select::find(f.buffer_mut(), talk, &shown) {
                let row = Rect::new(talk.x, r.y, talk.width, 1);
                app.targets.borrow_mut().push((row, Target::Action(action)));
            }
        }
    } else {
        ask::draw_talk(
            f,
            app,
            talk,
            ask::Talk {
                which: &chat,
                scroll: &app.ask.chat_scroll,
                max: &app.ask.chat_max,
                focus: app.ask.link,
            },
        );
    }
    if focus {
        app.detail_rows.set(talk.height as usize);
        app.detail_max_scroll.set(app.ask.chat_max.get());
    }
    f.render_widget(Paragraph::new(Line::styled("─".repeat(w), t.muted())), rule);
    let typing = focus && app.unit.typing && app.ask.link.is_none();
    f.render_widget(
        Paragraph::new(ask::input_line(app, " › ", &app.ask.draft, typing, w)),
        input,
    );
}

/// What the ask panel says before the first question about `u`: how to
/// ask, then the tools that bring more light into it (Q7), each with its
/// key.
fn empty_ask(app: &App, u: &Unit, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines = Vec::new();
    let intro = if app.has_agent {
        "Ask about this unit in your own words: \"why is the retry here?\", \"who else calls it?\", \
\"show me the test for it\". The agent reads the code (read-only) and this conversation so far, \
so a question can follow on from the last answer. The conversation is kept with the unit."
    } else {
        "No agent: asking needs one. `teer setup` picks Claude Code, Codex or a model API."
    };
    for l in wrap(intro, w.saturating_sub(1)) {
        lines.push(Line::styled(format!(" {l}"), t.muted()));
    }
    if app.has_agent && app.web_may_ask() {
        let web = "The first question asks once whether the agent may also search the web.";
        for l in wrap(web, w.saturating_sub(1)) {
            lines.push(Line::styled(format!(" {l}"), t.muted()));
        }
    }
    lines.push(Line::default());
    lines.push(Line::styled(format!(" {}", answers::MENU), t.bold()));
    let key_w = TOOLS
        .iter()
        .map(|t| tool_key(app, t.0).width())
        .max()
        .unwrap_or(1);
    let name_w = TOOLS.iter().map(|t| t.2.width()).max().unwrap_or(0);
    for (kind, _, name, what) in TOOLS {
        // Without an agent only teer's own test search works.
        if !app.has_agent && kind != DiveKind::Tests {
            continue;
        }
        let done = u.dive.results.contains_key(&kind);
        let mut spans = vec![
            Span::styled(format!(" {:>key_w$}  ", tool_key(app, kind)), t.accent()),
            Span::styled(format!("{name:<name_w$}"), t.bold()),
        ];
        let room = w.saturating_sub(key_w + name_w + 6);
        let what = if done { "answered: it opens" } else { what };
        spans.push(Span::styled(
            format!("  {}", truncate(what, room)),
            t.muted(),
        ));
        lines.push(Line::from(spans));
    }
    lines
}

// ---- Behavior ----------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::git::{ChangedFile, FileStatus};
    use crate::keymap::{Keymap, parse_keys};
    use crate::review::model::{Cite, Range, Review, SessionRef, UnitKind, UnitStatus};
    use crate::session::app::{Effect, Screen};
    use std::path::PathBuf;

    fn unit(i: usize, file: &str) -> Unit {
        Unit {
            id: format!("u{i}"),
            file: PathBuf::from(file),
            symbol: format!("s{i}"),
            kind: UnitKind::Function,
            status: UnitStatus::Changed,
            old_range: Some(Range::new(1, 20)),
            new_range: Some(Range::new(1, 20)),
            renamed_from: None,
            fingerprint: format!("fp{i}"),
            changes: Vec::new(),
            prep: Prep::Skipped,
            callers: Vec::new(),
            callees: Vec::new(),
            decision: Decision::Pending,
            summary: None,
            nbc_reason: None,
            dive: Default::default(),
            accepted_by: None,
            reasons: Vec::new(),
            dive_pending: Default::default(),
            checks: Default::default(),
            security: Vec::new(),
        }
    }

    /// Twenty lines, changed at 2 and 17: two hunks.
    fn file(path: &str) -> ChangedFile {
        let old: Vec<String> = (1..=20).map(|i| format!("l{i}")).collect();
        let mut new = old.clone();
        new[1] = "L2".into();
        new[16] = "L17".into();
        ChangedFile {
            path: PathBuf::from(path),
            old_path: None,
            status: FileStatus::Modified,
            old_content: Some(old.join("\n")),
            new_content: Some(new.join("\n")),
        }
    }

    /// Four units in `a.rs` and `b.rs`, an agent with a session.
    fn app() -> App {
        let units = vec![
            unit(0, "a.rs"),
            unit(1, "a.rs"),
            unit(2, "b.rs"),
            unit(3, "b.rs"),
        ];
        let review = Review {
            id: "t".into(),
            repo_root: PathBuf::from("/r"),
            base: "b".into(),
            head: crate::git::WORKTREE.into(),
            head_commit: "h".into(),
            agent_session: Some(SessionRef::fresh("mock", PathBuf::from("/r"))),
            pr: None,
            intent: Default::default(),
            created_at: chrono::Utc::now(),
            files: vec![file("a.rs"), file("b.rs")],
            units,
            skipped: Vec::new(),
            graph: Default::default(),
            questions: Vec::new(),
            version: 0,
            history: Default::default(),
            branch: None,
            cursor: None,
        };
        let mut a = App::new(review, Config::default(), Keymap::default());
        a.has_agent = true;
        a
    }

    fn press(a: &mut App, keys: &str) -> Vec<Effect> {
        let mut out = Vec::new();
        for k in parse_keys(keys).unwrap() {
            out.extend(a.on_key(k.event()));
        }
        out
    }

    fn render(a: &App, w: u16, h: u16) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| super::super::ui::draw(f, a)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn recommends(effects: &[Effect]) -> Vec<String> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Dive {
                    unit_id,
                    kind: DiveKind::Recommend,
                    ..
                } => Some(unit_id.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn opening_asks_nothing_space_a_or_enter_on_the_tile_asks_once() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        let effects = a.open_unit(id.clone());
        assert_eq!(a.screen, Screen::Unit);
        assert!(recommends(&effects).is_empty(), "nothing before asked");
        assert!(a.review.unit(&id).unwrap().dive_pending.is_empty());
        assert_eq!(recommends(&press(&mut a, "<space>a")), vec![id.clone()]);
        assert_eq!(a.unit.panel, Some(REC));
        // On its way: not asked again; the key closes the tile.
        assert!(recommends(&press(&mut a, "<space>a")).is_empty());
        assert!(!a.unit.tiles.contains(&REC));
        // `enter` on the tile asks the next unit's.
        press(&mut a, "n");
        let next = a.unit.id.clone();
        assert_ne!(next, id);
        press(&mut a, "<tab>");
        assert_eq!(a.unit.panel, Some(REC));
        assert_eq!(recommends(&press(&mut a, "<cr>")), [next]);
    }

    #[test]
    fn the_text_has_the_sections_and_hunk_jumps_scroll_to_each_change() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        let text = render(&a, 100, 50);
        for s in [preview::WHY_HEADING, "What changed", "Advice"] {
            assert!(text.contains(s), "{s}:\n{text}");
        }
        assert!(!text.contains("Second model"), "{text}");
        // Short: the text scrolls.
        let text = render(&a, 100, 16);
        let hunks = a.unit.hunks.borrow().clone();
        assert_eq!(hunks.len(), 2, "{text}");
        assert_eq!(a.detail_offset(), 0);
        // The first hunk at the top, under the line that says what is above.
        press(&mut a, "]c");
        assert_eq!(a.detail_offset(), hunks[0]);
        let text = render(&a, 100, 16);
        let rows: Vec<&str> = text.lines().collect();
        assert!(rows[3].contains("lines above"), "{text}");
        assert!(rows[4].contains("2 - l2"), "{text}");
        // The second, as far as the text scrolls.
        press(&mut a, "]c");
        assert_eq!(a.detail_offset(), hunks[1].min(a.unit.text_max.get()));
        render(&a, 100, 16);
        press(&mut a, "[c");
        assert_eq!(a.detail_offset(), hunks[0]);
        assert_eq!(a.status, "");
        press(&mut a, "[c");
        assert_eq!(a.detail_offset(), hunks[0]);
        assert_eq!(a.status, "No more hunks above.");
    }

    #[test]
    fn deciding_moves_to_the_next_pending_unit_and_stays_here() {
        let mut a = app();
        let first = a.selected_id().unwrap();
        let from = a.screen;
        a.open_unit(first.clone());
        press(&mut a, "a");
        assert_eq!(a.screen, Screen::Unit);
        assert_eq!(a.review.unit(&first).unwrap().decision, Decision::Accepted);
        assert_ne!(a.unit.id, first);
        assert_eq!(a.selected_id().as_deref(), Some(a.unit.id.as_str()));
        press(&mut a, "h");
        assert_eq!(a.screen, from);
        assert_eq!(a.selected_id().as_deref(), Some(a.unit.id.as_str()));
    }

    #[test]
    fn h_after_a_look_aside_goes_back_to_the_row_opened_on() {
        let mut a = app();
        let first = a.selected_id().unwrap();
        let row = a.sel;
        let from = a.screen;
        a.open_unit(first.clone());
        // `gd`, `gr`, a link or `/` open another unit to look at it.
        let other = a.review.units[a.order[row + 2]].id.clone();
        assert!(a.go_to_unit(&other).is_some());
        assert_eq!(a.unit.id, other);
        press(&mut a, "h");
        assert_eq!((a.screen, a.sel), (from, row));
        // Moving on here (`n`) is the queue's cursor moving.
        a.open_unit(first);
        press(&mut a, "n");
        let moved = a.unit.id.clone();
        press(&mut a, "h");
        assert_eq!(a.selected_id(), Some(moved));
    }

    const TESTS: Panel = Panel::Tool(DiveKind::Tests);
    const RUN: Panel = Panel::Tool(DiveKind::Behavior);

    const REC: Panel = Panel::Recommend;

    #[test]
    fn each_tool_adds_a_tile_and_the_focus_goes_round_them() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        press(&mut a, "<space>t<space>r");
        assert_eq!(a.unit.tiles, [REC, TESTS, RUN]);
        assert_eq!(a.unit.panel, Some(RUN));
        // The text first, then the tiles in the order they opened.
        press(&mut a, "<tab>");
        assert_eq!(a.unit.panel, None);
        press(&mut a, "<tab>");
        assert_eq!(a.unit.panel, Some(REC));
        press(&mut a, "<tab>");
        assert_eq!(a.unit.panel, Some(TESTS));
        press(&mut a, "gTgT");
        assert_eq!(a.unit.panel, None);
        press(&mut a, "<s-tab>");
        assert_eq!(a.unit.panel, Some(RUN));
        // An open tile's key gives it the focus; in focus, closes it.
        press(&mut a, "<space>t");
        assert_eq!((a.unit.panel, a.unit.tiles.len()), (Some(TESTS), 3));
        press(&mut a, "<space>t");
        assert_eq!(a.unit.tiles, [REC, RUN]);
        assert_eq!(a.unit.panel, None);
    }

    #[test]
    fn h_closes_the_tile_in_focus_then_goes_back() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        let from = a.screen;
        a.open_unit(id);
        press(&mut a, "<space>t<space>r");
        // The one opened before it gets the focus.
        press(&mut a, "h");
        assert_eq!(
            (a.unit.tiles.clone(), a.unit.panel),
            (vec![REC, TESTS], Some(TESTS))
        );
        // Not what to do about it, open from the start: the text.
        press(&mut a, "h");
        assert_eq!((a.unit.tiles.clone(), a.unit.panel), (vec![REC], None));
        assert_eq!(a.screen, Screen::Unit);
        press(&mut a, "h");
        assert_eq!(a.screen, from);
        // In focus, it closes as any tile does; the next unit opens it again.
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        press(&mut a, "<tab>h");
        assert!(a.unit.tiles.is_empty());
        press(&mut a, "n");
        assert_eq!(a.unit.tiles, [REC]);
    }

    #[test]
    fn zz_zooms_what_has_the_focus_and_h_ends_the_zoom_first() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        // The text, with what to do about it beside it.
        press(&mut a, "zz");
        assert!(a.unit.zoom);
        let zoomed = render(&a, 120, 30);
        assert!(zoomed.contains("Why this level"), "{zoomed}");
        assert!(!zoomed.contains("Advice"), "{zoomed}");
        press(&mut a, "zz");
        assert!(!a.unit.zoom);
        press(&mut a, "<space>t");
        let side = render(&a, 120, 30);
        assert!(
            side.contains("Tests") && side.contains("Why this level"),
            "{side}"
        );
        press(&mut a, "zz");
        let zoomed = render(&a, 120, 30);
        assert!(zoomed.contains("Tests"), "{zoomed}");
        assert!(!zoomed.contains("Why this level"), "{zoomed}");
        press(&mut a, "h");
        assert!(!a.unit.zoom);
        assert_eq!(a.unit.panel, Some(TESTS));
        // A tile opening ends a zoom, so it shows.
        press(&mut a, "zz<space>r");
        assert!(!a.unit.zoom);
    }

    #[test]
    fn esc_stops_typing_and_the_ask_tile_stays_open() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        press(&mut a, "<space>tK");
        assert_eq!(a.unit.tiles, [REC, TESTS, Panel::Ask]);
        press(&mut a, "x<esc>");
        assert_eq!(a.ask.draft, "x");
        assert_eq!(a.unit.panel, Some(Panel::Ask));
        assert!(!a.unit.typing);
        // The view's keys work again: tab moves on, i types again.
        press(&mut a, "<tab>");
        assert_eq!(a.unit.panel, None);
        press(&mut a, "<s-tab>i");
        assert!(a.unit.typing);
        press(&mut a, "y");
        assert_eq!(a.ask.draft, "xy");
    }

    #[test]
    fn the_text_beside_its_tiles_wide_and_above_them_narrow() {
        let none = |_: u16, _: u16| 0;
        let big = |_: Panel, _: u16| None;
        let wide = Rect::new(0, 0, 120, 30);
        let (text, cells, _) = tile_areas(&[TESTS], wide, none, big);
        assert_eq!((text.width, cells[0].1.x), (60, 60));
        assert_eq!(cells[0].1.height, 30);
        // Two tiles stack in one column beside the text.
        let (text, cells, _) = tile_areas(&[TESTS, RUN], wide, none, big);
        assert_eq!(text.width, 60);
        assert_eq!(cells[0].1.x, cells[1].1.x);
        assert!(cells[0].1.y < cells[1].1.y);
        // Room for two to a row: they share it; the ask tile has its own,
        // twice as tall.
        let room = Rect::new(0, 0, 200, 30);
        let (text, cells, _) = tile_areas(&[TESTS, RUN, Panel::Ask], room, none, big);
        assert_eq!(text.width, 80);
        assert_eq!(cells[0].1.y, cells[1].1.y);
        assert!(cells[0].1.x < cells[1].1.x);
        assert_eq!(cells[2].1.width, 120);
        assert_eq!(cells[2].1.height, 2 * cells[0].1.height);
        // Narrow: the text on top.
        let (text, cells, _) = tile_areas(&[TESTS], Rect::new(0, 0, 80, 30), none, big);
        assert_eq!((text.width, text.height), (80, 15));
        assert_eq!((cells[0].1.y, cells[0].1.width), (15, 80));
        // The keys tile under the tiles, as tall as it says.
        let (text, cells, keys) = tile_areas(&[REC, TESTS], wide, |w, _| w / 10, big);
        assert_eq!((keys.x, keys.width, keys.height), (60, 60, 6));
        assert_eq!(keys.bottom(), 30);
        assert_eq!(cells[1].1.bottom(), keys.y);
        assert_eq!(text.height, 30);
        // A small tile: a row of its own that tall, the room left to the
        // others, or blank.
        let small = |p: Panel, _: u16| (p == REC).then_some(6);
        let (_, cells, _) = tile_areas(&[REC, TESTS, RUN], room, none, small);
        assert_eq!((cells[0].1.height, cells[0].1.width), (6, 120));
        assert_eq!(cells[1].1.y, 6);
        assert_eq!((cells[1].1.height, cells[1].1.y), (24, cells[2].1.y));
        let (_, cells, _) = tile_areas(&[REC], wide, none, small);
        assert_eq!((cells[0].1.y, cells[0].1.height), (0, 6));
    }

    #[test]
    fn the_advice_tile_is_open_from_the_start_beside_the_keys_and_waits() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        assert_eq!((a.unit.tiles.clone(), a.unit.panel), (vec![REC], None));
        let text = render(&a, 120, 40);
        let rows: Vec<&str> = text.lines().collect();
        // The tile beside the text, saying which key asks.
        let head = rows
            .iter()
            .position(|r| r.contains("╭ ␣a Advice"))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(rows[head].find("╭ ␣a").unwrap() >= 58, "{text}");
        assert!(
            text.contains("␣a ask the agent what to do about it:"),
            "{text}"
        );
        assert!(!text.contains("asking the agent"), "{text}");
        assert!(text.contains("␣a advice"), "in the keys tile:\n{text}");
        // The keys tile under it: what can be done here.
        let keys = rows.iter().position(|r| r.contains("╭ Keys")).unwrap();
        assert!(keys > head, "{text}");
        // Small while it only asks, the question in the middle of it.
        let (left, right) = tile_edges(&rows, head);
        let foot = advice_foot(&rows, head, left);
        // The question, its explanation on two rows, a blank row around.
        assert_eq!(foot - head, 6, "{text}");
        let q = rows[head + 2];
        let start = q.find("␣a ask").map(|i| q[..i].chars().count()).unwrap();
        let end = start + "␣a ask the agent what to do about it:".chars().count();
        assert!((start - left - 1).abs_diff(right - end) <= 1, "{text}");
        for item in [
            "a accept",
            "f flag",
            "K ask",
            "␣t tests",
            "n N pending",
            "]c [c hunks",
            "? every key",
        ] {
            assert!(text.contains(item), "{item}:\n{text}");
        }
        // A click on the question asks.
        let (row, line) = rows
            .iter()
            .enumerate()
            .find(|(_, l)| l.contains("ask the agent what"))
            .unwrap();
        let col = line[..line.find("ask the agent").unwrap()].chars().count() as u16;
        let mut effects = Vec::new();
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            effects.extend(a.on_mouse(MouseEvent {
                kind,
                column: col,
                row: row as u16,
                modifiers: KeyModifiers::NONE,
            }));
        }
        assert_eq!(recommends(&effects), [a.unit.id.clone()]);
        let text = render(&a, 120, 40);
        assert!(text.contains("asking the agent"), "{text}");
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(advice_foot(&rows, head, left) - head, 4, "{text}");
        // Answered: who wrote it on its bottom edge.
        let id = a.unit.id.clone();
        let u = a.review.unit_mut(&id).unwrap();
        u.dive_pending.remove(&DiveKind::Recommend);
        u.dive.results.insert(
            DiveKind::Recommend,
            crate::review::model::DiveResult {
                text: "Keep the retry, log the failure.".into(),
                ..Default::default()
            },
        );
        let text = render(&a, 120, 40);
        assert!(text.contains("Keep the retry, log the failure."), "{text}");
        assert!(text.contains("╰ agent ("), "{text}");
        // Then it grows down to the keys tile.
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(advice_foot(&rows, head, left) + 1, keys, "{text}");
    }

    /// The columns of the left and right edge of the tile whose top is on
    /// `row`, beside the text.
    fn tile_edges(rows: &[&str], row: usize) -> (usize, usize) {
        let cells: Vec<char> = rows[row].chars().collect();
        let left = (40..cells.len()).find(|&i| cells[i] == '╭').unwrap();
        let right = (left..cells.len()).find(|&i| cells[i] == '╮').unwrap();
        (left, right)
    }

    /// The row of the bottom edge of the tile whose top is on `head`, its
    /// left edge in column `left`.
    fn advice_foot(rows: &[&str], head: usize, left: usize) -> usize {
        (head + 1..rows.len())
            .find(|&i| rows[i].chars().nth(left) == Some('╰'))
            .unwrap()
    }

    #[test]
    fn a_click_on_a_key_in_the_keys_tile_runs_it() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        let text = render(&a, 120, 40);
        let (row, line) = text
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains("␣t tests"))
            .unwrap_or_else(|| panic!("{text}"));
        let col = line[..line.find("␣t tests").unwrap()].chars().count() as u16;
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            a.on_mouse(MouseEvent {
                kind,
                column: col + 1,
                row: row as u16,
                modifiers: KeyModifiers::NONE,
            });
        }
        assert_eq!(a.unit.panel, Some(TESTS));
    }

    #[test]
    fn without_an_agent_no_recommendation_and_only_the_keys_that_work() {
        let mut a = app();
        a.has_agent = false;
        a.review.agent_session = None;
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        assert!(a.unit.tiles.is_empty());
        let text = render(&a, 120, 40);
        assert!(!text.contains("Advice"), "{text}");
        assert!(text.contains("╭ Keys"), "{text}");
        assert!(text.contains("␣t tests"), "{text}");
        assert!(!text.contains("K ask"), "{text}");
    }

    /// Unit `id`'s tests as teer's search found them, not yet read.
    fn found(a: &mut App, id: &str, total: usize) {
        use crate::review::model::{Coverage, FoundTest, Reach, TestSearch};
        let tests = (0..total)
            .map(|k| FoundTest {
                name: format!("test_{k}"),
                cite: Cite {
                    file: PathBuf::from("tests/t.rs"),
                    start: 1,
                    end: 2,
                },
                reach: Reach::Names,
            })
            .collect();
        let u = a.review.unit_mut(id).unwrap();
        u.dive_pending.remove(&DiveKind::Tests);
        u.dive.results.insert(
            DiveKind::Tests,
            crate::review::model::DiveResult {
                coverage: Some(Coverage::found(TestSearch { tests, total })),
                ..Default::default()
            },
        );
    }

    fn reads_tests(effects: &[Effect]) -> bool {
        effects
            .iter()
            .any(|e| matches!(e, Effect::InterpretTests { .. }))
    }

    #[test]
    fn space_t_only_searches_then_asks_whether_the_agent_reads_them() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id.clone());
        let effects = press(&mut a, "<space>t");
        assert!(!reads_tests(&effects), "{effects:?}");
        found(&mut a, &id, 2);
        let text = render(&a, 120, 40);
        assert!(text.contains("2 tests reach it"), "{text}");
        assert!(text.contains("l/⏎ the agent reads them"), "{text}");
        assert!(
            text.contains("l agent reads them"),
            "the tile's keys:\n{text}"
        );
        // Closed and opened again: still only asks.
        let effects = press(&mut a, "<space>t<space>t");
        assert!(!reads_tests(&effects), "{effects:?}");
        // Yes: the agent reads them, once.
        let effects = press(&mut a, "<cr>");
        assert!(reads_tests(&effects), "{effects:?}");
        assert!(!reads_tests(&press(&mut a, "<cr>")));
        let text = render(&a, 120, 40);
        assert!(!text.contains("the agent reads them:"), "{text}");
        assert!(text.contains("asking the agent"), "{text}");
    }

    #[test]
    fn with_no_test_found_it_asks_whether_the_agent_looks_and_a_click_says_yes() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id.clone());
        press(&mut a, "<space>t");
        found(&mut a, &id, 0);
        let text = render(&a, 120, 40);
        assert!(text.contains("l agent looks ·"), "the tile's keys:\n{text}");
        // The focus elsewhere: the click still asks.
        press(&mut a, "<tab>");
        let text = render(&a, 120, 40);
        let (row, line) = text
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains(answers::LOOK_TESTS))
            .unwrap_or_else(|| panic!("{text}"));
        let col = line[..line.find(answers::LOOK_TESTS).unwrap()]
            .chars()
            .count() as u16;
        let mut effects = Vec::new();
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            effects.extend(a.on_mouse(MouseEvent {
                kind,
                column: col + 2,
                row: row as u16,
                modifiers: KeyModifiers::NONE,
            }));
        }
        assert!(reads_tests(&effects), "{effects:?}");
        assert_eq!(a.unit.panel, Some(TESTS));
    }

    #[test]
    fn without_an_agent_the_tests_tile_asks_nothing() {
        let mut a = app();
        a.has_agent = false;
        a.review.agent_session = None;
        let id = a.selected_id().unwrap();
        a.open_unit(id.clone());
        press(&mut a, "<space>t");
        found(&mut a, &id, 1);
        let text = render(&a, 120, 40);
        assert!(text.contains("1 test reaches it"), "{text}");
        assert!(!text.contains("the agent reads them"), "{text}");
        assert!(!reads_tests(&press(&mut a, "<cr>")));
    }

    #[test]
    fn the_diff_is_in_a_box_titled_like_the_queues() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        // The text alone.
        press(&mut a, "zz");
        let text = render(&a, 100, 50);
        let rows: Vec<&str> = text.lines().collect();
        let top = rows
            .iter()
            .position(|r| r.contains("╭ code change"))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(rows[top].trim_end().ends_with("+2 −2 ╮"), "{text}");
        assert!(rows[top + 1].starts_with(" │ "), "{text}");
        assert!(rows[top + 1].trim_end().ends_with('│'), "{text}");
        let bottom = rows.iter().position(|r| r.contains('╰')).unwrap();
        assert!(bottom > top + 2, "{text}");
        // Every row in between is boxed, the right edge in one column.
        let edge = rows[top].chars().count() - rows[top].trim_end().chars().count();
        for r in &rows[top..=bottom] {
            assert_eq!(
                r.chars().count() - r.trim_end().chars().count(),
                edge,
                "{text}"
            );
        }
    }
}
