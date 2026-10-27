//! Copying from the screen with the mouse. teer takes the mouse for its
//! clicks, so the terminal can't select text itself: a drag selects text
//! inside the pane it starts in and the release copies it, and a double
//! click copies the word under the pointer (a name, a path, a number).

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use unicode_width::UnicodeWidthStr;

use crate::util::truncate;

/// The longest gap between the two clicks of a double click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Where a selection may run: a pane, and how many columns at its left hold
/// no text of its own (the line numbers and signs of a code change).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub rect: Rect,
    pub gutter: u16,
}

impl Area {
    pub fn new(rect: Rect, gutter: u16) -> Area {
        Area { rect, gutter }
    }

    /// The first and the last column a selection takes text from.
    fn left(&self) -> u16 {
        (self.rect.x + self.gutter).min(self.right())
    }

    fn right(&self) -> u16 {
        self.rect.right().saturating_sub(1)
    }

    fn contains(&self, col: u16, row: u16) -> bool {
        col >= self.rect.x
            && col < self.rect.right()
            && row >= self.rect.y
            && row < self.rect.bottom()
    }

    /// The cell nearest (`col`, `row`) inside the area.
    fn clamp(&self, col: u16, row: u16) -> (u16, u16) {
        let bottom = self.rect.bottom().saturating_sub(1);
        (
            col.clamp(self.left(), self.right()),
            row.clamp(self.rect.y, bottom.max(self.rect.y)),
        )
    }
}

/// The area under (`col`, `row`) drawn last, which is the innermost: a pane
/// is drawn before what it holds. The whole screen when there is none.
pub fn area_at(areas: &[Area], screen: Rect, col: u16, row: u16) -> Area {
    areas
        .iter()
        .rev()
        .find(|a| !a.rect.is_empty() && a.contains(col, row))
        .copied()
        .unwrap_or(Area::new(screen, 0))
}

/// Text marked with the mouse, from where the button went down to where
/// the pointer is, both kept inside the area it started in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub area: Area,
    pub from: (u16, u16),
    pub to: (u16, u16),
    /// Copied already: it stays marked until the next key or click.
    pub copied: bool,
}

impl Selection {
    pub fn start(area: Area, col: u16, row: u16) -> Selection {
        let at = area.clamp(col, row);
        Selection {
            area,
            from: at,
            to: at,
            copied: false,
        }
    }

    pub fn extend(&mut self, col: u16, row: u16) {
        self.to = self.area.clamp(col, row);
    }

    /// The pointer moved off the cell it started on: a release copies.
    pub fn moved(&self) -> bool {
        self.from != self.to
    }

    /// Each row it takes and the first and last column there, top to
    /// bottom: like a terminal, from the start to the end of the area's
    /// text in between.
    fn rows(&self) -> Vec<(u16, u16, u16)> {
        let key = |p: (u16, u16)| (p.1, p.0);
        let (a, b) = if key(self.from) <= key(self.to) {
            (self.from, self.to)
        } else {
            (self.to, self.from)
        };
        (a.1..=b.1)
            .map(|y| {
                let x0 = if y == a.1 { a.0 } else { self.area.left() };
                let x1 = if y == b.1 { b.0 } else { self.area.right() };
                (y, x0, x1)
            })
            .collect()
    }
}

/// The cells of row `y` from `x0` to `x1`, each with its column; a wide
/// character once, without the cell it hides.
fn cells(buf: &Buffer, y: u16, x0: u16, x1: u16) -> Vec<(u16, &str)> {
    let mut out = Vec::new();
    let mut x = x0;
    while x <= x1 {
        let Some(cell) = buf.cell((x, y)) else { break };
        let s = cell.symbol();
        out.push((x, s));
        x += s.width().max(1) as u16;
    }
    out
}

/// The selected text: its rows without the blanks at their ends.
pub fn text(buf: &Buffer, sel: &Selection) -> String {
    let rows: Vec<String> = sel
        .rows()
        .into_iter()
        .map(|(y, x0, x1)| {
            let row: String = cells(buf, y, x0, x1).into_iter().map(|(_, s)| s).collect();
            row.trim_end().to_string()
        })
        .collect();
    rows.join("\n").trim_end_matches('\n').to_string()
}

/// Whether `s` belongs to a word: letters, digits and what names, paths,
/// numbers and links are made of.
fn in_word(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || "_-./:@#~+%?&=$".contains(c))
}

/// The word at (`col`, `row`) inside `area` and the columns it takes,
/// without the punctuation that ends a sentence.
pub fn word_at(buf: &Buffer, area: Area, col: u16, row: u16) -> Option<(String, u16, u16)> {
    if !area.contains(col, row) || col < area.left() {
        return None;
    }
    let row_cells = cells(buf, row, area.left(), area.right());
    let i = row_cells.iter().rposition(|(x, _)| *x <= col)?;
    if !in_word(row_cells[i].1) {
        return None;
    }
    let mut s = i;
    while s > 0 && in_word(row_cells[s - 1].1) {
        s -= 1;
    }
    let mut e = i;
    while e + 1 < row_cells.len() && in_word(row_cells[e + 1].1) {
        e += 1;
    }
    while e > s && ".,:;?".contains(row_cells[e].1) {
        e -= 1;
    }
    let word = row_cells[s..=e].iter().map(|(_, s)| *s).collect();
    let end = row_cells[e].0 + row_cells[e].1.width().max(1) as u16 - 1;
    Some((word, row_cells[s].0, end))
}

/// Where `text` is first drawn inside `area`, on one row.
pub fn find(buf: &Buffer, area: Rect, text: &str) -> Option<Rect> {
    if text.is_empty() {
        return None;
    }
    for y in area.top()..area.bottom() {
        let row = cells(buf, y, area.left(), area.right().saturating_sub(1));
        let mut joined = String::new();
        let mut at = Vec::with_capacity(row.len());
        for (x, s) in &row {
            at.push((joined.len(), *x));
            joined.push_str(s);
        }
        if let Some(i) = joined.find(text) {
            let x = at
                .iter()
                .rev()
                .find(|(b, _)| *b <= i)
                .map_or(area.x, |p| p.1);
            return Some(Rect::new(x, y, text.width() as u16, 1));
        }
    }
    None
}

/// Mark the selected cells on `buf`.
pub fn paint(buf: &mut Buffer, sel: &Selection) {
    for (y, x0, x1) in sel.rows() {
        for x in x0..=x1 {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.modifier.toggle(Modifier::REVERSED);
            }
        }
    }
}

/// How the status line names copied `text`.
pub fn name(text: &str) -> String {
    match text.lines().count() {
        0 | 1 => format!("“{}”", truncate(text, 40)),
        n => format!("{n} lines"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    /// A buffer `w` wide holding `rows`.
    fn buf(w: u16, rows: &[&str]) -> Buffer {
        let mut b = Buffer::empty(Rect::new(0, 0, w, rows.len() as u16));
        for (y, r) in rows.iter().enumerate() {
            b.set_line(0, y as u16, &Line::raw(*r), w);
        }
        b
    }

    fn sel(area: Area, from: (u16, u16), to: (u16, u16)) -> Selection {
        let mut s = Selection::start(area, from.0, from.1);
        s.extend(to.0, to.1);
        s
    }

    #[test]
    fn a_drag_takes_the_rows_between_like_a_terminal() {
        let b = buf(12, &["alpha beta", "gamma", "delta epsi"]);
        let all = Area::new(b.area, 0);
        assert_eq!(text(&b, &sel(all, (6, 0), (4, 2))), "beta\ngamma\ndelta");
        // Upwards is the same text.
        assert_eq!(text(&b, &sel(all, (4, 2), (6, 0))), "beta\ngamma\ndelta");
        assert_eq!(text(&b, &sel(all, (0, 0), (4, 0))), "alpha");
    }

    #[test]
    fn a_selection_stays_inside_its_area_and_skips_the_gutter() {
        let b = buf(16, &["12 + let a = 1;", "13 - let b = 2;", "status line"]);
        let code = Area::new(Rect::new(0, 0, 16, 2), 5);
        // Dragged from the line numbers to below the box.
        let s = sel(code, (0, 0), (15, 5));
        assert_eq!(s.to, (15, 1));
        assert_eq!(text(&b, &s), "let a = 1;\nlet b = 2;");
    }

    #[test]
    fn the_innermost_area_takes_the_drag() {
        let pane = Area::new(Rect::new(0, 0, 20, 10), 0);
        let code = Area::new(Rect::new(2, 2, 10, 4), 4);
        let screen = Rect::new(0, 0, 40, 12);
        assert_eq!(area_at(&[pane, code], screen, 3, 3), code);
        assert_eq!(area_at(&[pane, code], screen, 15, 3), pane);
        assert_eq!(area_at(&[pane, code], screen, 30, 3), Area::new(screen, 0));
    }

    #[test]
    fn wide_characters_are_copied_once() {
        let b = buf(10, &["a界b"]);
        let s = sel(Area::new(b.area, 0), (0, 0), (9, 0));
        assert_eq!(text(&b, &s), "a界b");
    }

    #[test]
    fn a_double_click_takes_names_paths_and_numbers() {
        let b = buf(40, &["see src/tui/app.rs:120, then on_mouse()."]);
        let all = Area::new(b.area, 0);
        assert_eq!(
            word_at(&b, all, 8, 0),
            Some(("src/tui/app.rs:120".into(), 4, 21))
        );
        assert_eq!(word_at(&b, all, 30, 0).unwrap().0, "on_mouse");
        assert_eq!(word_at(&b, all, 3, 0), None, "a blank");
        assert_eq!(word_at(&b, all, 0, 0).unwrap().0, "see");
    }

    #[test]
    fn find_gives_where_text_is_drawn() {
        let b = buf(30, &["╭─────╮", "│ ⏎ go to · esc close"]);
        assert_eq!(find(&b, b.area, "esc close"), Some(Rect::new(12, 1, 9, 1)));
        assert_eq!(find(&b, b.area, "⏎ go to"), Some(Rect::new(2, 1, 7, 1)));
        assert_eq!(find(&b, Rect::new(0, 0, 30, 1), "esc"), None);
    }

    #[test]
    fn the_status_names_a_word_or_counts_lines() {
        assert_eq!(name("on_mouse"), "“on_mouse”");
        assert_eq!(name("a\nb\nc"), "3 lines");
    }
}
