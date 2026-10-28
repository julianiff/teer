//! The Ask view (`/`): questions about the code and the agent's answers,
//! oldest first. Each answer names the units of this change it has to do
//! with and shows the code it means, read by teer, in boxes; code of the
//! change is marked so, its changed lines with `+`. The question being
//! typed sits at the bottom. An answer also shows on the unit it belongs
//! to ([`note_lines`]). On the Flagged screen the same conversation, kept
//! to one unit, is its Chat tile ([`draw_chat`]).

use std::cell::Cell;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Target};
use super::preview::barred;
use super::slim;
use crate::config::Action;
use crate::review::model::{Question, Shown, Unit};
use crate::util::{sanitize, truncate, wrap};

/// What a question on its way says.
pub const ASKING: &str = "the agent reads the code…";
/// The view's title.
pub const TITLE: &str = " Ask about the code ";
/// Lines of an answer a unit's note shows before it is cut.
const NOTE_LINES: usize = 4;

pub fn draw_ask(f: &mut Frame, app: &App) {
    let t = &app.theme;
    let area = f.area();
    let box_w = area.width.saturating_sub(4).min(120);
    let box_h = area.height.saturating_sub(2);
    if box_w < 24 || box_h < 8 {
        f.render_widget(Clear, area);
        return;
    }
    let r = Rect::new(
        area.x + (area.width - box_w) / 2,
        area.y + (area.height - box_h) / 2,
        box_w,
        box_h,
    );
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.accent())
        .title(Span::styled(TITLE, t.bold()));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let inner = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    let [talk, rule, input, keys] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let w = inner.width as usize;

    draw_talk(
        f,
        app,
        talk,
        Talk {
            which: &app.talk(),
            chat: false,
            scroll: &app.ask.scroll,
            max: &app.ask.max_scroll,
            focus: app.ask.link,
        },
    );
    f.render_widget(Paragraph::new(Line::styled("─".repeat(w), t.muted())), rule);
    let typing = app.ask.link.is_none();
    f.render_widget(
        Paragraph::new(input_line(app, " ask › ", &app.ask.input, typing, w)),
        input,
    );
    let keys_text = if app.ask.link.is_some() {
        "↑ ↓ links · ⏎ open · tab type · esc back"
    } else {
        "⏎ ask · tab links · ↑ ↓ scroll · ctrl+r ask again · esc close"
    };
    f.render_widget(
        Paragraph::new(Line::styled(truncate(keys_text, w), t.muted())),
        keys,
    );
    super::ui::text_area(app, talk, 0);
    super::ui::text_area(app, keys, 0);
    super::ui::click_hints(f, app, keys, keys_text);
}

/// A conversation to draw: the questions, by index; whether it is one
/// unit's chat; its scroll (`None` follows the end) and how far it went as
/// last drawn; the link in focus, kept in view.
pub struct Talk<'a> {
    pub which: &'a [usize],
    pub chat: bool,
    pub scroll: &'a Cell<Option<usize>>,
    pub max: &'a Cell<usize>,
    pub focus: Option<usize>,
}

/// Draw `talk` in `area`, each link on show a click target.
pub fn draw_talk(f: &mut Frame, app: &App, area: Rect, talk: Talk) {
    let (lines, links) = conversation(app, talk.which, talk.chat, area.width as usize);
    let h = area.height as usize;
    let max = lines.len().saturating_sub(h);
    talk.max.set(max);
    let mut top = talk.scroll.get().unwrap_or(max).min(max);
    if let Some(sel) = talk.focus
        && let Some(&(at, _)) = links.iter().find(|(_, l)| *l == sel)
    {
        if at < top {
            top = at;
        } else if at >= top + h {
            top = at + 1 - h;
        }
        talk.scroll.set((top < max).then_some(top));
    }
    for &(at, link) in &links {
        if (top..top + h).contains(&at) {
            app.targets.borrow_mut().push((
                Rect::new(area.x, area.y + (at - top) as u16, area.width, 1),
                Target::AskLink(link),
            ));
        }
    }
    let shown: Vec<Line> = lines.into_iter().skip(top).take(h).collect();
    f.render_widget(Paragraph::new(shown), area);
}

/// The chat about `u` in `area` (a tile's inside): the conversation, a
/// rule, and the question being typed.
pub fn draw_chat(f: &mut Frame, app: &App, u: &Unit, area: Rect, focus: bool) {
    let t = &app.theme;
    if area.height < 3 || area.width < 4 {
        return;
    }
    let area = Rect::new(area.x + 1, area.y, area.width - 2, area.height);
    let [talk, rule, input] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let w = area.width as usize;
    draw_talk(
        f,
        app,
        talk,
        Talk {
            which: &app.chat_of(&u.id),
            chat: true,
            scroll: &app.ask.chat_scroll,
            max: &app.ask.chat_max,
            focus: None,
        },
    );
    if focus {
        app.detail_rows.set(talk.height as usize);
        app.detail_max_scroll.set(app.ask.chat_max.get());
    }
    f.render_widget(Paragraph::new(Line::styled("─".repeat(w), t.muted())), rule);
    let typing = focus && app.flagged.typing;
    let line = if typing || !app.ask.draft.is_empty() {
        input_line(app, " › ", &app.ask.draft, typing, w)
    } else {
        let start = format!(" › {} to ask about it", app.keys.label(Action::Ask));
        Line::styled(truncate(&start, w), t.muted())
    };
    f.render_widget(Paragraph::new(line), input);
}

/// What the chat's border says about its keys.
pub fn chat_keys(app: &App, focus: bool) -> String {
    if focus && app.flagged.typing {
        " ⏎ ask · ctrl+r again · esc done ".into()
    } else {
        format!(" {} type ", app.keys.label(Action::Ask))
    }
}

/// `lead` then `text▏`: the end of the text stays in view. Muted while
/// not `typing`.
fn input_line(app: &App, lead: &'static str, text: &str, typing: bool, w: usize) -> Line<'static> {
    let t = &app.theme;
    let room = w.saturating_sub(lead.width() + 1);
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0;
    for c in sanitize(text).chars().rev() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > room {
            break;
        }
        used += cw;
        tail.push(c);
    }
    let text: String = tail.into_iter().rev().collect();
    if !typing {
        return Line::from(vec![
            Span::styled(lead, t.muted()),
            Span::styled(text, t.muted()),
        ]);
    }
    Line::from(vec![
        Span::styled(lead, t.accent()),
        Span::raw(text),
        Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED)),
    ])
}

/// The conversation of questions `which` as lines `w` wide, and where each
/// link is: (line, index into [`App::ask_links`]). Built in the same order
/// as the links. A `chat` is about one unit, so where its answers are
/// noted goes without saying.
fn conversation(
    app: &App,
    which: &[usize],
    chat: bool,
    w: usize,
) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let t = &app.theme;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut links = Vec::new();
    if which.is_empty() {
        let intro = if !app.has_agent {
            "No agent: asking needs one. `teer setup` picks Claude Code, Codex or a model API."
        } else if chat {
            "Ask about this unit in your own words: \"why is the retry here?\", \"who else calls it?\", \
\"show me the test for it\". The agent reads the code (read-only) and this conversation so far, \
so a question can follow on from the last answer. The conversation is kept with the unit."
        } else {
            "Ask anything about the code: \"show me the Cart class\", \"how are retries done?\", \
\"who calls charge_order?\". The agent reads the repository (read-only) and points at the code; \
teer shows those lines, marks what this change touched, and links the units of the change the \
question has to do with. Each answer is also noted on the unit it is about."
        };
        for l in wrap(intro, w.saturating_sub(1)) {
            lines.push(Line::styled(format!(" {l}"), t.muted()));
        }
        return (lines, links);
    }
    let mut next_link = 0;
    for (k, &n) in which.iter().enumerate() {
        let q = &app.review.questions[n];
        if k > 0 {
            lines.push(Line::default());
        }
        let last = k + 1 == which.len();
        question_lines(app, n, q, (last, chat), w, &mut lines);
        let Some(a) = &q.answer else {
            continue;
        };
        lines.push(Line::default());
        lines.push(barred(app, Line::styled(agent_label(app), t.muted()), 0));
        for l in wrap(&a.text, w.saturating_sub(2)) {
            lines.push(barred(app, Line::raw(l), 0));
        }
        if !a.related.is_empty() {
            lines.push(Line::default());
            lines.push(Line::styled("  In this change", t.bold()));
        }
        for r in &a.related {
            let link = app.listed(&r.id).then(|| {
                next_link += 1;
                next_link - 1
            });
            if let Some(link) = link {
                links.push((lines.len(), link));
            }
            let selected = link.is_some() && link == app.ask.link;
            lines.push(related_line(app, &r.id, &r.why, selected, w));
        }
        for s in &a.shown {
            lines.push(Line::default());
            let selected = app.ask.link == Some(next_link);
            links.push((lines.len(), next_link));
            next_link += 1;
            lines.extend(code_box(app, s, selected, w));
        }
    }
    (lines, links)
}

/// `› the question`, where its answer is noted (not in a chat), and how it
/// stands; `ctrl+r` asks the `last` again.
fn question_lines(
    app: &App,
    n: usize,
    q: &Question,
    (last, chat): (bool, bool),
    w: usize,
    lines: &mut Vec<Line<'static>>,
) {
    let t = &app.theme;
    for (k, l) in wrap(&q.text, w.saturating_sub(2)).into_iter().enumerate() {
        let lead = if k == 0 { "› " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(lead, t.accent().add_modifier(Modifier::BOLD)),
            Span::styled(l, t.bold()),
        ]));
    }
    if let Some(u) = q.unit.as_deref().and_then(|id| app.review.unit(id))
        && !chat
    {
        let on = if q.answer.is_some() {
            "noted on"
        } else {
            "about"
        };
        lines.push(Line::styled(
            truncate(&format!("  {on} {}", slim::row_name(u)), w),
            t.muted(),
        ));
    }
    if app.ask.pending.contains(&n) {
        lines.push(Line::styled(
            format!("  {} {ASKING}", app.spinner()),
            t.muted(),
        ));
    } else if let Some(e) = &q.error {
        let again = if last { " · ctrl+r asks again" } else { "" };
        lines.push(Line::styled(
            truncate(&format!("  ✗ {}{again}", sanitize(e)), w),
            t.needs_fix(),
        ));
    } else if q.answer.is_none() {
        let again = if last { " · ctrl+r asks it" } else { "" };
        lines.push(Line::styled(format!("  not answered{again}"), t.muted()));
    }
}

/// Who answers: the deep model, as for the Flagged questions.
fn agent_label(app: &App) -> String {
    format!(
        "agent ({})",
        slim::model_name(app, crate::config::Role::Deep)
    )
}

/// A unit the answer has to do with: its level's icon, name and place, and
/// why. `▌` when it is the link in focus; muted when it isn't in the list.
fn related_line(app: &App, id: &str, why: &str, selected: bool, w: usize) -> Line<'static> {
    let t = &app.theme;
    let Some(u) = app.review.unit(id) else {
        return Line::default();
    };
    let level = app.assessment(id).map(|a| a.level);
    let icon = level.map_or(" ", |l| l.icon());
    let icon_style = level.map_or(t.muted(), |l| slim::level_style(app, l));
    let gutter = if selected {
        Span::styled("▌", t.accent())
    } else {
        Span::raw(" ")
    };
    let name_style = if !app.listed(id) {
        t.muted()
    } else if selected {
        t.accent().add_modifier(Modifier::BOLD)
    } else {
        t.bold()
    };
    let name = slim::row_name(u);
    let place = slim::place(u);
    let why = if why.is_empty() {
        String::new()
    } else {
        format!(" — {why}")
    };
    let used = 4 + name.width();
    Line::from(vec![
        gutter,
        Span::raw(" "),
        Span::styled(format!("{icon} "), icon_style),
        Span::styled(name, name_style),
        Span::styled(
            truncate(&format!("  {place}{why}"), w.saturating_sub(used)),
            t.muted(),
        ),
    ])
}

/// Shown code in a rounded box `w` wide: `file:start-end`, what it is, and
/// whether this change touched the file on its top border; the lines
/// numbered and coloured, those the change added or edited with `+`.
fn code_box(app: &App, s: &Shown, selected: bool, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let border = if selected {
        t.accent().add_modifier(Modifier::BOLD)
    } else if s.in_change {
        t.note_border()
    } else {
        t.muted()
    };
    let place = format!(
        "{}:{}-{}",
        sanitize(&s.cite.file.display().to_string()),
        s.cite.start,
        s.cite.end
    );
    let mut title = vec![Span::styled(format!(" {place} "), t.bold())];
    if !s.why.is_empty() {
        title.push(Span::styled(format!("{} ", s.why), t.text()));
    }
    if s.in_change {
        title.push(Span::styled("· in this change ", t.accent()));
    }
    let inner = w.saturating_sub(4);
    let title_w: usize = title.iter().map(|s| s.content.width()).sum();
    let title = if title_w + 3 > w {
        vec![Span::styled(
            truncate(&format!(" {place} "), w.saturating_sub(3)),
            t.bold(),
        )]
    } else {
        title
    };
    let title_w: usize = title.iter().map(|s| s.content.width()).sum();
    let mut top = vec![Span::styled("╭", border)];
    top.extend(title);
    top.push(Span::styled(
        format!("{}╮", "─".repeat(w.saturating_sub(title_w + 2))),
        border,
    ));
    let mut out = vec![Line::from(top)];
    for row in super::diff::listing(app, &s.cite.file, s.cite.start, &s.code, &s.changed, inner) {
        for line in row {
            let used: usize = line.spans.iter().map(|s| s.content.width()).sum();
            let mut spans = vec![Span::styled("│ ", border)];
            spans.extend(line.spans);
            spans.push(Span::raw(" ".repeat(inner.saturating_sub(used))));
            spans.push(Span::styled(" │", border));
            out.push(Line::from(spans));
        }
    }
    out.push(Line::styled(
        format!("╰{}╯", "─".repeat(w.saturating_sub(2))),
        border,
    ));
    out
}

/// The answers noted on unit `u`, newest first, as cards `w` wide: the
/// question on the top border, the start of the answer inside, and what
/// else the answer holds. Empty when nothing was asked about it.
pub fn note_lines(app: &App, id: &str, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let border = t.accent();
    let mut out = Vec::new();
    for q in app.answers_for(id) {
        let Some(a) = &q.answer else {
            continue;
        };
        let inner = w.saturating_sub(4);
        let title = truncate(&format!(" ? {} ", q.text.trim()), w.saturating_sub(3));
        out.push(Line::from(vec![
            Span::styled("╭", border),
            Span::styled(title.clone(), border.add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{}╮", "─".repeat(w.saturating_sub(title.width() + 2))),
                border,
            ),
        ]));
        let mut body = wrap(&a.text, inner);
        if body.len() > NOTE_LINES {
            body.truncate(NOTE_LINES);
            if let Some(last) = body.last_mut() {
                *last = format!("{}…", truncate(last, inner.saturating_sub(1)));
            }
        }
        let more = match a.shown.len() {
            0 => String::new(),
            1 => format!("1 piece of code · {} shows it", app.keys.label(Action::Ask)),
            n => format!(
                "{n} pieces of code · {} shows them",
                app.keys.label(Action::Ask)
            ),
        };
        let card_line = |text: String, style: Style| {
            Line::from(vec![
                Span::styled("│ ", border),
                Span::styled(crate::util::truncate_pad(&text, inner), style),
                Span::styled(" │", border),
            ])
        };
        for l in body {
            out.push(card_line(l, t.note()));
        }
        if !more.is_empty() {
            out.push(card_line(more, t.muted()));
        }
        out.push(Line::styled(
            format!("╰{}╯", "─".repeat(w.saturating_sub(2))),
            border,
        ));
    }
    out
}
