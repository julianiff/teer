//! The queue: what teer opens on. The tree of the changed files (see
//! [`super::slim`]) and the changes of the file under the cursor (see
//! [`super::preview`]) beside it on a wide terminal, else below it.
//!
//! Above them all, while the security lens is on, the attack-surface card
//! (see [`crate::security::surface`]): a row per class the change touches
//! (entry points, sinks, checks, packages, ci…), then what was checked and
//! what nobody did. With nothing found it is one muted line that says only
//! that; on a short terminal, one line. A click on an item
//! selects its unit.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::app::{App, Target};
use super::slim;
use super::{preview, ui};
use crate::security::Weight;
use crate::security::order::severity_of;
use crate::security::surface::{self, CHECKED, Item, NOT_CHECKED, Row, Surface};
use crate::util::truncate;

/// Most rows the attack-surface card takes, its borders included.
const CARD_MOST: usize = 8;
/// The card is drawn whole while it takes at most a third of the queue's
/// rows and the queue is this wide; else it is one line.
const CARD_SHARE: usize = 3;
const CARD_MIN_WIDTH: usize = 40;
/// Below this many rows the list gets them all: no card.
const CARD_MIN_AREA: u16 = 6;

/// Draw the queue into `area`: the attack-surface card on top, the tree,
/// and the changes of the file under the cursor beside or below it.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let area = ui::banner(f, app, area);
    // Improve mode lists the inserts, not the units.
    if app.mode.improves() {
        super::improve::draw(f, app, area);
        return;
    }
    let tree = app.rows();
    if tree.is_empty() {
        ui::draw_empty(f, app, area);
        return;
    }
    let area = draw_surface(f, app, area);
    let (list, pane) = preview::layout_of(app, area);
    let rows = slim::build_queue(app, list.width);
    let at = app.cursor_row().unwrap_or(0);
    ui::draw_rows(f, app, list, &rows, at, Target::Row);
    if let Some((rect, kind)) = pane {
        preview::draw_preview(f, app, rect, kind);
    }
}

/// Draw the attack-surface card on top of `area` while it is shown
/// (`zs`), each item a click target that selects its unit's file; what is
/// left of `area`.
fn draw_surface(f: &mut Frame, app: &App, area: Rect) -> Rect {
    if !app.security.enabled
        || !app.security.surface
        || area.height < CARD_MIN_AREA
        || area.width < 4
    {
        return area;
    }
    let s = surface::surface(&app.review);
    let card = surface_card(app, &s, area.width as usize, area.height as usize);
    let h = (card.lines.len() as u16).min(area.height);
    let top = Rect::new(area.x, area.y, area.width, h);
    let mut targets = app.targets.borrow_mut();
    for spot in &card.spots {
        if let Some(pos) = app.position_of(&spot.unit) {
            let at = Rect::new(
                area.x + spot.x as u16,
                area.y + spot.line as u16,
                spot.w as u16,
                1,
            );
            targets.push((at, Target::Unit(pos)));
        }
    }
    drop(targets);
    ui::text_area(app, top, 0);
    f.render_widget(Paragraph::new(card.lines), top);
    Rect::new(area.x, area.y + h, area.width, area.height - h)
}

/// The attack-surface card as lines, and where its items are.
pub(super) struct Card {
    pub lines: Vec<Line<'static>>,
    pub spots: Vec<Spot>,
}

/// An item of the card on screen: its line, first cell, cells, and the
/// unit it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Spot {
    pub line: usize,
    pub x: usize,
    pub w: usize,
    pub unit: String,
}

/// An item's cells in a row: the first, how many, its unit (none for ` · `
/// and `+N more`), and its spans.
type Placed = (usize, usize, Option<String>, Vec<Span<'static>>);

/// `checked: auth · … · config` and `not: denial of service · …`.
fn checked_texts() -> (String, String) {
    (
        format!("checked: {}", CHECKED.join(" · ")),
        format!("not: {}", NOT_CHECKED.join(" · ")),
    )
}

/// The card for `s`, `width` cells wide, over a queue of `rows` rows. With
/// nothing found, one muted line that says so: never that it is safe. Else
/// a rounded box titled "attack surface": a row per class
/// (at most [`CARD_MOST`] rows in all; past that, the most severe classes
/// keep their rows and one row names the rest), then what was checked and
/// what wasn't. When the box would take more than a third of the rows, or
/// the width is short, one line: the classes with how many items each,
/// then what was checked.
pub(super) fn surface_card(app: &App, s: &Surface, width: usize, rows: usize) -> Card {
    let t = &app.theme;
    let one = |spans: Vec<Span<'static>>| {
        let mut line = Line::from(spans);
        if line.width() > width {
            line = Line::styled(truncate(&plain(&line), width), t.muted());
        }
        Card {
            lines: vec![line],
            spots: Vec::new(),
        }
    };
    if s.is_empty() {
        return one(vec![Span::styled(
            format!(" {} nothing found", crate::mode::Mode::Security.icon()),
            t.muted(),
        )]);
    }
    let (checked, not) = checked_texts();
    let inner = width.saturating_sub(4);
    // What was checked on one line when it fits, else two.
    let gap = "   ";
    let checked_lines = if checked.width() + gap.width() + not.width() <= inner {
        vec![format!("{checked}{gap}{not}")]
    } else {
        vec![checked.clone(), not]
    };
    let room = CARD_MOST - 2 - checked_lines.len();
    let height = 2 + s.rows.len().min(room) + checked_lines.len();
    if width < CARD_MIN_WIDTH || height * CARD_SHARE > rows {
        let mut spans = vec![Span::styled(" attack surface: ", t.bold())];
        for (k, r) in s.rows.iter().enumerate() {
            if k > 0 {
                spans.push(Span::styled(" · ", t.muted()));
            }
            spans.push(Span::styled(
                format!("{} {}", r.label, r.items.len()),
                weight_style(app, r.items[0].weight),
            ));
        }
        spans.push(Span::styled(format!("{gap}{checked}"), t.muted()));
        return one(spans);
    }
    // Too many classes: the most severe keep their rows, in the card's
    // order, and one row names the rest.
    let keep: Vec<usize> = if s.rows.len() > room {
        let mut by_rank: Vec<usize> = (0..s.rows.len()).collect();
        by_rank.sort_by_key(|&i| std::cmp::Reverse(s.rows[i].rank()));
        by_rank.truncate(room - 1);
        by_rank
    } else {
        (0..s.rows.len()).collect()
    };
    let rows_where = |kept: bool| -> Vec<&Row> {
        (s.rows.iter().enumerate())
            .filter(|(i, _)| keep.contains(i) == kept)
            .map(|(_, r)| r)
            .collect()
    };
    let (kept, left) = (rows_where(true), rows_where(false));
    let more = "more";
    let label_w = kept
        .iter()
        .map(|r| r.label.width())
        .chain((!left.is_empty()).then(|| more.width()))
        .max()
        .unwrap_or(0);
    let boxed = |content: Vec<Span<'static>>| {
        let used: usize = content.iter().map(|s| s.content.width()).sum();
        let mut spans = vec![Span::styled("│ ", t.muted())];
        spans.extend(content);
        spans.push(Span::raw(" ".repeat(inner.saturating_sub(used))));
        spans.push(Span::styled(" │", t.muted()));
        Line::from(spans)
    };
    let mut lines = vec![slim::top_edge(app, width, "attack surface", t.text())];
    let mut spots = Vec::new();
    let items_w = inner.saturating_sub(label_w + 2);
    // Cells from the card's left edge to a row's first item.
    let x0 = 2 + label_w + 2;
    for r in &kept {
        let line = lines.len();
        let mut content = vec![Span::styled(format!("{:<label_w$}  ", r.label), t.muted())];
        for (x, w, unit, spans) in fit_items(app, &r.items, items_w) {
            if let Some(unit) = unit {
                spots.push(Spot {
                    line,
                    x: x0 + x,
                    w,
                    unit,
                });
            }
            content.extend(spans);
        }
        lines.push(boxed(content));
    }
    if !left.is_empty() {
        let names: Vec<String> = left
            .iter()
            .map(|r| format!("{} {}", r.label, r.items.len()))
            .collect();
        lines.push(boxed(vec![
            Span::styled(format!("{more:<label_w$}  "), t.muted()),
            Span::styled(truncate(&names.join(" · "), items_w), t.muted()),
        ]));
    }
    for l in checked_lines {
        lines.push(boxed(vec![Span::styled(truncate(&l, inner), t.muted())]));
    }
    lines.push(slim::bottom_edge(app, width));
    Card { lines, spots }
}

/// The style of an item that weighs `w`.
fn weight_style(app: &App, w: Weight) -> Style {
    match w {
        Weight::Note => app.theme.text(),
        w => app.theme.risk(severity_of(w)),
    }
}

/// An item as spans: `! ` before a finding, `✗ ` before a check that is
/// gone, its text in its weight's colour, ` ×2` after when on several units.
fn item_spans(app: &App, it: &Item) -> Vec<Span<'static>> {
    let style = weight_style(app, it.weight);
    let mut spans = Vec::new();
    if it.finding {
        spans.push(Span::styled("! ", style.add_modifier(Modifier::BOLD)));
    } else if it.gone {
        spans.push(Span::styled("✗ ", style));
    }
    spans.push(Span::styled(it.text.clone(), style));
    if it.units > 1 {
        spans.push(Span::styled(format!(" ×{}", it.units), app.theme.muted()));
    }
    spans
}

/// The items of a row in `w` cells: as many whole items as fit with ` · `
/// between them and `+N more` for the rest; the first cut short when not
/// even it fits.
fn fit_items(app: &App, items: &[Item], w: usize) -> Vec<Placed> {
    let t = &app.theme;
    let sep = " · ";
    let more = |n: usize| format!("{sep}+{n} more");
    let spans: Vec<Vec<Span<'static>>> = items.iter().map(|it| item_spans(app, it)).collect();
    let widths: Vec<usize> = spans
        .iter()
        .map(|s| s.iter().map(|s| s.content.width()).sum())
        .collect();
    let n = items.len();
    // Cells of the first `k` items, the separators and what says how many
    // are left.
    let cells = |k: usize| {
        let rest = if k < n { more(n - k).width() } else { 0 };
        widths[..k].iter().sum::<usize>() + sep.width() * k.saturating_sub(1) + rest
    };
    let mut out: Vec<Placed> = Vec::new();
    let Some(k) = (1..=n).rev().find(|&k| cells(k) <= w) else {
        // Not even the first whole: it, cut, and how many more if they fit.
        let Some(first) = items.first() else {
            return out;
        };
        let tail = if n > 1 { more(n - 1) } else { String::new() };
        let tail = if tail.width() + 4 <= w {
            tail
        } else {
            String::new()
        };
        let text = truncate(&plain(&Line::from(spans[0].clone())), w - tail.width());
        let cut = text.width();
        out.push((
            0,
            cut,
            Some(first.unit.clone()),
            vec![Span::styled(text, weight_style(app, first.weight))],
        ));
        if !tail.is_empty() {
            out.push((cut, tail.width(), None, vec![Span::styled(tail, t.muted())]));
        }
        return out;
    };
    let mut x = 0;
    for (i, s) in spans.into_iter().take(k).enumerate() {
        if i > 0 {
            out.push((x, sep.width(), None, vec![Span::styled(sep, t.muted())]));
            x += sep.width();
        }
        out.push((x, widths[i], Some(items[i].unit.clone()), s));
        x += widths[i];
    }
    if k < n {
        let tail = more(n - k);
        out.push((x, tail.width(), None, vec![Span::styled(tail, t.muted())]));
    }
    out
}

/// A line's text without its styles.
fn plain(l: &Line) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::keymap::Keymap;
    use crate::review::model::{Decision, Prep, Range, Review, Unit, UnitKind, UnitStatus};
    use crate::session::app::Screen;
    use std::path::PathBuf;
    use unicode_width::UnicodeWidthStr;

    fn unit(i: usize, file: &str, decision: Decision) -> Unit {
        Unit {
            id: format!("u{i}"),
            file: PathBuf::from(file),
            symbol: format!("symbol_{i}"),
            kind: UnitKind::Function,
            status: UnitStatus::Changed,
            old_range: None,
            new_range: Some(Range::new(1, 4)),
            nested: Vec::new(),
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
            security: Vec::new(),
            signature: None,
        }
    }

    fn app(units: Vec<Unit>) -> App {
        let review = Review {
            id: "q".into(),
            repo_root: PathBuf::from("/r"),
            base: "b".into(),
            head: crate::git::WORKTREE.into(),
            head_commit: "h".into(),
            agent_session: None,
            pr: None,
            intent: Default::default(),
            created_at: chrono::Utc::now(),
            files: Vec::new(),
            units,
            skipped: Vec::new(),
            graph: Default::default(),
            questions: Vec::new(),
            version: 0,
            history: Default::default(),
            branch: None,
            cursor: None,
            mode: None,
        };
        let mut app = App::new(review, Config::default(), Keymap::default());
        app.screen = Screen::Queue;
        app.rebuild_order();
        app
    }

    fn three() -> App {
        let mut flagged = unit(1, "src/billing/routes.rs", Decision::Investigate);
        flagged.dive.note =
            "The retry is gone and nothing replaces it when the call times out.".into();
        app(vec![
            unit(0, "src/billing/charge.rs", Decision::Pending),
            flagged,
            unit(2, "src/ui/cart.rs", Decision::Accepted),
        ])
    }

    /// `app` drawn `w` x `h`, one string per row.
    fn render(app: &App, w: u16, h: u16) -> Vec<String> {
        use ratatui::{Terminal, backend::TestBackend};
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| ui::draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn the_queue_draws_rows_preview_and_bar_at_any_size() {
        let mut app = three();
        app.review.intent.summary = Some("Refunds, retried.".into());
        let s = render(&app, 160, 30);
        assert_eq!(s[0].trim_end(), " your changes");
        assert_eq!(s[1].trim_end(), " Refunds, retried.");
        // The tree on the left, the cursor on its first file; beside it
        // that file's unit.
        let tree: Vec<String> = s[2..8]
            .iter()
            .map(|l| {
                l.split('│')
                    .next()
                    .unwrap_or_default()
                    .trim_end()
                    .to_string()
            })
            .collect();
        assert_eq!(
            tree,
            [
                " ▾ src",
                "   ▾ billing",
                "▌    ┃ charge.rs",
                "     ┃ routes.rs ⚑",
                "   ▾ ui",
                "     ┃ cart.rs",
            ]
        );
        assert!(s[2].contains("┃ symbol_0"), "{}", s[2]);
        let bar = s.last().unwrap();
        assert!(
            bar.starts_with(" 1 pending · 1 flagged · 1 accepted"),
            "{bar}"
        );
        assert!(bar.trim_end().ends_with("? keys"), "{bar}");
        // Each row a click target, the pane the wheel's.
        let targets = app.targets.borrow().clone();
        let rows = targets
            .iter()
            .filter(|(_, t)| matches!(t, crate::tui::app::Target::Row(_)))
            .count();
        assert_eq!(rows, 6);
        assert!(
            targets
                .iter()
                .any(|(_, t)| *t == crate::tui::app::Target::Detail)
        );
        for (w, h) in [
            (0, 0),
            (1, 1),
            (10, 3),
            (30, 8),
            (80, 24),
            (109, 40),
            (110, 40),
        ] {
            let s = render(&app, w, h);
            assert_eq!(s.len(), h as usize);
            for l in &s {
                assert_eq!(l.width(), w as usize, "{w}x{h}: {l}");
            }
        }
    }
}
