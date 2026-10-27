//! The reviewer's notes on screen: the card under a unit's code, the editor
//! (`m`), and the notes view (`N`) that takes them out of teer.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Target};
use super::diff;
use crate::config::Action;
use crate::review::model::Unit;
use crate::util::{sanitize, truncate, truncate_pad, wrap};

/// Most rows of note text on a card.
const CARD_LINES: usize = 6;
/// Most rows of text the editor shows; earlier ones scroll away.
const EDITOR_LINES: usize = 8;
/// Rows of the editor besides its text: borders, padding, a blank, the keys.
const EDITOR_FRAME: usize = 6;

/// The note on `u` as a card `w` wide, titled `✎ you`, saying when it was
/// written on other code. Empty without a note.
pub fn card(app: &App, u: &Unit, w: usize) -> Vec<Line<'static>> {
    let note = u.dive.note.trim();
    if note.is_empty() || w < 8 {
        return Vec::new();
    }
    let t = &app.theme;
    let border = t.muted();
    let inner = w - 4;
    let title = " ✎ you ";
    let mut out = vec![Line::from(vec![
        Span::styled("╭", border),
        Span::styled(title, t.accent().add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("{}╮", "─".repeat(w.saturating_sub(title.width() + 2))),
            border,
        ),
    ])];
    let line = |text: String, style: Style| {
        Line::from(vec![
            Span::styled("│ ", border),
            Span::styled(truncate_pad(&text, inner), style),
            Span::styled(" │", border),
        ])
    };
    let mut body = wrap(&sanitize(note), inner);
    if body.len() > CARD_LINES {
        body.truncate(CARD_LINES);
        if let Some(last) = body.last_mut() {
            *last = format!("{}…", truncate(last, inner.saturating_sub(1)));
        }
    }
    out.extend(body.into_iter().map(|l| line(l, t.text())));
    if u.note_is_stale() {
        out.push(line(
            format!(
                "the code changed since · {} confirms",
                app.keys.label(Action::EditNote)
            ),
            t.muted(),
        ));
    }
    out.push(Line::styled(
        format!("╰{}╯", "─".repeat(w.saturating_sub(2))),
        border,
    ));
    out
}

/// The code box of `u` (see [`diff::draw_box`]) with the note's card under
/// it. Without room for both, the box alone.
pub fn draw_box(f: &mut Frame, app: &App, area: Rect, u: &Unit, notes: diff::Notes) -> bool {
    let card = card(app, u, area.width as usize);
    let rows = diff::box_rows(app, u, area.width, notes);
    let h = area.height as usize;
    if card.is_empty() || rows == 0 || h < card.len() + 3 {
        return diff::draw_box(f, app, area, u, notes);
    }
    let box_h = rows.min(h - card.len());
    let [code, below] = Layout::vertical([
        Constraint::Length(box_h as u16),
        Constraint::Length(card.len() as u16),
    ])
    .areas(area);
    if !diff::draw_box(f, app, code, u, notes) {
        return false;
    }
    f.render_widget(Paragraph::new(card), below);
    true
}

fn rounded(title: String, style: Style) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(style)
        .title(Span::styled(title, style.add_modifier(Modifier::BOLD)))
}

/// The note editor for unit `id`: the text wrapped, its end in view.
pub fn draw_editor(f: &mut Frame, app: &App, id: &str, text: &str) {
    let t = &app.theme;
    let area = f.area();
    let w = area.width.saturating_sub(4).min(72);
    let inner_w = w.saturating_sub(6) as usize;
    if inner_w < 4 {
        return;
    }
    let name = app
        .review
        .unit(id)
        .map(crate::notes::place)
        .unwrap_or_default();
    let room = inner_w.saturating_sub(1);
    let mut lines = wrap(&sanitize(text), room);
    if lines.is_empty() {
        lines.push(String::new());
    }
    // Wrapping drops a trailing space; the cursor still goes after it.
    if text.ends_with(' ') && !lines.last().is_some_and(String::is_empty) {
        match lines.last_mut() {
            Some(l) if l.width() + 1 < room => l.push(' '),
            _ => lines.push(String::new()),
        }
    }
    let fit = EDITOR_LINES.min((area.height as usize).saturating_sub(EDITOR_FRAME).max(1));
    let skip = lines.len().saturating_sub(fit);
    let last = lines.len() - 1;
    let mut rows: Vec<Line> = lines
        .into_iter()
        .enumerate()
        .skip(skip)
        .map(|(i, l)| {
            let mut spans = vec![Span::raw(l)];
            if i == last {
                spans.push(Span::styled(
                    " ",
                    Style::new().add_modifier(Modifier::REVERSED),
                ));
            }
            Line::from(spans)
        })
        .collect();
    rows.push(Line::default());
    rows.push(Line::from(vec![
        Span::styled("enter", t.accent()),
        Span::raw(" save · "),
        Span::styled("esc", t.accent()),
        Span::raw(" cancel · an empty note is deleted"),
    ]));
    let h = rows.len() as u16 + 4;
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h.min(area.height),
    );
    f.render_widget(Clear, r);
    let title = format!(" Note on {} ", truncate(&name, inner_w.saturating_sub(10)));
    let block = rounded(title, t.accent()).padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(r);
    f.render_widget(block, r);
    f.render_widget(Paragraph::new(rows), inner);
    super::ui::text_area(app, inner, 0);
    super::ui::click_hints(f, app, inner, "enter save · esc cancel");
}

/// The keys of the notes view, as `(key, label)`: Claude Code only for a
/// local review, posting only for a pull request.
fn view_keys(app: &App) -> Vec<(String, &'static str)> {
    let mut keys = vec![
        ("enter".to_string(), "go to"),
        (app.keys.label(Action::EditNote), "edit"),
        ("d".into(), "delete"),
        ("y".into(), "copy all"),
    ];
    if app.review.pr.is_some() {
        keys.push(("p".into(), "post on the PR"));
    } else {
        keys.push(("c".into(), "hand to Claude Code"));
    }
    keys.push(("esc".into(), "close"));
    keys
}

/// The notes view: every note by file, the selected one marked, and the
/// ways out at the bottom.
pub fn draw_view(f: &mut Frame, app: &App, sel: usize) {
    let t = &app.theme;
    let area = f.area();
    let w = area.width.saturating_sub(4).min(100);
    let h = area.height.saturating_sub(2);
    if w < 20 || h < 6 {
        return;
    }
    let noted = crate::notes::noted(&app.review);
    let sel = sel.min(noted.len().saturating_sub(1));
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + 1, w, h);
    f.render_widget(Clear, r);
    let count = match noted.len() {
        1 => "1 note".to_string(),
        n => format!("{n} notes"),
    };
    let block = rounded(format!(" Notes · {count} "), t.accent()).padding(Padding::new(1, 1, 0, 0));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let width = inner.width as usize;

    // The notes, which lines hold each, and the selected one.
    let mut lines: Vec<Line> = Vec::new();
    let mut places = Vec::new();
    let mut at = (0, 0);
    let mut file = None;
    for (i, u) in noted.iter().enumerate() {
        if file != Some(&u.file) {
            if file.is_some() {
                lines.push(Line::default());
            }
            file = Some(&u.file);
            lines.push(Line::styled(
                truncate(&sanitize(&u.file.to_string_lossy()), width),
                t.muted().add_modifier(Modifier::BOLD),
            ));
        }
        let start = lines.len();
        let selected = i == sel;
        let level = app
            .assessment(&u.id)
            .map(|a| a.level)
            .unwrap_or(crate::review::attention::Level::Routine);
        let place = crate::notes::place(u);
        let lines_at = place
            .rsplit_once(':')
            .map(|(_, l)| l.to_string())
            .unwrap_or_default();
        let mut head = vec![
            Span::styled(if selected { "▌" } else { " " }, t.accent()),
            Span::styled(
                format!("{} ", level.icon()),
                super::slim::level_style(app, level),
            ),
        ];
        let name_style = if selected {
            t.accent().add_modifier(Modifier::BOLD)
        } else {
            t.text()
        };
        let room = width.saturating_sub(4 + lines_at.width() + 2);
        head.push(Span::styled(
            truncate(&sanitize(&u.symbol), room),
            name_style,
        ));
        head.push(Span::styled(format!("  {lines_at}"), t.muted()));
        lines.push(Line::from(head));
        for l in wrap(&sanitize(u.dive.note.trim()), width.saturating_sub(4)) {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(l, t.text()),
            ]));
        }
        if u.note_is_stale() {
            lines.push(Line::styled(
                "    the code changed since this note",
                t.muted(),
            ));
        }
        if selected {
            at = (start, lines.len());
        }
        places.push((start, lines.len()));
    }

    let keys = view_keys(app);
    let mut spans = Vec::new();
    for (i, (k, l)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", t.muted()));
        }
        spans.push(Span::styled(k.clone(), t.accent()));
        spans.push(Span::raw(format!(" {l}")));
    }
    let foot = Line::from(spans);
    let [body, _, keys_row] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    // Scroll just enough to show the whole selected note.
    let rows = body.height as usize;
    let top = if at.1 > rows {
        (at.1 - rows).min(at.0)
    } else {
        0
    };
    let shown: Vec<Line> = lines.into_iter().skip(top).take(rows).collect();
    f.render_widget(Paragraph::new(shown), body);
    f.render_widget(Paragraph::new(foot), keys_row);
    super::ui::text_area(app, inner, 0);
    // A click on a note selects it.
    for (i, (start, end)) in places.into_iter().enumerate() {
        for line in start.max(top)..end.min(top + rows) {
            let y = body.y + (line - top) as u16;
            let at = Rect::new(body.x, y, body.width, 1);
            app.targets.borrow_mut().push((at, Target::Note(i)));
        }
    }
    let hints: Vec<String> = keys.iter().map(|(k, l)| format!("{k} {l}")).collect();
    super::ui::click_hints(f, app, keys_row, &hints.join(" · "));
}
