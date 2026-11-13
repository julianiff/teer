//! The review on the terminal. [`App`] is the review's state
//! ([`session::App`], which it derefs to) and what only the terminal keeps:
//! the colours, where each clickable thing was
//! drawn, the screen as drawn and the text marked with the mouse. Terminal
//! keys and mouse events come in here.

use std::cell::{Cell, RefCell};
use std::ops::{Deref, DerefMut};
use std::time::Instant;

use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::select;
use super::theme::Theme;
use crate::config::Config;
use crate::keymap::{Action, Key, Keymap};
use crate::review::model::Review;
use crate::session::app::SCROLL_STEP;
use crate::session::unit::Panel;
use crate::session::{self, AppEvent, Effect, Overlay, Screen};

/// Clickable regions registered while drawing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Action(Action),
    Unit(usize),
    /// The detail area (the queue's preview, the unit view's text): the
    /// wheel scrolls it, a click does nothing.
    Detail,
    /// A tile beside the unit view's text: the wheel scrolls it, a click
    /// gives it the focus (on the ask tile, typing).
    Panel(Panel),
    /// A link in the ask panel, by index into
    /// [`session::App::ask_links`].
    AskLink(usize),
    /// A key named on screen (`esc close`): a click presses it.
    Key(KeyEvent),
    /// A note in the notes view, by its place there: a click selects it.
    Note(usize),
    /// A unit's box in the flagged list, by its place there: a click gives
    /// it the focus.
    Flagged(usize),
    /// The question on the unit view's Tests tile: a click has the agent
    /// read the tests teer found.
    ReadTests,
    /// A question the ask tile offers, by its place in
    /// [`session::App::suggestions`]: a click asks it.
    Suggest(usize),
}

pub struct App {
    /// The review and everything done to it.
    pub core: session::App,
    pub theme: Theme,

    // Written while drawing.
    pub targets: RefCell<Vec<(Rect, Target)>>,
    /// Where a mouse selection may run (see [`super::select`]).
    pub text_areas: RefCell<Vec<select::Area>>,
    /// The screen as last drawn, to copy from.
    pub shown: RefCell<ratatui::buffer::Buffer>,
    /// Text marked with the mouse, being selected or just copied.
    pub selection: Option<select::Selection>,
    /// When and where the last click was, to tell a double click.
    pub last_click: Option<(Instant, u16, u16)>,
    /// The queue's first row shown, as last drawn.
    pub slim_scroll: Cell<usize>,
    /// Syntax colours of the files drawn so far (see [`crate::syntax`]).
    pub syntax: crate::syntax::Cache,
}

/// Drawing reads the review's state everywhere: through the deref, as if
/// it were the terminal's own.
impl Deref for App {
    type Target = session::App;

    fn deref(&self) -> &session::App {
        &self.core
    }
}

impl DerefMut for App {
    fn deref_mut(&mut self) -> &mut session::App {
        &mut self.core
    }
}

impl App {
    pub fn new(review: Review, cfg: Config, keys: Keymap) -> App {
        App::wrap(session::App::new(review, cfg, keys))
    }

    /// The terminal's view of `core`.
    pub fn wrap(core: session::App) -> App {
        App {
            theme: Theme::new(core.cfg.theme),
            core,
            targets: RefCell::new(Vec::new()),
            text_areas: RefCell::new(Vec::new()),
            shown: RefCell::default(),
            selection: None,
            last_click: None,
            slim_scroll: Cell::new(0),
            syntax: Default::default(),
        }
    }

    pub fn on_event(&mut self, ev: Event) -> Vec<Effect> {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            Event::Mouse(m) => self.on_mouse(m),
            _ => Vec::new(),
        }
    }

    /// A key typed: the text marked with the mouse is let go, and the
    /// review takes the key ([`session::App::on_key`]).
    pub fn on_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        self.selection = None;
        self.core.on_key(Key::from_event(&k))
    }

    /// An event from the runtime ([`session::App::on_app_event`]). A
    /// re-run's review has new contents: their colours are worked out again.
    pub fn on_app_event(&mut self, ev: AppEvent) -> Vec<Effect> {
        if matches!(ev, AppEvent::Rerun(Ok(_))) {
            self.syntax.clear();
        }
        self.core.on_app_event(ev)
    }

    fn hit(&self, col: u16, row: u16) -> Option<Target> {
        self.targets
            .borrow()
            .iter()
            .rev()
            .find(|(r, _)| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height)
            .map(|(_, t)| *t)
    }

    /// The text area under (`col`, `row`) as last drawn.
    fn text_area_at(&self, col: u16, row: u16) -> select::Area {
        select::area_at(
            &self.text_areas.borrow(),
            self.shown.borrow().area,
            col,
            row,
        )
    }

    /// A second click on the same cell soon after the first.
    fn double_click(&mut self, col: u16, row: u16) -> bool {
        let now = std::time::Instant::now();
        let double = self.last_click.is_some_and(|(at, c, r)| {
            (c, r) == (col, row) && now.duration_since(at) <= select::DOUBLE_CLICK
        });
        self.last_click = (!double).then_some((now, col, row));
        double
    }

    /// Copy what the selection marks; it stays marked.
    fn copy_selection(&mut self, mut s: select::Selection) -> Vec<Effect> {
        let text = select::text(&self.shown.borrow(), &s);
        if text.trim().is_empty() {
            self.selection = None;
            return Vec::new();
        }
        s.copied = true;
        self.selection = Some(s);
        vec![Effect::Copy {
            what: select::name(&text),
            text,
        }]
    }

    /// Copy the word under (`col`, `row`), marked like a selection.
    fn copy_word(&mut self, col: u16, row: u16) -> Vec<Effect> {
        let area = self.text_area_at(col, row);
        let Some((word, x0, x1)) = select::word_at(&self.shown.borrow(), area, col, row) else {
            return Vec::new();
        };
        let mut s = select::Selection::start(area, x0, row);
        s.extend(x1, row);
        s.copied = true;
        self.selection = Some(s);
        vec![Effect::Copy {
            what: select::name(&word),
            text: word,
        }]
    }

    pub fn on_mouse(&mut self, m: MouseEvent) -> Vec<Effect> {
        let effects = self.mouse(m);
        self.save_cursor(effects)
    }

    /// [`App::on_mouse`], before the cursor is saved.
    fn mouse(&mut self, m: MouseEvent) -> Vec<Effect> {
        // The button going down starts a selection, a drag extends it, and
        // the release copies it; a release where it went down is a click.
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let area = self.text_area_at(m.column, m.row);
                self.selection = Some(select::Selection::start(area, m.column, m.row));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(s) = self.selection.as_mut() {
                    s.extend(m.column, m.row);
                }
            }
            // The text moves away under it.
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => self.selection = None,
            _ => {}
        }
        if m.kind == MouseEventKind::Up(MouseButton::Left) {
            match self.selection.take() {
                Some(s) if s.moved() && !s.copied => return self.copy_selection(s),
                _ => {}
            }
            // A double click copies the word, except on what a click does something with.
            let button = matches!(
                self.hit(m.column, m.row),
                Some(
                    Target::Action(_)
                        | Target::Key(_)
                        | Target::AskLink(_)
                        | Target::Note(_)
                        | Target::ReadTests
                        | Target::Suggest(_)
                )
            );
            if self.double_click(m.column, m.row) && !button {
                return self.copy_word(m.column, m.row);
            }
        }
        match m.kind {
            // Over the detail area the wheel scrolls it; elsewhere it moves
            // the selection.
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = m.kind == MouseEventKind::ScrollDown;
                let by = if down { SCROLL_STEP } else { -SCROLL_STEP };
                let hit = self.hit(m.column, m.row);
                if self.screen == Screen::Unit {
                    // A link is in the ask tile.
                    let over = match hit {
                        Some(Target::Panel(p)) => Some(p),
                        Some(Target::AskLink(_) | Target::Suggest(_)) => Some(Panel::Ask),
                        _ => None,
                    };
                    return self.unit_wheel(over, by);
                }
                if self.screen == Screen::Flagged {
                    return self.flagged_scroll(by);
                }
                if hit == Some(Target::Detail) {
                    return self.scroll_detail(by);
                }
                let a = if down {
                    Action::MoveDown
                } else {
                    Action::MoveUp
                };
                self.run_action(a, None)
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.overlay != Overlay::None {
                    match (self.hit(m.column, m.row), &self.overlay) {
                        (Some(Target::Key(k)), _) => return self.on_key(k),
                        (Some(Target::Note(i)), Overlay::Notes { .. }) => {
                            self.overlay = Overlay::Notes { sel: i };
                        }
                        _ => {}
                    }
                    return Vec::new();
                }
                match self.hit(m.column, m.row) {
                    Some(Target::Key(k)) => self.on_key(k),
                    Some(Target::Note(_)) => Vec::new(),
                    Some(Target::Action(a)) => self.run_action(a, None),
                    Some(Target::Unit(pos)) => self.move_sel(pos),
                    Some(Target::AskLink(i)) => self.follow_ask_link(i),
                    Some(Target::Flagged(i)) => {
                        self.flagged_click(i);
                        Vec::new()
                    }
                    Some(Target::ReadTests) => self.read_tests(),
                    Some(Target::Suggest(i)) => self.take_suggestion(i),
                    Some(Target::Panel(p)) => {
                        self.unit_click_tile(Some(p));
                        Vec::new()
                    }
                    Some(Target::Detail) if self.screen == Screen::Unit => {
                        self.unit_click_tile(None);
                        Vec::new()
                    }
                    Some(Target::Detail) | None => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::Decision;
    use crossterm::event::KeyModifiers;

    fn app(n: usize) -> App {
        let core = crate::session::app::tests::app(n);
        let mut a = App::new(core.review, Config::default(), Keymap::default());
        a.sel = 0;
        a
    }

    fn dec(a: &App, i: usize) -> Decision {
        a.review.units[i].decision
    }

    fn click(a: &mut App, pos: usize, modifiers: KeyModifiers) {
        *a.targets.borrow_mut() = vec![(Rect::new(0, pos as u16, 10, 1), Target::Unit(pos))];
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 1,
            row: pos as u16,
            modifiers,
        });
    }

    #[test]
    fn click_selects_a_row_and_a_second_click_opens_it() {
        let mut a = app(5);
        click(&mut a, 2, KeyModifiers::NONE);
        assert_eq!(a.sel, 2);
        assert_eq!(a.screen, Screen::Queue);
        // Modifiers change nothing: there is no multi-select.
        click(&mut a, 4, KeyModifiers::CONTROL);
        assert_eq!(a.sel, 4);
        click(&mut a, 1, KeyModifiers::SHIFT);
        assert_eq!(a.sel, 1);
        assert!(
            a.review
                .units
                .iter()
                .all(|u| u.decision == Decision::Pending)
        );
    }

    #[test]
    fn a_click_on_a_key_runs_its_action() {
        let mut a = app(3);
        *a.targets.borrow_mut() =
            vec![(Rect::new(0, 0, 5, 1), Target::Action(Action::DecideAccept))];
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(dec(&a, 0), Decision::Accepted);
        assert_eq!(a.review.history.undo.len(), 1);
    }
}
