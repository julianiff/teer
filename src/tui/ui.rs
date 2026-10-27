//! Drawing: header, footer buttons, and each screen.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Overlay, Screen, Target};
use super::diff::Notes;
use super::select;
use super::slim;
use super::swipe::Dir;
use crate::config::Action;
use crate::git::WORKTREE;
use crate::review::attention::Level;
use crate::util::{sanitize, truncate, wrap};

pub fn draw(f: &mut Frame, app: &App) {
    app.targets.borrow_mut().clear();
    app.text_areas.borrow_mut().clear();
    // Nothing to scroll unless a detail area says so.
    app.detail_max_scroll.set(0);
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_header(f, app, header);
    text_area(app, header, 0);
    text_area(app, footer, 0);
    match app.screen {
        Screen::Slim => draw_slim(f, app, body),
        Screen::Swipe => draw_swipe(f, app, body),
        Screen::Flagged => super::flagged::draw_flagged(f, app, body),
    }
    draw_footer(f, app, footer);
    // Over an overlay a drag selects only what the overlay says it holds.
    if app.overlay != Overlay::None {
        app.text_areas.borrow_mut().clear();
    }
    match &app.overlay {
        Overlay::ConfirmReset => draw_confirm_reset(f, app),
        Overlay::Help => draw_help(f, app),
        Overlay::Note { id, text, .. } => crate::tui::notes::draw_editor(f, app, id, text),
        Overlay::Notes { sel } => crate::tui::notes::draw_view(f, app, *sel),
        Overlay::ConfirmDeleteNote { id, sel } => {
            crate::tui::notes::draw_view(f, app, *sel);
            let name = app.review.unit(id).map(|u| sanitize(&u.symbol)).unwrap_or_default();
            draw_confirm(f, app, format!("Delete the note on {name}? (y/n)"))
        }
        Overlay::ConfirmWeb => draw_confirm(
            f,
            app,
            "Web search sends generic queries (never your code) to external services. Continue? (y/n)"
                .into(),
        ),
        Overlay::ConfirmAcceptFile { file, ids } => {
            let what = sanitize(&file.display().to_string());
            let units = if ids.len() == 1 { "unit" } else { "units" };
            draw_confirm(
                f,
                app,
                format!("Accept {} pending {units} in \"{what}\"? (y/n)", ids.len()),
            )
        }
        Overlay::AllDone => draw_all_done(f, app),
        Overlay::PostReview {
            verdict,
            notes,
            intro,
            inline_on,
            scroll,
            editing,
        } => draw_post_review(
            f,
            app,
            *verdict,
            notes,
            intro,
            *inline_on,
            *scroll,
            editing.as_deref(),
        ),
        Overlay::Commit {
            message,
            draft,
            scroll,
            editing,
            branch,
            others,
        } => draw_commit(
            f,
            app,
            message,
            *draft,
            *scroll,
            editing.as_deref(),
            branch,
            *others,
        ),
        Overlay::Commits { sel, unit } => super::commits::draw_commits(f, app, *sel, *unit),
        Overlay::Ask => super::ask::draw_ask(f, app),
        Overlay::None => {}
    }
    // What is on the screen, to copy from; then the selection marked on it,
    // unless the drag is a swipe.
    app.shown.borrow_mut().clone_from(f.buffer_mut());
    let swiping = app.screen == Screen::Swipe && app.swipe.drag.sideways();
    if let Some(s) = app.selection.filter(|s| s.moved() && !swiping) {
        select::paint(f.buffer_mut(), &s);
    }
}

fn target(app: &App, r: Rect, t: Target) {
    app.targets.borrow_mut().push((r, t));
}

/// Let a mouse selection run in `r`, leaving out `gutter` columns at its
/// left (see [`select::Area`]).
pub(super) fn text_area(app: &App, r: Rect, gutter: u16) {
    app.text_areas
        .borrow_mut()
        .push(select::Area::new(r, gutter));
}

/// The key a hint names: `⏎`, `esc`, `^d`, `ctrl+r`, or `y` for `y/⏎`.
fn key_of(label: &str) -> Option<KeyEvent> {
    let first = label.split('/').next()?;
    let code = |c| Some(KeyEvent::new(c, KeyModifiers::NONE));
    match first {
        "⏎" => code(KeyCode::Enter),
        "↑" => code(KeyCode::Up),
        "↓" => code(KeyCode::Down),
        "←" => code(KeyCode::Left),
        "→" => code(KeyCode::Right),
        _ => match first.strip_prefix('^') {
            Some(c) if !c.is_empty() => crate::config::parse_key(&format!("ctrl+{c}")),
            _ => crate::config::parse_key(first),
        },
    }
}

/// Make each key hint of `hints` (`key what · key what`, as drawn inside
/// `area`) a click target pressing its key. Hints that name two keys
/// (`j k move`) or none (`any other key`) are left alone.
pub(super) fn click_hints(f: &mut Frame, app: &App, area: Rect, hints: &str) {
    for hint in hints.split('·').map(str::trim) {
        let Some((key, what)) = hint.split_once(' ') else {
            continue;
        };
        let Some(k) = key_of(key) else { continue };
        let first = what.split(' ').next().unwrap_or_default();
        if first.chars().count() <= 2 && key_of(first).is_some() {
            continue;
        }
        if let Some(r) = select::find(f.buffer_mut(), area, hint) {
            target(app, r, Target::Key(k));
        }
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let r = &app.review;
    // What the change is called, nothing else: the commits view (`C`) has
    // the PR's number, author, size and description.
    let short = |s: &str| s.chars().take(7).collect::<String>();
    let title = if r.head == WORKTREE {
        "Uncommitted changes".to_string()
    } else {
        crate::review::intent::headline(r)
            .0
            .map(|s| sanitize(&s))
            .unwrap_or_else(|| format!("{}..{}", short(&r.base), short(&r.head)))
    };
    let mut right = Vec::new();
    let (done, total) = app.prep_progress();
    if done < total {
        right.push(Span::styled(
            format!("{} preparing {done}/{total}  ", app.spinner()),
            t.muted(),
        ));
    }
    let flagged = app.flagged_units().len();
    if flagged > 0 {
        let text = if app.screen == Screen::Flagged {
            format!("Flagged ({flagged})  ")
        } else {
            format!("{flagged} flagged  ")
        };
        right.push(Span::styled(text, t.badge_investigate()));
    }
    // The status on the right is never cut and keeps a gap before it.
    let rw: usize = right.iter().map(|s| s.content.width()).sum();
    let budget = (area.width as usize).saturating_sub(if rw > 0 { rw + 2 } else { 0 });
    let title = truncate(&format!(" {title}"), budget);
    let pad = (area.width as usize).saturating_sub(title.width() + rw);
    let mut line = vec![Span::styled(title, t.bold()), Span::raw(" ".repeat(pad))];
    line.extend(right);
    f.render_widget(Paragraph::new(Line::from(line)), area);
}

/// Footer buttons, highest priority first (the footer drops from the end).
/// `R` reset and `l` accept are deliberately absent: they work everywhere
/// but are listed only in the help overlay.
fn buttons_for(app: &App) -> Vec<(Target, String, &'static str)> {
    let k = |a: Action| app.keys.label(a);
    let b = |a: Action, label: &'static str| (Target::Action(a), k(a), label);
    let mut v = match app.screen {
        Screen::Slim => {
            let mut v = vec![
                b(Action::Edit, "open in editor"),
                b(Action::Swipe, "swipe"),
                b(Action::Help, "help"),
            ];
            if !app.flagged_units().is_empty() {
                v.push(b(Action::Flagged, "flagged"));
            }
            v.push(b(Action::Investigate, "investigate"));
            v.push(b(Action::Ask, "ask"));
            v
        }
        // The tiles name the keys that ask, so the footer doesn't repeat them.
        Screen::Flagged => vec![
            b(Action::ResolveOk, "ok"),
            b(Action::ResolveFix, "needs fix"),
            (
                Target::Action(Action::NextTile),
                format!("{} {}", k(Action::PrevTile), k(Action::NextTile)),
                "tile",
            ),
            b(
                Action::Zoom,
                if app.flagged.zoom { "unzoom" } else { "zoom" },
            ),
            b(Action::DiveRerun, "ask again"),
            b(Action::Ask, "chat"),
            b(Action::OpenVisual, "browser"),
            b(Action::Back, "back"),
            b(Action::Help, "help"),
        ],
        Screen::Swipe if app.swipe_current().is_none() => vec![
            b(Action::PrevCard, "previous"),
            b(Action::Undo, "undo"),
            b(Action::Back, "back"),
        ],
        Screen::Swipe => vec![
            b(Action::Accept, "accept · next"),
            b(Action::PrevCard, "previous"),
            (
                Target::Action(Action::Investigate),
                format!("← {}", k(Action::Investigate)),
                "investigate",
            ),
            b(Action::Undo, "undo"),
            b(Action::Back, "back"),
            // After the way back: when narrow, it is dropped first, and a
            // cut box still offers it in its last row.
            b(Action::Edit, "open in editor"),
        ],
    };
    // Only while the detail area is cut (it was drawn just before).
    if app.detail_max_scroll.get() > 0 {
        v.push((
            Target::Action(Action::PreviewDown),
            format!("{} {}", k(Action::PreviewDown), k(Action::PreviewUp)),
            "scroll",
        ));
    }
    // The notes last: when narrow, they are dropped first.
    let on_unit = match app.screen {
        Screen::Slim => app.selected().is_some(),
        Screen::Swipe => app.swipe_current().is_some(),
        Screen::Flagged => false,
    };
    if on_unit {
        v.push(b(Action::EditNote, "note"));
    }
    if app.screen == Screen::Slim && !crate::notes::noted(&app.review).is_empty() {
        v.push(b(Action::Notes, "notes"));
    }
    v
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let buttons = buttons_for(app);
    let keys: Vec<(String, &str)> = buttons.iter().map(|(_, k, l)| (k.clone(), *l)).collect();
    let (line, widths) = footer_line(&app.theme, &keys, &app.status, area.width as usize);
    let mut x = area.x;
    for ((tgt, ..), w) in buttons.into_iter().zip(widths) {
        target(app, Rect::new(x, area.y, w as u16, 1), tgt);
        x += w as u16 + 1;
    }
    f.render_widget(Paragraph::new(line), area);
}

/// A footer `w` cells wide: the buttons (`key label`) that fit, left to
/// right, then `status`, muted, at the right edge. Also the cells each
/// button shown takes, without the blank after it.
pub(super) fn footer_line(
    t: &super::theme::Theme,
    buttons: &[(String, &str)],
    status: &str,
    w: usize,
) -> (Line<'static>, Vec<usize>) {
    let mut spans = Vec::new();
    let mut widths = Vec::new();
    let mut used = 0;
    for (key, label) in buttons {
        let bw = key.width() + label.width() + 3;
        if used + bw > w {
            break;
        }
        spans.push(Span::styled(
            format!(" {key}"),
            t.button().patch(t.accent()),
        ));
        spans.push(Span::styled(format!(" {label} "), t.button()));
        spans.push(Span::raw(" "));
        widths.push(bw);
        used += bw + 1;
    }
    let status = truncate(status, w.saturating_sub(used + 1));
    spans.push(Span::raw(
        " ".repeat(w.saturating_sub(used + status.width())),
    ));
    spans.push(Span::styled(status, t.muted()));
    (Line::from(spans), widths)
}

fn pad(r: Rect, x: u16, y: u16) -> Rect {
    Rect::new(
        r.x + x.min(r.width),
        r.y + y.min(r.height),
        r.width.saturating_sub(2 * x),
        r.height.saturating_sub(y),
    )
}

fn draw_slim(f: &mut Frame, app: &App, area: Rect) {
    // Say once, above the review, why there are no behavior statements.
    let area = match app.agent_note.as_deref() {
        Some(note) if !app.has_agent && note != crate::cli::NO_AGENT && area.height > 2 => {
            let [banner, rest] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
            f.render_widget(
                Paragraph::new(Line::styled(
                    truncate(&format!(" {note}"), banner.width as usize),
                    Style::new().fg(app.theme.risk_medium),
                )),
                banner,
            );
            rest
        }
        _ => area,
    };
    if app.order.is_empty() {
        text_area(app, area, 0);
        // Nothing to decide; the muted rows still say what changed.
        let rows = slim::build(app, area.width);
        if !rows.lines.is_empty() {
            f.render_widget(Paragraph::new(rows.lines), area);
            return;
        }
        let msg = "Nothing to review in these changes.";
        f.render_widget(
            Paragraph::new(Line::styled(msg, app.theme.muted())).wrap(Wrap { trim: true }),
            pad(area, 1, 1),
        );
        return;
    }
    let (list, pane) = crate::tui::preview::layout_of(app, area);
    let h = list.height as usize;
    // The blank lines between boxes and files cost rows: only when the list
    // has room for them.
    let rows = slim::build_with(app, list.width, h >= slim::PADDING_MIN_ROWS);
    let (s, e) = rows.spans.get(app.sel).copied().unwrap_or((0, 0));
    let title = rows.titles.get(app.sel).copied().unwrap_or(s);
    let head = rows.heads.get(app.sel).copied().unwrap_or(s);
    let end = rows.ends.get(app.sel).copied().unwrap_or(e);
    let mut top = app.slim_scroll.get();
    // Keep the selected unit in view, with its box title above it when the
    // two fit together, else with its file's name.
    if s < top {
        top = s;
    }
    if e > top + h {
        top = if e - s >= h { s } else { e - h };
    }
    if top > title && e <= title + h {
        top = title;
    } else if top > head && e <= head + h {
        top = head;
    }
    // Below the last unit of a box, its bottom edge and the blank after it,
    // when they fit with the unit.
    if end > top + h && end - s <= h {
        top = end - h;
    }
    top = top.min(rows.lines.len().saturating_sub(1));
    app.slim_scroll.set(top);
    let visible: Vec<Line> = rows.lines.iter().skip(top).take(h).cloned().collect();
    for (k, owner) in rows.owner.iter().skip(top).take(h).enumerate() {
        if let Some(pos) = owner {
            target(
                app,
                Rect::new(list.x, list.y + k as u16, list.width, 1),
                Target::Unit(*pos),
            );
        }
    }
    f.render_widget(Paragraph::new(visible), list);
    // Inside the boxes' borders.
    text_area(app, pad(list, 1, 0), 0);
    if let Some((rect, kind)) = pane {
        crate::tui::preview::draw_preview(f, app, rect, kind);
    }
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

/// Cells between the swipe card's border and its text, on each side.
const CARD_PAD: u16 = 2;

fn draw_swipe(f: &mut Frame, app: &App, area: Rect) {
    use crate::review::model::Decision;
    let t = &app.theme;
    let Some(id) = app.swipe_current() else {
        draw_swipe_summary(f, app, area);
        return;
    };
    let Some(u) = app.review.unit(id) else { return };
    let a = app.assessment(&u.id).cloned();
    let level = a.as_ref().map(|a| a.level).unwrap_or(Level::Routine);
    // A card with code is wider, so the lines aren't cut as soon, but keeps
    // a few cells on each side to follow a drag.
    let code = crate::tui::diff::shows(app, u);
    let card_w = if code {
        100.min(area.width.saturating_sub(8)).max(76)
    } else {
        76
    }
    .min(area.width);
    // Text width inside the borders and the padding.
    let w = card_w.saturating_sub(2 + 2 * CARD_PAD) as usize;
    let title = format!("{} {}", level.icon(), level.title());
    // A symbol says which file it is in; a whole file is named already.
    let mut name = vec![Span::styled(slim::row_name(u), t.bold())];
    if u.kind != crate::review::model::UnitKind::File {
        name.push(Span::styled(
            format!("  {}", slim::file_name(&u.file)),
            t.muted(),
        ));
    }
    let mut lines = vec![
        Line::styled(
            title,
            slim::level_style(app, level).add_modifier(Modifier::BOLD),
        ),
        Line::from(name),
    ];
    // The commits that changed it, by subject.
    let subjects: Vec<&str> = app
        .review
        .commits_of(u)
        .iter()
        .map(|c| c.subject.as_str())
        .collect();
    if !subjects.is_empty() {
        lines.push(Line::styled(truncate(&subjects.join(" · "), w), t.muted()));
    }
    let checks = crate::tui::preview::check_lines(app, u, w, 0, false);
    if !checks.is_empty() {
        lines.push(Line::default());
        lines.extend(checks);
    }
    lines.push(Line::default());
    // What the agent wrote sits behind a bar. A routine card says it in one
    // sentence; the others write their statements into the code change.
    let notes = if level == Level::Routine {
        Notes::Off
    } else {
        Notes::Inline
    };
    let boxed = code && crate::tui::diff::box_rows(app, u, w as u16, notes) > 0;
    let said = if level == Level::Routine {
        let mut said = u.clone();
        if said.summary.is_none() {
            said.summary = u.changes.first().map(|c| c.statement.clone());
        }
        crate::tui::preview::agent_lines(app, &said, w.saturating_sub(2), false, 0)
    } else {
        crate::tui::preview::agent_lines(app, u, w.saturating_sub(2), !boxed, 0)
    };
    if said.is_empty() {
        let note = slim::prep_note(app, u).unwrap_or_default();
        lines.extend(
            wrap(&note, w)
                .into_iter()
                .map(|l| Line::styled(l, t.muted())),
        );
    }
    lines.extend(said);
    // The reviewer's note, here when there is no code box to put it under.
    let card = if boxed {
        Vec::new()
    } else {
        crate::tui::notes::card(app, u, w)
    };
    if !card.is_empty() {
        lines.push(Line::default());
        lines.extend(card);
    }
    // Answers to questions asked about it, so they are seen.
    let asked = crate::tui::ask::note_lines(app, &u.id, w);
    if !asked.is_empty() {
        lines.push(Line::default());
        lines.extend(asked);
    }
    // The tallest card: two rows under it for the accept button and air.
    let max_h = area.height.saturating_sub(2).max(8);
    let code_rows = if boxed {
        // A blank line, then the box, and the note under it.
        1 + crate::tui::diff::box_rows(app, u, w as u16, notes)
            + crate::tui::notes::card(app, u, w).len()
    } else {
        0
    };
    let total = lines.len() + code_rows;
    // The card grows with how much there is to judge, and with its code;
    // past the terminal's height its code scrolls.
    let floor = match level {
        Level::Attention => 26,
        Level::Look => 14,
        _ => 10,
    };
    let wanted = if code || level == Level::Attention {
        // Borders and the state line with a blank row above it.
        floor.max((total + 4).min(u16::MAX as usize) as u16)
    } else {
        floor
    };
    let base = centered(area, card_w, wanted.min(max_h));
    app.swipe_card.set(base);
    // Follow the finger while swiping.
    let drag = app.swipe.drag;
    let drag = if drag.sideways() {
        drag
    } else {
        Default::default()
    };
    let dx = drag.dx.clamp(
        -(base.x as i32),
        (area.width as i32) - (base.x + base.width) as i32,
    );
    let dy = drag.dy.clamp(
        -((base.y - area.y) as i32),
        (area.y + area.height) as i32 - (base.y + base.height) as i32,
    );
    let card = Rect::new(
        (base.x as i32 + dx) as u16,
        (base.y as i32 + dy) as u16,
        base.width,
        base.height,
    );
    let tint = match drag.preview(base) {
        Some(Dir::Right) => Style::new().fg(t.added),
        Some(Dir::Left) => Style::new().fg(t.removed),
        None if drag.dragging() && drag.dx > 0 => {
            Style::new().fg(t.added).add_modifier(Modifier::DIM)
        }
        None if drag.dragging() && drag.dx < 0 => {
            Style::new().fg(t.removed).add_modifier(Modifier::DIM)
        }
        None => t.muted(),
    };
    let total = app.swipe.cards.len();
    let (_, flagged, _) = app.decision_counts();
    let counter = format!(" {} of {} ", app.swipe.cursor + 1, total);
    let label = match drag.preview(base) {
        Some(Dir::Right) => " accept ",
        Some(Dir::Left) => " investigate ",
        None => "",
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(tint)
        .padding(ratatui::widgets::Padding::horizontal(CARD_PAD))
        .title(Span::styled(counter, t.muted()))
        .title_bottom(Line::styled(label, tint.add_modifier(Modifier::BOLD)).right_aligned());
    f.render_widget(Clear, card);
    let inner = block.inner(card);
    f.render_widget(block, card);

    // Where this card stands, so going back shows what was decided.
    let (state, state_style) = match u.decision {
        Decision::Accepted => ("✓ accepted", t.ok()),
        Decision::Investigate => ("⚑ flagged", t.bad()),
        Decision::Pending => ("not reviewed yet", t.muted()),
    };
    let mut foot_line = vec![Span::styled(state, state_style)];
    if flagged > 0 {
        foot_line.push(Span::styled(format!("  ·  {flagged} flagged"), t.muted()));
    }
    let [content, _, foot] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    text_area(app, inner, 0);
    let h = content.height as usize;
    let min_box = crate::tui::preview::MIN_BOX_ROWS;
    if boxed && h > min_box {
        // The head stays put; the code change takes the rest and scrolls.
        let head_h = lines.len().min(h - min_box);
        let air = u16::from(h >= lines.len() + 1 + min_box);
        let [top, _, code_area] = Layout::vertical([
            Constraint::Length(head_h as u16),
            Constraint::Length(air),
            Constraint::Min(0),
        ])
        .areas(content);
        lines.truncate(head_h);
        f.render_widget(Paragraph::new(lines), top);
        crate::tui::notes::draw_box(f, app, code_area, u, notes);
    } else {
        let blocks = lines.into_iter().map(|l| vec![l]).collect();
        let lines = crate::tui::preview::scrolled(app, blocks, w, h, 0, false);
        f.render_widget(Paragraph::new(lines), content);
    }
    f.render_widget(Paragraph::new(Line::from(foot_line)), foot);

    // The accept button sits under the card's bottom-right corner, outside
    // the card so a click on it isn't taken for the start of a drag.
    let key = app.keys.label(Action::Accept);
    let text = format!(" accept [{key}] → ");
    let bw = text.width() as u16;
    let by = base.y + base.height;
    if by < area.y + area.height && bw <= base.width {
        let r = Rect::new(base.x + base.width - bw, by, bw, 1);
        f.render_widget(Paragraph::new(Span::styled(text, t.accept_button())), r);
        target(app, r, Target::Action(Action::Accept));
    }
}

fn draw_swipe_summary(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let (acc, flagged, pending) = app.decision_counts();
    let mut lines = vec![
        Line::styled("All changes triaged", t.bold()),
        Line::default(),
        Line::from(vec![
            Span::styled(format!("{acc}"), t.ok()),
            Span::raw(" accepted"),
        ]),
        Line::from(vec![
            Span::styled(format!("{flagged}"), t.bad()),
            Span::raw(" flagged"),
        ]),
    ];
    if flagged > 0 {
        let last = lines.last_mut().unwrap();
        last.spans.push(Span::raw(" — press "));
        last.spans
            .push(Span::styled(app.keys.label(Action::Flagged), t.accent()));
        last.spans.push(Span::raw(" to see them"));
    }
    if pending > 0 {
        lines.push(Line::from(vec![
            Span::styled(format!("{pending}"), t.bold()),
            Span::raw(" not reviewed"),
        ]));
    }
    lines.push(Line::default());
    if app.swipe.cards.is_empty() {
        lines.push(Line::styled("Nothing needs a look.", t.muted()));
    } else {
        lines.push(Line::from(vec![
            Span::styled(app.keys.label(Action::PrevCard), t.accent()),
            Span::styled(" back to the last card", t.muted()),
        ]));
    }
    let box_ = centered(area, 56, lines.len() as u16 + 4);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(t.muted());
    let inner = pad(block.inner(box_), 2, 1);
    f.render_widget(block, box_);
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_confirm_reset(f: &mut Frame, app: &App) {
    let t = &app.theme;
    let lines = vec![
        Line::styled("Reset the review?", t.bold()),
        Line::styled(
            "Clears every decision, as if nothing was looked at.",
            t.muted(),
        ),
        Line::default(),
        Line::from(vec![
            Span::styled("enter", t.accent()),
            Span::raw(" reset · "),
            Span::styled("esc", t.accent()),
            Span::raw(" cancel"),
        ]),
    ];
    let rows = lines.len() as u16;
    let inner = modal(f, app, Paragraph::new(lines), 60, rows, t.accent());
    click_hints(f, app, inner, "enter reset · esc cancel");
}

/// `p` in a box `w` wide and `rows` high inside its border and padding,
/// centred on a cleared area, its rounded border in `border`; a mouse
/// selection runs inside it. Where its text is.
fn modal(f: &mut Frame, app: &App, p: Paragraph, w: u16, rows: u16, border: Style) -> Rect {
    let r = centered(f.area(), w, rows + 4);
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(border)
        .padding(ratatui::widgets::Padding::new(2, 2, 1, 1));
    let inner = block.inner(r);
    f.render_widget(p.block(block), r);
    text_area(app, inner, 0);
    inner
}

fn draw_all_done(f: &mut Frame, app: &App) {
    let t = &app.theme;
    let ok = t.ok();
    let k = |a: Action| app.keys.label(a);
    if let Some(short) = &app.committed {
        let lines = vec![
            Line::styled(format!("Committed {}.", sanitize(short)), ok).centered(),
            Line::default(),
            Line::from(vec![
                Span::styled(k(Action::Quit), t.accent()),
                Span::raw(" quit  ·  any other key to continue"),
            ])
            .centered(),
        ];
        let rows = lines.len() as u16;
        let inner = modal(f, app, Paragraph::new(lines), 58, rows, ok);
        click_hints(f, app, inner, &format!("{} quit", k(Action::Quit)));
        return;
    }
    let mut lines = vec![Line::styled("Hurray! All changes reviewed.", ok).centered()];
    // Never say that without the files teer couldn't show.
    let name = |f: &crate::git::SkippedFile| {
        f.path
            .file_name()
            .map(|n| sanitize(&n.to_string_lossy()))
            .unwrap_or_default()
    };
    if let Some(note) = app.review.not_shown(name) {
        for l in wrap(&note, 50).into_iter().take(3) {
            lines.push(Line::styled(l, Style::new().fg(t.risk_medium)).centered());
        }
    }
    lines.extend([
        Line::default(),
        Line::from(vec![
            Span::styled(k(Action::Quit), t.accent()),
            Span::raw(" quit  ·  "),
            Span::styled(k(Action::Reset), t.accent()),
            Span::raw(" reset  ·  any other key to continue"),
        ])
        .centered(),
    ]);
    if let Some(pr) = &app.review.pr {
        lines.push(Line::default());
        lines.push(if pr.is_approved() {
            Line::styled(format!("PR #{} approved", pr.number), ok).centered()
        } else {
            Line::from(vec![
                Span::styled(k(Action::ApprovePr), t.accent()),
                Span::raw(format!(" approve PR #{} on GitHub", pr.number)),
            ])
            .centered()
        });
    }
    if app.can_commit() {
        lines.push(Line::default());
        lines.push(
            Line::from(vec![
                Span::styled(k(Action::Commit), t.accent()),
                Span::raw(" commit these changes"),
            ])
            .centered(),
        );
    }
    let rows = lines.len() as u16;
    let inner = modal(f, app, Paragraph::new(lines), 58, rows, ok);
    let mut hints = vec![
        format!("{} quit", k(Action::Quit)),
        format!("{} reset", k(Action::Reset)),
        format!("{} commit these changes", k(Action::Commit)),
    ];
    if let Some(pr) = &app.review.pr {
        hints.push(format!(
            "{} approve PR #{} on GitHub",
            k(Action::ApprovePr),
            pr.number
        ));
    }
    click_hints(f, app, inner, &hints.join(" · "));
}

fn draw_help(f: &mut Frame, app: &App) {
    let t = &app.theme;
    let k = |a: Action| app.keys.label(a);
    let rows: Vec<(String, &str)> = vec![
        (
            format!(
                "{} {} {} {}",
                k(Action::Down),
                k(Action::Up),
                k(Action::Top),
                k(Action::Bottom)
            ),
            "next / previous, first / last unit",
        ),
        (
            format!("{} {}", k(Action::PreviewDown), k(Action::PreviewUp)),
            "scroll the preview, card or answer",
        ),
        (
            format!(
                "{} {} {}",
                k(Action::Commits),
                k(Action::EditNote),
                k(Action::Notes)
            ),
            "commits / note on the unit / all notes",
        ),
        (k(Action::Edit), "open in the editor"),
        (k(Action::Swipe), "swipe mode"),
        (k(Action::Rerun), "re-run review on new changes"),
        (k(Action::Reset), "reset all decisions"),
        (
            format!("{} {}", k(Action::Accept), k(Action::Investigate),),
            "accept / investigate",
        ),
        (k(Action::PrevCard), "swipe: previous card"),
        (
            format!("{} {}", k(Action::Undo), k(Action::Flagged)),
            "undo / show flagged units",
        ),
        (
            format!("{} {}", k(Action::NextPending), k(Action::PrevPending)),
            "next / previous pending",
        ),
        (
            format!("{} {}", k(Action::NextGroup), k(Action::PrevGroup)),
            "next / previous file",
        ),
        (k(Action::AcceptFile), "accept all pending in the file"),
        (
            format!("{} {}", k(Action::Ask), k(Action::Quit)),
            "ask about the code (flagged: chat) / quit",
        ),
    ];
    let flagged_rows: Vec<(String, &str)> = vec![
        (
            format!(
                "{} {} {} {}",
                k(Action::DiveRecommend),
                k(Action::DiveVisualize),
                k(Action::DiveTests),
                k(Action::InterpretTests)
            ),
            "recommend / visualize / tests, agent reads them",
        ),
        (
            format!("{} {}", k(Action::DiveBehavior), k(Action::DiveDisagree)),
            "run before and after / disagreement",
        ),
        (
            format!(
                "{} {} {}",
                k(Action::PrevTile),
                k(Action::NextTile),
                k(Action::Zoom)
            ),
            "previous / next tile, zoom",
        ),
        (
            format!(
                "{} {} {} {}",
                k(Action::DiveRerun),
                k(Action::ResolveOk),
                k(Action::ResolveFix),
                k(Action::OpenVisual)
            ),
            "again, ok / needs fix, browser",
        ),
    ];
    let key_line = |(key, what): (String, &str)| {
        Line::from(vec![
            Span::styled(format!("{key:>10}  "), t.accent()),
            Span::raw(what.to_string()),
        ])
    };
    let mut lines: Vec<Line> = rows.into_iter().map(key_line).collect();
    // The Flagged-screen keys only when the terminal has room for them: a
    // blank and the heading, their rows, then the blank, three legend rows
    // and the border that follow.
    if f.area().height as usize >= lines.len() + flagged_rows.len() + 8 {
        lines.push(Line::default());
        lines.push(Line::styled(format!("{:>10}  Flagged", ""), t.bold()));
        lines.extend(flagged_rows.into_iter().map(key_line));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled(format!("{:>10}  ", "legend"), t.muted()),
        Span::styled("accepted", t.accepted_row()),
        Span::raw("  "),
        Span::styled("removed", Style::new().add_modifier(Modifier::CROSSED_OUT)),
        Span::raw("  "),
        Span::styled("flagged", t.flagged_row()),
    ]));
    let level = |l: Level, what: &'static str| {
        [
            Span::styled(l.icon(), slim::level_style(app, l)),
            Span::raw(format!(" {what}  ")),
        ]
    };
    lines.push(Line::from(
        [Span::raw(" ".repeat(12))]
            .into_iter()
            .chain(level(Level::Attention, "close look"))
            .chain(level(Level::Look, "worth a look"))
            .chain(level(Level::Routine, "routine"))
            .collect::<Vec<_>>(),
    ));
    lines.push(Line::from(vec![
        Span::raw(" ".repeat(12)),
        Span::styled("●", t.ok()),
        Span::raw(" ok  "),
        Span::styled("✗", t.needs_fix()),
        Span::raw(" needs fix"),
    ]));
    let r = centered(f.area(), 56, lines.len() as u16 + 2);
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(t.muted())
        .title(" keys ");
    text_area(app, block.inner(r), 0);
    f.render_widget(Paragraph::new(lines).block(block), r);
    // Any key closes it, and so does a click.
    target(
        app,
        f.area(),
        Target::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );
}

/// A yes/no question, wrapped to at most three rows.
fn draw_confirm(f: &mut Frame, app: &App, text: String) {
    let t = &app.theme;
    let p = Paragraph::new(Line::styled(text, t.bold())).wrap(Wrap { trim: true });
    let inner = modal(f, app, p, 60, 3, t.accent());
    if let Some(r) = select::find(f.buffer_mut(), inner, "(y/n)") {
        for (dx, c) in [(1, 'y'), (3, 'n')] {
            let k = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
            target(app, Rect::new(r.x + dx, r.y, 1, 1), Target::Key(k));
        }
    }
}

/// The post confirmation: question, summary, inline comments (scrollable,
/// clamped to the terminal), and the keys.
#[allow(clippy::too_many_arguments)]
fn draw_post_review(
    f: &mut Frame,
    app: &App,
    verdict: crate::approve::Verdict,
    notes: &[crate::approve::NoteEntry],
    intro: &str,
    inline_on: bool,
    scroll: usize,
    editing: Option<&str>,
) {
    use ratatui::widgets::Padding;
    let t = &app.theme;
    let area = f.area();
    let box_w = area.width.saturating_sub(2).clamp(0, 100);
    // Borders and padding take 4 columns.
    let w = box_w.saturating_sub(4) as usize;
    if box_w < 12 || area.height < 5 {
        f.render_widget(Clear, centered(area, box_w, area.height));
        return;
    }
    let Some(pr) = app.review.pr.as_ref() else {
        return;
    };
    let plan = crate::approve::build_plan(notes, intro, inline_on);
    let n_inline = plan.inline.len();

    let title = format!(
        "{} ({}/{}) on GitHub?  ·  {} inline comment{}",
        verdict.ask(pr.number),
        sanitize(&pr.owner),
        sanitize(&pr.repo),
        n_inline,
        if n_inline == 1 { "" } else { "s" }
    );
    let head: Vec<Line> = wrap(&title, w)
        .into_iter()
        .take(2)
        .map(|l| Line::styled(l, t.bold()))
        .collect();

    let keys: Vec<(&str, &str)> = if editing.is_some() {
        vec![("enter", "save"), ("esc", "back")]
    } else {
        let verdicts = app.verdicts();
        let next = verdicts
            .iter()
            .position(|v| *v == verdict)
            .map_or(verdicts[0], |i| verdicts[(i + 1) % verdicts.len()]);
        let mut keys = vec![
            ("y/⏎", "send"),
            ("e", "edit comment"),
            ("i", if inline_on { "inline off" } else { "inline on" }),
            ("↑↓", "scroll"),
            ("esc", "cancel"),
        ];
        if verdicts.len() > 1 {
            keys.insert(1, ("v", next.label()));
        }
        keys
    };
    let plain = keys
        .iter()
        .map(|(k, l)| format!("{k} {l}"))
        .collect::<Vec<_>>()
        .join(" · ");
    let foot_h = wrap(&plain, w).len().clamp(1, 2);
    let mut spans: Vec<Span> = Vec::new();
    for (i, (k, l)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" · "));
        }
        spans.push(Span::styled(*k, t.accent()));
        spans.push(Span::raw(format!(" {l}")));
    }
    let foot = Paragraph::new(Line::from(spans)).wrap(Wrap { trim: true });

    let head_h = head.len() as u16;
    let mut body: Vec<Line> = Vec::new();
    if let Some(text) = editing {
        body.push(Line::styled("Edit the summary (one line):", t.bold()));
        let clean = sanitize(text);
        let mut lines = wrap(&clean, w.saturating_sub(1));
        if lines.is_empty() {
            lines.push(String::new());
        }
        let last = lines.len() - 1;
        for (i, l) in lines.into_iter().enumerate() {
            let mut sp = vec![Span::raw(l)];
            if i == last {
                sp.push(Span::styled(
                    " ",
                    Style::new().add_modifier(Modifier::REVERSED),
                ));
            }
            body.push(Line::from(sp));
        }
    } else {
        body.push(Line::styled("Summary comment:", t.accent()));
        for l in wrap(&plan.body, w.saturating_sub(2)) {
            body.push(Line::from(format!("  {l}")));
        }
        body.push(Line::default());
        if plan.inline.is_empty() {
            body.push(Line::styled(
                if inline_on {
                    "No inline comments."
                } else {
                    "Inline comments are off."
                },
                t.muted(),
            ));
        } else {
            body.push(Line::styled("Inline comments:", t.accent()));
            for c in &plan.inline {
                let side = if c.side == crate::review::model::Side::Old {
                    " (old)"
                } else {
                    ""
                };
                let text = format!("{}:{}{side}  {}", sanitize(&c.path), c.line, c.body);
                for (i, l) in wrap(&text, w.saturating_sub(2)).into_iter().enumerate() {
                    body.push(Line::from(format!(
                        "{}{l}",
                        if i == 0 { "  " } else { "    " }
                    )));
                }
            }
        }
    }
    // The box fits its content, up to 26 rows, and never exceeds the screen.
    let want = (head_h as usize + body.len() + foot_h + 2).clamp(5, 26) as u16;
    let r = centered(area, box_w, want.min(area.height));
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(t.accent())
        .padding(Padding::new(1, 1, 0, 0));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let rows = inner.height.saturating_sub(head_h + foot_h as u16) as usize;
    let max = body.len().saturating_sub(rows);
    app.approve_max_scroll.set(max);
    let start = scroll.min(max);
    let shown: Vec<Line> = body.into_iter().skip(start).take(rows).collect();
    let mut y = inner.y;
    f.render_widget(
        Paragraph::new(head),
        Rect::new(inner.x, y, inner.width, head_h),
    );
    y += head_h;
    f.render_widget(
        Paragraph::new(shown),
        Rect::new(inner.x, y, inner.width, rows as u16),
    );
    y += rows as u16;
    let foot_area = Rect::new(inner.x, y, inner.width, foot_h as u16);
    f.render_widget(foot, foot_area);
    text_area(app, inner, 0);
    click_hints(f, app, foot_area, &plain);
}

/// Most file rows listed in the commit overlay before "and N more".
const COMMIT_FILES_SHOWN: usize = 8;

/// The commit confirmation: question, files, the note about other changes,
/// the message (scrollable, clamped to the terminal), and the keys.
#[allow(clippy::too_many_arguments)]
fn draw_commit(
    f: &mut Frame,
    app: &App,
    message: &str,
    draft: super::app::DraftState,
    scroll: usize,
    editing: Option<&str>,
    branch: &crate::commit::Branch,
    others: usize,
) {
    use super::app::DraftState;
    use crate::commit::Branch;
    use ratatui::widgets::Padding;
    let t = &app.theme;
    let area = f.area();
    let box_w = area.width.saturating_sub(2).clamp(0, 100);
    let w = box_w.saturating_sub(4) as usize;
    if box_w < 12 || area.height < 5 {
        f.render_widget(Clear, centered(area, box_w, area.height));
        return;
    }
    let rows = crate::commit::file_rows(&app.review);
    let n = rows.len();
    let plural = if n == 1 { "" } else { "s" };
    let title = match branch {
        Branch::Named(b) => format!("Commit {n} file{plural} on {}?", sanitize(b)),
        Branch::Detached => format!("Commit {n} file{plural} on detached HEAD?"),
        Branch::Unknown => format!("Commit {n} file{plural}?"),
    };
    let head: Vec<Line> = wrap(&title, w)
        .into_iter()
        .take(2)
        .map(|l| Line::styled(l, t.bold()))
        .collect();

    let keys: Vec<(&str, &str)> = if editing.is_some() {
        vec![("enter", "save"), ("esc", "back")]
    } else {
        vec![
            ("y/⏎", "commit"),
            ("e", "edit message"),
            ("↑↓", "scroll"),
            ("esc", "cancel"),
        ]
    };
    let plain = keys
        .iter()
        .map(|(k, l)| format!("{k} {l}"))
        .collect::<Vec<_>>()
        .join(" · ");
    let foot_h = wrap(&plain, w).len().clamp(1, 2);
    let mut spans: Vec<Span> = Vec::new();
    for (i, (k, l)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" · "));
        }
        spans.push(Span::styled(*k, t.accent()));
        spans.push(Span::raw(format!(" {l}")));
    }
    let foot = Paragraph::new(Line::from(spans)).wrap(Wrap { trim: true });

    let head_h = head.len() as u16;
    let mut body: Vec<Line> = Vec::new();
    body.push(Line::styled("Files:", t.accent()));
    for (sym, path) in rows.iter().take(COMMIT_FILES_SHOWN) {
        let color = match sym {
            '+' => t.added,
            '−' => t.removed,
            _ => t.changed,
        };
        let shown = truncate(path, w.saturating_sub(4));
        body.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(sym.to_string(), Style::new().fg(color)),
            Span::raw(format!(" {shown}")),
        ]));
    }
    if n > COMMIT_FILES_SHOWN {
        body.push(Line::styled(
            format!("  … and {} more", n - COMMIT_FILES_SHOWN),
            t.muted(),
        ));
    }
    if others > 0 {
        let text = if others == 1 {
            "1 other changed file is not part of this review and will not be included".to_string()
        } else {
            format!(
                "{others} other changed files are not part of this review and will not be included"
            )
        };
        body.push(Line::default());
        for l in wrap(&text, w) {
            body.push(Line::styled(l, t.muted()));
        }
    }
    body.push(Line::default());
    if let Some(text) = editing {
        body.push(Line::styled("Edit the subject (one line):", t.bold()));
        let clean = sanitize(text);
        let mut lines = wrap(&clean, w.saturating_sub(1));
        if lines.is_empty() {
            lines.push(String::new());
        }
        let last = lines.len() - 1;
        for (i, l) in lines.into_iter().enumerate() {
            let mut sp = vec![Span::raw(l)];
            if i == last {
                sp.push(Span::styled(
                    " ",
                    Style::new().add_modifier(Modifier::REVERSED),
                ));
            }
            body.push(Line::from(sp));
        }
    } else {
        let mut label = vec![Span::styled("Message:", t.accent())];
        match draft {
            DraftState::Pending => label.push(Span::styled(
                format!("  {} drafting message…", app.spinner()),
                t.muted(),
            )),
            DraftState::Drafted => label.push(Span::styled("  drafted by the agent", t.muted())),
            DraftState::Template => label.push(Span::styled("  template", t.muted())),
            DraftState::Edited => {}
        }
        body.push(Line::from(label));
        for l in wrap(message, w.saturating_sub(2)) {
            body.push(Line::from(format!("  {l}")));
        }
    }
    let want = (head_h as usize + body.len() + foot_h + 2).clamp(5, 28) as u16;
    let r = centered(area, box_w, want.min(area.height));
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(t.accent())
        .padding(Padding::new(1, 1, 0, 0));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let rows_h = inner.height.saturating_sub(head_h + foot_h as u16) as usize;
    let max = body.len().saturating_sub(rows_h);
    app.commit_max_scroll.set(max);
    let start = scroll.min(max);
    let shown: Vec<Line> = body.into_iter().skip(start).take(rows_h).collect();
    let mut y = inner.y;
    f.render_widget(
        Paragraph::new(head),
        Rect::new(inner.x, y, inner.width, head_h),
    );
    y += head_h;
    f.render_widget(
        Paragraph::new(shown),
        Rect::new(inner.x, y, inner.width, rows_h as u16),
    );
    y += rows_h as u16;
    let foot_area = Rect::new(inner.x, y, inner.width, foot_h as u16);
    f.render_widget(foot, foot_area);
    text_area(app, inner, 0);
    click_hints(f, app, foot_area, &plain);
}
