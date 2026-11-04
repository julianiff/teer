//! Drawing: header, bottom bar, and each screen.

use crate::session::names;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Target};
use super::select;
use super::slim;
use crate::git::WORKTREE;
use crate::keymap::{Action, Trigger};
use crate::review::attention::Level;
use crate::session::app::{JumpKind, Overlay, Screen};
use crate::session::help::{HelpRow, help_groups, help_panel, screen_actions, screen_name};
use crate::util::{sanitize, truncate, wrap};

pub fn draw(f: &mut Frame, app: &App) {
    app.targets.borrow_mut().clear();
    app.text_areas.borrow_mut().clear();
    // Nothing to scroll unless a detail area says so.
    app.detail_max_scroll.set(0);
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(header_rows(app)),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_header(f, app, header);
    text_area(app, header, 0);
    text_area(app, footer, 0);
    match app.screen {
        Screen::Queue => super::queue::draw(f, app, body),
        Screen::Unit => super::unit::draw(f, app, body),
        Screen::Flagged => super::flagged_list::draw(f, app, body),
    }
    draw_bar(f, app, footer);
    // Over an overlay a drag selects only what the overlay says it holds.
    if app.overlay != Overlay::None {
        app.text_areas.borrow_mut().clear();
    }
    match &app.overlay {
        Overlay::ConfirmReset => draw_confirm_reset(f, app),
        Overlay::Help => draw_help(f, app),
        // A note is typed in the bottom bar, except from the notes view.
        Overlay::Note { .. } if typed_note(app).is_some() => {}
        Overlay::Note { id, text, .. } => crate::tui::notes::draw_editor(f, app, id, text),
        Overlay::Notes { sel } => crate::tui::notes::draw_view(f, app, *sel),
        Overlay::ConfirmDeleteNote { id, sel } => {
            crate::tui::notes::draw_view(f, app, *sel);
            let name = app
                .review
                .unit(id)
                .map(|u| sanitize(&u.symbol))
                .unwrap_or_default();
            draw_confirm(f, app, format!("Delete the note on {name}?"))
        }
        Overlay::ConfirmWeb => draw_confirm(
            f,
            app,
            "Web search sends generic queries (never your code) to external services. Continue?"
                .into(),
        ),
        Overlay::Finish { .. } => super::finish::draw(f, app),
        Overlay::Jump { kind, query, sel } => draw_jump(f, app, kind, query, *sel),
        Overlay::Commits { sel, unit } => super::commits::draw_commits(f, app, *sel, *unit),
        Overlay::None => {}
    }
    // What is on the screen, to copy from; then the selection marked on it.
    app.shown.borrow_mut().clone_from(f.buffer_mut());
    if let Some(s) = app.selection.filter(|s| s.moved()) {
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

/// How the keys of the modes with fixed keys are shown: sheets
/// ([`Keymap::sheet`]), text inputs ([`Keymap::edit`]) and yes/no prompts
/// ([`Keymap::confirm`]). The keymap owns these keys and they can't be
/// rebound; a test checks each label against it.
///
/// [`Keymap::sheet`]: crate::keymap::Keymap::sheet
/// [`Keymap::edit`]: crate::keymap::Keymap::edit
/// [`Keymap::confirm`]: crate::keymap::Keymap::confirm
pub(super) mod fixed {
    /// Confirms a sheet, submits a text input, says yes.
    pub const ENTER: &str = "⏎";
    /// Cancels a sheet or a text input.
    pub const ESC: &str = "esc";
    /// Says yes, as enter does.
    pub const YES: &str = "y";
    /// Move in a list while typing its query.
    pub const ARROW_UP: &str = "↑";
    pub const ARROW_DOWN: &str = "↓";
    /// Asks the last question again.
    pub const RETRY: &str = "^r";
    /// Moves the focus from the question to the links.
    pub const TAB: &str = "tab";
}

/// What a click on a hint naming `label` does: press the key it names
/// (`⏎`, `esc`, `^d`, `y` for `y/⏎`), or run the action whose key it is
/// when that is a sequence or a command (`gg`, `␣t`, `:w`).
fn hint_target(app: &App, label: &str) -> Option<Target> {
    // `/` is a key of its own; elsewhere it separates two keys.
    let first = match label.split('/').next() {
        Some(k) if !k.is_empty() => k,
        _ => label,
    };
    if let Some([k]) = crate::keymap::parse_label(first).as_deref() {
        return Some(Target::Key(k.event()));
    }
    Action::all()
        .find(|a| app.keys.triggers(*a).any(|t| t.label() == first))
        .map(Target::Action)
}

/// Make each key hint of `hints` (`key what · key what`, as drawn inside
/// `area`) a click target pressing its key, or running its action when the
/// key is a sequence. Hints that name two keys (`j k move`) or none (`any
/// other key`) are left alone.
pub(super) fn click_hints(f: &mut Frame, app: &App, area: Rect, hints: &str) {
    for hint in hints.split('·').map(str::trim) {
        let Some((key, what)) = hint.split_once(' ') else {
            continue;
        };
        let Some(t) = hint_target(app, key) else {
            continue;
        };
        let first = what.split(' ').next().unwrap_or_default();
        let one_key = crate::keymap::parse_label(first).is_some_and(|k| k.len() == 1);
        if one_key || (first.chars().count() <= 2 && hint_target(app, first).is_some()) {
            continue;
        }
        if let Some(r) = select::find(f.buffer_mut(), area, hint) {
            target(app, r, t);
        }
    }
}

/// A `(key, what)` pair for [`hint_line`].
pub(super) fn hint(key: impl Into<String>, what: impl Into<String>) -> (String, String) {
    (key.into(), what.into())
}

/// `keys` (`(key, what)`) as one line, the keys in the accent, and as the
/// plain text [`click_hints`] reads.
pub(super) fn hint_line(app: &App, keys: &[(String, String)]) -> (Line<'static>, String) {
    let t = &app.theme;
    let mut spans = Vec::new();
    for (i, (k, what)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" · "));
        }
        spans.push(Span::styled(k.clone(), t.accent()));
        spans.push(Span::raw(format!(" {what}")));
    }
    let plain = keys
        .iter()
        .map(|(k, what)| format!("{k} {what}"))
        .collect::<Vec<_>>()
        .join(" · ");
    (Line::from(spans), plain)
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

/// The summary of the whole change the queue's header shows under what is
/// reviewed, on one line; `None` while there is none.
fn summary_line(app: &App) -> Option<String> {
    let s = app.review.intent.summary.as_deref()?;
    let s = sanitize(s).split_whitespace().collect::<Vec<_>>().join(" ");
    (app.screen == Screen::Queue && !s.is_empty()).then_some(s)
}

/// Rows of the header: what is reviewed, and on the queue the change's
/// summary under it when there is one.
fn header_rows(app: &App) -> u16 {
    1 + u16::from(summary_line(app).is_some())
}

/// The header: what is reviewed (`your changes`, `feature since main, 6
/// commits`, `PR #482 by ana`) and what the change is called, with what
/// teer is doing on the right; on the queue, the summary of the change
/// under it.
fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    if let Some(summary) = summary_line(app)
        && area.height > 1
    {
        let row = Rect::new(area.x, area.y + 1, area.width, 1);
        let text = truncate(&format!(" {summary}"), area.width as usize);
        f.render_widget(Paragraph::new(Line::styled(text, t.muted())), row);
    }
    let area = Rect::new(area.x, area.y, area.width, area.height.min(1));
    let r = &app.review;
    // What is reviewed, then what the change is called (the PR's title, the
    // only commit's subject): the commits view (`␣c`) has the rest.
    let mut title = r.subject();
    let named = r.pr.is_some() || (r.head != WORKTREE && r.intent.commits.len() == 1);
    if let (true, Some(name)) = (named, crate::review::intent::headline(r).0) {
        title = format!("{title} · {}", sanitize(&name));
    }
    let mut right = Vec::new();
    if !app.has_agent && app.agent_note.as_deref() == Some(crate::session::app::NO_AGENT) {
        right.push(Span::styled("no agent  ", t.muted()));
    }
    let (done, total) = app.prep_progress();
    if done < total {
        right.push(Span::styled(
            format!("{} preparing {done}/{total}  ", app.spinner()),
            t.muted(),
        ));
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

/// The buttons while a visual selection is on: what applies to it, and the
/// way out.
fn visual_buttons(app: &App) -> Vec<(Target, String, &'static str)> {
    let b = |a: Action, label: &'static str| (Target::Action(a), app.keys.label(a), label);
    vec![
        b(Action::DecideAccept, "accept"),
        b(Action::DecideFlag, "flag"),
        b(Action::ViewBack, "leave"),
    ]
}

/// What was typed of a sequence or count, then each key that can follow
/// and what it does; the tile keys only in the unit view, the one place
/// they do anything.
fn pending_spans(app: &App, typed: &str) -> Vec<Span<'static>> {
    let t = &app.theme;
    let mut spans = vec![Span::styled(
        typed.to_string(),
        t.accent().add_modifier(Modifier::BOLD),
    )];
    const TILES: [Action; 3] = [
        Action::MoveNextTile,
        Action::MovePrevTile,
        Action::ToggleZoom,
    ];
    let unit = app.screen == Screen::Unit && help_panel(app).is_none();
    let follow = app
        .keys
        .hint()
        .into_iter()
        .filter(|(_, a)| unit || !TILES.contains(a));
    for (i, (rest, a)) in follow.enumerate() {
        spans.push(Span::styled(if i == 0 { "  " } else { " · " }, t.muted()));
        spans.push(Span::styled(rest, t.accent()));
        spans.push(Span::raw(format!(" {}", a.describe())));
    }
    spans
}

/// `:`, the command typed so far and the cursor, then the commands it can
/// still become.
fn command_spans(app: &App, text: &str) -> Vec<Span<'static>> {
    let t = &app.theme;
    let mut spans = vec![
        Span::styled(format!(":{}", sanitize(text)), t.bold()),
        Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED)),
    ];
    let commands = app
        .keys
        .bindings()
        .filter_map(|(a, trigger)| match trigger {
            Trigger::Command(c) if c.starts_with(text) => Some((trigger.label(), a)),
            _ => None,
        });
    for (i, (label, a)) in commands.enumerate() {
        spans.push(Span::styled(if i == 0 { "   " } else { " · " }, t.muted()));
        spans.push(Span::styled(label, t.accent()));
        spans.push(Span::styled(format!(" {}", a.describe()), t.muted()));
    }
    spans
}

/// Cells `spans` take.
fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// What the bar says first: `4 pending · 1 flagged · 37 accepted`
/// (units accepted for you are accepted), or once nothing is pending
/// `Nothing pending. :w to finish` and what is flagged and accepted, if
/// anything. With it, which span names the finish key, to click.
fn count_spans(app: &App) -> (Vec<Span<'static>>, Option<usize>) {
    let t = &app.theme;
    let (accepted, flagged, pending) = app.decision_counts();
    let sep = || Span::styled(" · ", t.muted());
    let flagged_style = if flagged > 0 { t.bad() } else { t.muted() };
    if pending > 0 {
        let spans = vec![
            Span::styled(format!("{pending} pending"), t.bold()),
            sep(),
            Span::styled(format!("{flagged} flagged"), flagged_style),
            sep(),
            Span::styled(format!("{accepted} accepted"), t.muted()),
        ];
        return (spans, None);
    }
    let mut spans = vec![
        Span::styled("Nothing pending. ", t.bold()),
        Span::styled(app.keys.label(Action::FinishOpen), t.accent()),
        Span::styled(" to finish", t.bold()),
    ];
    if flagged > 0 {
        spans.extend([sep(), Span::styled(format!("{flagged} flagged"), t.bad())]);
    }
    if accepted > 0 {
        spans.extend([
            sep(),
            Span::styled(format!("{accepted} accepted"), t.muted()),
        ]);
    }
    (spans, Some(1))
}

/// The note being typed in the bar: the unit's id and the text so far.
/// `None` when no note is typed, and from the notes view (which keeps its
/// editor).
fn typed_note(app: &App) -> Option<(&str, &str)> {
    match &app.overlay {
        Overlay::Note {
            id,
            text,
            back: None,
            ..
        } => Some((id, text)),
        _ => None,
    }
}

/// The bottom bar of the screens (the queue, the unit view, and under
/// the `:w` sheet): the counts ([`count_spans`]), then the sequence or
/// count that waits and what can follow, the command being typed, or
/// `-- VISUAL --` and what applies to the selection; the status on the
/// right, or where the keys are. While a note is typed, the note line.
fn draw_bar(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let w = area.width as usize;
    if let Some((id, text)) = typed_note(app) {
        draw_note_line(f, app, area, id, text);
        return;
    }
    let (counts, finish) = count_spans(app);
    let counts_w = spans_width(&counts);
    // A command, a waiting sequence or count, or visual mode; with how many
    // cells of it stay in view: the counts give way first.
    let (mode, essential, visual) = if let Some(text) = app.keys.command_line() {
        (command_spans(app, text), text.width() + 2, None)
    } else if let Some(typed) = app.keys.pending() {
        (pending_spans(app, &typed), typed.width(), None)
    } else if let Some(rows) = app.visual_rows() {
        let lead = "-- VISUAL --";
        (vec![Span::styled(lead, t.bold())], lead.width(), Some(rows))
    } else {
        (Vec::new(), 0, None)
    };
    let waiting = !mode.is_empty() && visual.is_none();
    let with_counts = mode.is_empty() || 1 + counts_w + 2 + essential <= w;
    let mut left = vec![Span::raw(" ")];
    let finish = if with_counts {
        let at = finish.map(|i| left.len() + i);
        left.extend(counts);
        if !mode.is_empty() {
            left.push(Span::raw("  "));
        }
        at
    } else {
        None
    };
    left.extend(mode);
    if let Some(rows) = visual {
        // What applies to the selection, as buttons, and how many units it
        // holds, unless the status says something.
        let n = rows.count();
        let status = if app.status.is_empty() {
            format!("{n} unit{} ", if n == 1 { "" } else { "s" })
        } else {
            app.status.clone()
        };
        // Each button starts with a blank of its own.
        left.push(Span::raw(" "));
        let left = fit(left, w);
        finish_target(app, area, &left, finish);
        let lead_w = spans_width(&left);
        let buttons = visual_buttons(app);
        let keys: Vec<(String, &str)> = buttons.iter().map(|(_, k, l)| (k.clone(), *l)).collect();
        let (line, widths) = bar_line(t, &keys, &status, w - lead_w, true);
        let mut x = area.x + lead_w as u16;
        for ((tgt, ..), bw) in buttons.into_iter().zip(widths) {
            target(app, Rect::new(x, area.y, bw as u16, 1), tgt);
            x += bw as u16 + 1;
        }
        let mut spans = left;
        spans.extend(line.spans);
        f.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
    // On the right the status, which is never cut for the rest; with none,
    // in normal mode, the key that lists the keys.
    let mut help = false;
    let right: Vec<Span> = if waiting {
        Vec::new()
    } else if !app.status.is_empty() {
        vec![Span::styled(app.status.clone(), t.muted())]
    } else {
        help = true;
        vec![
            Span::styled(app.keys.label(Action::HelpOpen), t.accent()),
            Span::styled(" keys ", t.muted()),
        ]
    };
    let right_w = spans_width(&right).min(w.saturating_sub(2));
    // The key hint gives way to the counts.
    let (right, right_w) = if help && 1 + counts_w + 2 + right_w > w {
        (Vec::new(), 0)
    } else {
        (fit(right, right_w), right_w)
    };
    let left = fit(left, w.saturating_sub(right_w + usize::from(right_w > 0)));
    let left_w = spans_width(&left);
    finish_target(app, area, &left, finish);
    if help && right_w > 0 {
        let r = Rect::new(area.x + (w - right_w) as u16, area.y, right_w as u16, 1);
        target(app, r, Target::Action(Action::HelpOpen));
    }
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(w.saturating_sub(left_w + right_w))));
    spans.extend(right);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Make the finish key that `left[i]` names, drawn from the left of
/// `area`, a click target, unless it was cut.
fn finish_target(app: &App, area: Rect, left: &[Span], i: Option<usize>) {
    let Some(i) = i else { return };
    let key = app.keys.label(Action::FinishOpen);
    if left.get(i).is_some_and(|s| s.content == key) {
        let x = area.x + spans_width(&left[..i]) as u16;
        let r = Rect::new(x, area.y, key.width() as u16, 1);
        target(app, r, Target::Action(Action::FinishOpen));
    }
}

/// Most cells of a unit's name the note line shows.
const NOTE_LINE_NAME: usize = 24;

/// The note line at the bottom (`f`, `i`): which unit, the
/// text with the cursor after it and its end in view, and on the right
/// what enter and esc do.
fn draw_note_line(f: &mut Frame, app: &App, area: Rect, id: &str, text: &str) {
    use unicode_width::UnicodeWidthChar;
    let t = &app.theme;
    let w = area.width as usize;
    let name = app
        .review
        .unit(id)
        .map(names::row_name)
        .unwrap_or_else(|| sanitize(id));
    let (keys, plain) = hint_line(
        app,
        &[hint(fixed::ENTER, "save"), hint(fixed::ESC, "cancel")],
    );
    let keys_w = plain.width();
    // The keys only when the text keeps room.
    let show_keys = w >= keys_w + 2 + 40;
    let room = w.saturating_sub(if show_keys { keys_w + 2 } else { 0 });
    let mut spans = vec![
        Span::styled(" note on ", t.muted()),
        Span::styled(truncate(&name, NOTE_LINE_NAME), t.bold()),
        Span::styled(": ", t.muted()),
    ];
    let text_room = room.saturating_sub(spans_width(&spans) + 1);
    let typed = sanitize(text);
    // The end of the text in view, `…` in front when its start is cut.
    let shown = if typed.width() <= text_room {
        typed
    } else {
        let mut tail = Vec::new();
        let mut used = 1;
        for c in typed.chars().rev() {
            let cw = c.width().unwrap_or(0);
            if used + cw > text_room {
                break;
            }
            used += cw;
            tail.push(c);
        }
        std::iter::once('…').chain(tail.into_iter().rev()).collect()
    };
    spans.push(Span::raw(shown));
    spans.push(Span::styled(
        " ",
        Style::new().add_modifier(Modifier::REVERSED),
    ));
    let mut spans = fit(spans, room);
    if show_keys {
        let used = spans_width(&spans);
        spans.push(Span::raw(" ".repeat(w.saturating_sub(used + keys_w + 1))));
        spans.extend(keys.spans);
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    if show_keys {
        let keys_area = Rect::new(area.x + (w - keys_w - 1) as u16, area.y, keys_w as u16, 1);
        click_hints(f, app, keys_area, &plain);
    }
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
    bar_line(t, buttons, status, w, false)
}

/// [`footer_line`]; with `status_first` the status is never cut for the
/// buttons, which drop from the end to leave it room.
fn bar_line(
    t: &super::theme::Theme,
    buttons: &[(String, &str)],
    status: &str,
    w: usize,
    status_first: bool,
) -> (Line<'static>, Vec<usize>) {
    let reserved = if status_first && !status.is_empty() {
        truncate(status, w).width() + 1
    } else {
        0
    };
    let mut spans = Vec::new();
    let mut widths = Vec::new();
    let mut used = 0;
    for (key, label) in buttons {
        let bw = key.width() + label.width() + 3;
        if used + bw + reserved > w {
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

/// Say once, above the review, why there are no behavior statements: a
/// row on top of `area` when there is a reason other than `--no-agent`.
/// What is left of `area`.
pub(super) fn banner(f: &mut Frame, app: &App, area: Rect) -> Rect {
    match app.agent_note.as_deref() {
        Some(note)
            if !app.has_agent && note != crate::session::app::NO_AGENT && area.height > 2 =>
        {
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
    }
}

/// A list with no unit to decide: the muted rows that still say what
/// changed (files teer can't show, units accepted for you), or that there
/// is nothing to review.
pub(super) fn draw_empty(f: &mut Frame, app: &App, area: Rect) {
    text_area(app, area, 0);
    let rows = slim::build_queue(app, area.width, true);
    if !rows.lines.is_empty() {
        f.render_widget(Paragraph::new(rows.lines), area);
        return;
    }
    let msg = "Nothing to review in these changes.";
    f.render_widget(
        Paragraph::new(Line::styled(msg, app.theme.muted())).wrap(Wrap { trim: true }),
        pad(area, 1, 1),
    );
}

/// `rows` in `list`, scrolled so the unit under the cursor is in view, each
/// unit's row a click target that selects it.
pub(super) fn draw_rows(f: &mut Frame, app: &App, list: Rect, rows: &slim::Rows) {
    let h = list.height as usize;
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

/// `:e!`: "Reset all 42 decisions? y/n", one per unit; yes resets them and
/// re-runs the review.
fn draw_confirm_reset(f: &mut Frame, app: &App) {
    let t = &app.theme;
    let n = app.reset_count();
    let what = if n == 1 {
        "Reset the 1 decision? y/n".to_string()
    } else {
        format!("Reset all {n} decisions? y/n")
    };
    let mut lines = vec![
        Line::styled(what, t.bold()),
        Line::styled(
            "All units go back to pending and the review re-runs.",
            t.muted(),
        ),
        Line::default(),
    ];
    let (keys, plain) = hint_line(
        app,
        &[hint(fixed::ENTER, "reset"), hint(fixed::ESC, "cancel")],
    );
    lines.push(keys);
    let rows = lines.len() as u16;
    let inner = modal(f, app, Paragraph::new(lines), 60, rows, t.accent());
    click_hints(f, app, inner, &plain);
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

/// A line of the help's columns.
enum HelpLine<'a> {
    Group(&'static str),
    Row(&'a HelpRow),
    Blank,
}

/// `groups` in at most `cols` columns of `rows` lines, a blank line
/// between groups with `blanks`. A group starts a column when it doesn't
/// fit the one before, or with `split` runs on into the next one once its
/// name and a row fit. `None` when they don't fit.
fn help_columns<'a>(
    groups: &'a [(&'static str, Vec<HelpRow>)],
    cols: usize,
    rows: usize,
    blanks: bool,
    split: bool,
) -> Option<Vec<Vec<HelpLine<'a>>>> {
    let mut out: Vec<Vec<HelpLine>> = vec![Vec::new()];
    for (name, group) in groups {
        let col = out.last_mut().expect("a column");
        let gap = usize::from(blanks && !col.is_empty());
        let needs = if split { 2 } else { 1 + group.len() };
        if !col.is_empty() && col.len() + gap + needs > rows {
            out.push(Vec::new());
        }
        let col = out.last_mut().expect("a column");
        if blanks && !col.is_empty() {
            col.push(HelpLine::Blank);
        }
        col.push(HelpLine::Group(name));
        for r in group {
            if out.last().is_some_and(|c| c.len() >= rows) {
                out.push(Vec::new());
            }
            out.last_mut().expect("a column").push(HelpLine::Row(r));
        }
    }
    (out.len() <= cols && out.iter().all(|c| c.len() <= rows)).then_some(out)
}

/// The help (`?`): every key of the screen or panel it was opened on,
/// built from the keymap, grouped ([`GROUPS`]: Move, Decide, Look closer,
/// More, Finish), in as many columns as it takes and the terminal has room
/// for.
fn draw_help(f: &mut Frame, app: &App) {
    let t = &app.theme;
    let area = f.area();
    let groups = help_groups(app, screen_actions(app));
    let key_w = groups
        .iter()
        .flat_map(|(_, rows)| rows)
        .map(|r| r.keys.width())
        .max()
        .unwrap_or(0)
        .min(HELP_KEYS_MAX);
    let what_w = groups
        .iter()
        .flat_map(|(_, rows)| rows)
        .map(|r| r.what.width())
        .max()
        .unwrap_or(0);
    let natural = key_w + 2 + what_w;
    // Borders and a column of padding on each side.
    let room_w = (area.width as usize).saturating_sub(2 + 4);
    let room_h = (area.height as usize).saturating_sub(2);
    let col_w = |n: usize| natural.min(room_w.saturating_sub((n - 1) * HELP_GAP) / n);
    let fits = |n: usize| n == 1 || col_w(n) >= HELP_COL_MIN;
    // The legend of the list's rows under the columns of a screen's keys,
    // when it fits.
    let legend = if help_panel(app).is_none() {
        help_legend(app)
    } else {
        Vec::new()
    };
    let legend_h = if legend.is_empty() {
        0
    } else {
        1 + legend.len()
    };
    // Groups kept whole and apart before cramming; then room left for the
    // legend, even if that takes another column; then fewest columns.
    let mut chosen = None;
    'search: for (blanks, split) in [(true, false), (false, false), (false, true)] {
        for room in [room_h.saturating_sub(legend_h), room_h] {
            for n in (1..=4).filter(|n| fits(*n)) {
                if let Some(cols) = help_columns(&groups, n, room, blanks, split) {
                    chosen = Some((n, cols));
                    break 'search;
                }
            }
        }
    }
    // Too many for the terminal: as many columns as fit, cut at the bottom.
    let (n, cols) = chosen.unwrap_or_else(|| {
        let n = (1..=4).filter(|n| fits(*n)).max().unwrap_or(1);
        let tall =
            help_columns(&groups, usize::MAX, room_h.max(1), false, true).unwrap_or_default();
        let mut cols: Vec<Vec<HelpLine>> = tall.into_iter().take(n).collect();
        if let Some(last) = cols.last_mut() {
            last.pop();
            last.push(HelpLine::Group("… a taller window shows more"));
        }
        (n, cols)
    });
    let cw = col_w(n);
    let col_h = cols.iter().map(Vec::len).max().unwrap_or(0);
    let with_legend = !legend.is_empty() && col_h + 1 + legend.len() <= room_h;
    let inner_h = col_h + if with_legend { 1 + legend.len() } else { 0 };
    let legend_w = legend.iter().map(Line::width).max().unwrap_or(0);
    let mut inner_w = n * cw + (n - 1) * HELP_GAP;
    if with_legend {
        inner_w = inner_w.max(legend_w.min(room_w));
    }
    let r = centered(area, (inner_w + 4) as u16, (inner_h + 2) as u16);
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(t.muted())
        .padding(ratatui::widgets::Padding::horizontal(1))
        .title(format!(" keys · {} ", screen_name(app)))
        .title_bottom(Line::styled(" any key closes ", t.muted()).right_aligned());
    let inner = block.inner(r);
    f.render_widget(block, r);
    text_area(app, inner, 0);
    for (i, col) in cols.iter().enumerate() {
        let x = inner.x + (i * (cw + HELP_GAP)) as u16;
        let rect = Rect::new(x, inner.y, (cw as u16).min(inner.width), inner.height);
        // Each column's keys only as wide as its own, leaving its text room.
        let key_w = col
            .iter()
            .filter_map(|l| match l {
                HelpLine::Row(r) => Some(r.keys.width()),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            .min(key_w);
        let lines: Vec<Line> = col.iter().map(|l| help_line(app, l, key_w, cw)).collect();
        f.render_widget(Paragraph::new(lines), rect);
    }
    if with_legend {
        let y = inner.y + col_h as u16 + 1;
        let rect = Rect::new(
            inner.x,
            y,
            inner.width,
            inner.height.saturating_sub(col_h as u16 + 1),
        );
        f.render_widget(Paragraph::new(legend), rect);
    }
    // Any key closes it, and so does a click.
    if let Some([esc]) = crate::keymap::parse_label(fixed::ESC).as_deref() {
        target(app, area, Target::Key(esc.event()));
    }
}

/// Widest the help's key column gets; longer keys push their text.
const HELP_KEYS_MAX: usize = 13;
/// Narrowest a help column gets, and the cells between two.
const HELP_COL_MIN: usize = 30;
const HELP_GAP: usize = 3;

/// One line of a help column `w` wide, its keys right-aligned in `key_w`.
fn help_line(app: &App, l: &HelpLine, key_w: usize, w: usize) -> Line<'static> {
    let t = &app.theme;
    match l {
        HelpLine::Blank => Line::default(),
        HelpLine::Group(name) => Line::from(vec![
            Span::raw(" ".repeat(key_w + 2)),
            Span::styled(truncate(name, w.saturating_sub(key_w + 2)), t.bold()),
        ]),
        HelpLine::Row(r) => {
            let keys = format!("{:>key_w$}  ", r.keys);
            let room = w.saturating_sub(keys.width());
            let what = truncate(&r.what, room);
            Line::from(vec![Span::styled(keys, t.accent()), Span::raw(what)])
        }
    }
}

/// What the queue's glyphs and the states at the end of its rows mean, for
/// the help (the unit view's title shows the same).
fn help_legend(app: &App) -> Vec<Line<'static>> {
    let t = &app.theme;
    let level = |l: Level, what: &'static str| {
        [
            Span::styled(l.icon(), slim::level_style(app, l)),
            Span::raw(format!(" {what}  ")),
        ]
    };
    vec![
        Line::from(
            [Span::styled("legend  ", t.muted())]
                .into_iter()
                .chain(level(Level::Attention, "close look"))
                .chain(level(Level::Look, "worth a look"))
                .chain(level(Level::Routine, "routine"))
                .chain(level(Level::Same, "accepted for you"))
                .collect::<Vec<_>>(),
        ),
        Line::from(vec![
            Span::raw(" ".repeat(8)),
            Span::styled("pending", t.muted()),
            Span::raw("  "),
            Span::styled("accepted", t.accepted_state()),
            Span::raw("  "),
            Span::styled("flagged", t.flagged_row()),
            Span::styled(" · “note”  ", t.muted()),
            Span::styled("⚠", t.risk(crate::review::model::Risk::Medium)),
            Span::raw(" disputed  "),
            Span::styled("✎", t.accent()),
            Span::raw(" has a note"),
        ]),
    ]
}

/// A yes/no question, wrapped to at most three rows, and its keys: `y` or
/// enter says yes, anything else no.
fn draw_confirm(f: &mut Frame, app: &App, text: String) {
    let t = &app.theme;
    // The modal's border and padding take 6 columns.
    let w = (f.area().width.min(60) as usize).saturating_sub(6);
    let mut lines: Vec<Line> = wrap(&text, w)
        .into_iter()
        .take(3)
        .map(|l| Line::styled(l, t.bold()))
        .collect();
    let (keys, plain) = hint_line(
        app,
        &[
            hint(format!("{}/{}", fixed::YES, fixed::ENTER), "yes"),
            hint(fixed::ESC, "no"),
        ],
    );
    lines.push(Line::default());
    lines.push(keys);
    let rows = lines.len() as u16;
    let inner = modal(f, app, Paragraph::new(lines), 60, rows, t.accent());
    click_hints(f, app, inner, &plain);
}

/// The jump list (`/`, `gd`, `gr`): its title, the query being typed when
/// finding, the units it lists with the selected one marked, and its keys.
fn draw_jump(f: &mut Frame, app: &App, kind: &JumpKind, query: &str, sel: usize) {
    let t = &app.theme;
    let area = f.area();
    let items = app.jump_items();
    let name = |id: &str| {
        app.review
            .unit(id)
            .map(|u| sanitize(&u.symbol))
            .unwrap_or_else(|| sanitize(id))
    };
    let (title, finding) = match kind {
        JumpKind::Find => ("Find a unit".to_string(), true),
        JumpKind::Definition { of } => (format!("What {} calls", name(of)), false),
        JumpKind::Callers { of } => (format!("What calls {}", name(of)), false),
    };
    let k = |a: Action| app.keys.label(a);
    let keys = if finding {
        vec![
            hint(format!("{} {}", fixed::ARROW_UP, fixed::ARROW_DOWN), "move"),
            hint(fixed::ENTER, "go"),
            hint(fixed::ESC, "close"),
        ]
    } else {
        vec![
            hint(
                format!("{} {}", k(Action::MoveDown), k(Action::MoveUp)),
                "move",
            ),
            hint(k(Action::ViewOpen), "go"),
            hint(k(Action::ViewClose), "close"),
        ]
    };
    let box_w = area.width.saturating_sub(4).min(80);
    // The query and a rule above the list when finding; a blank and the
    // keys below it; the borders.
    let head = if finding { 2 } else { 0 };
    let want = head + items.len().max(1) + 2 + 2;
    let box_h = (want as u16).min(area.height.saturating_sub(2));
    if box_w < 20 || box_h < (head + 5) as u16 {
        f.render_widget(Clear, centered(area, box_w, box_h));
        return;
    }
    let r = centered(area, box_w, box_h);
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(t.accent())
        .padding(ratatui::widgets::Padding::horizontal(1))
        .title(Span::styled(
            format!(" {} ", truncate(&title, box_w.saturating_sub(6) as usize)),
            t.bold(),
        ));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let w = inner.width as usize;
    let [top, list, _, keys_row] = Layout::vertical([
        Constraint::Length(head as u16),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    if finding {
        // The end of the query stays in view, the cursor after it.
        let typed = sanitize(query);
        let room = w.saturating_sub(3);
        let skip = typed.width().saturating_sub(room);
        let shown: String = typed.chars().skip(skip).collect();
        let lines = vec![
            Line::from(vec![
                Span::styled("/ ", t.accent()),
                Span::raw(shown),
                Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED)),
            ]),
            Line::styled("─".repeat(w), t.muted()),
        ];
        f.render_widget(Paragraph::new(lines), top);
    }
    let rows = list.height as usize;
    let lines: Vec<Line> = if items.is_empty() {
        let empty = match kind {
            JumpKind::Find if query.is_empty() => "No units to find.".to_string(),
            JumpKind::Find => format!("Nothing matches \"{}\".", sanitize(query)),
            _ => "Nothing to list.".to_string(),
        };
        vec![Line::styled(truncate(&empty, w), t.muted())]
    } else {
        let sel = sel.min(items.len() - 1);
        let first = (sel + 1).saturating_sub(rows);
        items
            .iter()
            .enumerate()
            .skip(first)
            .take(rows)
            .filter_map(|(i, &ui)| Some(jump_row(app, app.review.units.get(ui)?, i == sel, w)))
            .collect()
    };
    f.render_widget(Paragraph::new(lines), list);
    let (keys, plain) = hint_line(app, &keys);
    f.render_widget(Paragraph::new(keys), keys_row);
    text_area(app, inner, 0);
    click_hints(f, app, keys_row, &plain);
}

/// A unit in the jump list, `w` cells: the bar when `selected`, its
/// level's icon, its name and, muted, its file.
fn jump_row(app: &App, u: &crate::review::model::Unit, selected: bool, w: usize) -> Line<'static> {
    let t = &app.theme;
    let level = app.assessment(&u.id).map_or(Level::Routine, |a| a.level);
    let name = names::row_name(u);
    let file = names::file_name(&u.file);
    let name_w = w.saturating_sub(4).min(name.width());
    let file_w = w.saturating_sub(4 + name_w + 2);
    let style = if selected {
        t.accent().add_modifier(Modifier::BOLD)
    } else {
        t.text()
    };
    let mut spans = vec![
        Span::styled(if selected { "▌" } else { " " }, t.accent()),
        Span::styled(format!("{} ", level.icon()), slim::level_style(app, level)),
        Span::raw(" "),
        Span::styled(truncate(&name, name_w), style),
    ];
    if u.kind != crate::review::model::UnitKind::File && file_w > 0 {
        spans.push(Span::styled(
            format!("  {}", truncate(&file, file_w)),
            t.muted(),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{Edit, Keymap, Sheet, parse_label};

    fn key(label: &str) -> crate::keymap::Key {
        match parse_label(label).as_deref() {
            Some([k]) => *k,
            other => panic!("{label} is not one key: {other:?}"),
        }
    }

    #[test]
    fn fixed_labels_are_the_keys_the_modes_read() {
        let km = Keymap::default();
        for (label, sheet) in [(fixed::ENTER, Sheet::Confirm), (fixed::ESC, Sheet::Cancel)] {
            assert_eq!(km.sheet(key(label)), Some(sheet), "{label}");
        }
        for (label, edit) in [
            (fixed::ENTER, Edit::Submit),
            (fixed::ESC, Edit::Cancel),
            (fixed::ARROW_UP, Edit::Up),
            (fixed::ARROW_DOWN, Edit::Down),
            (fixed::RETRY, Edit::Retry),
            (fixed::TAB, Edit::Tab),
        ] {
            assert_eq!(km.edit(key(label)), edit, "{label}");
        }
        assert!(km.confirm(key(fixed::YES)));
        assert!(km.confirm(key(fixed::ENTER)));
        assert!(!km.confirm(key(fixed::ESC)));
    }
}
