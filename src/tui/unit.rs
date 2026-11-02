//! The unit view (v2 Phase 5): any unit, with why it has its level, what
//! changed, what to do about it and the second model, answered before the
//! reviewer asks.
//!
//! One text scrolls under a title row: "Why this level", "What changed"
//! (the summary, then the diff with each statement under the lines it
//! cites; `]c` `[c` jump between hunks), "What to do about it" (the
//! recommendation, asked for when the unit opens and prefetched for the
//! units that come next) and "Second model" (only when it disputes the
//! description). Without an agent the last two are left out.
//!
//! Under it one panel opens at a time: `K` asks about the unit (the
//! conversation is kept with it), `<space>t` `<space>r` `<space>v` show the
//! tests, old against new and the diagram, and `gd` shows what the unit
//! calls outside the review, read-only. Deciding here moves on to the next
//! pending unit and stays in the view; `h` goes back to the queue.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::{
    App, Effect, JumpKind, Overlay, QUESTION_MAX, SCROLL_STEP, Screen, Target, typed,
};
use super::{ask, diff, flagged, preview, slim};
use crate::keymap::{Action, Edit};
use crate::review::dive::DiveEvent;
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
    Action::ToolTests,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
    Action::ToolHistory,
    Action::ToolNotes,
    Action::ToolSettings,
    Action::HelpOpen,
    Action::FinishOpen,
    Action::ReviewRerun,
    Action::ReviewReset,
    Action::ViewClose,
];

/// Pending units the recommendation is asked for when the queue opens:
/// the first of those that need a close look or are worth one.
pub const PREFETCH: usize = 3;

/// Most lines of one definition `gd` shows from outside the review.
const OUTSIDE_LINES: usize = 60;
/// Most definitions `gd` shows from outside the review.
const OUTSIDE_MOST: usize = 8;
/// Largest file `gd` reads a definition from.
const OUTSIDE_BYTES: usize = 512 * 1024;

/// What is open under the unit's text, one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// `K`: the conversation about the unit and the question being typed.
    Ask,
    /// A leader tool's answer: the tests (`Tests`), old against new
    /// (`Behavior`) or the diagram (`Visualize`).
    Tool(DiveKind),
    /// `gd`: what the unit calls outside the review, read-only.
    Outside,
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
    pub panel: Option<Panel>,
    /// Keys go into the ask panel's question; otherwise, while it is
    /// open, to its links ([`App::ask_links`], the one in focus in
    /// `App::ask.link`).
    pub typing: bool,
    /// The definitions `gd` found outside the review.
    pub outside: Vec<Shown>,
    /// Where each hunk of the diff starts, in lines of the text, as last
    /// drawn.
    pub hunks: RefCell<Vec<usize>>,
    /// How far the text can scroll and its rows, as last drawn.
    pub text_max: Cell<usize>,
    pub text_rows: Cell<usize>,
    /// How far a tool panel is scrolled, how far it can, and its rows.
    pub panel_offset: Cell<usize>,
    pub panel_max: Cell<usize>,
    pub panel_rows: Cell<usize>,
    /// Asked this session whether questions may use the web (Q7).
    pub web_asked: bool,
    /// The question waiting for that answer, by index.
    pub web_question: Option<usize>,
    /// Units whose tests the agent reads once teer's search is done (Q6).
    pub read_tests: HashSet<String>,
}

/// The leader tools, in the order the ask panel suggests them.
const TOOLS: [(DiveKind, Action); 3] = [
    (DiveKind::Tests, Action::ToolTests),
    (DiveKind::Behavior, Action::ToolRunOldNew),
    (DiveKind::Visualize, Action::ToolDiagram),
];

/// The tool `a` opens.
fn tool_kind(a: Action) -> Option<DiveKind> {
    TOOLS.iter().find(|t| t.1 == a).map(|t| t.0)
}

/// The key that opens tool `kind`.
fn tool_key(app: &App, kind: DiveKind) -> String {
    TOOLS
        .iter()
        .find(|t| t.0 == kind)
        .map_or(String::new(), |t| app.keys.label(t.1))
}

// ---- Drawing -----------------------------------------------------------

/// Draw the unit view into `area`: the title row, the text, and the panel
/// open under it.
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
    let panel = app.unit.panel.filter(|_| body.height >= 8);
    let (text, under) = match panel {
        Some(p) => {
            let share = if p == Panel::Ask { 55 } else { 50 };
            let h = (u32::from(body.height) * share / 100) as u16;
            let h = h.max(6).min(body.height.saturating_sub(3));
            let [text, under] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(h)]).areas(body);
            (text, Some((p, under)))
        }
        None => (body, None),
    };
    draw_text(f, app, u, text, under.is_none());
    if let Some((p, r)) = under {
        draw_panel(f, app, u, p, r);
    }
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

/// The heading of what the agent wrote: the heading, then who wrote it.
fn heading_by(app: &App, text: &str, by: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {text}"), app.theme.bold()),
        Span::styled(format!("  {by}"), app.theme.muted()),
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

    // What changed: the summary, then the diff with the statements in it.
    let code = diff::code_change_of(app, u, w.saturating_sub(1), diff::Notes::Full);
    let mut head = vec![Span::styled(" What changed", t.bold())];
    if !code.blocks.is_empty() {
        head.extend(diff::counts(app, code.added, code.removed));
    }
    add(&mut blocks, vec![Line::default(), Line::from(head)]);
    let mut said = preview::agent_lines(app, u, w.saturating_sub(2), code.blocks.is_empty(), 1);
    if u.prep != Prep::Ready && u.summary.is_none() && u.nbc_reason.is_none() {
        said.extend(slim::statement_lines(app, u, w, 1));
    }
    add(&mut blocks, said);
    let mut hunks = Vec::new();
    if !code.blocks.is_empty() {
        add(&mut blocks, vec![Line::default()]);
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
                .map(|b| b.into_iter().map(indented).collect()),
        );
    }
    let card = super::notes::card(app, u, w.saturating_sub(1));
    if !card.is_empty() {
        add(&mut blocks, vec![Line::default()]);
        add(&mut blocks, card.into_iter().map(indented).collect());
    }

    // What the agent says to do, and the second model when it disputes.
    if app.can_ask() {
        let rec = DiveKind::Recommend;
        let head = if u.dive.results.contains_key(&rec) {
            heading_by(
                app,
                "What to do about it",
                flagged::answer_label(app, u, rec),
            )
        } else {
            heading(app, "What to do about it")
        };
        add(&mut blocks, vec![Line::default(), head]);
        add(&mut blocks, flagged::section_body(app, u, rec, w));
        if disputed(app, u) {
            let by = flagged::answer_label(app, u, DiveKind::Disagree);
            add(
                &mut blocks,
                vec![Line::default(), heading_by(app, "Second model", by)],
            );
            add(&mut blocks, flagged::disagree_lines(app, u, w));
        }
    }
    (blocks, hunks)
}

/// The unit's text in `area`, scrolled where it was left. With `focus`
/// (no panel open) the scroll keys and the footer go by it.
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

/// The panel `p` in `area`: a rounded border in the accent, its name on
/// top, its keys at the bottom.
fn draw_panel(f: &mut Frame, app: &App, u: &Unit, p: Panel, area: Rect) {
    let t = &app.theme;
    if area.width < 12 || area.height < 3 {
        return;
    }
    app.targets.borrow_mut().push((area, Target::Panel));
    let k = |a: Action| app.keys.label(a);
    let (title, keys) = match p {
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
                    " {} {} links · {} open · {} type · {} close ",
                    k(Action::MoveDown),
                    k(Action::MoveUp),
                    k(Action::ViewOpen),
                    k(Action::NoteEdit),
                    k(Action::ViewBack)
                )
            } else {
                format!(
                    " {} ask · {} links · {} again · {} close ",
                    super::ui::fixed::ENTER,
                    super::ui::fixed::TAB,
                    super::ui::fixed::RETRY,
                    super::ui::fixed::ESC
                )
            };
            (Line::from(title), keys)
        }
        Panel::Tool(kind) => {
            let (name, _) = flagged::light(kind);
            let mut title = vec![
                Span::styled(format!(" {}", tool_key(app, kind)), t.accent()),
                Span::styled(format!(" {name} "), t.bold()),
            ];
            if flagged::pending(u, kind) {
                title.push(Span::styled(format!("{} ", app.spinner()), t.muted()));
            }
            let mut keys = format!(" {} close ", k(Action::ViewBack));
            if kind == DiveKind::Visualize {
                keys = format!(" {} browser ·{keys}", k(Action::GotoBrowser));
            }
            (Line::from(title), keys)
        }
        Panel::Outside => (
            Line::from(vec![
                Span::styled(format!(" {}", k(Action::GotoDefinition)), t.accent()),
                Span::styled(" What it calls outside the review ", t.bold()),
            ]),
            format!(" read-only · {} close ", k(Action::ViewBack)),
        ),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.accent())
        .title(title)
        .title_bottom(Line::styled(keys, t.muted()).right_aligned());
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
        Panel::Ask => draw_ask(f, app, u, inner),
        Panel::Tool(DiveKind::Visualize)
            if flagged::draw_picture(f, app, u, inner, inner.width as usize) =>
        {
            app.detail_max_scroll.set(0);
            app.detail_rows.set(inner.height as usize);
        }
        Panel::Tool(kind) => {
            let w = inner.width as usize;
            let lines = flagged::section_body(app, u, kind, w);
            draw_scrolled(f, app, lines, inner);
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
            draw_scrolled(f, app, lines, inner);
        }
    }
}

/// `lines` in `area`, from the panel's scroll on; the scroll keys go by it.
fn draw_scrolled(f: &mut Frame, app: &App, lines: Vec<Line<'static>>, area: Rect) {
    let w = area.width as usize;
    let h = area.height as usize;
    let blocks = lines.into_iter().map(|l| vec![l]).collect();
    let s = preview::scroll_rows(app, blocks, w, h, 0, false, app.unit.panel_offset.get());
    app.unit.panel_offset.set(s.offset);
    app.unit.panel_max.set(s.max);
    app.unit.panel_rows.set(h);
    app.detail_max_scroll.set(s.max);
    app.detail_rows.set(h);
    f.render_widget(Paragraph::new(s.lines), area);
}

/// The ask panel: the conversation about `u` (or, before the first
/// question, what to ask and the tools that bring more light), a rule, and
/// the question being typed.
fn draw_ask(f: &mut Frame, app: &App, u: &Unit, area: Rect) {
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
        for (kind, action) in TOOLS {
            let shown = format!("{}  {}", tool_key(app, kind), flagged::light(kind).0);
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
                chat: true,
                scroll: &app.ask.chat_scroll,
                max: &app.ask.chat_max,
                focus: app.ask.link,
            },
        );
    }
    app.detail_rows.set(talk.height as usize);
    app.detail_max_scroll.set(app.ask.chat_max.get());
    f.render_widget(Paragraph::new(Line::styled("─".repeat(w), t.muted())), rule);
    let typing = app.ask.link.is_none();
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
    lines.push(Line::styled(format!(" {}", flagged::MENU), t.bold()));
    let key_w = TOOLS
        .iter()
        .map(|(k, _)| tool_key(app, *k).width())
        .max()
        .unwrap_or(1);
    let name_w = TOOLS
        .iter()
        .map(|(k, _)| flagged::light(*k).0.width())
        .max()
        .unwrap_or(0);
    for (kind, _) in TOOLS {
        // Without an agent only teer's own test search works.
        if !app.has_agent && kind != DiveKind::Tests {
            continue;
        }
        let (name, what) = flagged::light(kind);
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
    /// the view, a unit gone to to look), with no panel open. Its
    /// recommendation is asked for unless it has one, and so is the next
    /// pending unit's.
    pub fn open_unit(&mut self, id: String) -> Vec<Effect> {
        if self.review.unit(&id).is_none() {
            return Vec::new();
        }
        let entering = self.screen != Screen::Unit;
        if entering {
            self.unit.from = Some(self.screen);
            self.screen = Screen::Unit;
        }
        let home = std::mem::take(&mut self.unit.home);
        self.close_panel();
        let effects = self.show_unit(id);
        if !entering {
            self.unit.home = home;
        }
        effects
    }

    /// The unit the view shows: the one whose note is being written while
    /// the note line is open (`f` moved on already), else the one on show.
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

    /// When the queue opens: ask for the recommendation of the first
    /// [`PREFETCH`] pending units that need a close look or are worth one,
    /// in queue order. Nothing without an agent, or on the swipe cards.
    pub(super) fn prefetch_start(&mut self) -> Vec<Effect> {
        use crate::review::attention::Level;
        if !self.can_ask() || self.screen == Screen::Swipe {
            return Vec::new();
        }
        let ids: Vec<String> = self
            .order
            .iter()
            .filter(|&&i| {
                let u = &self.review.units[i];
                u.decision == Decision::Pending && self.shown_level(i) >= Level::Look
            })
            .take(PREFETCH)
            .map(|&i| self.review.units[i].id.clone())
            .collect();
        ids.iter().flat_map(|id| self.recommend(id)).collect()
    }

    /// Ask for unit `id`'s recommendation unless it has one (kept with the
    /// unit while its code stays the same) or it is on its way.
    fn recommend(&mut self, id: &str) -> Vec<Effect> {
        if !self.can_ask() {
            return Vec::new();
        }
        self.dive_on(id, DiveKind::Recommend, false)
    }

    /// Show unit `id`, the queue's cursor on it; ask for what it and the
    /// next pending unit need (see [`App::open_unit`]).
    fn show_unit(&mut self, id: String) -> Vec<Effect> {
        if id != self.unit.id {
            self.close_panel();
            self.unit.outside.clear();
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
        effects.extend(self.recommend(&id));
        let next = pos
            .and_then(|p| self.next_pending_pos(p))
            .map(|p| self.review.units[self.order[p]].id.clone());
        if let Some(next) = next {
            effects.extend(self.recommend(&next));
        }
        effects
    }

    /// Close the panel; the conversation stays with the unit.
    fn close_panel(&mut self) {
        self.unit.panel = None;
        self.unit.typing = false;
        self.ask.link = None;
        self.ask.chat_scroll.set(None);
        self.unit.panel_offset.set(0);
    }

    /// Open `p` under the text, the scroll at its top.
    fn open_panel(&mut self, p: Panel) {
        self.close_panel();
        self.unit.panel = Some(p);
    }

    /// `K`: the ask panel, typing, at the end of the conversation.
    pub(super) fn open_ask_panel(&mut self) {
        if self.unit.panel != Some(Panel::Ask) {
            self.open_panel(Panel::Ask);
        }
        self.unit.typing = true;
        self.ask.link = None;
        self.ask.chat_scroll.set(None);
    }

    /// Whether the ask panel takes the keys: it is open and nothing is
    /// over it.
    pub(super) fn unit_panel_has_keys(&self) -> bool {
        self.screen == Screen::Unit
            && self.overlay == Overlay::None
            && self.unit.panel == Some(Panel::Ask)
    }

    /// Keys of the ask panel. Typing, a text input ([`Keymap::edit`]):
    /// letters go into the question, `enter` asks it, `ctrl+r` asks the
    /// last again, `tab` (`shift+tab`) moves to the first (last) link,
    /// `↑ ↓` and the page keys scroll, `esc` closes the panel. On the links
    /// see [`App::unit_link_key`].
    ///
    /// [`Keymap::edit`]: crate::keymap::Keymap::edit
    pub(super) fn unit_panel_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        if let Some(i) = self.ask.link {
            return self.unit_link_key(i, &k);
        }
        let page = self.detail_rows.get().saturating_sub(2).max(1) as isize;
        match self.keys.edit(&k) {
            Edit::Cancel => self.close_panel(),
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

    /// Keys on the ask panel's links (link `i` in focus), a panel
    /// ([`App::panel_action`]): `tab` and `j` move to the next, `shift+tab`
    /// and `k` to the one before (`tab` past the last, or `shift+tab`
    /// before the first, back to the question), `view.open` follows one
    /// (a unit opens here, code in the editor), `i` or `K` types again,
    /// `h`/`esc`/`q` close the panel; the scroll keys scroll.
    fn unit_link_key(&mut self, i: usize, k: &KeyEvent) -> Vec<Effect> {
        let Some(a) = self.panel_action(k) else {
            return Vec::new();
        };
        let n = self.ask_links().len();
        let last = n.saturating_sub(1);
        let page = self.detail_rows.get().saturating_sub(2).max(1) as isize;
        let typing = |app: &mut App| {
            app.ask.link = None;
            app.unit.typing = true;
        };
        match a {
            Action::MoveDown => self.ask.link = Some((i + 1).min(last)),
            Action::MoveUp => self.ask.link = Some(i.saturating_sub(1)),
            Action::V1NextTile if i + 1 < n => self.ask.link = Some(i + 1),
            Action::V1PrevTile if i > 0 => self.ask.link = Some(i - 1),
            Action::V1NextTile | Action::V1PrevTile | Action::NoteEdit | Action::AskOpen => {
                typing(self)
            }
            Action::MoveFirst => self.ask.link = Some(0),
            Action::MoveLast => self.ask.link = Some(last),
            Action::ViewOpen => return self.follow_ask_link(i),
            Action::ViewBack | Action::ViewClose => self.close_panel(),
            Action::ScrollPageDown | Action::ScrollHalfDown => return self.chat_scroll(page),
            Action::ScrollPageUp | Action::ScrollHalfUp => return self.chat_scroll(-page),
            _ => {}
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
            Action::ViewBack | Action::ViewClose if self.unit.panel.is_some() => self.close_panel(),
            Action::ViewBack | Action::ViewClose => self.unit_back(),
            _ => {}
        }
        Vec::new()
    }

    /// Back to the screen the view was opened from (the queue), its
    /// cursor on the row the view was opened on, or moved on to (see
    /// [`UnitView::home`]).
    fn unit_back(&mut self) {
        self.close_panel();
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
                if flag {
                    self.edit_note(id.clone(), None);
                }
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

    /// Scroll with the keys: the panel while one is open (the ask panel's
    /// conversation), else the text.
    pub(super) fn unit_scroll(&mut self, by: isize) -> Vec<Effect> {
        match self.unit.panel {
            Some(Panel::Ask) => return self.chat_scroll(by),
            Some(_) => self.scroll_panel(by),
            None => self.scroll_text(by),
        }
        Vec::new()
    }

    /// Scroll with the wheel: the panel when over it, else the text.
    pub(super) fn unit_wheel(&mut self, panel: bool, by: isize) -> Vec<Effect> {
        match self.unit.panel {
            Some(Panel::Ask) if panel => return self.chat_scroll(by),
            Some(_) if panel => self.scroll_panel(by),
            _ => self.scroll_text(by),
        }
        Vec::new()
    }

    /// A click on the panel: the ask panel types.
    pub(super) fn unit_click_panel(&mut self) {
        if self.unit.panel == Some(Panel::Ask) {
            self.unit.typing = true;
            self.ask.link = None;
        }
    }

    fn scroll_text(&mut self, by: isize) {
        let to = self
            .detail_offset()
            .saturating_add_signed(by)
            .min(self.unit.text_max.get());
        self.detail_scroll.borrow_mut().1 = to;
    }

    fn scroll_panel(&mut self, by: isize) {
        let to = self
            .unit
            .panel_offset
            .get()
            .saturating_add_signed(by)
            .min(self.unit.panel_max.get());
        self.unit.panel_offset.set(to);
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

    /// A leader tool (`<space>t` `<space>r` `<space>v`), or asking the
    /// recommendation or the second model again: its panel opens (again
    /// the same key closes it) and its answer is asked for unless it has
    /// one. After teer's test search, the agent reads the tests on its own
    /// when there is one (Q6).
    pub(super) fn unit_tool(&mut self, a: Action) -> Vec<Effect> {
        let id = self.unit.id.clone();
        if self.review.unit(&id).is_none() {
            return Vec::new();
        }
        match a {
            Action::V1Recommend | Action::V1Disagree => {
                if !self.can_ask() {
                    self.status = "No agent: `teer setup` picks one.".into();
                    return Vec::new();
                }
                let kind = if a == Action::V1Recommend {
                    DiveKind::Recommend
                } else {
                    DiveKind::Disagree
                };
                if kind == DiveKind::Recommend
                    && let Some(u) = self.review.unit_mut(&id)
                    && !u.dive_pending.contains(&kind)
                {
                    u.dive.results.remove(&kind);
                }
                return self.dive_on(&id, kind, false);
            }
            Action::V1InterpretTests => {
                self.open_panel(Panel::Tool(DiveKind::Tests));
                return self.interpret_tests_on(&id, false);
            }
            _ => {}
        }
        let Some(kind) = tool_kind(a) else {
            return Vec::new();
        };
        if self.unit.panel == Some(Panel::Tool(kind)) {
            self.close_panel();
            return Vec::new();
        }
        if kind != DiveKind::Tests && !self.can_ask() {
            let (name, _) = flagged::light(kind);
            self.status = format!("No agent: {name} needs one (`teer setup` picks it).");
            return Vec::new();
        }
        self.open_panel(Panel::Tool(kind));
        if kind != DiveKind::Tests {
            return self.dive_on(&id, kind, false);
        }
        let u = self.review.unit(&id).expect("checked above");
        let coverage = u.dive.results.get(&kind).and_then(|r| r.coverage.as_ref());
        let read = coverage.is_some_and(|c| c.judged);
        let found = coverage.is_some_and(|c| c.search.is_some());
        let busy = u.dive_pending.contains(&kind);
        if !self.can_ask() || read || busy {
            return self.dive_on(&id, kind, false);
        }
        if found {
            return self.interpret_tests_on(&id, false);
        }
        self.unit.read_tests.insert(id.clone());
        self.dive_on(&id, kind, false)
    }

    /// teer's test search came back for a unit whose tests the agent is
    /// to read: have it read them.
    pub(super) fn unit_tests_found(&mut self, ev: &DiveEvent) -> Vec<Effect> {
        let DiveEvent::Ready {
            id,
            kind: DiveKind::Tests,
            coverage: Some(c),
            ..
        } = ev
        else {
            return Vec::new();
        };
        if c.judged || !self.unit.read_tests.remove(id) || !self.can_ask() {
            return Vec::new();
        }
        self.interpret_tests_on(id, false)
    }

    /// `gd` in the unit view: the jump list of what the unit calls in the
    /// review (Q5), and what it calls outside the review, read-only in the
    /// panel.
    pub(super) fn unit_goto_definition(&mut self) -> Vec<Effect> {
        let id = self.unit.id.clone();
        let outside = self.outside_definitions(&id);
        let found = !outside.is_empty();
        if found {
            self.open_panel(Panel::Outside);
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
    fn opening_asks_for_its_recommendation_and_the_next_ones_once() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        let effects = a.open_unit(id.clone());
        assert_eq!(a.screen, Screen::Unit);
        let next = a.review.units[a.order[a.sel + 1]].id.clone();
        assert_eq!(recommends(&effects), [id.clone(), next]);
        // On their way, or answered: not asked again.
        assert!(recommends(&a.open_unit(id)).is_empty());
    }

    #[test]
    fn the_text_has_the_sections_and_hunk_jumps_scroll_to_each_change() {
        let mut a = app();
        let id = a.selected_id().unwrap();
        a.open_unit(id);
        let text = render(&a, 100, 50);
        for s in [preview::WHY_HEADING, "What changed", "What to do about it"] {
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
        assert!(a.go_to_unit(&other, true).is_some());
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
}
