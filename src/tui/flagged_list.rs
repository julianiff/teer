//! The flagged units one after another (`<space>f`), each as the queue's
//! preview shows it: why it has its level, what the agent said, the code
//! change with the statements written in, the note. One is selected: the
//! `▌` bar and a background behind it. `j` `k` (and `n` `N`) select the
//! next and previous one and scroll it to the top, `]c` `[c` go from hunk
//! to hunk, the scroll keys and the wheel scroll; a selected unit
//! scrolled out of view hands the selection to the one at the top. `l` or
//! `enter` looks closer (the unit view; `h` there comes back here), `a`
//! accepts it (it leaves the list), `i` writes its note, `h` goes back.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::app::{App, Effect, Screen, Target};
use super::unit::{code_edge, code_row};
use super::{diff, preview, slim};
use crate::keymap::Action;
use crate::review::model::{Decision, Unit};
use crate::util::truncate;

/// The actions the flagged list handles, for the help, in the order it
/// lists them inside each of its groups.
pub const ACTIONS: &[Action] = &[
    Action::MoveNextPending,
    Action::MovePrevPending,
    Action::MoveNextHunk,
    Action::MovePrevHunk,
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
    Action::NoteEdit,
    Action::HistoryUndo,
    Action::HistoryRedo,
    Action::ViewOpen,
    Action::AskOpen,
    Action::GotoDefinition,
    Action::GotoCallers,
    Action::GotoEditor,
    Action::ToolAdvice,
    Action::ToolTests,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
    Action::ToolFlagged,
    Action::ToolHistory,
    Action::ToolNotes,
    Action::HelpOpen,
    Action::FinishOpen,
    Action::ViewClose,
];

/// What the flagged list says an action does where `?` lists it, when
/// that differs from the keymap's words.
pub fn describe(a: Action) -> Option<&'static str> {
    Some(match a {
        Action::MoveNextPending | Action::MoveDown => "next flagged unit",
        Action::MovePrevPending | Action::MoveUp => "previous flagged unit",
        Action::ViewOpen => "look closer at it",
        Action::DecideAccept => "accept it: it leaves the list",
        Action::ToolFlagged => "back",
        _ => return None,
    })
}

/// The flagged list's state.
#[derive(Debug, Default)]
pub struct FlaggedView {
    /// The screen `h` goes back to.
    pub from: Option<Screen>,
    /// The selected unit.
    pub at: String,
    /// How far the list is scrolled, in lines.
    pub offset: Cell<usize>,
    /// The next draw scrolls the selected unit to the top.
    pub reveal: Cell<bool>,
    /// Each box as last drawn: its unit and the lines it spans.
    pub boxes: RefCell<Vec<(String, Range<usize>)>>,
    /// Where each box and each hunk in it starts, in lines, as last drawn.
    pub hunks: RefCell<Vec<usize>>,
    /// How far the list can scroll and its rows, as last drawn.
    pub max: Cell<usize>,
    pub rows: Cell<usize>,
}

// ---- Drawing -----------------------------------------------------------

/// Draw the flagged list into `area`: the title row, then the boxes.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    if area.width < 12 || area.height == 0 {
        return;
    }
    let ids = app.flagged_ids();
    let at = app.flagged_index(&ids);
    let [title, _, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(area.height >= 12)),
        Constraint::Min(0),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(title_line(app, ids.len(), at, title.width as usize)),
        title,
    );
    if ids.is_empty() {
        app.flagged.boxes.borrow_mut().clear();
        app.flagged.hunks.borrow_mut().clear();
        let text = format!(
            " Nothing is flagged. {} flags a unit; {} goes back.",
            app.keys.label(Action::DecideFlag),
            app.keys.label(Action::ViewBack)
        );
        f.render_widget(Paragraph::new(Line::styled(text, t.muted())), body);
        return;
    }
    if body.width < 12 || body.height == 0 {
        return;
    }
    let w = (body.width as usize).saturating_sub(2);
    let h = body.height as usize;
    let mut blocks: Vec<Vec<Line<'static>>> = Vec::new();
    let mut lines = 0;
    let mut boxes = Vec::with_capacity(ids.len());
    let mut hunks = Vec::new();
    for (k, id) in ids.iter().enumerate() {
        let Some(u) = app.review.unit(id) else {
            continue;
        };
        if k > 0 {
            blocks.push(vec![Line::default()]);
            lines += 1;
        }
        let start = lines;
        hunks.push(start);
        for b in entry(app, u, w, &mut hunks, start) {
            lines += b.len();
            blocks.push(
                b.into_iter()
                    .map(|l| row_of(app, l, w + 1, k == at))
                    .collect(),
            );
        }
        boxes.push((id.clone(), start..lines));
    }
    let mut offset = app.flagged.offset.get();
    if app.flagged.reveal.take()
        && let Some((_, r)) = boxes.get(at)
    {
        offset = r.start;
    }
    let starts: Vec<usize> = blocks
        .iter()
        .scan(0, |n, b| {
            let s = *n;
            *n += b.len();
            Some(s)
        })
        .collect();
    let s = preview::scroll_rows(app, blocks, w, h, 1, false, offset);
    app.flagged.offset.set(s.offset);
    app.flagged.max.set(s.max);
    app.flagged.rows.set(h);
    app.detail_max_scroll.set(s.max);
    app.detail_rows.set(h);
    // A click on a unit selects it: the rows each one shows.
    let above = starts.iter().copied().find(|&n| n >= s.offset).unwrap_or(0);
    let top = usize::from(above > 0 && h >= 3);
    let shown = s.lines.len().saturating_sub(top + usize::from(s.below > 0));
    let view = above..above + shown;
    let mut targets = app.targets.borrow_mut();
    for (k, (_, r)) in boxes.iter().enumerate() {
        let from = r.start.max(view.start);
        let to = r.end.min(view.end);
        if from < to {
            let y = body.y + (top + from - above) as u16;
            let rect = Rect::new(body.x, y, body.width, (to - from) as u16);
            targets.push((rect, Target::Flagged(k)));
        }
    }
    drop(targets);
    *app.flagged.boxes.borrow_mut() = boxes;
    *app.flagged.hunks.borrow_mut() = hunks;
    super::ui::text_area(app, body, 0);
    f.render_widget(Paragraph::new(s.lines), body);
}

/// The title row: `⚑ Flagged · N units`, and on the right which one has
/// the selection and the keys that move it.
fn title_line(app: &App, n: usize, at: usize, w: usize) -> Line<'static> {
    let t = &app.theme;
    let k = |a: Action| app.keys.label(a);
    let units = if n == 1 { "unit" } else { "units" };
    let mut spans = vec![
        Span::styled(" ⚑ Flagged", t.badge_investigate()),
        Span::styled(format!(" · {n} {units}"), t.muted()),
    ];
    let mut right = String::new();
    if n > 0 {
        right = format!(
            "{}/{n} · {} {} select · {} look closer · {} accept · {} back ",
            at + 1,
            k(Action::MoveDown),
            k(Action::MoveUp),
            k(Action::ViewOpen),
            k(Action::DecideAccept),
            k(Action::ViewBack),
        );
    }
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    let right = truncate(&right, w.saturating_sub(used + 2));
    spans.push(Span::raw(
        " ".repeat(w.saturating_sub(used + right.width())),
    ));
    spans.push(Span::styled(right, t.muted()));
    Line::from(spans)
}

/// Unit `u` as the queue's preview shows it, `w` cells wide, as blocks a
/// scroll keeps whole: its level, name and place; why it has its level,
/// its commits and what the agent said; then its code change in a box,
/// the statements written under the lines they cite, and the note under
/// it. Where each hunk starts, the entry starting at line `start`, goes
/// on `hunks`.
fn entry(
    app: &App,
    u: &Unit,
    w: usize,
    hunks: &mut Vec<usize>,
    start: usize,
) -> Vec<Vec<Line<'static>>> {
    let mut head = vec![Span::raw(" ")];
    if let Some(a) = app.assessment(&u.id) {
        let level = a.level.shown(u.auto_accepted());
        head.push(Span::styled(
            level.icon().to_string(),
            slim::level_style(app, level),
        ));
    }
    let used: usize = head.iter().map(Span::width).sum();
    head.extend(preview::unit_head(app, u, w.saturating_sub(used)).spans);
    let mut out = vec![vec![Line::from(head)]];
    // The code change in its box, when there is one; else its statements
    // and the note are said in the text, as the preview does.
    let code = diff::code_change_of(app, u, w.saturating_sub(5), diff::Notes::Inline);
    let boxed = !code.blocks.is_empty();
    let mut groups = preview::unit_groups(app, u, w, !boxed).into_iter();
    out.extend(
        groups
            .next()
            .unwrap_or_default()
            .into_iter()
            .map(|l| vec![l]),
    );
    for g in groups.filter(|g| !g.is_empty()) {
        out.push(vec![Line::default()]);
        out.extend(g.into_iter().map(|l| vec![l]));
    }
    if !boxed {
        return out;
    }
    out.push(vec![Line::default()]);
    let bw = w.saturating_sub(1);
    let counts = diff::counts(app, code.added, code.removed);
    out.push(vec![code_edge(app, bw, Some(counts))]);
    let mut at = start + out.iter().map(Vec::len).sum::<usize>();
    let mut starts = Vec::with_capacity(code.blocks.len());
    for b in &code.blocks {
        starts.push(at);
        at += b.len();
    }
    hunks.extend(code.hunks.iter().filter_map(|&h| starts.get(h).copied()));
    out.extend(
        code.blocks
            .into_iter()
            .map(|b| b.into_iter().map(|l| code_row(app, l, bw)).collect()),
    );
    out.push(vec![code_edge(app, bw, None)]);
    let card = super::notes::card(app, u, bw);
    out.extend(card.into_iter().map(|l| {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(l.spans);
        vec![Line::from(spans)]
    }));
    out
}

/// Line `l` of an entry, `w` cells wide after the bar's column: the bar
/// and the selection's background behind the whole row when `selected`.
/// Spans that have a background of their own keep it.
fn row_of(app: &App, l: Line<'static>, w: usize, selected: bool) -> Line<'static> {
    let t = &app.theme;
    if !selected {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(l.spans);
        return Line::from(spans).style(l.style);
    }
    let bg = t.selected_entry();
    let fill = w.saturating_sub(l.width());
    let base = l.style;
    let mut spans = vec![Span::styled("▌", t.accent().patch(bg))];
    spans.extend(l.spans.into_iter().map(|s| {
        let own = base.patch(s.style);
        let style = if own.bg.is_some() { own } else { own.patch(bg) };
        s.style(style)
    }));
    spans.push(Span::styled(" ".repeat(fill), bg));
    Line::from(spans)
}

// ---- Behavior ----------------------------------------------------------

impl App {
    /// The units flagged (`f`), in the queue's order; any the queue
    /// doesn't list come after, in the review's.
    pub fn flagged_ids(&self) -> Vec<String> {
        let flagged = |u: &&Unit| u.decision == Decision::Investigate;
        let mut seen = HashSet::new();
        let mut ids: Vec<String> = self
            .order
            .iter()
            .map(|&i| &self.review.units[i])
            .filter(flagged)
            .map(|u| {
                seen.insert(u.id.clone());
                u.id.clone()
            })
            .collect();
        ids.extend(
            self.review
                .units
                .iter()
                .filter(flagged)
                .filter(|u| !seen.contains(&u.id))
                .map(|u| u.id.clone()),
        );
        ids
    }

    /// Where the selected unit is among `ids`: the unit [`FlaggedView::at`]
    /// names; once it left the list, the one that took its place (the
    /// next, or the last).
    pub fn flagged_index(&self, ids: &[String]) -> usize {
        if let Some(i) = ids.iter().position(|id| *id == self.flagged.at) {
            return i;
        }
        let was = self
            .flagged
            .boxes
            .borrow()
            .iter()
            .position(|(id, _)| *id == self.flagged.at);
        // The units before it that are still flagged.
        let before = match was {
            Some(w) => self.flagged.boxes.borrow()[..w]
                .iter()
                .filter(|(id, _)| ids.contains(id))
                .count(),
            None => 0,
        };
        before.min(ids.len().saturating_sub(1))
    }

    /// `<space>f`: the flagged list, the unit on show selected when it
    /// is flagged, else the first. Pressed in the list, back.
    pub(super) fn open_flagged(&mut self) -> Vec<Effect> {
        if self.screen == Screen::Flagged {
            self.flagged_back();
            return Vec::new();
        }
        let ids = self.flagged_ids();
        let Some(first) = ids.first() else {
            self.status = format!(
                "Nothing is flagged yet: {} flags a unit.",
                self.keys.label(Action::DecideFlag)
            );
            return Vec::new();
        };
        let here = (self.current_unit().map(|u| u.id.clone())).filter(|id| ids.contains(id));
        self.flagged.at = here.unwrap_or_else(|| first.clone());
        self.flagged.from = Some(self.screen);
        self.flagged.reveal.set(true);
        self.flagged.boxes.borrow_mut().clear();
        self.screen = Screen::Flagged;
        Vec::new()
    }

    /// Back to the screen the list was opened from; in the queue, its
    /// cursor on the unit selected here.
    fn flagged_back(&mut self) {
        let to = self.flagged.from.take().unwrap_or(Screen::Queue);
        if to == Screen::Queue
            && let Some(p) = self.position_of(&self.flagged.at.clone())
        {
            self.sel = p;
        }
        self.screen = to;
    }

    /// Select the unit at `i` of `ids` and scroll it to the top.
    fn flagged_focus(&mut self, ids: &[String], i: usize) {
        if let Some(id) = ids.get(i) {
            self.flagged.at = id.clone();
            self.flagged.reveal.set(true);
        }
    }

    /// A click on the unit at `i`, as last drawn, selects it.
    pub(super) fn flagged_click(&mut self, i: usize) {
        let id = self.flagged.boxes.borrow().get(i).map(|(id, _)| id.clone());
        if let Some(id) = id {
            self.flagged.at = id;
        }
    }

    /// The actions of the flagged list (after those every screen shares).
    pub(super) fn flagged_action(&mut self, a: Action, count: Option<u32>) -> Vec<Effect> {
        let times = count.unwrap_or(1) as usize;
        let ids = self.flagged_ids();
        if ids.is_empty() {
            if matches!(a, Action::ViewBack | Action::ViewClose) {
                self.flagged_back();
            }
            return Vec::new();
        }
        let at = self.flagged_index(&ids);
        let id = ids[at].clone();
        match a {
            Action::MoveNextPending
            | Action::MovePrevPending
            | Action::MoveDown
            | Action::MoveUp => {
                let next = matches!(a, Action::MoveNextPending | Action::MoveDown);
                let to = if next {
                    (at + times).min(ids.len() - 1)
                } else {
                    at.saturating_sub(times)
                };
                if to == at {
                    self.status = "No more flagged units that way.".into();
                }
                self.flagged_focus(&ids, to);
            }
            Action::MoveNextHunk | Action::MovePrevHunk => {
                self.flagged_hunk(a == Action::MoveNextHunk, times);
            }
            Action::MoveFirst => self.flagged_focus(&ids, 0),
            Action::MoveLast => self.flagged_focus(&ids, ids.len() - 1),
            Action::ViewOpen => return self.open_unit(id),
            Action::ViewBack | Action::ViewClose => self.flagged_back(),
            Action::DecideAccept => {
                let mut effects = match self.position_of(&id) {
                    Some(p) => {
                        self.sel = p;
                        self.queue_action(a, None)
                    }
                    None => {
                        self.decide_ids(std::slice::from_ref(&id), Decision::Accepted);
                        vec![Effect::Save]
                    }
                };
                // The next one takes its place.
                let rest = self.flagged_ids();
                let next = ids[at + 1..].iter().find(|i| rest.contains(i));
                let next = next.or(rest.last()).cloned();
                if let Some(next) = next {
                    self.flagged.at = next;
                    self.flagged.reveal.set(true);
                }
                effects.extend(self.ensure_prepared());
                return effects;
            }
            Action::DecideAcceptFile | Action::DecideFlagFile => {
                self.status = "Whole files are decided in the queue or the unit view.".into();
            }
            _ => {}
        }
        Vec::new()
    }

    /// Scroll the list by `by` lines. A selected unit scrolled out of view
    /// hands the selection to the one at the top.
    pub(super) fn flagged_scroll(&mut self, by: isize) -> Vec<Effect> {
        let to = self
            .flagged
            .offset
            .get()
            .saturating_add_signed(by)
            .min(self.flagged.max.get());
        self.flagged.offset.set(to);
        let rows = self.flagged.rows.get().max(1);
        let view = to..to + rows;
        let boxes = self.flagged.boxes.borrow();
        let seen = |r: &Range<usize>| r.start < view.end && view.start < r.end;
        let out_of_view = boxes
            .iter()
            .find(|(id, _)| *id == self.flagged.at)
            .is_none_or(|(_, r)| !seen(r));
        let top = boxes
            .iter()
            .rev()
            .find(|(_, r)| r.start <= to)
            .or(boxes.first())
            .map(|(id, _)| id.clone());
        drop(boxes);
        if out_of_view && let Some(top) = top {
            self.flagged.at = top;
        }
        Vec::new()
    }

    /// `]c` (`[c`): scroll to the `times`-th box or hunk below (above) the
    /// top, as far as there are.
    fn flagged_hunk(&mut self, forward: bool, times: usize) {
        let hunks = self.flagged.hunks.borrow().clone();
        let at = self.flagged.offset.get();
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
        let to = to.min(self.flagged.max.get());
        if to == at {
            self.status = if forward {
                "No more hunks below."
            } else {
                "No more hunks above."
            }
            .into();
            return;
        }
        self.flagged_scroll(to as isize - at as isize);
    }

    /// `u` or `ctrl+r` changed unit `first` first: when it is flagged, the
    /// selection on it.
    pub(super) fn flagged_show_step(&mut self, first: &str) -> Vec<Effect> {
        let ids = self.flagged_ids();
        if let Some(i) = ids.iter().position(|id| id == first) {
            self.flagged_focus(&ids, i);
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::git::{ChangedFile, FileStatus};
    use crate::keymap::{Keymap, parse_keys};
    use crate::review::model::{Prep, Range as Lines, Review, SessionRef, UnitKind, UnitStatus};
    use std::path::PathBuf;

    fn unit(i: usize, file: &str, decision: Decision, note: &str) -> Unit {
        let mut u = Unit {
            id: format!("u{i}"),
            file: PathBuf::from(file),
            symbol: format!("s{i}"),
            kind: UnitKind::Function,
            status: UnitStatus::Changed,
            old_range: Some(Lines::new(1, 20)),
            new_range: Some(Lines::new(1, 20)),
            renamed_from: None,
            fingerprint: format!("fp{i}"),
            changes: Vec::new(),
            prep: Prep::Skipped,
            callers: Vec::new(),
            callees: Vec::new(),
            decision,
            summary: None,
            nbc_reason: None,
            dive: Default::default(),
            accepted_by: None,
            reasons: Vec::new(),
            dive_pending: Default::default(),
            checks: Default::default(),
        };
        u.dive.note = note.into();
        u
    }

    /// Twenty lines, changed at 2 and 17.
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

    /// Four units in `a.rs` and `b.rs`; `u1` and `u3` flagged.
    fn app() -> App {
        let units = vec![
            unit(0, "a.rs", Decision::Pending, ""),
            unit(1, "a.rs", Decision::Investigate, "handle the empty case"),
            unit(2, "b.rs", Decision::Pending, ""),
            unit(3, "b.rs", Decision::Investigate, ""),
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

    /// The screen as text, and each row's background where its text
    /// starts (column 3, past the bar).
    fn render_bg(a: &App, w: u16, h: u16) -> (String, Vec<ratatui::style::Color>) {
        use ratatui::{Terminal, backend::TestBackend};
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| super::super::ui::draw(f, a)).unwrap();
        let buf = term.backend().buffer().clone();
        let text = (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        let bg = (0..h).map(|y| buf[(w - 2, y)].bg).collect();
        (text, bg)
    }

    #[test]
    fn space_f_lists_the_flagged_units_as_the_preview_shows_them() {
        let mut a = app();
        press(&mut a, "<space>f");
        assert_eq!(a.screen, Screen::Flagged);
        assert_eq!(a.flagged.at, "u1");
        let text = render(&a, 100, 60);
        let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s}:\n{text}"));
        assert!(text.contains("⚑ Flagged · 2 units"), "{text}");
        assert!(text.contains("1/2"), "{text}");
        // One after the other, each with its code change in a box; the
        // note in its card under it, and no word about the one without.
        assert!(at("s1") < at("code change"), "{text}");
        assert!(at("code change") < at("✎ you"), "{text}");
        assert!(at("handle the empty case") < at("s3"), "{text}");
        assert_eq!(text.matches("✎").count(), 1, "{text}");
        assert!(!text.contains("no note"), "{text}");
        assert!(!text.contains("s0") && !text.contains("s2"), "{text}");
        assert_eq!(text.matches("+2 −2").count(), 2, "{text}");
    }

    #[test]
    fn the_selected_unit_has_the_bar_and_a_background_the_others_do_not() {
        let mut a = app();
        press(&mut a, "<space>f");
        let bar = a.theme.bar;
        let (text, bg) = render_bg(&a, 100, 60);
        let row = |s: &str| text.lines().position(|l| l.contains(s)).unwrap();
        let (first, second) = (row(" s1"), row(" s3"));
        assert_eq!(bg[first], bar, "{text}");
        assert_eq!(bg[row("handle the empty case")], bar, "{text}");
        assert!(text.lines().nth(first).unwrap().starts_with('▌'), "{text}");
        assert_ne!(bg[second], bar, "{text}");
        assert!(
            !text.lines().nth(second).unwrap().starts_with('▌'),
            "{text}"
        );
        press(&mut a, "j");
        let (text, bg) = render_bg(&a, 100, 60);
        let row = |s: &str| text.lines().position(|l| l.contains(s)).unwrap();
        assert_eq!(bg[row(" s3")], bar, "{text}");
    }

    #[test]
    fn j_selects_the_next_and_l_looks_closer_and_h_comes_back() {
        let mut a = app();
        let from = a.screen;
        press(&mut a, "<space>f");
        render(&a, 100, 20);
        press(&mut a, "j");
        assert_eq!(a.flagged.at, "u3");
        press(&mut a, "n");
        assert_eq!(a.status, "No more flagged units that way.");
        let text = render(&a, 100, 20);
        assert!(text.contains("2/2"), "{text}");
        // The selected one at the top.
        assert!(a.flagged.offset.get() > 0, "{text}");
        press(&mut a, "kN");
        assert_eq!(a.flagged.at, "u1");
        press(&mut a, "j");
        press(&mut a, "l");
        assert_eq!((a.screen, a.unit.id.as_str()), (Screen::Unit, "u3"));
        press(&mut a, "h");
        assert_eq!(a.screen, Screen::Flagged);
        press(&mut a, "h");
        assert_eq!(a.screen, from);
        assert_eq!(a.selected_id().as_deref(), Some("u3"));
    }

    #[test]
    fn from_the_unit_view_it_comes_back_there_and_then_to_the_queue() {
        let mut a = app();
        let from = a.screen;
        a.open_unit("u1".into());
        press(&mut a, "<space>f");
        assert_eq!(a.flagged.at, "u1", "the unit on show is selected");
        render(&a, 100, 40);
        press(&mut a, "nl");
        assert_eq!(a.unit.id, "u3");
        press(&mut a, "h");
        assert_eq!(a.screen, Screen::Flagged);
        press(&mut a, "h");
        assert_eq!(a.screen, from, "not back into the unit view");
        // From the unit view and straight back.
        a.open_unit("u0".into());
        press(&mut a, "<space>f");
        assert_eq!(a.flagged.at, "u1", "not flagged: the first one");
        press(&mut a, "h");
        assert_eq!((a.screen, a.unit.id.as_str()), (Screen::Unit, "u0"));
    }

    #[test]
    fn a_accepts_the_selected_unit_and_the_next_takes_its_place() {
        let mut a = app();
        press(&mut a, "<space>f");
        render(&a, 100, 40);
        press(&mut a, "a");
        assert_eq!(a.review.unit("u1").unwrap().decision, Decision::Accepted);
        assert_eq!(a.screen, Screen::Flagged);
        assert_eq!(a.flagged.at, "u3");
        let text = render(&a, 100, 40);
        assert!(text.contains("1 unit"), "{text}");
        // `u` puts the flag back, and the selection on it.
        press(&mut a, "u");
        assert_eq!(a.review.unit("u1").unwrap().decision, Decision::Investigate);
        assert_eq!(a.flagged.at, "u1");
        // The last one accepted: the list says so.
        press(&mut a, "aa");
        let text = render(&a, 100, 40);
        assert!(text.contains("Nothing is flagged"), "{text}");
    }

    #[test]
    fn nothing_flagged_says_how_to_flag_and_stays() {
        let mut a = app();
        for u in &mut a.review.units {
            u.decision = Decision::Pending;
        }
        press(&mut a, "<space>f");
        assert_eq!(a.screen, Screen::Queue);
        assert!(
            a.status.starts_with("Nothing is flagged yet"),
            "{}",
            a.status
        );
    }

    #[test]
    fn scrolling_the_selected_unit_out_of_view_moves_the_selection_and_hunks_jump() {
        let mut a = app();
        press(&mut a, "<space>f");
        render(&a, 100, 14);
        let second = a.flagged.boxes.borrow()[1].1.start;
        press(&mut a, "]c");
        assert!(a.flagged.offset.get() > 0);
        assert_eq!(a.flagged.at, "u1", "still in view");
        while a.flagged.offset.get() < second {
            press(&mut a, "<c-d>");
            render(&a, 100, 14);
        }
        assert_eq!(a.flagged.at, "u3");
        press(&mut a, "gg");
        assert_eq!(a.flagged.at, "u1");
    }

    #[test]
    fn a_click_on_a_unit_selects_it() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut a = app();
        press(&mut a, "<space>f");
        let text = render(&a, 100, 60);
        let row = text.lines().position(|l| l.contains(" s3")).unwrap() as u16;
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            a.on_mouse(MouseEvent {
                kind,
                column: 10,
                row,
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
        }
        assert_eq!(a.flagged.at, "u3");
    }
}
