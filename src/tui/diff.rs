//! The code change of a unit that changes behavior, as a compact diff in
//! a box of its own: in the queue's preview, the unit view and its
//! panels. The rows come from [`crate::review::diff`], so drawing never
//! runs git. Line numbers are those of the new file, where `gf` opens the
//! unit; removed lines show their number in the old file, styled apart.
//! Each statement can be written into the code under the rows it cites,
//! like a review comment; then the code change shows only what they cite
//! and the rest folds into a quiet `...` under the numbers. Code keeps its
//! syntax colours (see [`crate::syntax`]); removed lines are tinted.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::app::App;
use super::preview;
use crate::fix::ProposedHunk;
use crate::review::diff::{DIFF_LEVEL, Kind, Row, anchor, expand_tabs, rows_around};
use crate::review::model::{Change, LineRef, Marker, Prep, Unit};
use crate::syntax::Class;
use crate::util::sanitize;

/// Does this unit show its code change? Cheap enough for every frame: a
/// unit at [`DIFF_LEVEL`] or above changes behavior, so it has changed
/// lines, and no diff is taken.
pub fn shows(app: &App, u: &Unit) -> bool {
    app.assessment(&u.id).is_some_and(|a| a.level >= DIFF_LEVEL) && app.review.file_of(u).is_some()
}

/// `chars` in pieces of at most `w` columns, every character kept, as the
/// spaces each piece is indented by and the chars it holds. Pieces after the
/// first are indented like the line itself (by at most half of `w`), so a
/// wrapped statement still reads as one. A piece ends after the last space,
/// `(`, `[`, `{` or `,` past its first third and its indentation; only
/// without one is a word split.
fn wrap_ranges(chars: &[char], w: usize) -> Vec<(usize, std::ops::Range<usize>)> {
    use unicode_width::UnicodeWidthChar;
    let width = |cs: &[char]| cs.iter().map(|c| c.width().unwrap_or(0)).sum::<usize>();
    let n = chars.len();
    if w == 0 || width(chars) <= w {
        return vec![(0, 0..n)];
    }
    let ind = chars.iter().take_while(|&&c| c == ' ').count().min(w / 2);
    let mut out = Vec::new();
    let mut at = 0;
    while at < n {
        let (room, lead) = if out.is_empty() {
            (w, 0)
        } else {
            (w - ind, ind)
        };
        if width(&chars[at..]) <= room {
            out.push((lead, at..n));
            break;
        }
        let (mut cut, mut used, mut brk, mut text_seen) = (at, 0, None, false);
        for (k, &c) in chars[at..].iter().enumerate() {
            let cw = c.width().unwrap_or(0);
            if used + cw > room {
                break;
            }
            used += cw;
            cut = at + k + 1;
            if text_seen && used > room / 3 && matches!(c, ' ' | '(' | '[' | '{' | ',') {
                brk = Some(cut);
            }
            text_seen |= c != ' ';
        }
        // A glyph wider than the room still has to go somewhere.
        let cut = brk.unwrap_or(cut).max(at + 1);
        out.push((lead, at..cut));
        at = cut;
    }
    out
}

/// `chars` as spans in `base`, each run of one syntax class in its colour.
fn painted(
    app: &App,
    chars: &[char],
    classes: &[Option<Class>],
    base: Style,
) -> Vec<Span<'static>> {
    let class_at = |k: usize| classes.get(k).copied().flatten();
    let mut spans = Vec::new();
    let mut k = 0;
    while k < chars.len() {
        let class = class_at(k);
        let end = (k..chars.len())
            .find(|&e| class_at(e) != class)
            .unwrap_or(chars.len());
        let style = class.map_or(base, |c| base.patch(app.theme.syntax(c)));
        spans.push(Span::styled(
            chars[k..end].iter().collect::<String>(),
            style,
        ));
        k = end;
    }
    spans
}

/// Lines of a file as rows to draw, each the lines it wraps into at `w`
/// cells: `first` is the number of the first, the ones in `changed` carry
/// a green `+` (the change adds or edits them), and code keeps its syntax
/// colours (parsed on its own, see [`crate::syntax::Cache::piece`]).
pub fn listing(
    app: &App,
    path: &std::path::Path,
    first: u32,
    code: &[String],
    changed: &[u32],
    w: usize,
) -> Vec<Vec<Line<'static>>> {
    let t = &app.theme;
    let last = first + code.len().saturating_sub(1) as u32;
    let digits = last.to_string().len().max(3);
    // "123 + code": line number, sign, then the code.
    let code_w = w.saturating_sub(digits + 3);
    let classes = app.syntax.piece(path, &code.join("\n"));
    code.iter()
        .enumerate()
        .map(|(k, raw)| {
            let n = first + k as u32;
            let (sign, sign_style) = if changed.contains(&n) {
                ("+", t.text().fg(t.added))
            } else {
                (" ", t.text())
            };
            let line_classes = classes.get(k).map_or(&[][..], Vec::as_slice);
            let (text, line_classes) = expand_tabs(raw, line_classes);
            let clean = sanitize(&text);
            // Colours only for code that is drawn as it is in the file.
            let line_classes = if clean == text {
                &line_classes[..]
            } else {
                &[]
            };
            let chars: Vec<char> = clean.chars().collect();
            wrap_ranges(&chars, code_w)
                .into_iter()
                .enumerate()
                .map(|(p, (indent, range))| {
                    // Continuations leave the number and sign columns empty.
                    let (num, sign) = if p == 0 {
                        (n.to_string(), sign)
                    } else {
                        (String::new(), " ")
                    };
                    let mut spans = vec![
                        Span::styled(format!("{num:>digits$} "), t.line_no()),
                        Span::styled(format!("{sign} "), sign_style),
                        Span::styled(" ".repeat(indent), t.text()),
                    ];
                    let piece_classes = line_classes.get(range.start..).unwrap_or_default();
                    spans.extend(painted(app, &chars[range], piece_classes, t.text()));
                    Line::from(spans)
                })
                .collect()
        })
        .collect()
}

/// What the code change shows of the agent's statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notes {
    /// Nothing: the statements aren't drawn, and
    /// every change shows.
    Off,
    /// Each statement written into the code under the last row it cites,
    /// like a review comment; only the code they cite is shown.
    Inline,
    /// The statements written in as with `Inline`, and every change shown
    /// (the unit view, where `]c` and `[c` go from hunk to hunk).
    Full,
}

/// The code change of `u` as blocks, each row as the lines it wraps into,
/// so whoever cuts it can keep rows whole (see
/// [`super::preview::scrolled`]), each line at most `w` cells; how many
/// lines were added and removed; and the columns the line numbers and signs
/// take. Nothing when the unit doesn't show its code change.
pub fn code_change(
    app: &App,
    u: &Unit,
    w: usize,
    notes: Notes,
) -> (Vec<Vec<Line<'static>>>, usize, usize, u16) {
    let c = code_change_of(app, u, w, notes);
    (c.blocks, c.added, c.removed, c.gutter)
}

/// The code change of a unit as [`code_change`] draws it, and where its
/// hunks start.
#[derive(Debug, Default)]
pub struct CodeChange {
    pub blocks: Vec<Vec<Line<'static>>>,
    pub added: usize,
    pub removed: usize,
    /// Columns the line numbers and signs take.
    pub gutter: u16,
    /// The block each hunk (a run of added and removed rows) starts at, in
    /// order: where `]c` and `[c` go.
    pub hunks: Vec<usize>,
}

/// [`code_change`] with the hunks.
pub fn code_change_of(app: &App, u: &Unit, w: usize, notes: Notes) -> CodeChange {
    if app.assessment(&u.id).is_none_or(|a| a.level < DIFF_LEVEL) {
        return CodeChange::default();
    }
    let said = notes != Notes::Off && u.prep == Prep::Ready;
    // Where the statements are drawn inline, they show only the code they
    // cite; in full, the code they cite as well as every change.
    let cites: Vec<LineRef> = if said {
        u.changes
            .iter()
            .flat_map(|c| c.evidence.iter().copied())
            .collect()
    } else {
        Vec::new()
    };
    let every = notes == Notes::Full;
    let (rows, added, removed) = rows_around(&app.review, u, &cites, every, Some(&app.syntax));
    if rows.is_empty() {
        return CodeChange::default();
    }
    let changes = |k: usize| matches!(rows[k].kind, Kind::Added | Kind::Removed);
    let mut hunks: Vec<usize> = (0..rows.len())
        .filter(|&k| changes(k) && (k == 0 || !changes(k - 1)))
        .collect();
    let digits = digits_of(&rows);
    // Each row as its lines: long code wraps instead of being cut.
    let mut blocks: Vec<Vec<Line<'static>>> =
        rows.iter().map(|r| row_lines(app, r, digits, w)).collect();
    if said {
        // Each statement under the row it is anchored to, in their order;
        // inserted from the back so the row indices stay right.
        let mut at: Vec<(usize, usize)> = u
            .changes
            .iter()
            .enumerate()
            .map(|(n, c)| (anchor(c, &rows).map_or(rows.len(), |k| k + 1), n))
            .collect();
        at.sort();
        for &(k, n) in at.iter().rev() {
            let note = note_lines(app, &u.changes[n], digits + 1, w);
            blocks.insert(k, note);
        }
        // A row moves down by the notes inserted before it.
        for h in &mut hunks {
            let row = *h;
            *h = row + at.iter().filter(|&&(k, _)| k <= row).count();
        }
    }
    CodeChange {
        blocks,
        added,
        removed,
        gutter: (digits + 3) as u16,
        hunks,
    }
}

/// Columns the line numbers of `rows` take: three at least.
fn digits_of(rows: &[Row]) -> usize {
    rows.iter()
        .filter_map(|r| r.line.or(r.old))
        .max()
        .map_or(1, |n| n.to_string().len())
        .max(3)
}

/// Row `r` of a code change as the lines it wraps into at `w` cells,
/// `digits` columns for its number: "123 + code". Code keeps its syntax
/// colours; the sign says what changed, and a removed line is tinted as
/// well and has only its old number. A gap is quiet dots under the
/// numbers.
fn row_lines(app: &App, r: &Row, digits: usize, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let code_w = w.saturating_sub(digits + 3);
    let (sign, sign_fg, line_style) = match r.kind {
        Kind::Added => ("+", t.added, Style::new()),
        Kind::Removed => ("-", t.removed, t.removed_line()),
        Kind::Context => (" ", t.text, Style::new()),
        // Left out, quietly: dots under the numbers.
        Kind::Gap => {
            return vec![Line::styled(format!("{:>digits$}", "..."), t.muted())];
        }
    };
    let (num, num_style) = match r.kind {
        Kind::Removed => (r.old, t.old_line_no()),
        _ => (r.line, t.line_no()),
    };
    let num = num.map(|n| n.to_string()).unwrap_or_default();
    let code_style = t.text().patch(line_style);
    let sign_style = code_style.fg(sign_fg);
    // Colours only for code that is drawn as it is in the file.
    let text = sanitize(&r.text);
    let chars: Vec<char> = text.chars().collect();
    let classes = if text == r.text { &r.classes[..] } else { &[] };
    wrap_ranges(&chars, code_w)
        .into_iter()
        .enumerate()
        .map(|(k, (indent, range))| {
            // Continuations leave the number and sign columns empty.
            let (num, sign) = if k == 0 {
                (num.as_str(), sign)
            } else {
                ("", " ")
            };
            let mut spans = vec![
                Span::styled(format!("{num:>digits$} "), num_style),
                Span::styled(format!("{sign} "), sign_style),
                Span::styled(" ".repeat(indent), code_style),
            ];
            let piece = &chars[range.clone()];
            let piece_classes = classes.get(range.start..).unwrap_or_default();
            spans.extend(painted(app, piece, piece_classes, code_style));
            // A tinted line is tinted to the edge.
            if r.kind == Kind::Removed {
                use unicode_width::UnicodeWidthChar;
                let used: usize =
                    indent + piece.iter().map(|c| c.width().unwrap_or(0)).sum::<usize>();
                let fill = " ".repeat(code_w.saturating_sub(used));
                spans.push(Span::styled(fill, code_style));
            }
            Line::from(spans)
        })
        .collect()
}

/// A hunk the agent proposes as blocks drawn as the code change's rows
/// are, each at most `w` cells: the lines it removes and adds, with the
/// context they share, numbered and in their syntax colours (see
/// [`crate::session::fix::hunk_rows`]).
pub fn proposed_rows(app: &App, h: &ProposedHunk, w: usize) -> Vec<Vec<Line<'static>>> {
    let rows = crate::session::fix::hunk_rows(h, Some(&app.syntax));
    let digits = digits_of(&rows);
    rows.iter().map(|r| row_lines(app, r, digits, w)).collect()
}

/// A statement as a comment in the code, under the row above it: a small
/// rounded card from the sign column (`at` cells in) on, as wide as its
/// text needs and at most to `w`, the statement inside in the note style.
/// No `+ ~ −` marker as in the list: inside a diff it would read as a sign.
fn note_lines(app: &App, c: &Change, at: usize, w: usize) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthStr;
    let t = &app.theme;
    let lead = " ".repeat(at);
    // Borders and a space on each side take 4.
    let text = crate::util::wrap(&c.statement, w.saturating_sub(at + 4).max(8));
    let inner = text.iter().map(|l| l.width()).max().unwrap_or(0);
    let edge = |l: &str, r: &str| {
        Line::from(vec![
            Span::raw(lead.clone()),
            Span::styled(format!("{l}{}{r}", "─".repeat(inner + 2)), t.note_border()),
        ])
    };
    let mut lines = vec![edge("╭", "╮")];
    lines.extend(text.iter().map(|l| {
        Line::from(vec![
            Span::raw(lead.clone()),
            Span::styled("│ ", t.note_border()),
            Span::styled(crate::util::truncate_pad(l, inner), t.note()),
            Span::styled(" │", t.note_border()),
        ])
    }));
    lines.push(edge("╰", "╯"));
    lines
}

/// Rows the code change of `u` takes in a box `width` wide, borders
/// included; 0 when it shows none.
pub fn box_rows(app: &App, u: &Unit, width: u16, notes: Notes) -> usize {
    let w = width.saturating_sub(4) as usize;
    match code_change(app, u, w, notes).0 {
        b if b.is_empty() => 0,
        b => b.iter().map(Vec::len).sum::<usize>() + 2,
    }
}

/// The code box's title, on its top border.
pub const TITLE: &str = " code change ";

/// ` +3 −1 `: the lines the change adds and removes, for the top border.
pub fn counts(app: &App, added: usize, removed: usize) -> Vec<Span<'static>> {
    let t = &app.theme;
    let mut counts = Vec::new();
    if added > 0 {
        counts.push(Span::styled(format!(" +{added}"), t.marker(Marker::Added)));
    }
    if removed > 0 {
        counts.push(Span::styled(
            format!(" −{removed}"),
            t.marker(Marker::Removed),
        ));
    }
    counts.push(Span::raw(" "));
    counts
}

/// The code change of `u` in a box of its own filling `area`: a rounded
/// border titled `code change`, with how many lines were added and removed
/// on the right, and the rows inside, scrolling with the detail keys.
/// False, and nothing drawn, when there is no code change or no room.
pub fn draw_box(f: &mut Frame, app: &App, area: Rect, u: &Unit, notes: Notes) -> bool {
    if area.width < 12 || area.height < 3 {
        return false;
    }
    let w = (area.width - 4) as usize;
    let (blocks, added, removed, gutter) = code_change(app, u, w, notes);
    if blocks.is_empty() {
        return false;
    }
    // No taller than what it holds.
    let rows = blocks.iter().map(Vec::len).sum::<usize>() + 2;
    let area = Rect {
        height: area.height.min(rows.min(u16::MAX as usize) as u16),
        ..area
    };
    // While code is left below, the box has no bottom edge: it reads as
    // going on past the pane, and that row says how much is left. Scrolled
    // to the end, it closes.
    let h = area.height as usize;
    let s = preview::scroll_open(app, blocks, w, h - 1, true, app.detail_offset());
    let open = s.below > 0;
    preview::keep_scroll(app, &s, h - 2 + usize::from(open));
    let mut lines = s.lines;
    let borders = if open {
        let hint = lines.pop().unwrap_or_default();
        lines.resize(h - 2, Line::default());
        lines.push(hint);
        Borders::TOP | Borders::LEFT | Borders::RIGHT
    } else {
        Borders::ALL
    };
    let t = &app.theme;
    let block = Block::new()
        .borders(borders)
        .border_type(BorderType::Rounded)
        .border_style(t.text())
        .title(Span::styled(TITLE, t.bold()))
        .title(Line::from(counts(app, added, removed)).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    f.render_widget(Paragraph::new(lines), inner);
    super::ui::text_area(app, inner, gutter);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{ChangedFile, FileStatus};
    use crate::review::diff::{rows_near, unit_rows};
    use crate::review::model::{Decision, Range, Review, Side, UnitKind, UnitStatus};
    use std::path::PathBuf;

    /// [`wrap_ranges`] as strings.
    fn wrap_code(text: &str, w: usize) -> Vec<String> {
        let text = sanitize(text);
        let chars: Vec<char> = text.chars().collect();
        wrap_ranges(&chars, w)
            .into_iter()
            .map(|(lead, r)| {
                format!(
                    "{}{}",
                    " ".repeat(lead),
                    chars[r].iter().collect::<String>()
                )
            })
            .collect()
    }

    fn review(old: Option<&str>, new: Option<&str>, u: Unit) -> Review {
        Review {
            id: "x".into(),
            repo_root: PathBuf::from("/r"),
            base: "b".into(),
            head: "h".into(),
            head_commit: "h".into(),
            agent_session: None,
            pr: None,
            intent: Default::default(),
            created_at: chrono::Utc::now(),
            files: vec![ChangedFile {
                path: PathBuf::from("f.py"),
                old_path: None,
                status: FileStatus::Modified,
                old_content: old.map(String::from),
                new_content: new.map(String::from),
            }],
            units: vec![u],
            skipped: Vec::new(),
            graph: Default::default(),
            questions: Vec::new(),
            version: 0,
            history: Default::default(),
            branch: None,
            cursor: None,
        }
    }

    fn unit(status: UnitStatus, old: Option<(u32, u32)>, new: Option<(u32, u32)>) -> Unit {
        Unit {
            id: "f.py::f".into(),
            file: PathBuf::from("f.py"),
            symbol: "f".into(),
            kind: UnitKind::Function,
            status,
            old_range: old.map(|(a, b)| Range::new(a, b)),
            new_range: new.map(|(a, b)| Range::new(a, b)),
            renamed_from: None,
            fingerprint: String::new(),
            changes: Vec::new(),
            prep: Prep::Unavailable,
            callers: Vec::new(),
            callees: Vec::new(),
            decision: Decision::Pending,
            summary: None,
            nbc_reason: None,
            dive: Default::default(),
            accepted_by: None,
            reasons: Vec::new(),
            dive_pending: Default::default(),
            checks: Default::default(),
            security: Vec::new(),
        }
    }

    fn rows_of(old: Option<&str>, new: Option<&str>, u: Unit) -> Vec<(Kind, Option<u32>, String)> {
        let r = review(old, new, u);
        unit_rows(&r, &r.units[0])
            .into_iter()
            .map(|r| (r.kind, r.line, r.text))
            .collect()
    }

    #[test]
    fn changed_line_with_context_and_numbers_from_each_side() {
        let old = "x = 1\ndef f():\n    a = 1\n    return a\n";
        let new = "x = 1\n\ndef f():\n    a = 2\n    return a\n";
        let got = rows_of(
            Some(old),
            Some(new),
            unit(UnitStatus::Changed, Some((2, 4)), Some((3, 5))),
        );
        assert_eq!(
            got,
            vec![
                (Kind::Context, Some(3), "def f():".into()),
                (Kind::Removed, None, "    a = 1".into()),
                (Kind::Added, Some(4), "    a = 2".into()),
                (Kind::Context, Some(5), "    return a".into()),
            ]
        );
    }

    #[test]
    fn removed_and_unchanged_rows_know_their_old_line() {
        let old = "x = 1\ndef f():\n    a = 1\n    return a\n";
        let new = "x = 1\n\ndef f():\n    a = 2\n    return a\n";
        let r = review(
            Some(old),
            Some(new),
            unit(UnitStatus::Changed, Some((2, 4)), Some((3, 5))),
        );
        let got: Vec<(Option<u32>, Option<u32>)> = unit_rows(&r, &r.units[0])
            .into_iter()
            .map(|r| (r.old, r.line))
            .collect();
        assert_eq!(
            got,
            [
                (Some(2), Some(3)),
                (Some(3), None),
                (None, Some(4)),
                (Some(4), Some(5)),
            ]
        );
    }

    fn change(evidence: &[(Side, u32)]) -> Change {
        use crate::review::model::{LineRef, Marker, Risk};
        Change {
            marker: Marker::Changed,
            statement: "s".into(),
            evidence: evidence
                .iter()
                .map(|&(side, line)| LineRef { side, line })
                .collect(),
            risk: Risk::Low,
        }
    }

    /// The drawn code change of a described unit, as text, with the style of
    /// each line's number, its sign and its last span (the code, on a code
    /// row).
    fn drawn(theme: crate::config::Theme, notes: Notes) -> Vec<(String, Style, Style, Style)> {
        use crate::config::Config;
        use crate::keymap::Keymap;
        let old = "def f():\n    a = 1\n    return a\n";
        let new = "def f():\n    a = 2\n    return a\n";
        let mut u = unit(UnitStatus::Changed, Some((1, 3)), Some((1, 3)));
        u.prep = Prep::Ready;
        u.changes = vec![
            change(&[(Side::New, 2)]),
            change(&[(Side::Old, 2), (Side::New, 2)]),
        ];
        let cfg = Config {
            theme,
            ..Config::default()
        };
        let app = App::new(review(Some(old), Some(new), u), cfg, Keymap::default());
        let u = &app.review.units[0];
        code_change(&app, u, 40, notes)
            .0
            .into_iter()
            .flatten()
            .map(|l| {
                // Tinted lines are padded to the edge.
                let text: String = l.spans.iter().map(|s| s.content.to_string()).collect();
                let text = text.trim_end().to_string();
                // A note card's lead is blank; its border comes first.
                let spans: Vec<_> = l
                    .spans
                    .iter()
                    .skip_while(|s| s.content.trim().is_empty())
                    .collect();
                let style = |k: usize| spans.get(k).map_or(Style::new(), |s| s.style);
                (text, style(0), style(1), l.spans.last().unwrap().style)
            })
            .collect()
    }

    #[test]
    fn cited_rows_look_like_the_others_and_removed_ones_have_their_old_number() {
        use crate::config::Theme;
        use ratatui::style::Modifier;
        // Both statements cite the added row (the second also the removed
        // one), so both sit under it in a card each, in their order, from
        // the sign column; nothing else marks the rows they cite.
        let text: Vec<String> = drawn(Theme::Dark, Notes::Inline)
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(
            text,
            [
                "  1   def f():",
                "  2 -     a = 1",
                "  2 +     a = 2",
                "    ╭───╮",
                "    │ s │",
                "    ╰───╯",
                "    ╭───╮",
                "    │ s │",
                "    ╰───╯",
                "  3       return a",
            ]
        );
        let plain: Vec<String> = drawn(Theme::Dark, Notes::Off)
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(plain[1], "  2 -     a = 1");
        // The removed row's (old) number is italic, so it reads without
        // colour too; no row is bold.
        for theme in [Theme::Dark, Theme::Mono] {
            let rows = drawn(theme, Notes::Inline);
            assert!(
                rows[1].1.add_modifier.contains(Modifier::ITALIC),
                "{theme:?}"
            );
            assert!(
                !rows[2].1.add_modifier.contains(Modifier::ITALIC),
                "{theme:?}"
            );
            for (k, r) in rows.iter().enumerate() {
                assert!(!r.3.add_modifier.contains(Modifier::BOLD), "{theme:?} {k}");
            }
            // A note's card has its own border, its text the note style.
            let style = super::super::theme::Theme::new(theme);
            assert_eq!(rows[3].1, style.note_border(), "{theme:?}");
            assert_eq!(rows[4].1, style.note_border(), "{theme:?}");
            assert_eq!(rows[4].2, style.note(), "{theme:?}");
        }
    }

    /// Twenty lines `l1`..`l20`, changed at lines 2 and 17.
    fn twenty() -> (String, String) {
        let old: Vec<String> = (1..=20).map(|i| format!("l{i}")).collect();
        let mut new = old.clone();
        new[1] = "L2".into();
        new[16] = "L17".into();
        (old.join("\n"), new.join("\n"))
    }

    #[test]
    fn drawn_statements_show_only_the_code_they_cite() {
        use crate::config::Config;
        use crate::keymap::Keymap;
        let (old, new) = twenty();
        let mut u = unit(UnitStatus::Changed, Some((1, 20)), Some((1, 20)));
        u.prep = Prep::Ready;
        u.changes = vec![change(&[(Side::New, 17)])];
        let app = App::new(
            review(Some(&old), Some(&new), u),
            Config::default(),
            Keymap::default(),
        );
        let lines = |notes| -> Vec<Line<'static>> {
            code_change(&app, &app.review.units[0], 40, notes)
                .0
                .into_iter()
                .flatten()
                .collect()
        };
        let text = |notes| -> Vec<String> {
            lines(notes)
                .iter()
                .map(|l| {
                    let t: String = l.spans.iter().map(|s| s.content.to_string()).collect();
                    t.trim_end().to_string()
                })
                .collect()
        };
        // Line 17 with two lines on either side; what comes before, the
        // change at line 2 among it, is left out with dots under the numbers.
        assert_eq!(
            text(Notes::Inline),
            [
                "...",
                " 16   l16",
                " 17 - l17",
                " 17 + L17",
                "    ╭───╮",
                "    │ s │",
                "    ╰───╯",
                " 18   l18",
                " 19   l19",
            ]
        );
        // In full, every change shows too, the statement under its line.
        let full = text(Notes::Full);
        assert!(full.contains(&"  2 + L2".to_string()), "{full:?}");
        let note = full.iter().position(|l| l == "    │ s │").unwrap();
        assert_eq!(full[note - 2], " 17 + L17", "{full:?}");
        // Without statements, every change shows.
        let all = text(Notes::Off);
        assert!(all.contains(&"  2 + L2".to_string()), "{all:?}");
        assert!(all.contains(&"...".to_string()), "{all:?}");
        // The dots blend in: muted, no background.
        let dots = &lines(Notes::Inline)[0];
        assert!(dots.spans.iter().all(|s| s.style.bg.is_none()), "{dots:?}");
        assert_eq!(dots.style, app.theme.muted());
    }

    #[test]
    fn hunks_start_at_the_first_changed_row_of_each_run_past_the_notes() {
        use crate::config::Config;
        use crate::keymap::Keymap;
        let (old, new) = twenty();
        let mut u = unit(UnitStatus::Changed, Some((1, 20)), Some((1, 20)));
        u.prep = Prep::Ready;
        u.changes = vec![change(&[(Side::New, 2)]), change(&[(Side::New, 17)])];
        let app = App::new(
            review(Some(&old), Some(&new), u),
            Config::default(),
            Keymap::default(),
        );
        let text = |l: &Line| -> String { l.spans.iter().map(|s| s.content.to_string()).collect() };
        for notes in [Notes::Off, Notes::Inline, Notes::Full] {
            let c = code_change_of(&app, &app.review.units[0], 40, notes);
            assert_eq!(c.hunks.len(), 2, "{notes:?}");
            for &h in &c.hunks {
                let first = text(&c.blocks[h][0]);
                assert!(
                    first.starts_with("  2 -") || first.starts_with(" 17 -"),
                    "{notes:?} {first}"
                );
                let before = text(&c.blocks[h - 1][0]);
                assert!(
                    !before.contains(" - ") && !before.contains(" + "),
                    "{notes:?} {before}"
                );
            }
        }
        // Without statements: l1, then the first hunk; the second after
        // l3, l4, the gap, l15 and l16.
        let c = code_change_of(&app, &app.review.units[0], 40, Notes::Off);
        assert_eq!(c.hunks, [1, 8]);
    }

    #[test]
    fn code_keeps_its_syntax_colours_and_removed_lines_are_tinted() {
        use crate::config::{Config, Theme};
        use crate::keymap::Keymap;
        let old = "def f(order):\n\ta = 1\n\treturn a\n";
        let new = "def f(order):\n\ta = price(order)\n\treturn a\n";
        let app = |theme| {
            let cfg = Config {
                theme,
                ..Config::default()
            };
            let u = unit(UnitStatus::Changed, Some((1, 3)), Some((1, 3)));
            App::new(review(Some(old), Some(new), u), cfg, Keymap::default())
        };
        let spans = |app: &App, row: usize| -> Vec<Span<'static>> {
            code_change(app, &app.review.units[0], 50, Notes::Off)
                .0
                .into_iter()
                .flatten()
                .nth(row)
                .unwrap()
                .spans
        };
        let find = |spans: &[Span<'static>], text: &str| -> Style {
            spans.iter().find(|s| s.content == text).expect(text).style
        };
        let dark = app(Theme::Dark);
        let t = &dark.theme;
        // Rows: def f(order), - a = 1, + a = price(order), return a.
        let def = spans(&dark, 0);
        assert_eq!(find(&def, "def").fg, Some(t.keyword));
        assert_eq!(find(&def, "f").fg, Some(t.function));
        assert!(find(&def, "def").bg.is_none());
        let added = spans(&dark, 2);
        assert_eq!(find(&added, "price").fg, Some(t.function));
        assert_eq!(find(&added, "a").fg, Some(t.variable));
        // An added line isn't tinted; its sign is green.
        assert!(added.iter().all(|s| s.style.bg.is_none()), "{added:?}");
        assert_eq!(added[1].style.fg, Some(t.added));
        // A removed line is tinted from its sign to the edge.
        let removed = spans(&dark, 1);
        assert_eq!(find(&removed, "1").fg, Some(t.number));
        assert!(
            removed[1..]
                .iter()
                .all(|s| s.style.bg == Some(t.removed_bg)),
            "{removed:?}"
        );
        let width: usize = removed.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(width, 50);
        // A tab became four spaces and the colours moved with it.
        let text: String = added.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("+     a = price(order)"), "{text:?}");
        // Without colour: no colours, no tint; the signs say it.
        let mono = app(Theme::Mono);
        let added = spans(&mono, 2);
        assert!(spans(&mono, 1).iter().all(|s| s.style.bg.is_none()));
        assert!(
            added
                .iter()
                .all(|s| s.style.fg.is_none_or(|c| c == ratatui::style::Color::Reset))
        );
        assert!(added.iter().all(|s| s.style.bg.is_none()), "{added:?}");
    }

    #[test]
    fn cited_lines_keep_their_context_and_gaps_stand_for_the_rest() {
        let (old, new) = twenty();
        let r = review(
            Some(&old),
            Some(&new),
            unit(UnitStatus::Changed, Some((1, 20)), Some((1, 20))),
        );
        let u = &r.units[0];
        let cite = |side, line| LineRef { side, line };
        let shape = |cites: &[LineRef]| -> Vec<(Kind, Option<u32>)> {
            rows_near(&r, u, cites, None)
                .0
                .iter()
                .map(|r| (r.kind, r.line))
                .collect()
        };
        let gap = (Kind::Gap, None);
        let context =
            |lines: std::ops::RangeInclusive<u32>| lines.map(|n| (Kind::Context, Some(n)));
        // An unchanged line far from both changes, cited by its old number:
        // a gap on each side, each with a change in it.
        let mut want = vec![gap];
        want.extend(context(7..=11));
        want.push(gap);
        assert_eq!(shape(&[cite(Side::Old, 9)]), want);
        // The counts are those of the whole unit, not of the rows shown.
        let (_, added, removed) = rows_near(&r, u, &[cite(Side::Old, 9)], None);
        assert_eq!((added, removed), (2, 2));
        // A removed line is cited by its old number, not its new one.
        let rows = shape(&[cite(Side::Old, 2)]);
        assert_eq!(rows[0].1, Some(1));
        assert_eq!(rows.last(), Some(&gap));
        // One line between two cited places is shown, not folded; two are.
        let mut want = vec![gap];
        want.extend(context(4..=14));
        want.push(gap);
        assert_eq!(shape(&[cite(Side::New, 6), cite(Side::New, 12)]), want);
        let mut want = vec![gap];
        want.extend(context(4..=8));
        want.push(gap);
        want.extend(context(11..=15));
        want.push(gap);
        assert_eq!(shape(&[cite(Side::New, 6), cite(Side::New, 13)]), want);
        // Nothing cited inside the unit: every change, as without cites.
        assert_eq!(
            rows_near(&r, u, &[cite(Side::New, 99)], None).0,
            unit_rows(&r, u)
        );
    }

    #[test]
    fn a_note_whose_lines_are_not_shown_sits_under_the_nearest_row() {
        let body: Vec<String> = (0..12).map(|i| format!("l{i}")).collect();
        let mut changed = body.clone();
        changed[0] = "L0".into();
        changed[11] = "L11".into();
        let rows = unit_rows(
            &review(
                Some(&body.join("\n")),
                Some(&changed.join("\n")),
                unit(UnitStatus::Changed, Some((1, 12)), Some((1, 12))),
            ),
            &unit(UnitStatus::Changed, Some((1, 12)), Some((1, 12))),
        );
        // Line 9 sits in the gap; the nearest shown row is line 10.
        let k = anchor(&change(&[(Side::New, 9)]), &rows).unwrap();
        assert_eq!(rows[k].line, Some(10));
        assert_eq!(anchor(&change(&[]), &rows), None);
    }

    #[test]
    fn far_apart_changes_collapse_the_middle_into_a_gap() {
        let body: Vec<String> = (0..12).map(|i| format!("  l{i}")).collect();
        let mut changed = body.clone();
        changed[0] = "  L0".into();
        changed[11] = "  L11".into();
        let old = body.join("\n");
        let new = changed.join("\n");
        let got = rows_of(
            Some(&old),
            Some(&new),
            unit(UnitStatus::Changed, Some((1, 12)), Some((1, 12))),
        );
        let kinds: Vec<Kind> = got.iter().map(|r| r.0).collect();
        assert_eq!(
            kinds,
            [
                Kind::Removed,
                Kind::Added,
                Kind::Context,
                Kind::Context,
                Kind::Gap,
                Kind::Context,
                Kind::Context,
                Kind::Removed,
                Kind::Added,
            ]
        );
        // Shared indentation is gone, relative indentation stays.
        assert_eq!(got[0].2, "l0");
    }

    #[test]
    fn added_and_removed_units_are_all_one_side() {
        let got = rows_of(
            Some("a\n"),
            Some("a\ndef g():\n\treturn 1\n"),
            unit(UnitStatus::Added, None, Some((2, 3))),
        );
        assert_eq!(
            got,
            vec![
                (Kind::Added, Some(2), "def g():".into()),
                (Kind::Added, Some(3), "    return 1".into()),
            ]
        );
        let got = rows_of(
            Some("def g():\n  return 1\nb\n"),
            Some("b\n"),
            unit(UnitStatus::Removed, Some((1, 2)), None),
        );
        assert!(
            got.iter().all(|r| r.0 == Kind::Removed && r.1.is_none()),
            "{got:?}"
        );
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn unchanged_or_unknown_units_have_no_rows() {
        let same = "def f():\n  return 1\n";
        assert!(
            rows_of(
                Some(same),
                Some(same),
                unit(UnitStatus::Changed, Some((1, 2)), Some((1, 2)))
            )
            .is_empty()
        );
        let mut r = review(
            Some(same),
            Some(same),
            unit(UnitStatus::Changed, None, None),
        );
        r.units[0].file = PathBuf::from("other.py");
        assert!(unit_rows(&r, &r.units[0]).is_empty());
    }

    fn unwrapped(pieces: &[String], ind: usize) -> String {
        pieces
            .iter()
            .enumerate()
            .map(|(k, p)| if k == 0 { p.as_str() } else { &p[ind..] })
            .collect()
    }

    #[test]
    fn long_code_wraps_under_its_own_indentation_and_keeps_every_character() {
        use unicode_width::UnicodeWidthStr;
        let line = "    if order.currency not in SUPPORTED and order.total > LIMIT:";
        let got = wrap_code(line, 30);
        assert_eq!(
            got,
            [
                "    if order.currency not in ",
                "    SUPPORTED and order.total ",
                "    > LIMIT:"
            ]
        );
        assert!(got.iter().all(|p| p.width() <= 30));
        assert_eq!(unwrapped(&got, 4), line);
        assert_eq!(wrap_code("  x = 1", 30), ["  x = 1"]);
    }

    #[test]
    fn words_longer_than_the_line_are_split_and_wide_glyphs_counted() {
        use unicode_width::UnicodeWidthStr;
        let long = format!("  s = \"{}\"", "a".repeat(40));
        let got = wrap_code(&long, 16);
        assert!(
            got.len() > 2 && got.iter().all(|p| p.width() <= 16),
            "{got:?}"
        );
        assert_eq!(unwrapped(&got, 2), long);
        let wide = "ああああああああ";
        let got = wrap_code(wide, 5);
        assert!(got.iter().all(|p| p.width() <= 5), "{got:?}");
        assert_eq!(got.concat(), wide);
        // Narrower than one glyph: it still ends.
        assert_eq!(wrap_code("ああ", 1), ["あ", "あ"]);
        // Code from a pull request can't move the cursor.
        assert!(!wrap_code("a\u{1b}[2Jb", 10)[0].contains('\u{1b}'));
    }

    #[test]
    fn ranges_past_the_end_of_the_file_do_not_panic() {
        let got = rows_of(
            Some("a\n"),
            Some("b\n"),
            unit(UnitStatus::Changed, Some((5, 9)), Some((0, 3))),
        );
        assert!(got.is_empty(), "{got:?}");
    }
}
