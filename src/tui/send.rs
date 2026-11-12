//! The session picker (`s`): the agent sessions running in this
//! repository, the one the result goes to selected; under them the
//! message's first lines, and the keys. With pi installed but without
//! teer's extension, a last row installs it, after a question.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::App;
use super::ui::{click_hints, fixed, hint, hint_line, text_area};
use crate::keymap::Action;
use crate::send::Target;
use crate::session::Overlay;
use crate::session::send::{PI_QUESTION, PI_ROW, ago, now_ms};
use crate::util::{truncate, wrap};

/// Most lines of the message shown under the sessions.
const PREVIEW: usize = 5;

/// Widest the picker gets.
const MAX_W: u16 = 88;

/// Draw the picker over the screen.
pub fn draw(f: &mut Frame, app: &App, targets: &[Target], sel: usize, what: &str) {
    let t = &app.theme;
    let area = f.area();
    let text = match &app.overlay {
        Overlay::SendTo { text, .. } => text.as_str(),
        _ => "",
    };
    let box_w = area.width.saturating_sub(4).min(MAX_W);
    // Borders and a column of padding each side.
    let w = box_w.saturating_sub(4) as usize;
    if box_w < 24 || area.height < 7 {
        return;
    }
    let pi = app.send.pi_offer;
    let rows_n = targets.len() + usize::from(pi);
    let sel = sel.min(rows_n.saturating_sub(1));
    let asking = app.send.confirm_pi;

    // What goes above the rule: the sessions, or the question.
    let mut top: Vec<Line<'static>> = Vec::new();
    let mut sel_line = 0;
    if asking {
        for l in wrap(PI_QUESTION, w) {
            top.push(Line::styled(l, t.bold()));
        }
    } else {
        if targets.is_empty() {
            top.push(Line::styled(
                truncate("No agent session runs in this repository.", w),
                t.muted(),
            ));
        }
        let agent_w = targets
            .iter()
            .map(|x| x.agent.title().width())
            .max()
            .unwrap_or(0);
        let name_w = targets
            .iter()
            .map(|x| x.name.width())
            .max()
            .unwrap_or(0)
            .min(24);
        let now = now_ms();
        for (i, x) in targets.iter().enumerate() {
            if i == sel {
                sel_line = top.len();
            }
            let spans = target_spans(app, x, agent_w, name_w, now);
            top.push(row(app, spans, w, i == sel));
        }
        if pi {
            if sel == targets.len() {
                sel_line = top.len();
            }
            let spans = vec![Span::styled(PI_ROW.to_string(), t.muted())];
            top.push(row(app, spans, w, sel == targets.len()));
        }
    }

    // Its blank lines left out: as much of it as fits.
    let preview: Vec<Line<'static>> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(PREVIEW)
        .map(|l| Line::styled(truncate(l, w), t.muted()))
        .collect();
    let keys = if asking {
        vec![
            hint(format!("{}/{}", fixed::YES, fixed::ENTER), "yes"),
            hint(fixed::ESC, "no"),
        ]
    } else {
        let go = if pi && sel == targets.len() {
            "install"
        } else {
            "send"
        };
        let mut keys = vec![hint(fixed::ENTER, go)];
        if rows_n > 1 {
            let k = |a: Action| app.keys.label(a);
            keys.push(hint(
                format!("{} {}", k(Action::MoveDown), k(Action::MoveUp)),
                "move",
            ));
        }
        keys.push(hint(fixed::ESC, "cancel"));
        keys
    };

    // Borders, the rule, a blank and the keys; the preview gives way
    // first, then the sessions scroll.
    let room = area.height.saturating_sub(2) as usize;
    let fixed_rows = 2 + 1 + 1 + 1;
    let top_rows = top.len().min(room.saturating_sub(fixed_rows).max(1));
    let preview_rows = preview
        .len()
        .min(room.saturating_sub(fixed_rows + top_rows));
    let box_h = (fixed_rows + top_rows + preview_rows) as u16;
    let r = centered(area, box_w, box_h);
    f.render_widget(Clear, r);
    let title = format!(" Send {what} to ");
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.accent())
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            truncate(&title, box_w.saturating_sub(4) as usize),
            t.bold(),
        ));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let first = (sel_line + 1).saturating_sub(top_rows);
    let mut lines: Vec<Line<'static>> = top.into_iter().skip(first).take(top_rows).collect();
    lines.push(Line::styled("─".repeat(w), t.muted()));
    lines.extend(preview.into_iter().take(preview_rows));
    lines.push(Line::default());
    let (keys, plain) = hint_line(app, &keys);
    lines.push(keys);
    f.render_widget(Paragraph::new(lines), inner);
    text_area(app, inner, 0);
    let keys_row = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );
    click_hints(f, app, keys_row, &plain);
}

/// A session's row: its agent, its name, then muted whether it is busy,
/// how long since it was active, that it wrote the change, and where it
/// runs when that isn't the repository's root.
fn target_spans(
    app: &App,
    x: &Target,
    agent_w: usize,
    name_w: usize,
    now: u64,
) -> Vec<Span<'static>> {
    let t = &app.theme;
    let agent = x.agent.title();
    let mut spans = vec![
        Span::styled(format!("{agent:<agent_w$}  "), t.bold()),
        Span::styled(
            crate::util::truncate_pad(&x.name, name_w),
            t.text().add_modifier(Modifier::BOLD),
        ),
    ];
    let mut meta: Vec<String> = Vec::new();
    if !x.status.is_empty() {
        meta.push(crate::util::sanitize(&x.status));
    }
    let when = ago(x.active_at, now);
    if !when.is_empty() {
        meta.push(when);
    }
    if !meta.is_empty() {
        spans.push(Span::styled(format!("  {}", meta.join(" · ")), t.muted()));
    }
    if app.wrote_change(x) {
        spans.push(Span::styled("  wrote this change", t.accent()));
    }
    if let Ok(rel) = x.cwd.strip_prefix(&app.review.repo_root)
        && !rel.as_os_str().is_empty()
    {
        spans.push(Span::styled(
            format!("  in {}", crate::util::sanitize(&rel.display().to_string())),
            t.muted(),
        ));
    }
    spans
}

/// `spans` as a row `w` cells wide after the bar's column: the bar and
/// the selection's background behind the whole row when `selected`.
fn row(app: &App, spans: Vec<Span<'static>>, w: usize, selected: bool) -> Line<'static> {
    let t = &app.theme;
    let spans = fit(spans, w.saturating_sub(1));
    if !selected {
        let mut out = vec![Span::raw(" ")];
        out.extend(spans);
        return Line::from(out);
    }
    let bg = t.selected_entry();
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    let mut out = vec![Span::styled("▌", t.accent().patch(bg))];
    out.extend(spans.into_iter().map(|s| {
        let style = s.style.patch(bg);
        s.style(style)
    }));
    out.push(Span::styled(" ".repeat(w.saturating_sub(1 + used)), bg));
    Line::from(out)
}

/// `spans` cut to `w` cells, the last one shown ending in `…` when cut.
fn fit(spans: Vec<Span<'static>>, w: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0;
    for s in spans {
        let sw = s.content.width();
        if used + sw <= w {
            used += sw;
            out.push(s);
            continue;
        }
        let room = w.saturating_sub(used);
        if room > 0 {
            out.push(Span::styled(truncate(&s.content, room), s.style));
        }
        break;
    }
    out
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}
