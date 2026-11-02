//! Drawing: header, footer buttons, and each screen.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Grouping, JumpKind, Overlay, Screen, Target};
use super::diff::Notes;
use super::select;
use super::slim;
use super::swipe::Dir;
use crate::git::WORKTREE;
use crate::keymap::{Action, Trigger};
use crate::review::attention::Level;
use crate::review::model::Decision;
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
        // On the v2 screens a note is typed in the bottom bar.
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
        Overlay::ConfirmAcceptFile { file, ids } => {
            let what = sanitize(&file.display().to_string());
            let units = if ids.len() == 1 { "unit" } else { "units" };
            let n = ids.len();
            // Listed by unit, only the file's units in the cursor's box.
            let text = match app.grouping {
                Grouping::File => format!("Accept {n} pending {units} in \"{what}\"?"),
                Grouping::Unit => {
                    format!("Accept {n} pending {units} of \"{what}\" in this box?")
                }
            };
            draw_confirm(f, app, text)
        }
        Overlay::ConfirmFlagFile { file, ids } => {
            let what = sanitize(&file.display().to_string());
            let n = ids.len();
            let units = if n == 1 { "unit" } else { "units" };
            draw_confirm(f, app, format!("Flag {n} pending {units} in \"{what}\"?"))
        }
        Overlay::Finish { .. } => super::finish::draw(f, app),
        Overlay::AllDone => draw_all_done(f, app),
        Overlay::Jump { kind, query, sel } => draw_jump(f, app, kind, query, *sel),
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
    /// Edits a sheet's message.
    pub const EDIT: &str = "i";
    /// Cycles the review verdict.
    pub const VERDICT: &str = "v";
    /// Posts the notes as inline comments, or not.
    pub const INLINE: &str = "t";
    /// Quits from the all-reviewed sheet.
    pub const QUIT: &str = "q";
    /// Scroll a sheet.
    pub const DOWN: &str = "j";
    pub const UP: &str = "k";
    /// Move in a list while typing its query.
    pub const ARROW_UP: &str = "↑";
    pub const ARROW_DOWN: &str = "↓";
    /// Scroll the Ask view while typing a question.
    pub const PAGE_DOWN: &str = "pgdn";
    pub const PAGE_UP: &str = "pgup";
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

/// The screens of v2: the queue and the unit view. They share the bottom
/// bar ([`draw_bar`]).
fn v2_screen(app: &App) -> bool {
    matches!(app.screen, Screen::Queue | Screen::Unit)
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
    let mut title = crate::cli::subject(r);
    let named = r.pr.is_some() || (r.head != WORKTREE && r.intent.commits.len() == 1);
    if let (true, Some(name)) = (named, crate::review::intent::headline(r).0) {
        title = format!("{title} · {}", sanitize(&name));
    }
    let mut right = Vec::new();
    if !app.has_agent && app.agent_note.as_deref() == Some(crate::cli::NO_AGENT) {
        right.push(Span::styled("no agent  ", t.muted()));
    }
    let (done, total) = app.prep_progress();
    if done < total {
        right.push(Span::styled(
            format!("{} preparing {done}/{total}  ", app.spinner()),
            t.muted(),
        ));
    }
    // The v2 bottom bar counts the flagged units.
    let flagged = app.flagged_units().len();
    if flagged > 0 && !v2_screen(app) {
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

/// Footer buttons of the v1 screens, highest priority first (the footer
/// drops from the end). Accepting and `:e!` are deliberately absent: they
/// work everywhere but are listed only in the help overlay. The v2 screens
/// have none (see [`draw_bar`]).
fn buttons_for(app: &App) -> Vec<(Target, String, &'static str)> {
    let k = |a: Action| app.keys.label(a);
    let b = |a: Action, label: &'static str| (Target::Action(a), k(a), label);
    let mut v = match app.screen {
        Screen::Queue | Screen::Unit => return Vec::new(),
        Screen::Slim => {
            let mut v = vec![
                b(Action::GotoEditor, "open in editor"),
                b(Action::V1Swipe, "swipe"),
                b(Action::HelpOpen, "help"),
            ];
            // Only a flagged unit opens, for now.
            if app
                .selected()
                .is_some_and(|u| u.decision == Decision::Investigate)
            {
                v.push(b(Action::ViewOpen, "open"));
            }
            v.push(b(Action::DecideFlag, "flag"));
            v.push(b(Action::AskOpen, "ask"));
            v
        }
        // The tiles name the keys that ask, so the footer doesn't repeat them.
        Screen::Flagged => vec![
            b(Action::V1ResolveOk, "ok"),
            b(Action::V1ResolveFix, "needs fix"),
            (
                Target::Action(Action::V1NextTile),
                format!("{} {}", k(Action::V1PrevTile), k(Action::V1NextTile)),
                "tile",
            ),
            b(
                Action::V1Zoom,
                if app.flagged.zoom { "unzoom" } else { "zoom" },
            ),
            b(Action::V1DiveRerun, "ask again"),
            b(Action::AskOpen, "chat"),
            b(Action::GotoBrowser, "browser"),
            b(Action::ViewBack, "back"),
            b(Action::HelpOpen, "help"),
        ],
        Screen::Swipe if app.swipe_current().is_none() => vec![
            b(Action::MoveUp, "previous"),
            b(Action::HistoryUndo, "undo"),
            b(Action::ViewBack, "back"),
        ],
        Screen::Swipe => vec![
            b(Action::DecideAccept, "accept · next"),
            b(Action::MoveUp, "previous"),
            b(Action::DecideFlag, "flag"),
            b(Action::HistoryUndo, "undo"),
            b(Action::ViewBack, "back"),
            // After the way back: when narrow, it is dropped first, and a
            // cut box still offers it in its last row.
            b(Action::GotoEditor, "open in editor"),
        ],
    };
    // Only while the detail area is cut (it was drawn just before).
    if app.detail_max_scroll.get() > 0 {
        v.push((
            Target::Action(Action::ScrollHalfDown),
            format!("{} {}", k(Action::ScrollHalfDown), k(Action::ScrollHalfUp)),
            "scroll",
        ));
    }
    // The notes last: when narrow, they are dropped first.
    let on_unit = match app.screen {
        Screen::Slim => app.selected().is_some(),
        Screen::Swipe => app.swipe_current().is_some(),
        Screen::Flagged | Screen::Queue | Screen::Unit => false,
    };
    if on_unit {
        v.push(b(Action::NoteEdit, "note"));
    }
    if app.screen == Screen::Slim && !crate::notes::noted(&app.review).is_empty() {
        v.push(b(Action::ToolNotes, "notes"));
    }
    v
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

/// The bottom bar: on the v2 screens [`draw_bar`]; on the others the
/// command line while one is typed, the keys that can follow while a
/// sequence waits, else the buttons (after `-- VISUAL --` in visual mode)
/// and the status on the right.
fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    if v2_screen(app) {
        draw_bar(f, app, area);
        return;
    }
    let w = area.width as usize;
    if let Some(text) = app.keys.command_line() {
        f.render_widget(Paragraph::new(command_bar(app, text, w)), area);
        return;
    }
    if let Some(typed) = app.keys.pending() {
        f.render_widget(Paragraph::new(pending_bar(app, &typed, w)), area);
        return;
    }
    let t = &app.theme;
    let (lead, buttons, status) = match app.visual {
        Some(anchor) => {
            let n = anchor.abs_diff(app.sel) + 1;
            let status = if app.status.is_empty() {
                format!("{n} unit{} ", if n == 1 { "" } else { "s" })
            } else {
                app.status.clone()
            };
            (
                Span::styled(" -- VISUAL -- ", t.bold()),
                visual_buttons(app),
                status,
            )
        }
        None => (Span::raw(""), buttons_for(app), app.status.clone()),
    };
    let lead_w = lead.content.width().min(w);
    let keys: Vec<(String, &str)> = buttons.iter().map(|(_, k, l)| (k.clone(), *l)).collect();
    let (line, widths) = bar_line(t, &keys, &status, w - lead_w, true);
    let mut x = area.x + lead_w as u16;
    for ((tgt, ..), bw) in buttons.into_iter().zip(widths) {
        target(app, Rect::new(x, area.y, bw as u16, 1), tgt);
        x += bw as u16 + 1;
    }
    let mut spans = vec![lead];
    spans.extend(line.spans);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The bottom bar while a sequence or a count waits: what was typed, then
/// each key that can follow and what it does, cut at `w`.
fn pending_bar(app: &App, typed: &str, w: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(pending_spans(app, typed));
    Line::from(fit(spans, w))
}

/// What was typed of a sequence or count, then each key that can follow
/// and what it does.
fn pending_spans(app: &App, typed: &str) -> Vec<Span<'static>> {
    let t = &app.theme;
    let mut spans = vec![Span::styled(
        typed.to_string(),
        t.accent().add_modifier(Modifier::BOLD),
    )];
    for (i, (rest, a)) in app.keys.hint().into_iter().enumerate() {
        spans.push(Span::styled(if i == 0 { "  " } else { " · " }, t.muted()));
        spans.push(Span::styled(rest, t.accent()));
        spans.push(Span::raw(format!(" {}", a.describe())));
    }
    spans
}

/// The bottom bar while a command is typed: `:`, the text and the cursor,
/// then the commands it can still become.
fn command_bar(app: &App, text: &str, w: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(command_spans(app, text));
    Line::from(fit(spans, w))
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

/// What the v2 bar says first: `4 pending · 1 flagged · 37 accepted`
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

/// The note being typed in the v2 bar: the unit's id and the text so far.
/// `None` when no note is typed, on the v1 screens, and from the notes view
/// (which keeps its editor).
fn typed_note(app: &App) -> Option<(&str, &str)> {
    match &app.overlay {
        Overlay::Note {
            id,
            text,
            back: None,
            ..
        } if v2_screen(app) => Some((id, text)),
        _ => None,
    }
}

/// The bottom bar of the v2 screens (the queue, the unit view, and under
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
    } else if let Some(anchor) = app.visual {
        let lead = "-- VISUAL --";
        (
            vec![Span::styled(lead, t.bold())],
            lead.width(),
            Some(anchor),
        )
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
    if let Some(anchor) = visual {
        // What applies to the selection, as buttons, and how many units it
        // holds, unless the status says something.
        let n = anchor.abs_diff(app.sel) + 1;
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

/// The note line at the bottom (`f`, `i` on a v2 screen): which unit, the
/// text with the cursor after it and its end in view, and on the right
/// what enter and esc do.
fn draw_note_line(f: &mut Frame, app: &App, area: Rect, id: &str, text: &str) {
    use unicode_width::UnicodeWidthChar;
    let t = &app.theme;
    let w = area.width as usize;
    let name = app
        .review
        .unit(id)
        .map(slim::row_name)
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
    }
}

/// A list with no unit to decide: the muted rows that still say what
/// changed (files teer can't show, units accepted for you), or that there
/// is nothing to review.
pub(super) fn draw_empty(f: &mut Frame, app: &App, area: Rect, flavor: slim::Flavor) {
    text_area(app, area, 0);
    let rows = slim::build_rows(app, area.width, true, flavor);
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

fn draw_slim(f: &mut Frame, app: &App, area: Rect) {
    let area = banner(f, app, area);
    if app.order.is_empty() {
        draw_empty(f, app, area, slim::Flavor::List);
        return;
    }
    let (list, pane) = crate::tui::preview::layout_of(app, area);
    let h = list.height as usize;
    // The blank lines between boxes and files cost rows: only when the list
    // has room for them.
    let rows = slim::build_with(app, list.width, h >= slim::PADDING_MIN_ROWS);
    draw_rows(f, app, list, &rows);
    if let Some((rect, kind)) = pane {
        crate::tui::preview::draw_preview(f, app, rect, kind);
    }
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
    let key = app.keys.label(Action::DecideAccept);
    let text = format!(" accept [{key}] → ");
    let bw = text.width() as u16;
    let by = base.y + base.height;
    if by < area.y + area.height && bw <= base.width {
        let r = Rect::new(base.x + base.width - bw, by, bw, 1);
        f.render_widget(Paragraph::new(Span::styled(text, t.accept_button())), r);
        target(app, r, Target::Action(Action::DecideAccept));
    }
}

fn draw_swipe_summary(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let (acc, flagged, pending) = app.decision_counts();
    // The flagged units open from the list, which the footer leads back to.
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
            Span::styled(app.keys.label(Action::MoveUp), t.accent()),
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
    let mut lines = vec![
        Line::styled("Reset the review?", t.bold()),
        Line::styled(
            "Clears every decision, as if nothing was looked at.",
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

/// The all-reviewed sheet: `⏎` approves the PR or commits the changes
/// (whichever this review can), `q` quits, any other key goes on.
fn draw_all_done(f: &mut Frame, app: &App) {
    let t = &app.theme;
    let ok = t.ok();
    let quit = || {
        Line::from(vec![
            Span::styled(fixed::QUIT, t.accent()),
            Span::raw(" quit  ·  any other key to continue"),
        ])
        .centered()
    };
    let quit_hint = format!("{} quit", fixed::QUIT);
    if let Some(short) = &app.committed {
        let lines = vec![
            Line::styled(format!("Committed {}.", sanitize(short)), ok).centered(),
            Line::default(),
            quit(),
        ];
        let rows = lines.len() as u16;
        let inner = modal(f, app, Paragraph::new(lines), 58, rows, ok);
        click_hints(f, app, inner, &quit_hint);
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
    lines.extend([Line::default(), quit()]);
    // What enter does here, as the sheet decides it.
    let confirm = if app.can_approve() {
        app.review
            .pr
            .as_ref()
            .map(|pr| format!("approve PR #{} on GitHub", pr.number))
    } else if app.can_commit() {
        Some("commit these changes".to_string())
    } else {
        None
    };
    if let Some(pr) = app.review.pr.as_ref().filter(|pr| pr.is_approved()) {
        lines.push(Line::default());
        lines.push(Line::styled(format!("PR #{} approved", pr.number), ok).centered());
    }
    if let Some(what) = &confirm {
        lines.push(Line::default());
        lines.push(
            Line::from(vec![
                Span::styled(fixed::ENTER, t.accent()),
                Span::raw(format!(" {what}")),
            ])
            .centered(),
        );
    }
    let rows = lines.len() as u16;
    let inner = modal(f, app, Paragraph::new(lines), 58, rows, ok);
    let mut hints = vec![quit_hint];
    if let Some(what) = confirm {
        hints.push(format!("{} {what}", fixed::ENTER));
    }
    click_hints(f, app, inner, &hints.join(" · "));
}

/// The actions the list handles, in the order the help lists them.
const LIST: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::MoveNextPending,
    Action::MovePrevPending,
    Action::MoveNextFile,
    Action::MovePrevFile,
    Action::MoveNextHunk,
    Action::MovePrevHunk,
    Action::ScrollHalfDown,
    Action::ScrollHalfUp,
    Action::ScrollPageDown,
    Action::ScrollPageUp,
    Action::DecideAccept,
    Action::DecideFlag,
    Action::DecideAcceptFile,
    Action::DecideFlagFile,
    Action::NoteEdit,
    Action::VisualStart,
    Action::HistoryUndo,
    Action::HistoryRedo,
    Action::HistoryRepeat,
    Action::ViewOpen,
    Action::ViewBack,
    Action::ViewClose,
    Action::AskOpen,
    Action::ToggleAutoAccepted,
    Action::V1Swipe,
    Action::ToggleByFile,
    Action::HelpOpen,
    Action::FindUnit,
    Action::GotoDefinition,
    Action::GotoCallers,
    Action::GotoEditor,
    Action::GotoBrowser,
    Action::ToolTests,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
    Action::V1Recommend,
    Action::V1Disagree,
    Action::V1InterpretTests,
    Action::ToolHistory,
    Action::ToolNotes,
    Action::ToolSettings,
    Action::FinishOpen,
    Action::ReviewRerun,
    Action::ReviewReset,
];

/// The actions the swipe screen handles.
const SWIPE: &[Action] = &[
    Action::MoveUp,
    Action::MoveNextHunk,
    Action::MovePrevHunk,
    Action::ScrollHalfDown,
    Action::ScrollHalfUp,
    Action::ScrollPageDown,
    Action::ScrollPageUp,
    Action::DecideAccept,
    Action::DecideFlag,
    Action::NoteEdit,
    Action::HistoryUndo,
    Action::HistoryRedo,
    Action::HistoryRepeat,
    Action::ViewBack,
    Action::ViewClose,
    Action::AskOpen,
    Action::ToggleAutoAccepted,
    Action::V1Swipe,
    Action::HelpOpen,
    Action::FindUnit,
    Action::GotoDefinition,
    Action::GotoCallers,
    Action::GotoEditor,
    Action::GotoBrowser,
    Action::ToolTests,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
    Action::V1Recommend,
    Action::V1Disagree,
    Action::V1InterpretTests,
    Action::ToolHistory,
    Action::ToolNotes,
    Action::ToolSettings,
    Action::FinishOpen,
    Action::ReviewRerun,
    Action::ReviewReset,
];

/// The actions the Flagged screen handles.
const FLAGGED: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::MoveNextHunk,
    Action::MovePrevHunk,
    Action::ScrollHalfDown,
    Action::ScrollHalfUp,
    Action::ScrollPageDown,
    Action::ScrollPageUp,
    Action::V1NextTile,
    Action::V1PrevTile,
    Action::DecideAccept,
    Action::DecideFlag,
    Action::NoteEdit,
    Action::V1ResolveOk,
    Action::V1ResolveFix,
    Action::HistoryUndo,
    Action::HistoryRedo,
    Action::HistoryRepeat,
    Action::ViewOpen,
    Action::ViewBack,
    Action::ViewClose,
    Action::AskOpen,
    Action::V1Zoom,
    Action::ToggleAutoAccepted,
    Action::HelpOpen,
    Action::FindUnit,
    Action::GotoDefinition,
    Action::GotoCallers,
    Action::GotoEditor,
    Action::GotoBrowser,
    Action::ToolTests,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
    Action::V1Recommend,
    Action::V1Disagree,
    Action::V1InterpretTests,
    Action::V1DiveRerun,
    Action::ToolHistory,
    Action::ToolNotes,
    Action::ToolSettings,
    Action::FinishOpen,
    Action::ReviewRerun,
    Action::ReviewReset,
];

/// The actions the notes view handles.
const NOTES: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::NoteEdit,
    Action::NoteDelete,
    Action::ViewOpen,
    Action::ViewBack,
    Action::ViewClose,
    Action::HelpOpen,
    Action::FinishYank,
    Action::V1NotesToAgent,
    Action::V1NotesPost,
];

/// The actions the commits view handles.
const COMMITS: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::ViewOpen,
    Action::ViewBack,
    Action::ViewClose,
    Action::ToolHistory,
];

/// The actions a jump list (`gd`, `gr`) handles.
const JUMP: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::ViewOpen,
    Action::ViewBack,
    Action::ViewClose,
];

/// The actions the Ask view handles while the keys move between links.
const ASK: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::ViewOpen,
    Action::ViewBack,
    Action::V1NextTile,
    Action::ViewClose,
];

/// The panel open under the help, if any: its name and the actions it
/// handles.
fn help_panel(app: &App) -> Option<(&'static str, &'static [Action])> {
    match app.overlay {
        Overlay::Notes { .. } | Overlay::ConfirmDeleteNote { .. } => Some(("notes", NOTES)),
        Overlay::Commits { .. } => Some(("commits", COMMITS)),
        Overlay::Jump { .. } => Some(("jump list", JUMP)),
        Overlay::Finish { .. } => Some(("finish", super::finish::ACTIONS)),
        Overlay::Ask => Some(("ask", ASK)),
        _ => None,
    }
}

/// The actions the screen or panel the help is about handles: the open
/// panel, else the screen.
fn screen_actions(app: &App) -> &'static [Action] {
    match help_panel(app) {
        Some((_, actions)) => actions,
        None => match app.screen {
            Screen::Queue => super::queue::ACTIONS,
            Screen::Unit => super::unit::ACTIONS,
            Screen::Slim => LIST,
            Screen::Swipe => SWIPE,
            Screen::Flagged => FLAGGED,
        },
    }
}

/// What the help calls the screen or panel it is about.
fn screen_name(app: &App) -> &'static str {
    match help_panel(app) {
        Some((name, _)) => name,
        None => match app.screen {
            Screen::Queue => "queue",
            Screen::Unit => "unit",
            Screen::Slim => "list",
            Screen::Swipe => "swipe",
            Screen::Flagged => "Flagged",
        },
    }
}

/// The help's groups, in its order.
const GROUPS: [&str; 6] = ["move", "decide", "view", "go to", "tools", "finish"];

/// Which of [`GROUPS`] `a` is listed under.
fn group_of(a: Action) -> usize {
    use Action::*;
    match a {
        MoveDown | MoveUp | MoveFirst | MoveLast | MoveNextPending | MovePrevPending
        | MoveNextHunk | MovePrevHunk | MoveNextFile | MovePrevFile | ScrollHalfDown
        | ScrollHalfUp | ScrollPageDown | ScrollPageUp | V1NextTile | V1PrevTile => 0,
        DecideAccept | DecideFlag | NoteEdit | DecideAcceptFile | DecideFlagFile | VisualStart
        | HistoryUndo | HistoryRedo | HistoryRepeat | NoteDelete | V1ResolveOk | V1ResolveFix => 1,
        ViewOpen | ViewBack | ViewClose | AskOpen | ToggleAutoAccepted | HelpOpen | V1Swipe
        | ToggleByFile | V1Zoom => 2,
        FindUnit | GotoDefinition | GotoCallers | GotoEditor | GotoBrowser => 3,
        ToolTests | ToolRunOldNew | ToolDiagram | ToolHistory | ToolNotes | ToolSettings
        | V1Recommend | V1Disagree | V1InterpretTests | V1DiveRerun => 4,
        FinishOpen | FinishYank | ReviewRerun | ReviewReset | V1NotesToAgent | V1NotesPost => 5,
    }
}

/// Actions the help puts on one row when a screen has both.
const PAIRS: &[(Action, Action)] = &[
    (Action::MoveDown, Action::MoveUp),
    (Action::MoveFirst, Action::MoveLast),
    (Action::MoveNextPending, Action::MovePrevPending),
    (Action::MoveNextFile, Action::MovePrevFile),
    (Action::MoveNextHunk, Action::MovePrevHunk),
    (Action::ScrollHalfDown, Action::ScrollHalfUp),
    (Action::ScrollPageDown, Action::ScrollPageUp),
    (Action::V1NextTile, Action::V1PrevTile),
    (Action::DecideAccept, Action::DecideFlag),
    (Action::DecideAcceptFile, Action::DecideFlagFile),
    (Action::V1ResolveOk, Action::V1ResolveFix),
    (Action::HistoryUndo, Action::HistoryRedo),
    (Action::ViewOpen, Action::ViewBack),
    (Action::GotoDefinition, Action::GotoCallers),
];

/// What the help says after an action the v2 screens will drop.
const V1_SUFFIX: &str = " (until v2)";

/// Two descriptions on one row, the words they share said once:
/// `move down / up`, `next / previous file`.
fn joined(a: &str, b: &str) -> String {
    let x: Vec<&str> = a.split(' ').collect();
    let y: Vec<&str> = b.split(' ').collect();
    let pre = x.iter().zip(&y).take_while(|(p, q)| p == q).count();
    // Both keep a word of their own.
    let room = x.len().min(y.len()).saturating_sub(pre + 1);
    let suf = x
        .iter()
        .rev()
        .zip(y.iter().rev())
        .take(room)
        .take_while(|(p, q)| p == q)
        .count();
    if pre >= x.len().min(y.len()) {
        return format!("{a} / {b}");
    }
    let mut out = x[..pre].join(" ");
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(&x[pre..x.len() - suf].join(" "));
    out.push_str(" / ");
    out.push_str(&y[pre..y.len() - suf].join(" "));
    if suf > 0 {
        out.push(' ');
        out.push_str(&x[x.len() - suf..].join(" "));
    }
    out
}

/// One row of the help: the keys, what they do, and whether the v2 screens
/// will drop it.
struct HelpRow {
    keys: String,
    what: String,
    v1: bool,
}

/// The help's rows for `actions`, by group: each group's name and its
/// rows, pairs on one row, unbound actions left out.
fn help_groups(app: &App, actions: &[Action]) -> Vec<(&'static str, Vec<HelpRow>)> {
    let v1 = |a: Action| a.id().starts_with("v1.");
    let mut groups: Vec<(&'static str, Vec<HelpRow>)> =
        GROUPS.iter().map(|g| (*g, Vec::new())).collect();
    for &a in actions {
        // The second of a pair is on the first one's row.
        if PAIRS
            .iter()
            .any(|(x, y)| *y == a && actions.contains(x) && !app.keys.labels(*x).is_empty())
        {
            continue;
        }
        let keys = app.keys.labels(a);
        if keys.is_empty() {
            continue;
        }
        let other = PAIRS
            .iter()
            .find(|(x, y)| *x == a && actions.contains(y))
            .map(|(_, y)| *y)
            .filter(|y| !app.keys.labels(*y).is_empty());
        let row = match other {
            Some(b) => HelpRow {
                keys: format!("{keys} {}", app.keys.labels(b)),
                what: joined(a.describe(), b.describe()),
                v1: v1(a) || v1(b),
            },
            None => HelpRow {
                keys,
                what: a.describe().to_string(),
                v1: v1(a),
            },
        };
        groups[group_of(a)].1.push(row);
    }
    groups.retain(|(_, rows)| !rows.is_empty());
    groups
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
/// built from the keymap, grouped (move, decide, view, go to, tools,
/// finish), in as many columns as it takes and the terminal has room for.
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
        .map(|r| r.what.width() + if r.v1 { V1_SUFFIX.width() } else { 0 })
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
            let mut spans = vec![Span::styled(keys, t.accent())];
            let what = truncate(&r.what, room);
            let left = room.saturating_sub(what.width());
            spans.push(Span::raw(what));
            if r.v1 && left > 0 {
                spans.push(Span::styled(truncate(V1_SUFFIX, left), t.muted()));
            }
            Line::from(spans)
        }
    }
}

/// What the list's styles and icons mean, for the help.
fn help_legend(app: &App) -> Vec<Line<'static>> {
    let t = &app.theme;
    let level = |l: Level, what: &'static str| {
        [
            Span::styled(l.icon(), slim::level_style(app, l)),
            Span::raw(format!(" {what}  ")),
        ]
    };
    if app.screen == Screen::Queue {
        // The queue's glyphs, and the states at the end of its rows.
        return vec![
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
        ];
    }
    vec![
        Line::from(vec![
            Span::styled("legend  ", t.muted()),
            Span::styled("accepted", t.accepted_row()),
            Span::raw("  "),
            Span::styled("removed", Style::new().add_modifier(Modifier::CROSSED_OUT)),
            Span::raw("  "),
            Span::styled("flagged", t.flagged_row()),
        ]),
        Line::from(
            [Span::raw(" ".repeat(8))]
                .into_iter()
                .chain(level(Level::Attention, "close look"))
                .chain(level(Level::Look, "worth a look"))
                .chain(level(Level::Routine, "routine"))
                .collect::<Vec<_>>(),
        ),
        Line::from(vec![
            Span::raw(" ".repeat(8)),
            Span::styled("●", t.ok()),
            Span::raw(" ok  "),
            Span::styled("✗", t.needs_fix()),
            Span::raw(" needs fix"),
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
    let name = slim::row_name(u);
    let file = slim::file_name(&u.file);
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

    let keys = if editing.is_some() {
        vec![hint(fixed::ENTER, "save"), hint(fixed::ESC, "back")]
    } else {
        let verdicts = app.verdicts();
        let next = verdicts
            .iter()
            .position(|v| *v == verdict)
            .map_or(verdicts[0], |i| verdicts[(i + 1) % verdicts.len()]);
        let mut keys = vec![
            hint(fixed::ENTER, "send"),
            hint(fixed::EDIT, "edit comment"),
            hint(
                fixed::INLINE,
                if inline_on { "inline off" } else { "inline on" },
            ),
            hint(format!("{} {}", fixed::DOWN, fixed::UP), "scroll"),
            hint(fixed::ESC, "cancel"),
        ];
        if verdicts.len() > 1 {
            keys.insert(1, hint(fixed::VERDICT, next.label()));
        }
        keys
    };
    let (keys_line, plain) = hint_line(app, &keys);
    let foot_h = wrap(&plain, w).len().clamp(1, 2);
    let foot = Paragraph::new(keys_line).wrap(Wrap { trim: true });

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

    let keys = if editing.is_some() {
        vec![hint(fixed::ENTER, "save"), hint(fixed::ESC, "back")]
    } else {
        vec![
            hint(fixed::ENTER, "commit"),
            hint(fixed::EDIT, "edit message"),
            hint(format!("{} {}", fixed::DOWN, fixed::UP), "scroll"),
            hint(fixed::ESC, "cancel"),
        ]
    };
    let (keys_line, plain) = hint_line(app, &keys);
    let foot_h = wrap(&plain, w).len().clamp(1, 2);
    let foot = Paragraph::new(keys_line).wrap(Wrap { trim: true });

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{Edit, Keymap, Sheet, parse_label};

    fn key(label: &str) -> crossterm::event::KeyEvent {
        match parse_label(label).as_deref() {
            Some([k]) => k.event(),
            other => panic!("{label} is not one key: {other:?}"),
        }
    }

    #[test]
    fn fixed_labels_are_the_keys_the_modes_read() {
        let km = Keymap::default();
        for (label, sheet) in [
            (fixed::ENTER, Sheet::Confirm),
            (fixed::ESC, Sheet::Cancel),
            (fixed::EDIT, Sheet::Edit),
            (fixed::VERDICT, Sheet::Verdict),
            (fixed::INLINE, Sheet::Inline),
            (fixed::QUIT, Sheet::Quit),
            (fixed::DOWN, Sheet::Down),
            (fixed::UP, Sheet::Up),
        ] {
            assert_eq!(km.sheet(&key(label)), Some(sheet), "{label}");
        }
        for (label, edit) in [
            (fixed::ENTER, Edit::Submit),
            (fixed::ESC, Edit::Cancel),
            (fixed::ARROW_UP, Edit::Up),
            (fixed::ARROW_DOWN, Edit::Down),
            (fixed::PAGE_UP, Edit::PageUp),
            (fixed::PAGE_DOWN, Edit::PageDown),
            (fixed::RETRY, Edit::Retry),
            (fixed::TAB, Edit::Tab),
        ] {
            assert_eq!(km.edit(&key(label)), edit, "{label}");
        }
        assert!(km.confirm(&key(fixed::YES)));
        assert!(km.confirm(&key(fixed::ENTER)));
        assert!(!km.confirm(&key(fixed::ESC)));
    }

    #[test]
    fn a_pair_says_the_words_it_shares_once() {
        assert_eq!(joined("move down", "move up"), "move down / up");
        assert_eq!(joined("first unit", "last unit"), "first / last unit");
        assert_eq!(
            joined("accept the unit", "flag the unit"),
            "accept / flag the unit"
        );
        assert_eq!(
            joined("scroll half a page down", "scroll half a page up"),
            "scroll half a page down / up"
        );
        assert_eq!(
            joined("undo the last decision", "redo"),
            "undo the last decision / redo"
        );
        assert_eq!(joined("open", "back"), "open / back");
        assert_eq!(joined("same", "same"), "same / same");
    }

    #[test]
    fn every_action_is_on_some_screen_once() {
        let tables = [LIST, SWIPE, FLAGGED, NOTES, COMMITS, JUMP, ASK];
        for a in Action::all() {
            assert!(
                tables.iter().any(|t| t.contains(&a)),
                "{} is on no screen",
                a.id()
            );
        }
        for t in tables {
            for (i, a) in t.iter().enumerate() {
                assert!(!t[i + 1..].contains(a), "{} twice", a.id());
            }
        }
    }
}
