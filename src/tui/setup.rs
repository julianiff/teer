//! The setup screen (`teer setup`, and the first start in a terminal):
//! what prepares reviews, its API settings when it is an API, the fast
//! model, the second model, and a test answer from the fast model before
//! the choice is saved.

use std::io::IsTerminal;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use unicode_width::UnicodeWidthStr;

use super::slim::SPINNER;
use super::theme::Theme;
use crate::config::{ApiConfig, Config, Provider};
use crate::setup::{Choice, Found, PRESETS, Preset, Tool};
use crate::util::{sanitize, truncate, truncate_pad};

/// A terminal to draw the screen in.
pub fn can_run() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Provider,
    /// Base URL and key variable (API presets only).
    Api,
    Fast,
    Second,
    Check,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Load<T> {
    Waiting,
    Done(T),
    Failed(String),
}

/// What the loop around the screen does next.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// List the models of the chosen provider.
    Models,
    /// Ask the fast model for a test answer.
    Check,
    Save,
    /// Esc on the first step: "not now", or out of `teer setup`.
    Leave,
    /// ctrl+c: out of teer.
    Quit,
}

/// How the screen ended.
#[derive(Debug)]
pub enum Ended {
    /// The choice, saved: the config as teer reads it now.
    Saved(Box<Config>),
    /// Left without saving.
    Left,
    /// ctrl+c: teer stops.
    Quit,
}

/// Answers from the jobs; the number says which request they answer.
pub enum Reply {
    Found(Vec<Option<bool>>),
    Tools(Vec<Tool>),
    Models(u64, Result<Vec<String>, String>),
    Checked(u64, Result<Duration, String>),
}

pub struct Setup {
    /// The first start: leaving means "not now".
    pub first: bool,
    pub step: Step,
    pub cfg: Config,
    /// What was found of each preset; `None` while looking.
    pub found: Option<Vec<Option<bool>>>,
    /// What was found of the programs teer runs besides the agent; `None`
    /// while looking.
    pub tools: Option<Vec<Tool>>,
    pub preset: usize,
    pub url: String,
    pub key_env: String,
    /// The field in focus on the API step: 0 URL, 1 key variable.
    pub field: usize,
    pub models: Load<Vec<String>>,
    pub filter: String,
    pub sel: usize,
    pub fast: Option<String>,
    /// `Some(None)`: no second opinion.
    pub second: Option<Option<String>>,
    pub check: Load<Duration>,
    /// Counts requests, so an answer to an older one is dropped.
    pub asked: u64,
    pub started: Instant,
    pub theme: Theme,
    pub tick: u64,
}

/// A row of a model list: the value it picks and a note.
struct Row {
    value: Option<String>,
    label: String,
    note: String,
}

impl Setup {
    pub fn new(cfg: Config, first: bool) -> Setup {
        let preset = Preset::of(&cfg);
        let theme = Theme::new(cfg.theme);
        let mut s = Setup {
            first,
            step: Step::Provider,
            cfg,
            found: None,
            tools: None,
            preset,
            url: String::new(),
            key_env: String::new(),
            field: 0,
            models: Load::Waiting,
            filter: String::new(),
            sel: 0,
            fast: None,
            second: None,
            check: Load::Waiting,
            asked: 0,
            started: Instant::now(),
            theme,
            tick: 0,
        };
        s.fill_api();
        s
    }

    pub fn chosen(&self) -> Preset {
        PRESETS[self.preset]
    }

    /// The second model picked; `None` for no second opinion.
    pub fn second_choice(&self) -> Option<String> {
        self.second.clone().flatten()
    }

    /// The steps for the chosen agent: the API step only for an API.
    pub fn steps(&self) -> Vec<Step> {
        let mut v = vec![Step::Provider];
        if self.chosen().provider == Provider::Api {
            v.push(Step::Api);
        }
        v.extend([Step::Fast, Step::Second, Step::Check]);
        v
    }

    /// The API fields from the preset, or from the config when it is the
    /// one configured.
    fn fill_api(&mut self) {
        let p = self.chosen();
        if Preset::of(&self.cfg) == self.preset && self.cfg.provider == Some(Provider::Api) {
            self.url = self.cfg.api.base_url.clone();
            self.key_env = self.cfg.api.key_env.clone();
        } else {
            self.url = p.base_url.into();
            self.key_env = p.key_env.into();
        }
    }

    pub fn api(&self) -> ApiConfig {
        ApiConfig {
            format: self.chosen().format,
            base_url: self.url.trim().into(),
            key_env: self.key_env.trim().into(),
        }
    }

    /// The choice so far; `second` decides the second model.
    pub fn choice(&self, second: Option<String>) -> Choice {
        Choice {
            provider: self.chosen().provider,
            api: self.api(),
            fast: self.fast.clone().unwrap_or_default(),
            second,
        }
    }

    /// The model the config has now for this step, when the provider stays.
    fn current(&self, fast: bool) -> Option<String> {
        if self.cfg.provider != Some(self.chosen().provider) {
            return None;
        }
        if fast {
            &self.cfg.prepare_model
        } else {
            &self.cfg.second_model
        }
        .clone()
    }

    fn rows(&self) -> Vec<Row> {
        let p = self.chosen();
        let fast = self.step == Step::Fast;
        let suggest = if fast { p.suggest.0 } else { p.suggest.1 };
        let current = self.current(fast);
        let mut names: Vec<String> = match &self.models {
            Load::Done(m) => m.clone(),
            _ => Vec::new(),
        };
        if p.provider != Provider::Api {
            names.push("default".into());
        }
        let f = self.filter.trim().to_lowercase();
        let mut rows: Vec<Row> = names
            .into_iter()
            .filter(|n| f.is_empty() || n.to_lowercase().contains(&f))
            .map(|n| {
                let note = if n == "default" {
                    format!("{}'s own model", p.provider.title())
                } else if current.as_deref() == Some(n.as_str()) {
                    "now".into()
                } else if n == suggest {
                    "suggested".into()
                } else {
                    String::new()
                };
                Row {
                    label: n.clone(),
                    value: Some(n),
                    note,
                }
            })
            .collect();
        let typed = self.filter.trim();
        if !typed.is_empty() && !rows.iter().any(|r| r.value.as_deref() == Some(typed)) {
            rows.insert(
                0,
                Row {
                    value: Some(typed.into()),
                    label: typed.into(),
                    note: "as typed".into(),
                },
            );
        }
        if self.step == Step::Second && f.is_empty() {
            rows.push(Row {
                value: None,
                label: "none".into(),
                note: "no second opinion".into(),
            });
        }
        rows
    }

    /// Move into a model step: start on the model configured now or the
    /// suggestion.
    fn enter_models(&mut self, step: Step) {
        self.step = step;
        self.filter.clear();
        self.select_wanted();
    }

    fn select_wanted(&mut self) {
        let fast = self.step == Step::Fast;
        let p = self.chosen();
        let want = self.current(fast).or_else(|| {
            let s = if fast { p.suggest.0 } else { p.suggest.1 };
            (!s.is_empty()).then(|| s.to_string())
        });
        let rows = self.rows();
        self.sel = want
            .and_then(|w| {
                rows.iter()
                    .position(|r| r.value.as_deref() == Some(w.as_str()))
            })
            .unwrap_or(0);
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Option<Effect> {
        if k.kind == KeyEventKind::Release {
            return None;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            return Some(Effect::Quit);
        }
        let up = matches!(k.code, KeyCode::Up) || (ctrl && k.code == KeyCode::Char('p'));
        let down = matches!(k.code, KeyCode::Down) || (ctrl && k.code == KeyCode::Char('n'));
        match self.step {
            Step::Provider => {
                let n = PRESETS.len();
                match k.code {
                    _ if up || k.code == KeyCode::Char('k') => {
                        self.preset = (self.preset + n - 1) % n
                    }
                    _ if down || k.code == KeyCode::Char('j') => {
                        self.preset = (self.preset + 1) % n
                    }
                    KeyCode::Esc | KeyCode::Char('q') => return Some(Effect::Leave),
                    KeyCode::Enter => {
                        self.fill_api();
                        self.fast = None;
                        self.models = Load::Waiting;
                        if self.chosen().provider == Provider::Api {
                            self.step = Step::Api;
                            self.field = usize::from(!self.chosen().custom);
                        } else {
                            self.enter_models(Step::Fast);
                            return Some(Effect::Models);
                        }
                    }
                    _ => {}
                }
            }
            Step::Api => {
                let field = if self.field == 0 {
                    &mut self.url
                } else {
                    &mut self.key_env
                };
                match k.code {
                    KeyCode::Esc => self.step = Step::Provider,
                    KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                        self.field = 1 - self.field
                    }
                    KeyCode::Backspace => {
                        field.pop();
                    }
                    KeyCode::Char('u') if ctrl => field.clear(),
                    KeyCode::Char(c) if !ctrl => field.push(c),
                    KeyCode::Enter => {
                        if self.chosen().custom && self.url.trim().is_empty() {
                            self.field = 0;
                            return None;
                        }
                        self.models = Load::Waiting;
                        self.enter_models(Step::Fast);
                        return Some(Effect::Models);
                    }
                    _ => {}
                }
            }
            Step::Fast | Step::Second => {
                let rows = self.rows();
                match k.code {
                    _ if up => self.sel = self.sel.saturating_sub(1),
                    _ if down => self.sel = (self.sel + 1).min(rows.len().saturating_sub(1)),
                    KeyCode::Esc => {
                        if self.step == Step::Second {
                            self.enter_models(Step::Fast);
                        } else if self.chosen().provider == Provider::Api {
                            self.step = Step::Api;
                        } else {
                            self.step = Step::Provider;
                        }
                    }
                    KeyCode::Backspace => {
                        self.filter.pop();
                        self.sel = 0;
                    }
                    KeyCode::Char('u') if ctrl => {
                        self.filter.clear();
                        self.sel = 0;
                    }
                    KeyCode::Char(c) if !ctrl => {
                        self.filter.push(c);
                        self.sel = 0;
                    }
                    KeyCode::Enter => {
                        let row = rows.get(self.sel)?;
                        if self.step == Step::Fast {
                            let v = row.value.clone()?;
                            self.fast = Some(v);
                            self.enter_models(Step::Second);
                        } else {
                            self.second = Some(row.value.clone());
                            self.step = Step::Check;
                            self.check = Load::Waiting;
                            return Some(Effect::Check);
                        }
                    }
                    _ => {}
                }
            }
            Step::Check => match k.code {
                KeyCode::Esc => self.enter_models(Step::Second),
                KeyCode::Enter if !matches!(self.check, Load::Waiting) => {
                    return Some(Effect::Save);
                }
                _ => {}
            },
        }
        None
    }

    pub fn apply(&mut self, r: Reply) {
        match r {
            Reply::Found(f) => self.found = Some(f),
            Reply::Tools(t) => self.tools = Some(t),
            Reply::Models(n, m) if n == self.asked => {
                self.models = match m {
                    Ok(list) => Load::Done(list),
                    Err(e) => Load::Failed(e),
                };
                if self.filter.is_empty() {
                    self.select_wanted();
                }
            }
            Reply::Checked(n, c) if n == self.asked => {
                self.check = match c {
                    Ok(d) => Load::Done(d),
                    Err(e) => Load::Failed(e),
                }
            }
            _ => {}
        }
    }
}

pub fn draw(f: &mut Frame, s: &Setup) {
    let t = &s.theme;
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let steps = s.steps();
    let n = steps.iter().position(|x| *x == s.step).unwrap_or(0) + 1;
    let right = format!("{n}/{} ", steps.len());
    let left = vec![
        Span::styled(" teer ", t.accent().add_modifier(Modifier::BOLD)),
        Span::styled(" setup", t.muted()),
    ];
    let pad = (header.width as usize).saturating_sub(12 + right.len());
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(right, t.muted()));
    f.render_widget(Paragraph::new(Line::from(spans)), header);

    let body = Rect::new(
        body.x + 2.min(body.width),
        body.y + 1.min(body.height),
        body.width.saturating_sub(4),
        body.height.saturating_sub(1),
    );
    let w = body.width as usize;
    let mut lines = Vec::new();
    match s.step {
        Step::Provider => provider_lines(s, w, &mut lines),
        Step::Api => api_lines(s, w, &mut lines),
        Step::Fast | Step::Second => model_lines(s, w, body.height as usize, &mut lines),
        Step::Check => check_lines(s, w, &mut lines),
    }
    f.render_widget(Paragraph::new(lines), body);

    let leave = if s.first { "not now" } else { "quit" };
    let buttons: Vec<(String, &str)> = match s.step {
        Step::Provider => vec![
            ("⏎".into(), "next"),
            ("↑/↓".into(), "move"),
            ("esc".into(), leave),
        ],
        Step::Api => vec![
            ("⏎".into(), "next"),
            ("tab".into(), "field"),
            ("esc".into(), "back"),
        ],
        Step::Fast | Step::Second => vec![
            ("⏎".into(), "pick"),
            ("↑/↓".into(), "move"),
            ("esc".into(), "back"),
        ],
        Step::Check if matches!(s.check, Load::Failed(_)) => {
            vec![("⏎".into(), "save anyway"), ("esc".into(), "back")]
        }
        Step::Check => vec![("⏎".into(), "save"), ("esc".into(), "back")],
    };
    let right = match s.step {
        Step::Fast | Step::Second => "type to filter or name a model",
        _ => "",
    };
    let (line, _) = super::ui::footer_line(t, &buttons, right, footer.width as usize);
    f.render_widget(Paragraph::new(line), footer);
}

fn title(s: &Setup, lines: &mut Vec<Line<'static>>, head: &str, sub: &str) {
    let t = &s.theme;
    lines.push(Line::styled(head.to_string(), t.bold()));
    if !sub.is_empty() {
        lines.push(Line::styled(sub.to_string(), t.muted()));
    }
    lines.push(Line::default());
}

fn marker(s: &Setup, selected: bool) -> (Span<'static>, ratatui::style::Style) {
    let t = &s.theme;
    if selected {
        (
            Span::styled("▌ ", t.accent()),
            t.accent().add_modifier(Modifier::BOLD),
        )
    } else {
        (Span::raw("  "), t.text())
    }
}

fn provider_lines(s: &Setup, w: usize, lines: &mut Vec<Line<'static>>) {
    let t = &s.theme;
    let sub = if s.first {
        "teer asks an agent to describe every change. Pick one; `teer setup` changes it later."
    } else {
        "teer asks an agent to describe every change."
    };
    title(s, lines, "What prepares your reviews?", sub);
    let name_w = PRESETS.iter().map(|p| p.name.len()).max().unwrap_or(0) + 3;
    let looking = (
        SPINNER[(s.tick as usize) % SPINNER.len()].to_string(),
        t.muted(),
    );
    let found: Vec<_> = (PRESETS.iter().enumerate())
        .map(|(i, p)| match s.found.as_ref().map(|f| f[i]) {
            None => looking.clone(),
            Some(None) => (String::new(), t.muted()),
            Some(Some(true)) if p.provider == Provider::Api => {
                (format!("{} is set", p.key_env), t.ok())
            }
            Some(Some(false)) if p.provider == Provider::Api => {
                (format!("{} not set", p.key_env), t.muted())
            }
            Some(Some(true)) => ("found".into(), t.ok()),
            Some(Some(false)) => ("not installed".into(), t.muted()),
        })
        .collect();
    let tools: Vec<_> = match &s.tools {
        None => (crate::setup::tool_names(&s.cfg).into_iter())
            .map(|name| (name, looking.clone(), String::new()))
            .collect(),
        Some(tools) => tools.iter().map(|tool| tool_row(s, tool)).collect(),
    };
    let found_w = (found.iter().chain(tools.iter().map(|(_, f, _)| f)))
        .map(|(f, _)| f.width())
        .max()
        .unwrap_or(0)
        .clamp(13, 28)
        + 3;
    let rest = w.saturating_sub(2 + name_w + found_w);
    let row = |gutter, name: &str, style, (found, found_style): (String, _), about: &str| {
        Line::from(vec![
            gutter,
            Span::styled(truncate_pad(name, name_w), style),
            Span::styled(truncate_pad(&sanitize(&found), found_w), found_style),
            Span::styled(truncate(&sanitize(about), rest), t.muted()),
        ])
    };
    for (i, (p, found)) in PRESETS.iter().zip(found).enumerate() {
        let (gutter, style) = marker(s, i == s.preset);
        lines.push(row(gutter, p.name, style, found, p.about()));
    }
    lines.push(Line::default());
    lines.push(Line::styled("Also used", t.bold()));
    for (name, found, about) in tools {
        lines.push(row(Span::raw("  "), &name, t.text(), found, &about));
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        "Next: a fast model describes every change, a second one checks it.",
        t.muted(),
    ));
}

/// A program's row: what is found of it, styled, and what it is for.
fn tool_row(s: &Setup, tool: &Tool) -> (String, (String, ratatui::style::Style), String) {
    let t = &s.theme;
    let found = match &tool.found {
        Found::Ready(f) => (f.clone(), t.ok()),
        Found::Broken(f) => (f.clone(), t.bad()),
        Found::Missing(f) if tool.needed => (f.clone(), t.bad()),
        Found::Missing(f) => (f.clone(), t.muted()),
    };
    (tool.name.clone(), found, tool.about.clone())
}

fn api_lines(s: &Setup, w: usize, lines: &mut Vec<Line<'static>>) {
    let t = &s.theme;
    let p = s.chosen();
    title(
        s,
        lines,
        p.name,
        "The key stays in your environment: the config only names the variable.",
    );
    let cursor = |on: bool| if on { "▏" } else { "" };
    let url_shown = if s.url.is_empty() && s.field != 0 {
        format!("{} (default)", p.format.default_url())
    } else {
        format!("{}{}", s.url, cursor(s.field == 0))
    };
    let key_state = match s.key_env.trim() {
        "" => ("no key".to_string(), t.muted()),
        v if std::env::var(v).is_ok_and(|k| !k.trim().is_empty()) => ("set".to_string(), t.ok()),
        _ => ("not set in this shell".to_string(), t.bad()),
    };
    let field = |label: &str, value: String, on: bool| {
        let (gutter, style) = marker(s, on);
        Line::from(vec![
            gutter,
            Span::styled(format!("{label:<14}"), t.muted()),
            Span::styled(truncate(&value, w.saturating_sub(16)), style),
        ])
    };
    lines.push(field("Base URL", url_shown, s.field == 0));
    let mut key = field(
        "Key variable",
        format!("{}{}", s.key_env, cursor(s.field == 1)),
        s.field == 1,
    );
    key.spans
        .push(Span::styled(format!("   {}", key_state.0), key_state.1));
    lines.push(key);
    lines.push(Line::default());
    let speaks = match p.format {
        crate::config::ApiFormat::Anthropic => "Speaks Anthropic's Messages API.",
        crate::config::ApiFormat::Openai => "Speaks OpenAI's chat completions.",
    };
    lines.push(Line::styled(speaks, t.muted()));
    lines.push(Line::styled(
        "teer gives the model read-only tools for the repository: list, read, grep.",
        t.muted(),
    ));
}

fn model_lines(s: &Setup, w: usize, h: usize, lines: &mut Vec<Line<'static>>) {
    let t = &s.theme;
    let p = s.chosen();
    let api = p.provider == Provider::Api;
    let (head, sub) = if s.step == Step::Fast {
        (
            "Fast model",
            "Describes every change. Pick a quick, cheap one.".to_string(),
        )
    } else {
        let deep = if api {
            " and answers the Flagged questions and Ask"
        } else {
            ""
        };
        (
            "Second model",
            format!("Checks each description{deep}. Pick a stronger one."),
        )
    };
    title(s, lines, head, &sub);
    let filter = format!("filter  {}▏", s.filter);
    lines.push(Line::styled(filter, t.muted()));
    let rows = s.rows();
    match &s.models {
        Load::Waiting => lines.push(Line::styled(
            format!(
                "{} listing {}'s models…",
                SPINNER[(s.tick as usize) % SPINNER.len()],
                p.name
            ),
            t.muted(),
        )),
        Load::Failed(e) => lines.push(Line::styled(
            truncate(&format!("couldn't list the models: {}", sanitize(e)), w),
            t.bad(),
        )),
        Load::Done(_) => lines.push(Line::default()),
    }
    if rows.is_empty() {
        lines.push(Line::styled("  type a model name", t.muted()));
        return;
    }
    let room = h.saturating_sub(lines.len()).max(1);
    let top = s.sel.saturating_sub(room.saturating_sub(1));
    let label_w = rows
        .iter()
        .map(|r| r.label.len())
        .max()
        .unwrap_or(0)
        .min(w / 2)
        + 3;
    for (i, r) in rows.iter().enumerate().skip(top).take(room) {
        let (gutter, style) = marker(s, i == s.sel);
        lines.push(Line::from(vec![
            gutter,
            Span::styled(truncate_pad(&sanitize(&r.label), label_w), style),
            Span::styled(truncate(&r.note, w.saturating_sub(label_w + 2)), t.muted()),
        ]));
    }
}

fn check_lines(s: &Setup, w: usize, lines: &mut Vec<Line<'static>>) {
    let t = &s.theme;
    let p = s.chosen();
    title(s, lines, "Ready", "");
    let row = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<14}"), t.muted()),
            Span::styled(truncate(&v, w.saturating_sub(14)), t.text()),
        ])
    };
    lines.push(row("Agent", p.name.into()));
    if p.provider == Provider::Api {
        let a = s.api();
        lines.push(row("URL", a.url()));
        let key = if a.key_env.is_empty() {
            "none".into()
        } else {
            format!("${}", a.key_env)
        };
        lines.push(row("Key", key));
    }
    let fast = s.fast.clone().unwrap_or_default();
    lines.push(row("Fast model", sanitize(&fast)));
    let second = s
        .second_choice()
        .map_or("none: no second opinion".into(), |m| sanitize(&m));
    lines.push(row("Second model", second));
    lines.push(Line::default());
    match &s.check {
        Load::Waiting => {
            let secs = s.started.elapsed().as_secs();
            lines.push(Line::styled(
                format!(
                    "{} asking {} for a test answer… {secs}s",
                    SPINNER[(s.tick as usize) % SPINNER.len()],
                    sanitize(&fast)
                ),
                t.muted(),
            ));
        }
        Load::Done(d) => lines.push(Line::styled(
            format!("✓ {} answered in {:.1}s", sanitize(&fast), d.as_secs_f64()),
            t.ok(),
        )),
        Load::Failed(e) => {
            lines.push(Line::styled("✗ the test answer failed:", t.bad()));
            for l in sanitize(e).chars().collect::<Vec<_>>().chunks(w.max(20)) {
                lines.push(Line::styled(l.iter().collect::<String>(), t.bad()));
            }
        }
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        format!(
            "Saved to {} on enter.",
            crate::review::store::config_path().display()
        ),
        t.muted(),
    ));
}

/// Ask the fast model of `cfg` for a tiny answer, in an empty directory.
async fn test_answer(cfg: Config) -> Result<Duration, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let agent = crate::cli::discover_agent(dir.path(), false, &cfg)?;
    let session =
        crate::review::model::SessionRef::fresh(cfg.provider().name(), dir.path().to_path_buf());
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "ok": { "type": "boolean" } },
        "required": ["ok"],
        "additionalProperties": false
    });
    let prompt =
        "This is a connection test from teer, a code review tool. Answer with {\"ok\": true}.";
    let start = Instant::now();
    let ask = agent.adapter.ask_prepare(&session, prompt, &schema);
    match tokio::time::timeout(Duration::from_secs(120), ask).await {
        Err(_) => Err("no answer within 120s".into()),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Ok(Ok(v)) if v["ok"].as_bool().is_some() => Ok(start.elapsed()),
        Ok(Ok(v)) => Err(format!("unexpected answer: {v}")),
    }
}

fn start(s: &mut Setup, e: &Effect, tx: &UnboundedSender<Reply>) {
    s.asked += 1;
    let n = s.asked;
    let tx = tx.clone();
    match e {
        Effect::Models => {
            let (p, api) = (s.chosen(), s.api());
            let suggest = p.suggest.0;
            tokio::spawn(async move {
                let r = crate::setup::models(p.provider, &api, suggest)
                    .await
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Reply::Models(n, r));
            });
        }
        Effect::Check => {
            s.started = Instant::now();
            let cfg = s.choice(s.second_choice()).apply(&s.cfg);
            tokio::spawn(async move {
                let _ = tx.send(Reply::Checked(n, test_answer(cfg).await));
            });
        }
        Effect::Save | Effect::Leave | Effect::Quit => {}
    }
}

/// Show the screen; saving writes the choice into the config at `path`.
pub async fn run(cfg: Config, path: &Path, first: bool) -> Result<Ended> {
    let mut s = Setup::new(cfg, first);
    let (tx, mut rx) = unbounded_channel();
    let found = tx.clone();
    tokio::task::spawn_blocking(move || {
        let _ = found.send(Reply::Found(crate::setup::detect()));
    });
    let (tools, cfg) = (tx.clone(), s.cfg.clone());
    tokio::spawn(async move {
        let _ = tools.send(Reply::Tools(crate::setup::tools(&cfg).await));
    });
    let mut terminal = ratatui::init();
    super::ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
    let result: Result<Result<Choice, Ended>> = async {
        let mut events = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            terminal.draw(|f| draw(f, &s))?;
            tokio::select! {
                ev = events.next() => match ev {
                    Some(Ok(Event::Key(k))) => match s.on_key(k) {
                        Some(Effect::Leave) => return Ok(Err(Ended::Left)),
                        Some(Effect::Quit) => return Ok(Err(Ended::Quit)),
                        Some(Effect::Save) => return Ok(Ok(s.choice(s.second_choice()))),
                        Some(e) => start(&mut s, &e, &tx),
                        None => {}
                    },
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                    None => return Ok(Err(Ended::Quit)),
                },
                Some(r) = rx.recv() => s.apply(r),
                _ = tick.tick() => s.tick = s.tick.wrapping_add(1),
            }
        }
    }
    .await;
    super::restore_terminal();
    let choice = match result? {
        Ok(c) => c,
        Err(ended) => return Ok(ended),
    };
    crate::setup::save(path, &s.cfg, &choice)?;
    Ok(Ended::Saved(Box::new(Config::load(path)?)))
}
