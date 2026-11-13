//! The unit view (v2 Phase 5): any unit, with why it has its level, what
//! changed and the second model; the agent's advice when asked.
//!
//! One text scrolls under a title row: "Why this level" and "What changed"
//! (the summary, then the diff in its box with each statement under the
//! lines it cites; `]c` `[c` jump between hunks), then "Proposed change"
//! when the agent proposes one: each hunk boxed like the diff, where `]c`
//! `[c` stop too. The hunk at the top of the text is the current one, its
//! border in the accent: `p` puts it into the code, `P` every one.
//!
//! Beside it tiles open, tmux style. "Ask about it" is open as the unit
//! opens, when there is an agent: it offers the advice (what to do about
//! the unit: `<space>a`, or `enter` on it) and any other question (`K`);
//! nothing is asked before. The advice goes on top of the conversation,
//! with the "Second model" when that disputes the description; with the
//! tile closed, the text shows that at its end. The rest are added to the
//! ones open: `<space>t` shows the tests (teer's search; the tile then
//! asks whether the agent reads them), `<space>x` the security findings
//! (teer's facts first; `<space>p` proves them), and `gd` shows
//! what the unit calls outside the review, read-only. The text takes a tall column on the
//! left (on top when narrow), the tiles the rest. `tab` and `shift+tab`
//! move the focus, `zz` zooms the tile in focus, `h` closes it. The tiles
//! share their room as a terminal's panes do: each one opened takes a
//! part of the room the one before had, under it, then beside it. `<space>e`
//! with no change to propose opens one that says why. A lone
//! tile takes two thirds of its column; under it "Inspect further" lists
//! the keys that look closer at the code change (no focus). Deciding
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
use crate::fix::{HunkState, Proposal};
use crate::keymap::Action;
use crate::review::model::{AcceptedBy, Decision, DiveKind, Prep, Risk, Unit};
use crate::session::fix;
use crate::session::tools::{self, tool_key};
use crate::session::unit::{Panel, TileScroll};
use crate::util::{truncate, wrap};

/// From this width the text sits beside the tiles, else above them.
const SIDE_BY_SIDE: u16 = 90;
/// With more than one tile, the text gives up room (two fifths, not half)
/// once two tiles of this many columns fit beside it.
const TILE_W: u16 = 48;
/// The fewest rows and columns a pane keeps when the one after it opens
/// (see [`dwindle`]).
const PANE_H: u16 = 6;
const PANE_W: u16 = 28;
/// From this many rows a lone tile shares its column with "Inspect
/// further".
const INSPECT_FROM: u16 = 18;

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
    let (text, cells) = if tiles.is_empty() {
        (body, Vec::new())
    } else {
        let small = |p: Panel, w: u16| small_height(app, u, p, w);
        tile_areas(tiles, body, small)
    };
    // One tile open: it takes two thirds of its column, and under it a
    // tile says which keys look further into the code change.
    if let [(p, r)] = cells[..] {
        let column = Rect::new(r.x, r.y, r.width, body.bottom().saturating_sub(r.y));
        if column.height >= INSPECT_FROM {
            let [top, bottom] =
                Layout::vertical([Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)]).areas(column);
            draw_text(f, app, u, text, focus.is_none());
            draw_tile(f, app, u, p, top, focus == Some(p));
            draw_inspect(f, app, bottom);
            return;
        }
    }
    draw_text(f, app, u, text, focus.is_none());
    for (p, r) in cells {
        draw_tile(f, app, u, p, r, focus == Some(p));
    }
}

/// The title of the tile that says how to look further.
pub const INSPECT_TITLE: &str = " Inspect further ";

/// The keys that look further into the code change on show, each with
/// what it does: the ones the mode has, the tools only with what they
/// need.
fn inspect_items(app: &App) -> Vec<(String, &'static str)> {
    let k = |a: Action| app.keys.label(a);
    let agent = app.can_ask();
    let mut items = Vec::new();
    let mut add = |on: bool, a: Action, what: &'static str| {
        if on && app.mode.allows(a) {
            items.push((k(a), what));
        }
    };
    add(true, Action::ToolTests, "find the tests that reach it");
    add(
        app.security.enabled,
        Action::ToolSecurity,
        "check it for security issues",
    );
    add(agent, Action::ToolFix, "have the agent propose a change");
    add(true, Action::GotoDefinition, "show what it calls");
    add(
        true,
        Action::ToolHistory,
        "show its commits and the PR text",
    );
    add(true, Action::GotoEditor, "open it in your editor");
    items.push((
        format!("{} {}", k(Action::MoveNextHunk), k(Action::MovePrevHunk)),
        "go to the next or the last hunk",
    ));
    items
}

/// The "Inspect further" tile in `area`: a line that says what it is for,
/// then each key of [`inspect_items`] with what it does. It has no focus.
fn draw_inspect(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    if area.width < 12 || area.height < 3 {
        return;
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.muted())
        .title(Line::styled(INSPECT_TITLE, t.bold()));
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
    let w = inner.width as usize;
    let items = inspect_items(app);
    let key_w = items.iter().map(|(k, _)| k.width()).max().unwrap_or(0);
    let mut lines: Vec<Line<'static>> = wrap("These keys look further into this code change:", w)
        .into_iter()
        .map(|l| Line::styled(l, t.muted()))
        .collect();
    lines.push(Line::default());
    for (key, what) in items {
        let pad = " ".repeat(key_w.saturating_sub(key.width()) + 2);
        let room = w.saturating_sub(key_w + 2);
        lines.push(Line::from(vec![
            Span::styled(key, t.accent()),
            Span::raw(pad),
            Span::raw(truncate(what, room)),
        ]));
    }
    f.render_widget(
        Paragraph::new(preview::cut(app, lines, inner.height as usize)),
        inner,
    );
}

/// Where the text and each tile go in `area`. The text takes a tall
/// column on the left, [`SIDE_BY_SIDE`] wide or more (on top otherwise):
/// half the room beside one tile (above one tile), two fifths beside
/// more where two tiles fit side by side (above more).
/// The tiles share the rest as a terminal's panes do (see [`dwindle`]):
/// each one opened takes a part of the room the one before had. A lone
/// tile `small` gives rows for (at the column's width) is that tall, the
/// rest blank.
fn tile_areas(
    tiles: &[Panel],
    area: Rect,
    small: impl Fn(Panel, u16) -> Option<u16>,
) -> (Rect, Vec<(Panel, Rect)>) {
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
    if let [p] = tiles[..] {
        let h = small(p, rest.width).map_or(rest.height, |h| h.min(rest.height));
        return (text, vec![(p, Rect::new(rest.x, rest.y, rest.width, h))]);
    }
    let cells = dwindle(rest, tiles.len());
    (text, tiles.iter().copied().zip(cells).collect())
}

/// `area` cut into `n` panes, as tmux and kitty do when a pane opens: the
/// newest one takes two fifths of the room the one before it had, which
/// so gets smaller; under it first, then beside it, and so on in turn. A
/// cut that would leave a pane under [`PANE_H`] rows or [`PANE_W`]
/// columns goes the other way when that one has the room.
fn dwindle(area: Rect, n: usize) -> Vec<Rect> {
    let mut out = Vec::with_capacity(n);
    let mut rest = area;
    for i in 0..n {
        if i + 1 == n {
            out.push(rest);
            break;
        }
        let down = rest.height * 2 / 5 >= PANE_H;
        let across = rest.width * 2 / 5 >= PANE_W;
        let stack = match (down, across) {
            (true, false) => true,
            (false, true) => false,
            _ => i % 2 == 0,
        };
        let parts = [Constraint::Ratio(3, 5), Constraint::Ratio(2, 5)];
        let [first, next] = if stack {
            Layout::vertical(parts).areas(rest)
        } else {
            Layout::horizontal(parts).areas(rest)
        };
        out.push(first);
        rest = next;
    }
    out
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

/// The unit's text as blocks, and where its parts start, in lines.
struct Text {
    /// Blocks a scroll keeps whole.
    blocks: Vec<Vec<Line<'static>>>,
    /// The line each hunk of the diff starts at.
    hunks: Vec<usize>,
    /// The line each proposed hunk's box starts at.
    fixes: Vec<usize>,
    /// The line the "Proposed change" heading is on, when it shows.
    proposed: Option<usize>,
}

/// The unit's text `w` cells wide.
fn text_blocks(app: &App, u: &Unit, w: usize) -> Text {
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
    let mut why = vec![heading(app, preview::WHY_HEADING)];
    let reasons = preview::check_lines(app, u, w, 1);
    if reasons.is_empty()
        && let Some(a) = app.assessment(&u.id)
    {
        let level = a.level.shown(u.auto_accepted());
        why.push(Line::from(vec![
            Span::styled(format!(" {} ", level.icon()), slim::level_style(app, level)),
            Span::styled(level.title().to_string(), t.muted()),
        ]));
    }
    why.extend(reasons);
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

    // The second model goes in the ask tile; with it closed, here.
    if !app.unit.tiles.contains(&Panel::Ask) {
        add(&mut blocks, second_model_lines(app, u, w));
    }

    // The change the agent proposes, its hunks boxed like the code change.
    let at: usize = blocks.iter().map(Vec::len).sum();
    let (section, fixes) = proposed_blocks(app, u, w, at);
    // After its blank row.
    let proposed = (!section.is_empty()).then_some(at + 1);
    blocks.extend(section);
    Text {
        blocks,
        hunks,
        fixes,
        proposed,
    }
}

/// The "Proposed change" section `w` cells wide, its first line at row
/// `at` of the text, as blocks; and the row each hunk's box starts at.
/// Its heading says what it answers and who wrote it, then the summary,
/// the second model's word and how many hunks were dropped; then each
/// hunk in a box. The current one (`p` puts it in) has the accent border
/// and the keys on its bottom edge. While a change is on its way, the
/// heading with a spinner and what it is doing. Nothing with neither
/// (`<space>e` then opens a tile that says why: [`Panel::Fix`]).
fn proposed_blocks(
    app: &App,
    u: &Unit,
    w: usize,
    at: usize,
) -> (Vec<Vec<Line<'static>>>, Vec<usize>) {
    let t = &app.theme;
    let p = fix::proposal(u);
    let pending = tools::pending(u, DiveKind::Fix);
    if p.is_none() && !pending {
        return (Vec::new(), Vec::new());
    }
    let mut lines = vec![Line::default()];
    lines.extend(proposed_heading(app, u, pending, w));
    if pending {
        let wait = tools::waiting(app, u, DiveKind::Fix);
        lines.extend(
            wrap(&wait, w.saturating_sub(2))
                .into_iter()
                .map(|l| Line::styled(format!(" {l}"), t.muted())),
        );
    }
    let Some(p) = p else {
        return (lines.into_iter().map(|l| vec![l]).collect(), Vec::new());
    };
    lines.extend(
        wrap(&p.summary, w.saturating_sub(1))
            .into_iter()
            .map(|l| Line::raw(format!(" {l}"))),
    );
    match &p.second {
        Some(s) if s.agree => lines.push(Line::styled(" ✓ check agrees", t.ok())),
        Some(s) => lines.extend(
            wrap(&format!("⚠ disputed: {}", s.why), w.saturating_sub(1))
                .into_iter()
                .map(|l| Line::styled(format!(" {l}"), t.risk(Risk::Medium))),
        ),
        None => {}
    }
    if p.dropped > 0 {
        let s = if p.dropped == 1 { "" } else { "s" };
        lines.push(Line::styled(
            format!(" {} hunk{s} dropped (teer.log says why)", p.dropped),
            t.muted(),
        ));
    }
    let mut blocks: Vec<Vec<Line<'static>>> = lines.into_iter().map(|l| vec![l]).collect();
    // Each box after a blank row; where it starts decides the current one.
    let mut row = at + blocks.len();
    let mut starts = Vec::with_capacity(p.hunks.len());
    let mut boxes = Vec::with_capacity(p.hunks.len());
    for i in 0..p.hunks.len() {
        row += 1;
        starts.push(row);
        let b = hunk_box(app, p, i, w, false);
        row += b.iter().map(Vec::len).sum::<usize>();
        boxes.push(b);
    }
    if u.id == app.unit.id
        && let Some(c) = app.fix_at(&starts)
    {
        boxes[c] = hunk_box(app, p, c, w, true);
    }
    for b in boxes {
        blocks.push(vec![Line::default()]);
        blocks.extend(b);
    }
    (blocks, starts)
}

/// The heading of the proposed change `w` cells wide: what it answers
/// (the finding, wrapped under itself; the note; the advice), and who
/// wrote it after it, or on a line of its own where it doesn't fit; a
/// spinner while one is on its way.
fn proposed_heading(app: &App, u: &Unit, pending: bool, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut spans = vec![Span::styled(" Proposed change", t.bold())];
    if pending {
        spans.push(Span::styled(format!(" {}", app.spinner()), t.muted()));
    }
    let mut lines = Vec::new();
    let reason = fix::proposal(u)
        .map(|p| p.reason.clone())
        .or_else(|| fix::target(u));
    if let Some(r) = reason {
        let (what, severity) = fix::answers(&r);
        let (what, style) = match severity {
            Some(s) => (
                format!("{} {what} ({})", severity_glyph(s), s.word()),
                t.risk(s),
            ),
            None => (what, t.text()),
        };
        spans.push(Span::styled(" for ", t.muted()));
        let lead: usize = spans.iter().map(Span::width).sum();
        let mut parts = wrap(&what, w.saturating_sub(lead).max(12)).into_iter();
        spans.push(Span::styled(parts.next().unwrap_or_default(), style));
        lines.push(Line::from(std::mem::take(&mut spans)));
        lines.extend(parts.map(|l| Line::styled(format!("{}{l}", " ".repeat(lead)), style)));
    } else {
        lines.push(Line::from(std::mem::take(&mut spans)));
    }
    let by = tools::answer_label(app, u, DiveKind::Fix);
    let last = lines.last_mut().expect("a heading line");
    if last.width() + 2 + by.width() <= w {
        last.spans.push(Span::styled(format!("  {by}"), t.muted()));
    } else {
        lines.push(Line::styled(
            format!(" {}", truncate(&by, w.saturating_sub(1))),
            t.muted(),
        ));
    }
    lines
}

/// How a finding's severity shows, as the Security tile shows it.
fn severity_glyph(r: Risk) -> &'static str {
    match r {
        Risk::High => "▲",
        Risk::Medium => "◆",
        Risk::Low => "●",
    }
}

/// Hunk `i` of proposal `p` in a box `w` cells wide, one cell in, as
/// blocks: titled with its place, its state on the right; why it is
/// there, whether its file still parses, then its lines as the code
/// change draws its own. The `current` one has the accent border and the
/// keys that put it in on its bottom edge.
fn hunk_box(app: &App, p: &Proposal, i: usize, w: usize, current: bool) -> Vec<Vec<Line<'static>>> {
    let t = &app.theme;
    let h = &p.hunks[i];
    let border = if current { t.accent() } else { t.text() };
    let (state, state_style) = match h.state {
        HunkState::Proposed => (fix::state_word(h.state), t.muted()),
        HunkState::Applied => (fix::state_word(h.state), t.ok()),
        HunkState::Suggested => (fix::state_word(h.state), t.accent()),
    };
    let right = vec![Span::styled(format!(" {state} "), state_style)];
    let room = w.saturating_sub(state.width() + 6);
    let title = vec![Span::styled(
        format!(" {} ", truncate(&fix::place(h), room)),
        t.bold(),
    )];
    let inner = w.saturating_sub(4);
    let mut out = vec![vec![box_top(w, title, right, border)]];
    for l in wrap(&h.why, inner) {
        out.push(vec![box_row(Line::styled(l, t.muted()), w, border)]);
    }
    if !h.parses {
        let warn = Line::styled("⚠ doesn't parse", t.risk(Risk::Medium));
        out.push(vec![box_row(warn, w, border)]);
    }
    out.extend(
        diff::proposed_rows(app, h, inner)
            .into_iter()
            .map(|b| b.into_iter().map(|l| box_row(l, w, border)).collect()),
    );
    let keys = if current {
        fix_keys(app, p, i)
    } else {
        Vec::new()
    };
    out.push(vec![box_bottom_with(w, keys, border)]);
    out
}

/// The keys on the current hunk's bottom edge: `p put · P put all`
/// (`suggest` on a pull request), as far as there is something to put in.
fn fix_keys(app: &App, p: &Proposal, i: usize) -> Vec<Span<'static>> {
    let t = &app.theme;
    let verb = if app.review.pr.is_some() {
        "suggest"
    } else {
        "put"
    };
    let mut items = Vec::new();
    if p.hunks[i].state == HunkState::Proposed {
        items.push((app.keys.label(Action::FixApply), verb.to_string()));
    }
    if !p.open().is_empty() {
        items.push((app.keys.label(Action::FixApplyAll), format!("{verb} all")));
    }
    let mut spans = Vec::new();
    for (k, (key, what)) in items.into_iter().filter(|(k, _)| !k.is_empty()).enumerate() {
        let lead = if k == 0 { " " } else { " · " };
        spans.push(Span::styled(lead, t.muted()));
        spans.push(Span::styled(key, t.accent()));
        spans.push(Span::styled(format!(" {what}"), t.muted()));
    }
    if !spans.is_empty() {
        spans.push(Span::styled(" ", t.muted()));
    }
    spans
}

/// The second model `w` cells wide, after a blank line, when it disputes
/// the description (and there is an agent); else nothing.
fn second_model_lines(app: &App, u: &Unit, w: usize) -> Vec<Line<'static>> {
    if !second_model_shown(app, u) {
        return Vec::new();
    }
    let by = crate::session::tools::answer_label(app, u, DiveKind::Disagree);
    let mut lines = vec![Line::default(), heading_by(app, "Check", by, w)];
    lines.extend(answers::disagree_lines(app, u, w));
    lines
}

/// Whether the second model shows: there is an agent and it disputes the
/// description.
fn second_model_shown(app: &App, u: &Unit) -> bool {
    app.can_ask() && disputed(app, u)
}

/// What the ask tile shows above the conversation, `w` cells wide: the
/// advice once it was asked for (the answer, or that it is on its way)
/// under its heading and who gave it, then the second model when it
/// disputes the description. Empty before any of that.
fn advice_lines(app: &App, u: &Unit, w: usize) -> Vec<Line<'static>> {
    let kind = DiveKind::Recommend;
    let mut lines = Vec::new();
    let answered = u.dive.results.contains_key(&kind);
    if answered || tools::pending(u, kind) {
        let by = if answered {
            tools::answer_label(app, u, kind)
        } else {
            String::new()
        };
        lines.push(heading_by(app, answers::ADVICE, by, w));
        lines.extend(answers::section_body(app, u, kind, w));
    }
    let second = second_model_lines(app, u, w);
    // Its blank line parts it from the advice only.
    let skip = usize::from(lines.is_empty());
    lines.extend(second.into_iter().skip(skip));
    lines
}

/// Whether the ask tile has nothing to show yet: no advice asked for, no
/// second model, no question. Then it is small: what it offers, and the
/// question line.
fn ask_empty(app: &App, u: &Unit) -> bool {
    let kind = DiveKind::Recommend;
    app.chat_of(&u.id).is_empty()
        && !u.dive.results.contains_key(&kind)
        && !tools::pending(u, kind)
        && !second_model_shown(app, u)
}

/// Rows tile `p` needs `w` columns wide, borders included, when it is
/// small: the ask tile while it [has nothing yet](ask_empty), and the
/// Security tile until the agent's audit is there (teer's facts and a
/// line or two: no empty box under them). None for the others.
fn small_height(app: &App, u: &Unit, p: Panel, w: u16) -> Option<u16> {
    let inner = (w as usize).saturating_sub(4);
    match p {
        Panel::Ask if ask_empty(app, u) => {
            // What it says and offers, the rule, the question line, the
            // borders.
            let lines = empty_ask(app, inner + 2).len() + offer_lines(app, u, inner + 2).len();
            Some(lines as u16 + 2 + 2)
        }
        Panel::Tool(DiveKind::Security) if answers::audit_of(u).is_none() => {
            let lines = answers::section_body(app, u, DiveKind::Security, inner);
            // Without a trailing blank row.
            let n = lines.len() - lines.iter().rev().take_while(|l| l.width() == 0).count();
            Some(n.max(1) as u16 + 2)
        }
        _ => None,
    }
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

/// A box's bottom edge `w` cells wide, one cell in, drawn in `border`,
/// with `right` on the right (nothing: [`box_bottom`]).
fn box_bottom_with(w: usize, right: Vec<Span<'static>>, border: Style) -> Line<'static> {
    if right.is_empty() {
        return box_bottom(w, border);
    }
    let used = 2 + right.iter().map(Span::width).sum::<usize>();
    let mut spans = vec![
        Span::raw(" "),
        Span::styled("╰", border),
        Span::styled("─".repeat(w.saturating_sub(used)), border),
    ];
    spans.extend(right);
    spans.push(Span::styled("╯", border));
    Line::from(spans)
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
    let Text {
        blocks,
        hunks,
        fixes,
        proposed,
    } = text_blocks(app, u, w);
    *app.unit.hunks.borrow_mut() = hunks;
    // `<space>e` (or `p` from the queue) asked to see a proposed hunk
    // before its box was drawn, or the section: scroll to it now.
    if app.unit.reveal.take()
        && let Some(row) = app
            .unit
            .fix
            .and_then(|i| fixes.get(i).copied())
            .or(proposed)
    {
        app.detail_offset();
        app.detail_scroll.borrow_mut().1 = row;
    }
    *app.unit.fixes.borrow_mut() = fixes;
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
    // `s send` where the tile in focus has a result to send to an agent
    // session.
    let send = |keys: String| {
        if focus && app.can_send(u, p) {
            format!(" {} send ·{keys}", k(Action::SendSession))
        } else {
            keys
        }
    };
    let (title, keys) = match p {
        Panel::Ask => {
            let mut title = vec![
                Span::styled(format!(" {}", k(Action::AskOpen)), t.accent()),
                Span::styled(" Ask about it ", t.bold()),
            ];
            if tools::pending(u, DiveKind::Recommend)
                || app
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
                let keys = send(format!(
                    " {} type · {next} · {} close ",
                    k(Action::AskOpen),
                    k(Action::ViewBack)
                ));
                if app.offers_at_once() {
                    format!(" {} asks 1 ·{keys}", k(Action::ViewOpen))
                } else {
                    keys
                }
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
            let mut keys = send(format!(" {next} · {} close ", k(Action::ViewBack)));
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
        Panel::Fix => (
            Line::from(vec![
                Span::styled(format!(" {}", k(Action::ToolFix)), t.accent()),
                Span::styled(format!(" {} ", fix::TILE), t.bold()),
            ]),
            format!(" {next} · {} close ", k(Action::ViewBack)),
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
    // Who gave the findings.
    let by = match p {
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
        Panel::Ask => draw_ask(f, app, u, inner, focus),
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
        Panel::Fix => {
            // Why there is no change to propose, behind a bar.
            let w = (inner.width as usize).saturating_sub(2);
            let lines = wrap(&app.fix_tile_text(u), w)
                .into_iter()
                .map(|l| Line::styled(format!("▎ {l}"), t.accent()))
                .collect();
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

/// The ask tile: the conversation about `u` (or, before the first
/// question, what to ask and the tools that bring more light), a rule, and
/// the question being typed. With the `focus` the scroll keys go by it.
fn draw_ask(f: &mut Frame, app: &App, u: &Unit, area: Rect, focus: bool) {
    let t = &app.theme;
    let w = area.width as usize;
    // What the tile offers to ask next sits over the question line, cut
    // so the conversation keeps a row.
    let mut offered = offer_lines(app, u, w);
    offered.truncate((area.height as usize).saturating_sub(3));
    let [talk, offers, rule, input] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(offered.len() as u16),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    // A click on an offered question asks it.
    for (row, (_, i)) in offered.iter().enumerate() {
        if let Some(i) = i {
            let r = Rect::new(offers.x, offers.y + row as u16, offers.width, 1);
            app.targets.borrow_mut().push((r, Target::Suggest(*i)));
        }
    }
    let lines: Vec<Line> = offered.into_iter().map(|(l, _)| l).collect();
    f.render_widget(Paragraph::new(lines), offers);
    let chat = app.chat_of(&u.id);
    if ask_empty(app, u) {
        let lines = preview::cut(app, empty_ask(app, w), talk.height as usize);
        f.render_widget(Paragraph::new(lines), talk);
        app.ask.chat_max.set(0);
    } else {
        ask::draw_talk(
            f,
            app,
            talk,
            ask::Talk {
                head: advice_lines(app, u, w),
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

/// What the ask tile says before anything is asked, `w` cells wide: that
/// anything can be asked, with its key. What it offers to ask is under it
/// ([`offer_lines`]).
fn empty_ask(app: &App, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines = Vec::new();
    if !app.has_agent {
        let none =
            "No agent: asking needs one. `teer setup` picks Claude Code, Codex or a model API.";
        for l in wrap(none, w.saturating_sub(1)) {
            lines.push(Line::styled(format!(" {l}"), t.muted()));
        }
        return lines;
    }
    lines.push(Line::from(vec![
        Span::styled(format!(" {} ", app.keys.label(Action::AskOpen)), t.accent()),
        Span::styled(answers::ASK_ANYTHING, t.bold()),
    ]));
    lines
}

/// Rows a question offered takes at most.
const OFFER_ROWS: usize = 2;

/// The questions the ask tile offers to ask next about `u`
/// ([`App::suggestions_for`]), `w` cells wide: a line saying how one is
/// asked, then each under its number with what it follows up, and with
/// each line the question a click on it asks. Nothing when none is
/// offered.
fn offer_lines(app: &App, u: &Unit, w: usize) -> Vec<(Line<'static>, Option<usize>)> {
    let t = &app.theme;
    let offered = app.suggestions_for(u);
    if offered.is_empty() {
        return Vec::new();
    }
    let enter = super::ui::fixed::ENTER;
    let how = if offered[0].offer == crate::session::suggest::Offer::Audit {
        format!(" ask next: its number and {enter} asks it")
    } else if offered.len() == 1 {
        format!(" ask next: {enter} asks it")
    } else {
        format!(" ask next: {enter} asks 1, a number and {enter} another")
    };
    let mut lines = vec![(
        Line::styled(crate::util::truncate(&how, w), t.muted()),
        None,
    )];
    for (i, s) in offered.iter().enumerate() {
        let text = if s.about.is_empty() {
            s.text.clone()
        } else {
            format!("{} ({})", s.text, s.about)
        };
        let mut rows = wrap(&text, w.saturating_sub(4));
        if rows.len() > OFFER_ROWS {
            rows.truncate(OFFER_ROWS);
            if let Some(last) = rows.last_mut() {
                *last = crate::util::truncate(&format!("{last}…"), w.saturating_sub(4));
            }
        }
        for (k, row) in rows.into_iter().enumerate() {
            let lead = if k == 0 {
                Span::styled(format!(" {} ", i + 1), t.accent())
            } else {
                Span::raw("   ")
            };
            lines.push((Line::from(vec![lead, Span::styled(row, t.bold())]), Some(i)));
        }
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
            mode: None,
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
    fn opening_asks_nothing_enter_on_the_tile_asks_once() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        let effects = a.open_unit(id.clone());
        assert_eq!(a.screen, Screen::Unit);
        assert!(recommends(&effects).is_empty(), "nothing before asked");
        assert!(a.review.unit(&id).unwrap().dive_pending.is_empty());
        // `enter` on the tile takes the question it offers.
        assert_eq!(recommends(&press(&mut a, "<tab><cr>")), vec![id.clone()]);
        assert_eq!(a.unit.panel, Some(REC));
        // On its way: not asked again; a question is typed instead.
        assert!(recommends(&press(&mut a, "<cr>")).is_empty());
        assert!(a.unit.typing);
        press(&mut a, "<esc>");
        // The same on the next unit.
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
        for s in [preview::WHY_HEADING, "What changed", "K Ask"] {
            assert!(text.contains(s), "{s}:\n{text}");
        }
        assert!(!text.contains("Check  agent"), "{text}");
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
    const RUN: Panel = Panel::Tool(DiveKind::Security);

    const REC: Panel = Panel::Ask;

    #[test]
    fn each_tool_adds_a_tile_and_the_focus_goes_round_them() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        press(&mut a, "<space>t<space>x");
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
        press(&mut a, "<space>t<space>x");
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
        press(&mut a, "zz<space>x");
        assert!(!a.unit.zoom);
    }

    #[test]
    fn esc_stops_typing_and_the_ask_tile_stays_open() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        press(&mut a, "<space>tK");
        // The ask tile is the one open from the start.
        assert_eq!(a.unit.tiles, [REC, TESTS]);
        press(&mut a, "x<esc>");
        assert_eq!(a.ask.draft, "x");
        assert_eq!(a.unit.panel, Some(Panel::Ask));
        assert!(!a.unit.typing);
        // The view's keys work again: tab moves on, i types again.
        press(&mut a, "<tab>");
        assert_eq!(a.unit.panel, Some(TESTS));
        press(&mut a, "<s-tab>i");
        assert!(a.unit.typing);
        press(&mut a, "y");
        assert_eq!(a.ask.draft, "xy");
    }

    #[test]
    fn the_text_beside_its_tiles_wide_and_above_them_narrow() {
        let big = |_: Panel, _: u16| None;
        let wide = Rect::new(0, 0, 120, 30);
        let (text, cells) = tile_areas(&[TESTS], wide, big);
        assert_eq!((text.width, cells[0].1.x), (60, 60));
        assert_eq!(cells[0].1.height, 30);
        // Two tiles: the second takes two fifths of the first one's room,
        // under it.
        let (text, cells) = tile_areas(&[TESTS, RUN], wide, big);
        assert_eq!(text.width, 60);
        assert_eq!(cells[0].1.x, cells[1].1.x);
        assert_eq!((cells[0].1.height, cells[1].1.height), (18, 12));
        // A third takes its part of the second one's, beside it; a fourth
        // of the third one's, under it.
        let room = Rect::new(0, 0, 200, 30);
        let (text, cells) = tile_areas(&[Panel::Ask, TESTS, RUN, Panel::Fix], room, big);
        assert_eq!(text.width, 80);
        let r: Vec<Rect> = cells.iter().map(|c| c.1).collect();
        assert_eq!((r[0].width, r[0].height), (120, 18));
        assert_eq!((r[1].y, r[1].width, r[1].height), (18, 72, 12));
        assert_eq!((r[2].x, r[2].y, r[2].width), (152, 18, 48));
        assert_eq!((r[3].x, r[3].y), (152, r[2].y + r[2].height));
        assert_eq!(r[2].height + r[3].height, 12);
        // No room beside: under it again.
        let (_, cells) = tile_areas(&[Panel::Ask, TESTS, RUN], Rect::new(0, 0, 100, 60), big);
        assert_eq!(cells[1].1.x, cells[2].1.x);
        assert!(cells[1].1.y < cells[2].1.y);
        // Narrow: the text on top.
        let (text, cells) = tile_areas(&[TESTS], Rect::new(0, 0, 80, 30), big);
        assert_eq!((text.width, text.height), (80, 15));
        assert_eq!((cells[0].1.y, cells[0].1.width), (15, 80));
        // A lone small tile is that tall, the rest blank; with others it
        // shares the room as they do.
        let small = |p: Panel, _: u16| (p == REC).then_some(6);
        let (_, cells) = tile_areas(&[REC, TESTS], wide, small);
        assert_eq!(cells[0].1.height, 18);
        let (_, cells) = tile_areas(&[REC], wide, small);
        assert_eq!((cells[0].1.y, cells[0].1.height), (0, 6));
    }

    #[test]
    fn the_ask_tile_is_open_from_the_start_and_offers_the_advice() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        assert_eq!((a.unit.tiles.clone(), a.unit.panel), (vec![REC], None));
        let text = render(&a, 120, 40);
        let rows: Vec<&str> = text.lines().collect();
        // The tile beside the text, saying which keys ask.
        let head = rows
            .iter()
            .position(|r| r.contains("╭ K Ask"))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(rows[head].find("╭ K").unwrap() >= 58, "{text}");
        assert!(text.contains("K ask anything about it"), "{text}");
        // What it offers to ask, under its number.
        assert!(text.contains("ask next: ⏎ asks"), "{text}");
        assert!(
            text.contains("1 What should I do about this change"),
            "{text}"
        );
        assert!(!text.contains("asking the agent"), "{text}");
        assert!(!text.contains("Advice"), "no advice tile:\n{text}");
        assert!(!text.contains("␣a"), "no key of its own:\n{text}");
        // No keys tile: `?` lists the keys.
        assert!(!text.contains("╭ Keys"), "{text}");
        // Alone, it takes two thirds of its column; under it the tile
        // with the keys that look further.
        let (left, _) = tile_edges(&rows, head);
        let foot = advice_foot(&rows, head, left);
        assert_eq!(foot - head, 23, "{text}");
        assert!(rows[foot + 1].contains(INSPECT_TITLE), "{text}");
        // A click on the offer asks for the advice.
        let (row, line) = rows
            .iter()
            .enumerate()
            .find(|(_, l)| l.contains("What should I do"))
            .unwrap();
        let col = line[..line.find("What should I do").unwrap()]
            .chars()
            .count() as u16;
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
        assert!(text.contains("Advice"), "{text}");
        assert!(text.contains("asking the agent"), "{text}");
        // Asked for: the tile is as tall as the others, down to the bar.
        let rows: Vec<&str> = text.lines().collect();
        assert!(advice_foot(&rows, head, left) - head > 8, "{text}");
        // Answered: the advice under its heading, with who wrote it.
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
        assert!(text.contains("Advice  agent ("), "{text}");
        // A question typed after it goes under the advice, in the same tile.
        press(&mut a, "Kwhy<cr>");
        assert_eq!(a.unit.tiles, [REC]);
        let text = render(&a, 120, 40);
        assert!(text.contains("why"), "{text}");
    }

    #[test]
    fn enter_with_nothing_typed_asks_for_the_advice_once() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id.clone());
        // K types; enter on the empty question takes the tile's offer.
        assert_eq!(
            recommends(&press(&mut a, "K<cr>")),
            std::slice::from_ref(&id)
        );
        assert!(!a.unit.typing);
        // It is on its way: enter types a question now, and asks nothing.
        assert!(recommends(&press(&mut a, "<cr>")).is_empty());
        assert!(a.unit.typing);
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
    fn without_an_agent_no_recommendation_and_no_tile() {
        let mut a = app();
        a.has_agent = false;
        a.review.agent_session = None;
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        assert!(a.unit.tiles.is_empty());
        let text = render(&a, 120, 40);
        assert!(!text.contains("Advice"), "{text}");
        assert!(
            !text.contains("╭ Keys") && !text.contains("K ask"),
            "{text}"
        );
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
