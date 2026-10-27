//! The picker `teer pr list` opens on a terminal: the open pull requests
//! that involve you, by section, one of them selected. Enter reviews it
//! (as `teer pr <url>` would), q leaves. It is up while gh is still asked.

use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::slim::SPINNER;
use super::theme::Theme;
use crate::config::{Action, KeyMap};
use crate::pr_list::{LIMIT, ListedPr, Listing, RepoRef, SHOWN, Section, age, details};
use crate::util::{truncate, truncate_pad};

/// How the picker ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    /// Review this pull request.
    Pr(ListedPr),
    /// Left without picking one.
    Quit,
    /// gh found nothing to pick from.
    Empty(Listing),
}

pub struct Picker {
    pub repo: RepoRef,
    /// `None` while gh is still asked.
    pub listing: Option<Listing>,
    /// Every pull request in screen order, with its section.
    rows: Vec<(Section, ListedPr)>,
    pub sel: usize,
    /// First body line on screen.
    scroll: usize,
    /// Body lines on screen at the last draw; page keys move by this.
    page: usize,
    pub now: DateTime<Utc>,
    pub theme: Theme,
    pub keys: KeyMap,
    pub tick: u64,
}

impl Picker {
    pub fn new(repo: RepoRef, theme: Theme, keys: KeyMap, now: DateTime<Utc>) -> Picker {
        Picker {
            repo,
            listing: None,
            rows: Vec::new(),
            sel: 0,
            scroll: 0,
            page: 1,
            now,
            theme,
            keys,
            tick: 0,
        }
    }

    pub fn loaded(&mut self, l: Listing) {
        self.rows = SHOWN
            .iter()
            .flat_map(|s| l.section(*s).iter().map(|p| (*s, p.clone())))
            .collect();
        self.sel = 0;
        self.scroll = 0;
        self.listing = Some(l);
    }

    pub fn selected(&self) -> Option<&ListedPr> {
        self.rows.get(self.sel).map(|(_, p)| p)
    }

    /// Is pull request `p` the first of its section?
    fn section_starts(&self, p: usize) -> bool {
        p == 0 || self.rows[p - 1].0 != self.rows[p].0
    }

    /// Move or decide. `Some` ends the picker.
    pub fn on_key(&mut self, k: KeyEvent) -> Option<Picked> {
        if k.kind == KeyEventKind::Release {
            return None;
        }
        let is = |a: Action| self.keys.is(a, &k);
        let ctrl_c = k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl_c || is(Action::Quit) || is(Action::Back) {
            return Some(Picked::Quit);
        }
        let last = self.rows.len().saturating_sub(1);
        let page = self.page.max(1);
        if is(Action::Open) || k.code == KeyCode::Enter {
            return self.selected().cloned().map(Picked::Pr);
        } else if is(Action::Down) || k.code == KeyCode::Down {
            self.sel = (self.sel + 1).min(last);
        } else if is(Action::Up) || k.code == KeyCode::Up {
            self.sel = self.sel.saturating_sub(1);
        } else if is(Action::Top) || k.code == KeyCode::Home {
            self.sel = 0;
        } else if is(Action::Bottom) || k.code == KeyCode::End {
            self.sel = last;
        } else if is(Action::NextGroup) {
            if let Some(p) = (self.sel + 1..self.rows.len()).find(|&p| self.section_starts(p)) {
                self.sel = p;
            }
        } else if is(Action::PrevGroup) {
            // The first of this section, then the first of the one above.
            let start = (0..=self.sel).rev().find(|&p| self.section_starts(p));
            if let Some(start) = start.filter(|&s| s > 0) {
                self.sel = (0..start)
                    .rev()
                    .find(|&p| self.section_starts(p))
                    .unwrap_or(0);
            }
        } else if k.code == KeyCode::PageDown {
            self.sel = (self.sel + page).min(last);
        } else if k.code == KeyCode::PageUp {
            self.sel = self.sel.saturating_sub(page);
        }
        None
    }

    /// The body as lines `w` cells wide, and the line each pull request is on.
    fn body(&self, w: usize) -> (Vec<Line<'static>>, Vec<usize>) {
        let t = &self.theme;
        let Some(l) = &self.listing else {
            let spin = SPINNER[(self.tick % 10) as usize];
            let text = format!(
                "{spin} Asking GitHub for the pull requests of {}…",
                self.repo.name()
            );
            return (
                vec![Line::styled(truncate(&text, w), t.muted())],
                Vec::new(),
            );
        };
        let num_w = self
            .rows
            .iter()
            .map(|(_, p)| p.number.to_string().len() + 1)
            .max()
            .unwrap_or(2);
        let age_w = self
            .rows
            .iter()
            .map(|(_, p)| age(p.updated_at, self.now).width())
            .max()
            .unwrap_or(0);
        let mut meta_w = self
            .rows
            .iter()
            .map(|(s, p)| details(p, *s).width())
            .max()
            .unwrap_or(0)
            .min(32);
        // "▌ #12  title  meta  age"
        let fixed = |meta_w: usize| 2 + num_w + 2 + 2 + meta_w + 2 + age_w;
        if w < fixed(meta_w) + 20 {
            meta_w = 0;
        }
        // No wider than the longest title, so the details stay next to it.
        let widest = self.rows.iter().map(|(_, p)| p.title.width()).max();
        let title_w = w
            .saturating_sub(fixed(meta_w))
            .min(widest.unwrap_or(0))
            .max(10);

        let mut lines = Vec::new();
        let mut at = Vec::new();
        for (k, s) in SHOWN.iter().enumerate() {
            if k > 0 {
                lines.push(Line::default());
            }
            let prs = l.section(*s);
            lines.push(Line::from(vec![
                Span::styled(s.title().to_string(), t.bold()),
                Span::styled(format!(" · {}", prs.len()), t.muted()),
            ]));
            if prs.is_empty() {
                lines.push(Line::styled("  none", t.muted()));
            }
            for p in prs {
                let selected = at.len() == self.sel;
                at.push(lines.len());
                let (gutter, num, title) = if selected {
                    let strong = t.accent().add_modifier(Modifier::BOLD);
                    (Span::styled("▌", t.accent()), t.accent(), strong)
                } else {
                    (Span::raw(" "), t.muted(), t.text())
                };
                let mut spans = vec![
                    gutter,
                    Span::styled(format!(" {:>num_w$}  ", format!("#{}", p.number)), num),
                    Span::styled(truncate_pad(&p.title, title_w), title),
                ];
                if meta_w > 0 {
                    spans.push(Span::styled(
                        format!("  {}", truncate_pad(&details(p, *s), meta_w)),
                        t.muted(),
                    ));
                }
                spans.push(Span::styled(
                    format!("  {:>age_w$}", age(p.updated_at, self.now)),
                    t.muted(),
                ));
                lines.push(Line::from(spans));
            }
        }
        if l.truncated {
            lines.push(Line::default());
            lines.push(Line::styled(
                format!("Only the first {LIMIT} of a section are listed."),
                t.muted(),
            ));
        }
        (lines, at)
    }

    /// Scroll so the selected pull request is on screen, with its section
    /// title when it is the first of its section.
    fn fit(&mut self, at: &[usize], total: usize) {
        let h = self.page.max(1);
        if let Some(&line) = at.get(self.sel) {
            let top = if self.sel == 0 {
                0
            } else if self.section_starts(self.sel) {
                line - 1
            } else {
                line
            };
            if top < self.scroll {
                self.scroll = top;
            }
            if line >= self.scroll + h {
                self.scroll = line + 1 - h;
            }
            if self.sel + 1 == self.rows.len() {
                // The last one brings along whatever is below it.
                self.scroll = self.scroll.max(total.saturating_sub(h).min(line));
            }
        }
        self.scroll = self.scroll.min(total.saturating_sub(h));
    }
}

pub fn draw(f: &mut Frame, p: &mut Picker) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_header(f, p, header);
    let body = Rect::new(
        body.x + 1.min(body.width),
        body.y + 1.min(body.height),
        body.width.saturating_sub(2),
        body.height.saturating_sub(1),
    );
    let (lines, at) = p.body(body.width as usize);
    p.page = body.height as usize;
    p.fit(&at, lines.len());
    let shown: Vec<Line> = lines.into_iter().skip(p.scroll).take(p.page).collect();
    f.render_widget(Paragraph::new(shown), body);
    draw_footer(f, p, footer);
}

fn draw_header(f: &mut Frame, p: &Picker, area: Rect) {
    let t = &p.theme;
    let line = Line::from(vec![
        Span::styled(" teer ", t.accent().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {} · pull requests", p.repo.name()), t.muted()),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_footer(f: &mut Frame, p: &Picker, area: Rect) {
    let buttons = [
        (p.keys.label(Action::Open), "review"),
        (
            format!(
                "{}/{}",
                p.keys.label(Action::Down),
                p.keys.label(Action::Up)
            ),
            "move",
        ),
        (p.keys.label(Action::Quit), "quit"),
    ];
    let url = p.selected().map(|s| s.url.as_str()).unwrap_or_default();
    let (line, _) = super::ui::footer_line(&p.theme, &buttons, url, area.width as usize);
    f.render_widget(Paragraph::new(line), area);
}

/// Open the picker on the terminal while `listing` (the gh searches) runs.
/// Leaving the picker while gh is still asked drops `listing`, which ends gh.
pub async fn pick(
    repo: RepoRef,
    listing: impl Future<Output = Result<Listing>>,
    theme: Theme,
    keys: KeyMap,
) -> Result<Picked> {
    let mut p = Picker::new(repo, theme, keys, Utc::now());
    let mut terminal = ratatui::init();
    super::ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
    let result: Result<Picked> = async {
        let mut listing = std::pin::pin!(listing);
        let mut loading = true;
        let mut events = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            terminal.draw(|f| draw(f, &mut p))?;
            tokio::select! {
                l = &mut listing, if loading => {
                    loading = false;
                    let l = l?;
                    if l.is_empty() {
                        return Ok(Picked::Empty(l));
                    }
                    p.loaded(l);
                }
                ev = events.next() => match ev {
                    Some(Ok(Event::Key(k))) => {
                        if let Some(done) = p.on_key(k) {
                            return Ok(done);
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                    None => return Ok(Picked::Quit),
                },
                _ = tick.tick(), if loading => p.tick = p.tick.wrapping_add(1),
            }
        }
    }
    .await;
    super::restore_terminal();
    result
}
