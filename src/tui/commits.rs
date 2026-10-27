//! The commits view (`C` in the list): the PR's title and description, the
//! commits between base and head with their type and tickets, and for the
//! selected commit its message and the units it changed.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::App;
use crate::config::Action;
use crate::review::intent::{self, Tag};
use crate::review::model::{Commit, Decision, UnitStatus};
use crate::util::{sanitize, truncate, wrap};

/// `text` as spans with its ticket references (`MD-732`, `#12`) highlighted.
fn with_tickets(app: &App, text: &str, style: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut at = 0;
    for (a, b) in intent::tickets(text) {
        if a > at {
            out.push(Span::styled(text[at..a].to_string(), style));
        }
        out.push(Span::styled(
            text[a..b].to_string(),
            app.theme.accent().add_modifier(Modifier::BOLD),
        ));
        at = b;
    }
    if at < text.len() {
        out.push(Span::styled(text[at..].to_string(), style));
    }
    out
}

/// The style of a commit's type label: new things green, fixes and reverts
/// red, fixups and merges muted, the rest in the accent colour.
fn tag_style(app: &App, tag: &Tag) -> Style {
    let t = &app.theme;
    let s = match tag.label.as_deref() {
        _ if tag.minor => t.muted(),
        Some("feat") => Style::new().fg(t.added),
        Some("fix" | "revert") => Style::new().fg(t.removed),
        _ => t.accent(),
    };
    s.add_modifier(Modifier::BOLD)
}

/// One row of the commit list: sha, type, subject (tickets highlighted),
/// author on the right; fixups and merges muted.
fn commit_row(app: &App, c: &Commit, tag_w: usize, selected: bool, w: usize) -> Line<'static> {
    let t = &app.theme;
    let tag = intent::tag(c);
    let label = match &tag.label {
        Some(l) if tag.breaking => format!("{l}!"),
        Some(l) => l.clone(),
        None => String::new(),
    };
    let author = truncate(&sanitize(&c.author), 20);
    let mut spans = vec![
        Span::styled(if selected { "▌" } else { " " }, t.accent()),
        Span::styled(format!("{} ", sanitize(&c.short)), t.muted()),
    ];
    if tag_w > 0 {
        let pad = tag_w.saturating_sub(label.width());
        spans.push(Span::styled(label, tag_style(app, &tag)));
        spans.push(Span::raw(" ".repeat(pad + 1)));
    }
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    let room = w.saturating_sub(used + author.width() + 2);
    let subject = truncate(&tag.subject, room);
    let style = match (tag.minor, selected) {
        (true, _) => t.muted(),
        (false, true) => t.bold(),
        (false, false) => Style::new(),
    };
    let sw = subject.width();
    spans.extend(with_tickets(app, &subject, style));
    let gap = w.saturating_sub(used + sw + author.width());
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(author, t.muted()));
    Line::from(spans)
}

/// The PR's title, author, size and description (or why there is none),
/// and the summary of the whole change.
fn intro_lines(app: &App, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let r = &app.review;
    let mut lines = Vec::new();
    if let Some(s) = &r.intent.summary {
        for l in wrap(s, w) {
            lines.push(Line::styled(l, t.muted()));
        }
        lines.push(Line::default());
    }
    match (&r.pr, &r.intent.pr) {
        (Some(pr), Some(m)) => {
            lines.push(Line::from(vec![
                Span::styled(format!("PR #{} ", pr.number), t.muted()),
                Span::styled(truncate(&m.title, w.saturating_sub(8)), t.bold()),
            ]));
            let mut about = Vec::new();
            if !m.author.is_empty() {
                about.push(format!("@{}", sanitize(&m.author)));
            }
            about.push(intent::size_text(m.size));
            lines.push(Line::styled(truncate(&about.join(" · "), w), t.muted()));
            if !m.body.trim().is_empty() {
                lines.push(Line::default());
                for l in wrap(m.body.trim(), w) {
                    lines.push(Line::from(with_tickets(app, &l, Style::new())));
                }
            }
        }
        (Some(pr), None) => lines.push(Line::styled(
            truncate(
                &format!(
                    "PR #{}: no description, gh isn't installed or didn't answer.",
                    pr.number
                ),
                w,
            ),
            t.muted(),
        )),
        _ => {}
    }
    lines
}

/// The selected commit: its id, author and date, its message body, the
/// units it changed (the cursor on one of them while `unit` is set) and its
/// files. Also the row of the selected unit.
fn detail_lines(app: &App, n: usize, unit: Option<usize>, w: usize) -> (Vec<Line<'static>>, usize) {
    let t = &app.theme;
    let r = &app.review;
    let Some(c) = r.intent.commits.get(n) else {
        return (Vec::new(), 0);
    };
    let mut lines = Vec::new();
    let mut head = format!("{} · {} · {}", c.sha, sanitize(&c.author), c.date);
    if c.merge {
        head.push_str(" · merge");
    }
    lines.push(Line::styled(truncate(&head, w), t.muted()));
    if !c.body.trim().is_empty() {
        for l in wrap(c.body.trim(), w) {
            lines.push(Line::from(with_tickets(app, &l, Style::new())));
        }
    }
    lines.push(Line::default());
    let units = app.commit_units(n);
    let quiet = r
        .units
        .iter()
        .filter(|u| u.status == UnitStatus::NoBehaviorChange && r.changed_by(u, c))
        .count();
    let heading = match units.len() {
        0 => "Changes no unit in the list.".to_string(),
        1 => "Changes 1 unit:".to_string(),
        k => format!("Changes {k} units:"),
    };
    lines.push(Line::styled(heading, t.accent()));
    let mut at = lines.len();
    for (j, &pos) in units.iter().enumerate() {
        let u = &r.units[app.order[pos]];
        let on = unit == Some(j);
        if on {
            at = lines.len();
        }
        let name_style = match u.decision {
            Decision::Accepted => t.accepted_row(),
            Decision::Investigate => t.flagged_row(),
            Decision::Pending if on => t.bold(),
            Decision::Pending => Style::new(),
        };
        let place = super::slim::place(u);
        let name = truncate(
            &super::slim::row_name(u),
            w.saturating_sub(place.width() + 4).max(8),
        );
        lines.push(Line::from(vec![
            Span::styled(if on { " ▌" } else { "  " }, t.accent()),
            Span::styled(name, name_style),
            Span::styled(format!("  {}", truncate(&sanitize(&place), w)), t.muted()),
        ]));
    }
    if quiet > 0 {
        lines.push(Line::styled(
            format!("  and {quiet} with no behavior change"),
            t.muted(),
        ));
    }
    if !c.files.is_empty() {
        let files: Vec<String> = c
            .files
            .iter()
            .map(|f| sanitize(&f.path.display().to_string()))
            .collect();
        lines.push(Line::styled(
            truncate(&format!("files: {}", files.join(", ")), w),
            t.muted(),
        ));
    }
    (lines, at)
}

pub fn draw_commits(f: &mut Frame, app: &App, sel: usize, unit: Option<usize>) {
    let t = &app.theme;
    let area = f.area();
    let box_w = area.width.saturating_sub(4).min(110);
    // Borders and the padding take 4 columns.
    let w = box_w.saturating_sub(4) as usize;
    let commits = &app.review.intent.commits;
    let mut intro = intro_lines(app, w);
    let (detail, at) = detail_lines(app, sel, unit, w);
    // As tall as its content (borders, the divider and the keys add 4), up
    // to the screen less a row above and below.
    let wanted = intro.len() + commits.len() + detail.len() + 4;
    let box_h = area.height.saturating_sub(2).min(wanted as u16);
    if box_w < 16 || box_h < 6 {
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
    let title = match commits.len() {
        0 => " Pull request ".to_string(),
        1 => " 1 commit ".to_string(),
        n => format!(" {n} commits "),
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.accent())
        .title(Span::styled(title, t.bold()));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let inner = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    let h = inner.height as usize;

    let k = |a: Action| app.keys.label(a);
    let keys = if unit.is_some() {
        format!(
            "{} {} move · ⏎ go to the unit · esc back",
            k(Action::Down),
            k(Action::Up)
        )
    } else if app.commit_units(sel).is_empty() {
        format!("{} {} move · esc close", k(Action::Down), k(Action::Up))
    } else {
        format!(
            "{} {} move · ⏎ its units · esc close",
            k(Action::Down),
            k(Action::Up)
        )
    };
    let rest = h.saturating_sub(1);

    let intro_h = if commits.is_empty() {
        intro.len().min(rest)
    } else {
        intro.len().min((rest / 3).max(3)).min(rest)
    };
    if intro.len() > intro_h && intro_h > 0 {
        intro.truncate(intro_h);
        intro[intro_h - 1] = Line::styled("…", t.muted());
    }
    let after_intro = rest.saturating_sub(intro_h);
    let rows_h = commits.len().min((after_intro / 2).max(3)).min(after_intro);
    let detail_h = after_intro.saturating_sub(rows_h + 1);

    let tag_w = commits
        .iter()
        .map(|c| {
            let tag = intent::tag(c);
            tag.label
                .map_or(0, |l| l.width() + usize::from(tag.breaking))
        })
        .max()
        .unwrap_or(0);
    let start = if sel < rows_h { 0 } else { sel + 1 - rows_h };
    let rows: Vec<Line> = commits
        .iter()
        .enumerate()
        .skip(start)
        .take(rows_h)
        .map(|(i, c)| commit_row(app, c, tag_w, i == sel, w))
        .collect();

    let skip = if unit.is_some() && at >= detail_h {
        at + 1 - detail_h
    } else {
        0
    };
    let detail: Vec<Line> = detail.into_iter().skip(skip).take(detail_h).collect();

    let mut y = inner.y;
    let mut put = |lines: Vec<Line<'static>>, height: usize| {
        if height == 0 {
            return;
        }
        f.render_widget(
            Paragraph::new(lines),
            Rect::new(inner.x, y, inner.width, height as u16),
        );
        y += height as u16;
    };
    put(intro, intro_h);
    put(rows, rows_h);
    if !commits.is_empty() && detail_h > 0 {
        put(vec![Line::styled("─".repeat(w), t.muted())], 1);
        put(detail, detail_h);
    }
    let keys_row = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );
    f.render_widget(
        Paragraph::new(Line::styled(truncate(&keys, w), t.muted())),
        keys_row,
    );
    super::ui::text_area(app, inner, 0);
    super::ui::click_hints(f, app, keys_row, &keys);
}
