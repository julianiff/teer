//! Swipe gestures: mouse drags (and touch sent as mouse events) become
//! accept (right) or investigate (left). Vertical drags do nothing.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Accept.
    Right,
    /// Investigate.
    Left,
}

/// A drag counts as a swipe once it passes a third of the card's width. Mostly
/// vertical drags (twice as far down or up as across) are ignored.
pub fn classify(dx: i32, dy: i32, width: u16) -> Option<Dir> {
    let wt = (width as i32 / 3).max(1);
    if dx.abs() >= wt && dx.abs() * 2 >= dy.abs() {
        return Some(if dx > 0 { Dir::Right } else { Dir::Left });
    }
    None
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DragTracker {
    start: Option<(u16, u16)>,
    pub dx: i32,
    pub dy: i32,
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

impl DragTracker {
    pub fn dragging(&self) -> bool {
        self.start.is_some() && (self.dx != 0 || self.dy != 0)
    }

    /// A drag that is mostly sideways, so a swipe: the card follows it. A
    /// mostly vertical one selects text on the card instead.
    pub fn sideways(&self) -> bool {
        self.start.is_some() && self.dx != 0 && self.dx.abs() * 2 >= self.dy.abs()
    }

    /// Feed a mouse event; returns a direction when a swipe completes.
    pub fn on_mouse(&mut self, ev: MouseEvent, card: Rect) -> Option<Dir> {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.start = inside(card, ev.column, ev.row).then_some((ev.column, ev.row));
                self.dx = 0;
                self.dy = 0;
                None
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((sx, sy)) = self.start {
                    self.dx = ev.column as i32 - sx as i32;
                    self.dy = ev.row as i32 - sy as i32;
                }
                None
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let dir = self
                    .start
                    .and_then(|_| classify(self.dx, self.dy, card.width));
                *self = DragTracker::default();
                dir
            }
            _ => None,
        }
    }

    /// The swipe that would happen if released now (for tinting the card).
    pub fn preview(&self, card: Rect) -> Option<Dir> {
        self.start
            .and_then(|_| classify(self.dx, self.dy, card.width))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn ev(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    const CARD: Rect = Rect {
        x: 10,
        y: 5,
        width: 60,
        height: 15,
    };

    fn drag(from: (u16, u16), to: (u16, u16)) -> Option<Dir> {
        let mut t = DragTracker::default();
        assert_eq!(
            t.on_mouse(
                ev(MouseEventKind::Down(MouseButton::Left), from.0, from.1),
                CARD
            ),
            None
        );
        // Intermediate drag events, as terminals send them.
        let steps = 5;
        for i in 1..=steps {
            let x = from.0 as i32 + (to.0 as i32 - from.0 as i32) * i / steps;
            let y = from.1 as i32 + (to.1 as i32 - from.1 as i32) * i / steps;
            t.on_mouse(
                ev(MouseEventKind::Drag(MouseButton::Left), x as u16, y as u16),
                CARD,
            );
        }
        t.on_mouse(ev(MouseEventKind::Up(MouseButton::Left), to.0, to.1), CARD)
    }

    #[test]
    fn horizontal_threshold_is_a_third_of_the_width() {
        // Width 60: threshold 20 columns.
        assert_eq!(drag((40, 10), (60, 10)), Some(Dir::Right));
        assert_eq!(drag((40, 10), (59, 10)), None);
        assert_eq!(drag((40, 10), (20, 11)), Some(Dir::Left));
        assert_eq!(drag((40, 10), (21, 10)), None);
    }

    #[test]
    fn vertical_drags_are_ignored() {
        assert_eq!(drag((40, 12), (40, 7)), None);
        assert_eq!(drag((40, 8), (41, 13)), None);
        assert_eq!(drag((40, 8), (40, 12)), None);
        // Mostly vertical, even with some sideways travel.
        assert_eq!(drag((40, 2), (50, 19)), None);
    }

    #[test]
    fn drags_starting_outside_the_card_are_ignored() {
        assert_eq!(drag((2, 2), (60, 2)), None);
    }

    #[test]
    fn preview_tracks_the_current_drag() {
        let mut t = DragTracker::default();
        t.on_mouse(ev(MouseEventKind::Down(MouseButton::Left), 30, 10), CARD);
        t.on_mouse(ev(MouseEventKind::Drag(MouseButton::Left), 55, 10), CARD);
        assert!(t.dragging());
        assert_eq!(t.dx, 25);
        assert_eq!(t.preview(CARD), Some(Dir::Right));
    }
}
