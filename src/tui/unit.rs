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
//! show the tests (teer's search; the tile then asks whether the agent
//! reads them), old against new and the diagram, and `gd` shows what the
//! unit calls outside the review, read-only. Under them the keys tile says
//! what can be done here. The text takes a tall column on the left (on top
//! when narrow), the tiles the rest. `tab` and `shift+tab` move the focus,
//! `zz` zooms the tile in focus, `h` closes it. Deciding here moves on to
//! the next pending unit and stays in the view; `h` on the text goes back
//! to the queue.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::answers::{self, TOOLS, tool_key, tool_kind};
use super::app::{
    App, Effect, JumpKind, Overlay, QUESTION_MAX, SCROLL_STEP, Screen, Target, typed,
};
use super::{ask, diff, preview, slim};
use crate::keymap::{Action, Edit};
use crate::review::model::{AcceptedBy, Cite, Decision, DiveKind, Prep, Shown, Unit};
use crate::util::{truncate, wrap};

/// The actions the unit view handles, for the help, in the order it lists
/// them inside each of its groups (Move, Decide, Look closer, More, Finish).
pub const ACTIONS: &[Action] = &[
    Action::MoveNextPending,
    Action::MovePrevPending,
    Action::MoveNextHunk,
    Action::MovePrevHunk,
    Action::MoveNextFile,
    Action::MovePrevFile,
    Action::MoveNextTile,
    Action::MovePrevTile,
    Action::ToggleZoom,
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::ScrollHalfDown,
    Action::ScrollHalfUp,
    Action::ScrollPageDown,
    Action::ScrollPageUp,
    Action::FindUnit,
    Action::ViewBack,
    Action::DecideAccept,
    Action::DecideFlag,
    Action::DecideAcceptFile,
    Action::DecideFlagFile,
    Action::NoteEdit,
    Action::HistoryUndo,
    Action::HistoryRedo,
    Action::HistoryRepeat,
    Action::AskOpen,
    Action::GotoDefinition,
    Action::GotoCallers,
    Action::GotoEditor,
    Action::GotoBrowser,
    Action::ToolAdvice,
    Action::ToolTests,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
    Action::ToolHistory,
    Action::ToolNotes,
    Action::ToolFlagged,
    Action::ToolSettings,
    Action::HelpOpen,
    Action::FinishOpen,
    Action::ReviewRerun,
    Action::ReviewReset,
    Action::ViewClose,
];

/// An item of the keys tile: the actions it stands for (the keys of all
/// of them shown, the first one run on a click) and a word or two for it.
type KeyItem = (&'static [Action], &'static str);

/// What the keys tile lists, by group.
const KEY_GROUPS: &[(&str, &[KeyItem])] = &[
    (
        "Decide",
        &[
            (&[Action::DecideAccept], "accept"),
            (&[Action::DecideFlag], "flag"),
            (&[Action::NoteEdit], "note"),
            (&[Action::HistoryUndo], "undo"),
        ],
    ),
    (
        "Look closer",
        &[
            (&[Action::ToolAdvice], "advice"),
            (&[Action::AskOpen], "ask"),
            (&[Action::ToolTests], "tests"),
            (&[Action::ToolRunOldNew], "old vs new"),
            (&[Action::ToolDiagram], "diagram"),
            (&[Action::GotoDefinition], "calls"),
            (&[Action::GotoCallers], "callers"),
            (&[Action::GotoEditor], "editor"),
            (&[Action::ToolFlagged], "all flagged"),
        ],
    ),
    (
        "Move",
        &[
            (
                &[Action::MoveNextPending, Action::MovePrevPending],
                "pending",
            ),
            (&[Action::MoveNextHunk, Action::MovePrevHunk], "hunks"),
            (&[Action::MoveNextTile], "tiles"),
            (&[Action::ToggleZoom], "zoom"),
            (&[Action::ViewBack], "back"),
        ],
    ),
    (
        "Finish",
        &[
            (&[Action::FinishOpen], "finish"),
            (&[Action::HelpOpen], "every key"),
        ],
    ),
];

/// Blanks between two items of the keys tile.
const KEY_GAP: usize = 2;

/// Items of the keys tile that need an agent.
const AGENT_KEYS: [Action; 4] = [
    Action::ToolAdvice,
    Action::AskOpen,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
];

/// Most lines of one definition `gd` shows from outside the review.
const OUTSIDE_LINES: usize = 60;
/// Most definitions `gd` shows from outside the review.
const OUTSIDE_MOST: usize = 8;
/// Largest file `gd` reads a definition from.
const OUTSIDE_BYTES: usize = 512 * 1024;

/// From this width the text sits beside the tiles, else above them.
const SIDE_BY_SIDE: u16 = 90;
/// Two tiles share a row once each gets this many columns.
const TILE_W: u16 = 48;

/// A tile beside the unit's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    /// What the agent says to do about the unit: open as the unit opens.
    Recommend,
    /// `K`: the conversation about the unit and the question being typed.
    Ask,
    /// A leader tool's answer: the tests (`Tests`), old against new
    /// (`Behavior`) or the diagram (`Visualize`).
    Tool(DiveKind),
    /// `gd`: what the unit calls outside the review, read-only.
    Outside,
}

/// How far a tile is scrolled, how far it can, and its rows, as last drawn.
#[derive(Debug, Clone, Copy, Default)]
pub struct TileScroll {
    pub offset: usize,
    pub max: usize,
    pub rows: usize,
}

/// The unit view's state.
#[derive(Debug, Default)]
pub struct UnitView {
    /// The unit on show.
    pub id: String,
    /// The screen `h` goes back to.
    pub from: Option<Screen>,
    /// The unit whose row `h` puts the queue's cursor on: the one opened,
    /// or the one moving or deciding here went on to. A unit gone to to
    /// look (`gd`, `gr`, a link, `/`) leaves it.
    pub home: String,
    /// The tiles open beside the text, in the order they opened.
    pub tiles: Vec<Panel>,
    /// The tile in focus; `None` is the unit's text.
    pub panel: Option<Panel>,
    /// The tile in focus fills the view (`zz`).
    pub zoom: bool,
    /// Keys go into the ask tile's question; else, with a link in focus
    /// (`App::ask.link`), to the links ([`App::ask_links`]); else, with the
    /// ask tile in focus, to the view as on any tile.
    pub typing: bool,
    /// The definitions `gd` found outside the review.
    pub outside: Vec<Shown>,
    /// Where each hunk of the diff starts, in lines of the text, as last
    /// drawn.
    pub hunks: RefCell<Vec<usize>>,
    /// How far the text can scroll and its rows, as last drawn.
    pub text_max: Cell<usize>,
    pub text_rows: Cell<usize>,
    /// Each tile's scroll but the ask tile's (that is the conversation's,
    /// `App::ask.chat_scroll`).
    pub scroll: RefCell<HashMap<Panel, TileScroll>>,
    /// Asked this session whether questions may use the web (Q7).
    pub web_asked: bool,
    /// The question waiting for that answer, by index.
    pub web_question: Option<usize>,
}

impl UnitView {
    /// Where tile `p` is scrolled to.
    pub fn tile_offset(&self, p: Panel) -> usize {
        self.scroll.borrow().get(&p).map_or(0, |s| s.offset)
    }
}

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
        tile_areas(tiles, body, keys_h)
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
/// as tall as the others. The keys tile is under them, as tall as
/// `keys_h` says for the width and the room.
fn tile_areas(
    tiles: &[Panel],
    area: Rect,
    keys_h: impl Fn(u16, u16) -> u16,
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
    let mut rows: Vec<Vec<Panel>> = Vec::new();
    for &p in tiles {
        match rows.last_mut() {
            Some(row) if two && row.len() == 1 && p != Panel::Ask && row[0] != Panel::Ask => {
                row.push(p);
            }
            _ => rows.push(vec![p]),
        }
    }
    let heights = rows
        .iter()
        .map(|r| Constraint::Fill(if r[0] == Panel::Ask { 2 } else { 1 }));
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
    if !app.can_ask() || !disputed(app, u) {
        return Vec::new();
    }
    let by = answers::answer_label(app, u, DiveKind::Disagree);
    let mut lines = vec![Line::default(), heading_by(app, "Second model", by, w)];
    lines.extend(answers::disagree_lines(app, u, w));
    lines
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
            if answers::pending(u, DiveKind::Recommend) {
                title.push(Span::styled(format!("{} ", app.spinner()), t.muted()));
            }
            let mut keys = format!(" {next} · {} close ", k(Action::ViewBack));
            if answers::may_advise(app, u) {
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
            let (name, _) = answers::light(kind);
            let mut title = vec![
                Span::styled(format!(" {}", tool_key(app, kind)), t.accent()),
                Span::styled(format!(" {name} "), t.bold()),
            ];
            if answers::pending(u, kind) {
                title.push(Span::styled(format!("{} ", app.spinner()), t.muted()));
            }
            let mut keys = format!(" {next} · {} close ", k(Action::ViewBack));
            if kind == DiveKind::Visualize {
                keys = format!(" {} browser ·{keys}", k(Action::GotoBrowser));
            }
            if kind == DiveKind::Tests && answers::may_read_tests(app, u) {
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
    // Who gave the advice.
    if p == Panel::Recommend && u.dive.results.contains_key(&DiveKind::Recommend) {
        let by = answers::answer_label(app, u, DiveKind::Recommend);
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
            let mut lines = answers::section_body(app, u, DiveKind::Recommend, w);
            lines.extend(second_model_lines(app, u, w));
            draw_scrolled(f, app, p, lines, inner, focus);
            // A click on the question asks.
            if answers::may_advise(app, u)
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
                && answers::may_read_tests(app, u)
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
    let label_w = KEY_GROUPS.iter().map(|g| g.0.width()).max().unwrap_or(0) + 2;
    // Too narrow for the names in a column of their own: none.
    let label_w = if w >= label_w + 24 { label_w } else { 0 };
    let mut lines = Vec::new();
    let mut at = Vec::new();
    for (name, items) in KEY_GROUPS {
        let mut spans: Vec<Span<'static>> = Vec::new();
        if label_w > 0 {
            spans.push(Span::styled(format!("{name:<label_w$}"), t.muted()));
        }
        let mut x = label_w;
        let mut first = true;
        for (actions, what) in *items {
            if !app.can_ask() && actions.iter().any(|a| AGENT_KEYS.contains(a)) {
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

impl App {
    /// Open the unit view on unit `id` (from the queue: `l`, `enter`; in
    /// the view, a unit gone to to look), with only the advice tile open.
    pub fn open_unit(&mut self, id: String) -> Vec<Effect> {
        if self.review.unit(&id).is_none() {
            return Vec::new();
        }
        let entering = self.screen != Screen::Unit;
        if entering {
            // From the flagged list opened from here: back there, then on
            // back to where this view was opened from.
            if self.screen == Screen::Flagged && self.flagged.from == Some(Screen::Unit) {
                self.flagged.from = self.unit.from;
            }
            self.unit.from = Some(self.screen);
            self.screen = Screen::Unit;
        }
        let home = std::mem::take(&mut self.unit.home);
        self.close_tiles();
        let effects = self.show_unit(id);
        if !entering {
            self.unit.home = home;
        }
        effects
    }

    /// The unit the view shows: the one whose note is being written while
    /// the note line is open (from the notes view it may be another), else
    /// the one on show.
    pub fn unit_on_show(&self) -> Option<&Unit> {
        if let Overlay::Note { id, .. } = &self.overlay
            && let Some(u) = self.review.unit(id)
        {
            return Some(u);
        }
        self.review.unit(&self.unit.id)
    }

    /// May a question use the web: the config allows it, it isn't a pull
    /// request, and the reviewer said yes this session.
    pub fn web_allowed(&self) -> bool {
        self.cfg.dive_web && self.review.pr.is_none() && self.dive_web_ok
    }

    /// The next question that could use the web asks first.
    fn web_may_ask(&self) -> bool {
        self.cfg.dive_web && self.review.pr.is_none() && !self.dive_web_ok && !self.unit.web_asked
    }

    /// Show unit `id`, the queue's cursor on it, the advice tile open (with
    /// an agent). Nothing is asked: the tile says which key asks.
    fn show_unit(&mut self, id: String) -> Vec<Effect> {
        if id != self.unit.id {
            self.close_tiles();
            self.unit.outside.clear();
        }
        if self.can_ask() && !self.unit.tiles.contains(&Panel::Recommend) {
            self.unit.tiles.insert(0, Panel::Recommend);
        }
        self.unit.id = id.clone();
        self.unit.home = id.clone();
        let mut effects = Vec::new();
        let pos = self.position_of(&id);
        if let Some(p) = pos
            && p != self.sel
        {
            effects.extend(self.move_sel(p));
        }
        effects
    }

    /// Close every tile, the focus back on the text; the conversation stays
    /// with the unit.
    fn close_tiles(&mut self) {
        self.unit.tiles.clear();
        self.unit.scroll.borrow_mut().clear();
        self.unit.zoom = false;
        self.focus_tile(None);
        self.ask.chat_scroll.set(None);
    }

    /// Give tile `p` the focus (`None`: the text); typing in the ask tile
    /// stops.
    fn focus_tile(&mut self, p: Option<Panel>) {
        self.unit.panel = p;
        self.unit.typing = false;
        self.ask.link = None;
    }

    /// Open tile `p` after the ones open, its scroll at the top, and give it
    /// the focus; open already, it only gets the focus. A zoom ends, so it
    /// shows.
    fn open_tile(&mut self, p: Panel) {
        if !self.unit.tiles.contains(&p) {
            self.unit.tiles.push(p);
            self.unit.scroll.borrow_mut().remove(&p);
            if p == Panel::Ask {
                self.ask.chat_scroll.set(None);
            }
        }
        self.unit.zoom = false;
        self.focus_tile(Some(p));
    }

    /// Close tile `p`; the focus goes to the tile opened before it, else
    /// to the text. The advice tile, open from the start, never
    /// gets it this way: `h` again goes on to the text, then back.
    fn close_tile(&mut self, p: Panel) {
        let Some(i) = self.unit.tiles.iter().position(|&t| t == p) else {
            return;
        };
        self.unit.tiles.remove(i);
        self.unit.scroll.borrow_mut().remove(&p);
        if self.unit.tiles.is_empty() {
            self.unit.zoom = false;
        }
        if self.unit.panel == Some(p) {
            let before = self.unit.tiles[..i]
                .iter()
                .rev()
                .copied()
                .find(|&t| t != Panel::Recommend);
            self.focus_tile(before);
        }
    }

    /// `tab` (`shift+tab`): the focus on the next (previous) tile, the text
    /// first, `times` over.
    fn tile_step(&mut self, forward: bool, times: usize) {
        let mut ring: Vec<Option<Panel>> = vec![None];
        ring.extend(self.unit.tiles.iter().copied().map(Some));
        let n = ring.len();
        if n == 1 {
            return;
        }
        let i = ring.iter().position(|&p| p == self.unit.panel).unwrap_or(0);
        let step = times.max(1) % n;
        let to = if forward {
            (i + step) % n
        } else {
            (i + n - step) % n
        };
        self.focus_tile(ring[to]);
    }

    /// `K`: the ask tile, typing, at the end of the conversation.
    pub(super) fn open_ask_panel(&mut self) {
        self.open_tile(Panel::Ask);
        self.unit.typing = true;
        self.ask.chat_scroll.set(None);
    }

    /// Is the ask tile in focus, with nothing over it?
    pub(super) fn unit_ask_focused(&self) -> bool {
        self.screen == Screen::Unit
            && self.overlay == Overlay::None
            && self.unit.panel == Some(Panel::Ask)
    }

    /// Whether the ask tile takes the keys: it is in focus, typing or on
    /// its links.
    pub(super) fn unit_panel_has_keys(&self) -> bool {
        self.unit_ask_focused() && (self.unit.typing || self.ask.link.is_some())
    }

    /// Keys of the ask tile. Typing, a text input ([`Keymap::edit`]):
    /// letters go into the question, `enter` asks it, `ctrl+r` asks the
    /// last again, `tab` (`shift+tab`) moves to the first (last) link,
    /// `↑ ↓` and the page keys scroll, `esc` stops typing (the tile keeps
    /// the focus, and the view's keys work again). On the links see
    /// [`App::unit_link_key`].
    ///
    /// [`Keymap::edit`]: crate::keymap::Keymap::edit
    pub(super) fn unit_panel_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        if let Some(i) = self.ask.link {
            return self.unit_link_key(i, &k);
        }
        let page = self.detail_rows.get().saturating_sub(2).max(1) as isize;
        match self.keys.edit(&k) {
            Edit::Cancel => self.unit.typing = false,
            Edit::Submit => return self.unit_send(),
            Edit::Tab => {
                // The newest answer's first link.
                let links = self.ask_links();
                let newest = links.last().map(|(q, _)| *q);
                self.ask.link = links.iter().position(|(q, _)| Some(*q) == newest);
                self.unit.typing = self.ask.link.is_none();
            }
            Edit::BackTab => {
                self.ask.link = self.ask_links().len().checked_sub(1);
                self.unit.typing = self.ask.link.is_none();
            }
            Edit::Down => return self.chat_scroll(SCROLL_STEP),
            Edit::Up => return self.chat_scroll(-SCROLL_STEP),
            Edit::PageDown => return self.chat_scroll(page),
            Edit::PageUp => return self.chat_scroll(-page),
            Edit::Retry => {
                let last = self.chat_of(&self.unit.id).last().copied();
                if let Some(n) = last {
                    return self.ask_question(n);
                }
            }
            e => {
                typed(&mut self.ask.draft, e, QUESTION_MAX);
            }
        }
        Vec::new()
    }

    /// Keys on the ask tile's links (link `i` in focus), a panel
    /// ([`App::panel_action`]): `tab` and `j` move to the next, `shift+tab`
    /// and `k` to the one before (`tab` past the last, or `shift+tab`
    /// before the first, back to the question; `tab` and `shift+tab` are
    /// the question's own keys, [`Keymap::edit`]), `view.open` follows one
    /// (a unit opens here, code in the editor), `i` or `K` types again,
    /// `h`/`esc`/`q` leave the links (the tile keeps the focus), `gt` `gT`
    /// move to another tile and `zz` zooms; the scroll keys scroll.
    ///
    /// [`Keymap::edit`]: crate::keymap::Keymap::edit
    fn unit_link_key(&mut self, i: usize, k: &KeyEvent) -> Vec<Effect> {
        let n = self.ask_links().len();
        let last = n.saturating_sub(1);
        let page = self.detail_rows.get().saturating_sub(2).max(1) as isize;
        let typing = |app: &mut App| {
            app.ask.link = None;
            app.unit.typing = true;
        };
        match self.keys.edit(k) {
            Edit::Tab if i + 1 < n => self.ask.link = Some(i + 1),
            Edit::BackTab if i > 0 => self.ask.link = Some(i - 1),
            Edit::Tab | Edit::BackTab => typing(self),
            _ => match self.panel_action(k) {
                Some(Action::MoveDown) => self.ask.link = Some((i + 1).min(last)),
                Some(Action::MoveUp) => self.ask.link = Some(i.saturating_sub(1)),
                Some(Action::NoteEdit | Action::AskOpen) => typing(self),
                Some(Action::MoveFirst) => self.ask.link = Some(0),
                Some(Action::MoveLast) => self.ask.link = Some(last),
                Some(Action::ViewOpen) => return self.follow_ask_link(i),
                Some(Action::ViewBack | Action::ViewClose) => self.ask.link = None,
                Some(a @ (Action::MoveNextTile | Action::MovePrevTile)) => {
                    self.tile_step(a == Action::MoveNextTile, 1);
                }
                Some(Action::ToggleZoom) => self.toggle_zoom(),
                Some(Action::ScrollPageDown | Action::ScrollHalfDown) => {
                    return self.chat_scroll(page);
                }
                Some(Action::ScrollPageUp | Action::ScrollHalfUp) => {
                    return self.chat_scroll(-page);
                }
                _ => {}
            },
        }
        Vec::new()
    }

    /// Ask what was typed, about the unit on show. The first question that
    /// could use the web asks first whether it may (once a session, Q7);
    /// the question waits for the answer.
    fn unit_send(&mut self) -> Vec<Effect> {
        let text = std::mem::take(&mut self.ask.draft);
        self.ask.chat_scroll.set(None);
        let id = self.unit.id.clone();
        if self.has_agent && self.web_may_ask() {
            let Some(index) = self.add_question(&text, Some(id)) else {
                return Vec::new();
            };
            self.unit.web_asked = true;
            self.unit.web_question = Some(index);
            self.overlay = Overlay::ConfirmWeb;
            return vec![Effect::Save];
        }
        match self.add_question(&text, Some(id)) {
            Some(index) => self.ask_question(index),
            None => Vec::new(),
        }
    }

    /// The answer to the web question, when a question of the ask panel
    /// waits for it: it is asked now, with the web or without.
    pub(super) fn unit_answer_web(&mut self, yes: bool) -> Option<Vec<Effect>> {
        let index = self.unit.web_question.take()?;
        self.dive_web_ok = yes;
        if !yes {
            self.status = "No web search this session: the agent answers from the code.".into();
        }
        Some(self.ask_question(index))
    }

    /// The actions of the unit view (after those every screen shares).
    pub(super) fn unit_action(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        let times = count.unwrap_or(1) as usize;
        match a {
            Action::DecideAccept
            | Action::DecideFlag
            | Action::DecideAcceptFile
            | Action::DecideFlagFile => return self.unit_decide(a, count),
            Action::MoveNextPending | Action::MovePrevPending => {
                let id = self.unit.id.clone();
                if let Some(p) = self.position_of(&id) {
                    self.sel = p;
                }
                let mut effects = self.jump_pending(a == Action::MoveNextPending, times);
                effects.extend(self.follow_sel());
                return effects;
            }
            Action::MoveNextFile | Action::MovePrevFile => {
                let id = self.unit.id.clone();
                if let Some(p) = self.position_of(&id) {
                    self.sel = p;
                }
                let mut effects = self.jump_file(a == Action::MoveNextFile, times);
                effects.extend(self.follow_sel());
                return effects;
            }
            Action::MoveNextHunk => self.jump_hunk(true, times),
            Action::MovePrevHunk => self.jump_hunk(false, times),
            Action::MoveDown => return self.unit_scroll(times as isize),
            Action::MoveUp => return self.unit_scroll(-(times as isize)),
            Action::MoveFirst => return self.unit_scroll(isize::MIN / 2),
            Action::MoveLast => return self.unit_scroll(isize::MAX / 2),
            Action::MoveNextTile | Action::MovePrevTile => {
                self.tile_step(a == Action::MoveNextTile, times);
            }
            Action::ToggleZoom => self.toggle_zoom(),
            // On the advice tile, the agent is asked.
            Action::ViewOpen if self.unit.panel == Some(Panel::Recommend) => {
                return self.ask_advice();
            }
            // On the Tests tile, the agent reads what teer found.
            Action::ViewOpen if self.unit.panel == Some(Panel::Tool(DiveKind::Tests)) => {
                return self.read_tests();
            }
            // A zoom ends first, then the tile in focus closes; on the text,
            // back to the queue.
            Action::ViewBack | Action::ViewClose if self.unit.zoom => self.unit.zoom = false,
            Action::ViewBack | Action::ViewClose => match self.unit.panel {
                Some(p) => self.close_tile(p),
                None => self.unit_back(),
            },
            _ => {}
        }
        Vec::new()
    }

    /// Back to the screen the view was opened from (the queue), its
    /// cursor on the row the view was opened on, or moved on to (see
    /// [`UnitView::home`]).
    fn unit_back(&mut self) {
        self.close_tiles();
        if let Some(p) = self.position_of(&self.unit.home.clone()) {
            self.sel = p;
        }
        self.screen = self.unit.from.take().unwrap_or(Screen::Queue);
    }

    /// The queue's cursor moved (`n`, `N`, `]f`, `[f`): show its unit.
    fn follow_sel(&mut self) -> Vec<Effect> {
        match self.selected_id() {
            Some(id) if id != self.unit.id => self.show_unit(id),
            _ => Vec::new(),
        }
    }

    /// Decide as the queue does (`a`, `f`, `A`, `F`, with a count), on the
    /// unit on show; then show the next pending unit. A unit the queue
    /// doesn't list is decided alone.
    fn unit_decide(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        let id = self.unit.id.clone();
        let mut effects = match self.position_of(&id) {
            Some(p) => {
                self.sel = p;
                self.queue_action(a, count)
            }
            None => {
                let flag = matches!(a, Action::DecideFlag | Action::DecideFlagFile);
                let d = if flag {
                    Decision::Investigate
                } else {
                    Decision::Accepted
                };
                self.decide_ids(std::slice::from_ref(&id), d);
                vec![Effect::Save]
            }
        };
        effects.extend(self.unit_follow(&id));
        effects
    }

    /// After deciding unit `was`: the unit the queue moved on to; when it
    /// stayed on a decided unit, the next pending one after it, else
    /// before it. With nothing pending the view stays.
    fn unit_follow(&mut self, was: &str) -> Vec<Effect> {
        if let Some(id) = self.selected_id()
            && id != was
            && self.position_of(was).is_some()
        {
            return self.show_unit(id);
        }
        let decided = self
            .review
            .unit(was)
            .is_some_and(|u| u.decision != Decision::Pending);
        if !decided {
            return Vec::new();
        }
        let from = self.position_of(was).unwrap_or(self.sel);
        let next = self
            .next_pending_pos(from)
            .or_else(|| self.prev_pending_pos(from));
        match next {
            Some(p) => {
                let id = self.review.units[self.order[p]].id.clone();
                self.show_unit(id)
            }
            None => Vec::new(),
        }
    }

    /// `u` or `ctrl+r` changed unit `first` first: show it.
    pub(super) fn unit_show_step(&mut self, first: &str) -> Vec<Effect> {
        if self.review.unit(first).is_none() {
            return Vec::new();
        }
        self.show_unit(first.to_string())
    }

    /// Scroll with the keys: the tile in focus (the ask tile's
    /// conversation), else the text.
    pub(super) fn unit_scroll(&mut self, by: isize) -> Vec<Effect> {
        self.unit_wheel(self.unit.panel, by)
    }

    /// Scroll tile `over` (the text with none) by `by`: the wheel scrolls
    /// what it is over, in focus or not.
    pub(super) fn unit_wheel(&mut self, over: Option<Panel>, by: isize) -> Vec<Effect> {
        match over.filter(|p| self.unit.tiles.contains(p)) {
            Some(Panel::Ask) => return self.chat_scroll(by),
            Some(p) => self.scroll_tile(p, by),
            None => self.scroll_text(by),
        }
        Vec::new()
    }

    /// A click on tile `p` (`None`: the text) gives it the focus; on the
    /// ask tile, typing.
    pub(super) fn unit_click_tile(&mut self, p: Option<Panel>) {
        if p.is_some_and(|p| !self.unit.tiles.contains(&p)) {
            return;
        }
        self.focus_tile(p);
        if p == Some(Panel::Ask) {
            self.unit.typing = true;
        }
    }

    /// `zz`: the tile in focus (or the text) fills the view, or no longer
    /// does. The keys tile is always beside the text, so there is always
    /// something to hide.
    fn toggle_zoom(&mut self) {
        self.unit.zoom = !self.unit.zoom;
    }

    fn scroll_text(&mut self, by: isize) {
        let to = self
            .detail_offset()
            .saturating_add_signed(by)
            .min(self.unit.text_max.get());
        self.detail_scroll.borrow_mut().1 = to;
    }

    fn scroll_tile(&mut self, p: Panel, by: isize) {
        let mut scroll = self.unit.scroll.borrow_mut();
        let s = scroll.entry(p).or_default();
        s.offset = s.offset.saturating_add_signed(by).min(s.max);
    }

    /// `]c` (`[c`): scroll the text to the `times`-th hunk below (above)
    /// the top, as far as there are.
    fn jump_hunk(&mut self, forward: bool, times: usize) {
        let hunks = self.unit.hunks.borrow().clone();
        if hunks.is_empty() {
            self.status = "No code change to jump in.".into();
            return;
        }
        let at = self.detail_offset();
        let mut to = at;
        for _ in 0..times.max(1) {
            let next = if forward {
                hunks.iter().copied().find(|&h| h > to)
            } else {
                hunks.iter().rev().copied().find(|&h| h < to)
            };
            match next {
                Some(h) => to = h,
                None => break,
            }
        }
        let to = to.min(self.unit.text_max.get());
        if self.unit.text_max.get() == 0 {
            self.status = "The whole change is on screen.".into();
            return;
        }
        if to == at {
            self.status = if forward {
                "No more hunks below."
            } else {
                "No more hunks above."
            }
            .into();
            return;
        }
        self.detail_scroll.borrow_mut().1 = to;
    }

    /// A leader tool (`<space>a` `<space>t` `<space>r` `<space>v`): its tile opens
    /// beside the ones open and its answer is asked for unless it has one.
    /// Open already, the same key gives it the focus, and in focus closes
    /// it. `<space>t` runs only teer's test search; the tile then asks
    /// whether the agent reads what it found ([`App::read_tests`]).
    pub(super) fn unit_tool(&mut self, a: Action) -> Vec<Effect> {
        let id = self.unit.id.clone();
        if self.review.unit(&id).is_none() {
            return Vec::new();
        }
        if a == Action::ToolAdvice {
            return self.toggle_advice();
        }
        let Some(kind) = tool_kind(a) else {
            return Vec::new();
        };
        let tile = Panel::Tool(kind);
        if self.unit.panel == Some(tile) {
            self.close_tile(tile);
            return Vec::new();
        }
        if self.unit.tiles.contains(&tile) {
            self.focus_tile(Some(tile));
            return Vec::new();
        }
        if kind != DiveKind::Tests && !self.can_ask() {
            let (name, _) = answers::light(kind);
            self.status = format!("No agent: {name} needs one (`teer setup` picks it).");
            return Vec::new();
        }
        self.open_tile(tile);
        self.dive_on(&id, kind)
    }

    /// `<space>a`: the advice tile, asked when it hasn't been. In focus
    /// with an answer (or one on its way), the same key closes it.
    fn toggle_advice(&mut self) -> Vec<Effect> {
        let tile = Panel::Recommend;
        if self.unit.panel == Some(tile)
            && self
                .review
                .unit(&self.unit.id)
                .is_some_and(|u| !answers::may_advise(self, u))
        {
            self.close_tile(tile);
            return Vec::new();
        }
        self.ask_advice()
    }

    /// Ask the agent for advice on the unit on show: what to do about it.
    /// The advice tile opens first, in focus; answered already or on its
    /// way, nothing more is asked.
    pub(super) fn ask_advice(&mut self) -> Vec<Effect> {
        if !self.can_ask() {
            self.status = "No agent: advice needs one (`teer setup` picks it).".into();
            return Vec::new();
        }
        let tile = Panel::Recommend;
        if !self.unit.tiles.contains(&tile) {
            self.unit.tiles.insert(0, tile);
            self.unit.scroll.borrow_mut().remove(&tile);
        }
        self.unit.zoom = false;
        self.focus_tile(Some(tile));
        let id = self.unit.id.clone();
        self.dive_on(&id, DiveKind::Recommend)
    }

    /// The Tests tile's question answered yes (`enter` on it, or a click):
    /// the agent reads what teer's search found for the unit on show.
    pub(super) fn read_tests(&mut self) -> Vec<Effect> {
        let tile = Panel::Tool(DiveKind::Tests);
        if !self.unit.tiles.contains(&tile) {
            return Vec::new();
        }
        self.focus_tile(Some(tile));
        let id = self.unit.id.clone();
        let Some(u) = self.review.unit(&id) else {
            return Vec::new();
        };
        if !answers::may_read_tests(self, u) {
            if !self.can_ask() {
                self.status =
                    "No agent: reading the tests needs one (`teer setup` picks it).".into();
            }
            return Vec::new();
        }
        self.interpret_tests_on(&id)
    }

    /// `gd` in the unit view: the jump list of what the unit calls in the
    /// review (Q5), and what it calls outside the review, read-only in a
    /// tile.
    pub(super) fn unit_goto_definition(&mut self) -> Vec<Effect> {
        let id = self.unit.id.clone();
        let outside = self.outside_definitions(&id);
        let found = !outside.is_empty();
        if found {
            self.open_tile(Panel::Outside);
            self.unit.outside = outside;
        }
        let effects = self.open_jump(JumpKind::Definition { of: id.clone() });
        if found && !matches!(self.overlay, Overlay::Jump { .. }) {
            let symbol = self
                .review
                .unit(&id)
                .map(slim::row_name)
                .unwrap_or_default();
            self.status = format!(
                "{symbol} calls nothing else in this review; what it calls outside is shown below."
            );
        }
        effects
    }

    /// What unit `id` calls outside the review, from the call graph: each
    /// definition's lines read from the repository (at most
    /// [`OUTSIDE_LINES`] of each, [`OUTSIDE_MOST`] of them).
    fn outside_definitions(&self, id: &str) -> Vec<Shown> {
        let g = &self.review.graph;
        let Some(from) = g.nodes.iter().position(|n| n.id == id) else {
            return Vec::new();
        };
        let root = &self.review.repo_root;
        let mut seen = HashSet::new();
        g.edges
            .iter()
            .filter(|(caller, _)| *caller == from)
            .filter_map(|&(_, to)| g.nodes.get(to))
            .filter(|n| !n.in_review && seen.insert(n.id.clone()))
            .filter_map(|n| {
                let text = crate::git::read_text_file(root, &n.file, OUTSIDE_BYTES)?;
                let start = n.range.start.max(1);
                let code: Vec<String> = text
                    .lines()
                    .skip(start as usize - 1)
                    .take((n.range.len() as usize).clamp(1, OUTSIDE_LINES))
                    .map(String::from)
                    .collect();
                (!code.is_empty()).then(|| Shown {
                    cite: Cite {
                        file: n.file.clone(),
                        start,
                        end: start + code.len() as u32 - 1,
                    },
                    why: crate::util::sanitize(&n.symbol),
                    code,
                    in_change: false,
                    changed: Vec::new(),
                })
            })
            .take(OUTSIDE_MOST)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::git::{ChangedFile, FileStatus};
    use crate::keymap::{Keymap, parse_keys};
    use crate::review::model::{Range, Review, SessionRef, UnitKind, UnitStatus};
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
        a.open_unit(a.selected_id().unwrap());
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
        let wide = Rect::new(0, 0, 120, 30);
        let (text, cells, _) = tile_areas(&[TESTS], wide, none);
        assert_eq!((text.width, cells[0].1.x), (60, 60));
        assert_eq!(cells[0].1.height, 30);
        // Two tiles stack in one column beside the text.
        let (text, cells, _) = tile_areas(&[TESTS, RUN], wide, none);
        assert_eq!(text.width, 60);
        assert_eq!(cells[0].1.x, cells[1].1.x);
        assert!(cells[0].1.y < cells[1].1.y);
        // Room for two to a row: they share it; the ask tile has its own,
        // twice as tall.
        let room = Rect::new(0, 0, 200, 30);
        let (text, cells, _) = tile_areas(&[TESTS, RUN, Panel::Ask], room, none);
        assert_eq!(text.width, 80);
        assert_eq!(cells[0].1.y, cells[1].1.y);
        assert!(cells[0].1.x < cells[1].1.x);
        assert_eq!(cells[2].1.width, 120);
        assert_eq!(cells[2].1.height, 2 * cells[0].1.height);
        // Narrow: the text on top.
        let (text, cells, _) = tile_areas(&[TESTS], Rect::new(0, 0, 80, 30), none);
        assert_eq!((text.width, text.height), (80, 15));
        assert_eq!((cells[0].1.y, cells[0].1.width), (15, 80));
        // The keys tile under the tiles, as tall as it says.
        let (text, cells, keys) = tile_areas(&[REC, TESTS], wide, |w, _| w / 10);
        assert_eq!((keys.x, keys.width, keys.height), (60, 60, 6));
        assert_eq!(keys.bottom(), 30);
        assert_eq!(cells[1].1.bottom(), keys.y);
        assert_eq!(text.height, 30);
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
        // Answered: who wrote it on its bottom edge.
        let u = a.review.unit_mut(&a.unit.id.clone()).unwrap();
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
