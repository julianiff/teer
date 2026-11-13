//! The mode picker (`<space>m`): each mode, what it is for and the tools
//! it has, the one the review is in marked; under them the keys.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::App;
use super::ui::{click_hints, fixed, hint, hint_line};
use crate::keymap::Action;
use crate::mode::Mode;
use crate::util::{truncate, wrap};

/// Widest the picker gets.
const MAX_W: u16 = 84;

/// Draw the picker over the screen, mode `sel` selected.
pub fn draw(f: &mut Frame, app: &App, sel: usize) {
    let t = &app.theme;
    let area = f.area();
    let box_w = area.width.saturating_sub(4).min(MAX_W);
    // Borders and a column of padding each side.
    let w = box_w.saturating_sub(4) as usize;
    if box_w < 30 || area.height < 8 {
        return;
    }
    let name_w = Mode::EVERY
        .iter()
        .map(|m| m.name().width())
        .max()
        .unwrap_or(0);
    // The bar's column, the icon and a space, the name, a gap.
    let lead = 1 + 3 + name_w + 2;
    let text_w = w.saturating_sub(lead).max(1);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, m) in Mode::EVERY.into_iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        let on = i == sel;
        let bg = if on {
            t.selected_entry()
        } else {
            Default::default()
        };
        let mut about = m.about().to_string();
        if m == app.mode {
            about.push_str(" (now)");
        }
        let about = truncate(&about, text_w);
        let pad = w.saturating_sub(lead + about.width());
        lines.push(Line::from(vec![
            Span::styled(if on { "▌" } else { " " }, t.accent().patch(bg)),
            // An icon is two cells; a mode without one keeps the column.
            Span::styled(
                match m.icon() {
                    "" => "   ".to_string(),
                    icon => format!("{icon} "),
                },
                bg,
            ),
            Span::styled(format!("{:<name_w$}  ", m.name()), t.bold().patch(bg)),
            Span::styled(about, t.text().patch(bg)),
            Span::styled(" ".repeat(pad), bg),
        ]));
        for l in wrap(m.tools(), text_w) {
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(lead)),
                Span::styled(l, t.muted()),
            ]));
        }
    }
    let k = |a: Action| app.keys.label(a);
    let keys = vec![
        hint(fixed::ENTER, "use"),
        hint(
            format!("{} {}", k(Action::MoveDown), k(Action::MoveUp)),
            "move",
        ),
        hint(fixed::ESC, "cancel"),
    ];
    // Borders, the rule, a blank and the keys.
    let room = (area.height.saturating_sub(2) as usize).saturating_sub(3);
    lines.truncate(room.max(1));
    lines.push(Line::styled("─".repeat(w), t.muted()));
    lines.push(Line::default());
    let (keys, plain) = hint_line(app, &keys);
    lines.push(keys);
    let box_h = lines.len() as u16 + 2;
    let r = Rect::new(
        area.x + (area.width - box_w) / 2,
        area.y + (area.height - box_h.min(area.height)) / 2,
        box_w,
        box_h.min(area.height),
    );
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.accent())
        .padding(Padding::horizontal(1))
        .title(Span::styled(" Mode: what is this review for? ", t.bold()));
    let inner = block.inner(r);
    f.render_widget(block, r);
    f.render_widget(Paragraph::new(lines), inner);
    let keys_row = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );
    click_hints(f, app, keys_row, &plain);
}
