//! A unit's conversation (`K`, the unit view's ask panel, see
//! [`super::unit`]): questions about the unit and the agent's answers,
//! oldest first. Each answer names the units of this change it has to do
//! with and shows the code it means, read by teer, in boxes; code of the
//! change is marked so, its changed lines with `+`. An answer also shows on
//! the unit it belongs to ([`note_lines`]).

use crate::session::names;
use std::cell::Cell;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::app::{App, Target};
use super::preview::barred;
use super::slim;
use super::ui::fixed;
use crate::keymap::Action;
use crate::review::model::{Question, Shown};
use crate::session::app::LiveAnswer;
use crate::util::{sanitize, truncate, wrap};

/// What a question on its way says.
pub const ASKING: &str = "the agent reads the code…";
/// While the answer comes in.
pub const WRITING: &str = "the agent writes the answer…";
/// Lines of an answer a unit's note shows before it is cut.
const NOTE_LINES: usize = 4;

/// A conversation to draw: the questions about one unit, by index; its
/// scroll (`None` follows the end) and how far it went as last drawn; the
/// link in focus, kept in view.
pub struct Talk<'a> {
    pub which: &'a [usize],
    pub scroll: &'a Cell<Option<usize>>,
    pub max: &'a Cell<usize>,
    pub focus: Option<usize>,
}

/// Draw `talk` in `area`, each link on show a click target.
pub fn draw_talk(f: &mut Frame, app: &App, area: Rect, talk: Talk) {
    let (lines, links) = conversation(app, talk.which, area.width as usize);
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

/// `lead` then `text▏`: the end of the text stays in view. Muted while
/// not `typing`.
pub(super) fn input_line(
    app: &App,
    lead: &'static str,
    text: &str,
    typing: bool,
    w: usize,
) -> Line<'static> {
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

/// The conversation of questions `which`, all about one unit, as lines `w`
/// wide, and where each link is: (line, index into [`App::ask_links`]).
/// Built in the same order as the links.
fn conversation(app: &App, which: &[usize], w: usize) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let t = &app.theme;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut links = Vec::new();
    let mut next_link = 0;
    for (k, &n) in which.iter().enumerate() {
        let q = &app.review.questions[n];
        if k > 0 {
            lines.push(Line::default());
        }
        let last = k + 1 == which.len();
        question_lines(app, n, q, last, w, &mut lines);
        // The answer being written, in the old one's place.
        if let Some(text) = app.live_text(n) {
            lines.push(Line::default());
            lines.push(barred(app, Line::styled(agent_label(app), t.muted()), 0));
            for l in wrap(text, w.saturating_sub(2)) {
                lines.push(barred(app, Line::raw(l), 0));
            }
            continue;
        }
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

/// `› the question` and how it stands; `^r` asks the `last` again.
fn question_lines(
    app: &App,
    n: usize,
    q: &Question,
    last: bool,
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
    if app.ask.pending.contains(&n) {
        let live = app.ask.live.get(&n);
        let doing = match live {
            _ if app.live_text(n).is_some() => WRITING.to_string(),
            Some(LiveAnswer {
                step: Some(step),
                steps,
                ..
            }) => format!("step {steps}: {step}"),
            _ => ASKING.to_string(),
        };
        lines.push(Line::styled(
            truncate(&format!("  {} {doing}", app.spinner()), w),
            t.muted(),
        ));
    } else if let Some(e) = &q.error {
        let again = if last {
            format!(" · {} asks again", fixed::RETRY)
        } else {
            String::new()
        };
        lines.push(Line::styled(
            truncate(&format!("  ✗ {}{again}", sanitize(e)), w),
            t.needs_fix(),
        ));
    } else if q.answer.is_none() {
        let again = if last {
            format!(" · {} asks it", fixed::RETRY)
        } else {
            String::new()
        };
        lines.push(Line::styled(format!("  not answered{again}"), t.muted()));
    }
}

/// Who answers: the deep model, as for the unit view's other questions.
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
    let name = names::row_name(u);
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
pub(super) fn code_box(app: &App, s: &Shown, selected: bool, w: usize) -> Vec<Line<'static>> {
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
            1 => format!(
                "1 piece of code · {} shows it",
                app.keys.label(Action::AskOpen)
            ),
            n => format!(
                "{n} pieces of code · {} shows them",
                app.keys.label(Action::AskOpen)
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
