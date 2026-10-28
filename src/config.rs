//! `~/.config/teer/config.toml`.
//!
//! ```toml
//! theme = "dark"            # dark | light | mono
//! concurrency = 4           # agent requests in flight
//! languages = ["rust", "python", "typescript", "javascript", "php"]
//! large_review = 150        # above this many units, prepare lazily per file
//! dive_web = true           # a Flagged-screen question may search the web (asks once per session)
//! images = true             # draw Visualize pictures in the terminal when it can show images
//! provider = "claude"       # claude | codex | api; `teer setup` picks it (asked on first start)
//! prepare_model = "haiku"   # fast model that describes units; "default" = the CLI's own
//! prepare_effort = "low"    # low | medium | high | xhigh | max | default
//! order = "story"           # inside a file, story: units that call each other together, caller first | line
//! second_opinion = true     # a second model checks each description; disagreements are warnings
//! second_model = "sonnet"   # model of that check, also of the Flagged questions with an API
//! second_effort = "low"     # low | medium | high | xhigh | max | default
//! behavior_timeout = 120    # seconds one Behavior probe run may take (each side)
//!
//! [api]                     # provider = "api" only
//! format = "anthropic"      # anthropic (Messages API) | openai (chat completions: OpenAI, OpenRouter, Ollama, ...)
//! base_url = ""             # empty: https://api.anthropic.com or https://api.openai.com/v1
//! key_env = "ANTHROPIC_API_KEY"  # the environment variable holding the key; empty for none
//!
//! [keys]
//! accept = "l"
//! investigate = "i"
//! ```

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    Mono,
}

/// How units are ordered inside a file of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    /// Units that call each other stay together, the caller first (see
    /// `review::story`).
    #[default]
    Story,
    /// Line by line.
    Line,
}

/// What prepares the reviews: a coding agent's headless mode, or a model
/// API with teer's own read-only tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// `claude -p` (Claude Code).
    #[default]
    Claude,
    /// `codex exec` (Codex).
    Codex,
    /// A model API with a key ([`ApiConfig`]).
    Api,
}

impl Provider {
    /// The config value, also the agent name saved with a review.
    pub fn name(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
            Provider::Api => "api",
        }
    }

    /// The CLI's name in words; the API has none.
    pub fn title(self) -> &'static str {
        match self {
            Provider::Claude => "Claude Code",
            Provider::Codex => "Codex",
            Provider::Api => "the model API",
        }
    }

    /// The program run headless, for the CLIs.
    pub fn program(self) -> Option<&'static str> {
        match self {
            Provider::Claude => Some(crate::agent::commands::CLAUDE_BIN),
            Provider::Codex => Some(crate::agent::commands::CODEX_BIN),
            Provider::Api => None,
        }
    }
}

/// How a model API is spoken to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApiFormat {
    /// Anthropic's Messages API.
    #[default]
    Anthropic,
    /// OpenAI's chat completions, which OpenRouter, Ollama, LM Studio, vLLM
    /// and many more speak too.
    Openai,
}

impl ApiFormat {
    pub fn name(self) -> &'static str {
        match self {
            ApiFormat::Anthropic => "anthropic",
            ApiFormat::Openai => "openai",
        }
    }

    pub fn default_url(self) -> &'static str {
        match self {
            ApiFormat::Anthropic => "https://api.anthropic.com",
            ApiFormat::Openai => "https://api.openai.com/v1",
        }
    }
}

/// `[api]`: the model API for `provider = "api"`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    pub format: ApiFormat,
    /// Empty: the format's own ([`ApiFormat::default_url`]).
    pub base_url: String,
    /// The environment variable that holds the key: the key itself is never
    /// written to the config. Empty for servers that take none (Ollama).
    pub key_env: String,
}

impl ApiConfig {
    pub fn url(&self) -> String {
        let u = self.base_url.trim().trim_end_matches('/');
        if u.is_empty() {
            self.format.default_url().into()
        } else {
            u.into()
        }
    }
}

/// What a model is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Describes units: fast and cheap (`prepare_model`).
    Fast,
    /// Checks those descriptions (`second_model`).
    Second,
    /// Flagged questions, Ask, Behavior probes, commit messages: the CLI's
    /// own default model, or the second model with an API.
    Deep,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: Theme,
    pub concurrency: usize,
    pub languages: Option<Vec<String>>,
    pub large_review: usize,
    /// Whether Flagged-screen questions that can use web search may do so.
    pub dive_web: bool,
    /// Draw Visualize pictures inside the terminal when it supports images
    /// (asked once at start-up); false keeps the text list.
    pub images: bool,
    /// What prepares reviews. `None` until `teer setup` ran, which teer
    /// offers on the first start in a terminal.
    pub provider: Option<Provider>,
    pub api: ApiConfig,
    /// The model that describes units: an alias such as `haiku`, or a full
    /// model name; `default` leaves the CLI's own. Unset: the provider's
    /// fast default ([`Config::model`]).
    pub prepare_model: Option<String>,
    /// The effort for describing units; `default` leaves the provider's own.
    pub prepare_effort: Option<String>,
    pub order: Order,
    /// After describing units, a second model checks each description; where
    /// it disagrees the unit gets a warning and at least "Worth a look".
    pub second_opinion: bool,
    /// The model of that check, like `prepare_model`. With an API it also
    /// answers the Flagged questions.
    pub second_model: Option<String>,
    /// The effort for that check, like `prepare_effort`.
    pub second_effort: Option<String>,
    /// Seconds one run of a Behavior probe may take (before and after each get this long).
    pub behavior_timeout: u64,
    /// The command `e` opens a unit with, and commit messages and review
    /// text are edited in: `{file}` and `{line}` are filled in.
    pub editor: String,
    pub keys: HashMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: Theme::Dark,
            concurrency: 4,
            languages: None,
            large_review: 150,
            dive_web: true,
            images: true,
            provider: None,
            api: ApiConfig::default(),
            prepare_model: None,
            prepare_effort: None,
            order: Order::Story,
            second_opinion: true,
            second_model: None,
            second_effort: None,
            behavior_timeout: 120,
            editor: crate::editor::DEFAULT.into(),
            keys: HashMap::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                toml::from_str(&s).with_context(|| format!("invalid config at {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("can't read {}", path.display())),
        }
    }

    /// `NO_COLOR` set and not empty (no-color.org) means the mono theme:
    /// the terminal library drops every colour then anyway, and mono marks
    /// with bold, dim and underline instead, so accepted rows still fade.
    pub fn with_no_color(mut self, no_color: Option<&std::ffi::OsStr>) -> Config {
        if no_color.is_some_and(|v| !v.is_empty()) {
            self.theme = Theme::Mono;
        }
        self
    }

    pub fn editor(&self) -> Result<crate::editor::Editor> {
        crate::editor::Editor::parse(&self.editor)
    }

    pub fn keymap(&self) -> Result<KeyMap> {
        KeyMap::with_overrides(&self.keys)
    }

    /// The provider: the configured one, else Claude Code, which was the
    /// only one before there was a choice.
    pub fn provider(&self) -> Provider {
        self.provider.unwrap_or_default()
    }

    /// `prepare_model` and `prepare_effort`, checked (see [`Self::model`]).
    pub fn prepare_model(&self) -> Result<crate::agent::PrepareModel> {
        self.model(Role::Fast)
    }

    /// `second_model` and `second_effort`, checked (see [`Self::model`]).
    pub fn second_model(&self) -> Result<crate::agent::PrepareModel> {
        self.model(Role::Second)
    }

    /// The model and effort for `role`, the provider's default where unset,
    /// checked: `default` (or empty) means none given, which only a CLI can
    /// fill in; a model name is letters, digits and `.-_[]/:`; an effort
    /// one of the provider's levels.
    pub fn model(&self, role: Role) -> Result<crate::agent::PrepareModel> {
        let p = self.provider();
        let (fast, second) = self.default_models();
        let (model, effort, name) = match role {
            Role::Fast => (&self.prepare_model, &self.prepare_effort, "prepare"),
            Role::Second | Role::Deep => (&self.second_model, &self.second_effort, "second"),
        };
        let default = if role == Role::Fast { fast } else { second };
        let model = model.clone().or(default.map(String::from));
        let effort = effort.clone().or(self.default_effort().map(String::from));
        let mut m = model_flags(
            p,
            self.api.format,
            model.as_deref().unwrap_or(""),
            effort.as_deref().unwrap_or(""),
            name,
        )?;
        if role == Role::Deep {
            if p == Provider::Api {
                // The second model on the provider's own effort.
                m.effort = None;
            } else {
                // The CLI's own default, as the user runs it.
                m = crate::agent::PrepareModel::default();
            }
        }
        if p == Provider::Api && m.model.is_none() {
            anyhow::bail!(
                "{name}_model is not set: a model API needs a model name (run `teer setup`)"
            );
        }
        Ok(m)
    }

    /// The fast and second model when the config names none.
    fn default_models(&self) -> (Option<&'static str>, Option<&'static str>) {
        match (self.provider(), self.api.format) {
            (Provider::Claude, _) => (Some("haiku"), Some("sonnet")),
            (Provider::Codex, _) => (Some("default"), Some("default")),
            (Provider::Api, ApiFormat::Anthropic) => {
                (Some("claude-haiku-5-5"), Some("claude-sonnet-5-5"))
            }
            (Provider::Api, ApiFormat::Openai) => (None, None),
        }
    }

    /// The effort when the config names none: low where the levels are
    /// known; an OpenAI-compatible model may not take one at all.
    fn default_effort(&self) -> Option<&'static str> {
        match (self.provider(), self.api.format) {
            (Provider::Api, ApiFormat::Openai) => None,
            _ => Some("low"),
        }
    }

    /// The model for `role` in words, for the screens: `haiku`, or
    /// `Claude Code's default model` when the CLI picks.
    pub fn model_label(&self, role: Role) -> String {
        let p = self.provider();
        match self.model(role).ok().and_then(|m| m.model) {
            Some(m) => crate::util::sanitize(&m),
            None => format!("{}'s default model", p.title()),
        }
    }
}

/// The effort levels each provider takes.
fn efforts(p: Provider, format: ApiFormat) -> &'static [&'static str] {
    match (p, format) {
        (Provider::Claude, _) | (Provider::Api, ApiFormat::Anthropic) => {
            &["low", "medium", "high", "xhigh", "max"]
        }
        (Provider::Codex, _) => &["minimal", "low", "medium", "high", "xhigh", "max"],
        (Provider::Api, ApiFormat::Openai) => {
            &["none", "minimal", "low", "medium", "high", "xhigh"]
        }
    }
}

fn model_flags(
    p: Provider,
    format: ApiFormat,
    model: &str,
    effort: &str,
    name: &str,
) -> Result<crate::agent::PrepareModel> {
    let unset = |s: &str| {
        let s = s.trim();
        (!(s.is_empty() || s.eq_ignore_ascii_case("default"))).then(|| s.to_string())
    };
    let model = unset(model);
    if let Some(m) = &model {
        let ok = !m.starts_with('-')
            && m.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-[]/:".contains(c));
        if !ok {
            anyhow::bail!("invalid {name}_model '{m}' in config");
        }
    }
    let effort = unset(effort).map(|e| e.to_ascii_lowercase());
    let levels = efforts(p, format);
    if let Some(e) = &effort
        && !levels.contains(&e.as_str())
    {
        anyhow::bail!(
            "invalid {name}_effort '{e}' in config for {} ({} or default)",
            p.name(),
            levels.join(", ")
        );
    }
    Ok(crate::agent::PrepareModel { model, effort })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Down,
    Up,
    /// Scroll the detail area down: the list preview, the swipe card, the
    /// open tab on the Flagged screen.
    PreviewDown,
    /// Scroll the detail area up.
    PreviewUp,
    Top,
    Bottom,
    Open,
    Swipe,
    Rerun,
    Reset,
    Edit,
    Accept,
    Investigate,
    /// Opens the Flagged screen (all units marked investigate). Config name
    /// `flagged`.
    Flagged,
    DiveRecommend,
    DiveVisualize,
    /// Run the unit before and after the change (Flagged screen only).
    DiveBehavior,
    /// Show (or ask for) the second opinion on the unit (Flagged screen only).
    DiveDisagree,
    /// The tests that reach the unit, found by teer's own search (Flagged screen only).
    DiveTests,
    /// The agent reads the tests the search found: which cover the unit, and
    /// dangerous cases none does (Flagged screen only).
    InterpretTests,
    /// Open the selected unit's Visualize page in the browser (Flagged screen only).
    OpenVisual,
    ResolveOk,
    ResolveFix,
    /// Write the note on the unit on show (any screen).
    EditNote,
    /// Open the notes: every unit's note, to copy, hand to Claude Code, or
    /// post on the pull request.
    Notes,
    /// Re-ask the open Flagged-screen tab (Flagged screen only).
    DiveRerun,
    PrevTile,
    NextTile,
    /// The Flagged tile in focus fills the screen, or no longer does.
    Zoom,
    Undo,
    Back,
    Help,
    NextPending,
    PrevPending,
    /// Previous card (swipe only); decisions are left alone.
    PrevCard,
    /// The next file in the list, the next group in the PR picker (no
    /// wrapping).
    NextGroup,
    /// The file above in the list, the group above in the PR picker.
    PrevGroup,
    /// Accept every pending unit in the selected file, after a confirmation
    /// (list only).
    AcceptFile,
    /// List the units in their files, or each in its own level's box (list
    /// only).
    Grouping,
    /// Approve the pull request on GitHub (only in the "all reviewed" popup
    /// of a `teer pr` review).
    ApprovePr,
    /// Commit the reviewed changes (only in the "all reviewed" popup of a
    /// review of uncommitted changes).
    Commit,
    /// Show the commits between base and head and the PR's description (list only).
    Commits,
    /// Opens the Ask view: questions about the code, answered by the agent.
    Ask,
}

impl Action {
    const ALL: &[(Action, &'static str, &'static str)] = &[
        (Action::Quit, "quit", "q"),
        (Action::Down, "down", "j"),
        (Action::Up, "up", "k"),
        // PageDown and PageUp scroll a page (see App::on_key).
        (Action::PreviewDown, "preview_down", "J"),
        (Action::PreviewUp, "preview_up", "K"),
        (Action::Top, "top", "g"),
        (Action::Bottom, "bottom", "G"),
        (Action::Open, "open", "enter"),
        (Action::Swipe, "swipe", "s"),
        (Action::Rerun, "rerun", "r"),
        (Action::Reset, "reset", "R"),
        (Action::Edit, "edit", "e"),
        (Action::Accept, "accept", "l"),
        // `h` and Left also investigate in the list, Left in swipe (see
        // App::action_for); `i` is the configurable key.
        (Action::Investigate, "investigate", "i"),
        (Action::Flagged, "flagged", "d"),
        (Action::DiveRecommend, "dive_recommend", "n"),
        (Action::DiveVisualize, "dive_visualize", "v"),
        (Action::DiveBehavior, "dive_behavior", "t"),
        (Action::DiveDisagree, "dive_disagree", "x"),
        // `c` commits only inside the all-done popup, which needs nothing flagged.
        (Action::DiveTests, "dive_tests", "c"),
        // `a` approves only inside the all-done popup, which needs nothing flagged.
        (Action::InterpretTests, "interpret_tests", "a"),
        (Action::OpenVisual, "open_visual", "b"),
        (Action::ResolveOk, "resolve_ok", "o"),
        (Action::ResolveFix, "resolve_fix", "f"),
        (Action::EditNote, "edit_note", "m"),
        (Action::Notes, "notes", "N"),
        (Action::DiveRerun, "dive_rerun", "r"),
        (Action::PrevTile, "prev_tile", "left"),
        (Action::NextTile, "next_tile", "right"),
        (Action::Zoom, "zoom", "z"),
        (Action::Undo, "undo", "u"),
        (Action::Back, "back", "esc"),
        (Action::Help, "help", "?"),
        // List triage. `l`/`i` accept/investigate in the list as in swipe.
        (Action::NextPending, "next_pending", "]"),
        (Action::PrevPending, "prev_pending", "["),
        // Swipe only: go back one card without changing its decision.
        (Action::PrevCard, "prev_card", "h"),
        (Action::NextGroup, "next_group", "ctrl+d"),
        (Action::PrevGroup, "prev_group", "ctrl+u"),
        (Action::AcceptFile, "accept_file", "A"),
        // `f` resolves "needs fix" only on the Flagged screen.
        (Action::Grouping, "grouping", "f"),
        (Action::ApprovePr, "approve_pr", "a"),
        (Action::Commit, "commit", "c"),
        (Action::Commits, "commits", "C"),
        (Action::Ask, "ask", "/"),
    ];
}

/// Parse "j", "G", "enter", "tab", "esc", "ctrl+c", "left", "space".
pub fn parse_key(s: &str) -> Option<KeyEvent> {
    let (mods, key) = match s.rsplit_once('+') {
        Some((m, k)) if !k.is_empty() => (m, k),
        _ => ("", s),
    };
    let mut modifiers = KeyModifiers::NONE;
    for m in mods.split('+').filter(|m| !m.is_empty()) {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" => modifiers |= KeyModifiers::CONTROL,
            "alt" => modifiers |= KeyModifiers::ALT,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            _ => return None,
        }
    }
    let code = match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "backspace" => KeyCode::Backspace,
        _ if key.chars().count() == 1 => KeyCode::Char(key.chars().next()?),
        _ => return None,
    };
    Some(KeyEvent::new(code, modifiers))
}

#[derive(Debug, Clone)]
pub struct KeyMap {
    bindings: HashMap<Action, KeyEvent>,
}

impl KeyMap {
    pub fn with_overrides(overrides: &HashMap<String, String>) -> Result<KeyMap> {
        let mut bindings = HashMap::new();
        for (action, name, default) in Action::ALL {
            let key = overrides.get(*name).map(String::as_str).unwrap_or(default);
            let ev = parse_key(key)
                .with_context(|| format!("unknown key '{key}' for '{name}' in config"))?;
            bindings.insert(*action, ev);
        }
        for name in overrides.keys() {
            if !Action::ALL.iter().any(|(_, n, _)| n == name) {
                anyhow::bail!("unknown action '{name}' in [keys]");
            }
        }
        Ok(KeyMap { bindings })
    }

    pub fn is(&self, action: Action, ev: &KeyEvent) -> bool {
        let Some(b) = self.bindings.get(&action) else {
            return false;
        };
        // Shifted letters arrive as uppercase chars, sometimes with SHIFT set.
        let norm = |m: KeyModifiers| m - KeyModifiers::SHIFT;
        b.code == ev.code && norm(b.modifiers) == norm(ev.modifiers)
    }

    pub fn label(&self, action: Action) -> String {
        let Some(b) = self.bindings.get(&action) else {
            return String::new();
        };
        let key = match b.code {
            KeyCode::Enter => "⏎".to_string(),
            KeyCode::Tab => "tab".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Char(' ') => "space".into(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            other => format!("{other:?}"),
        };
        if b.modifiers.contains(KeyModifiers::CONTROL) {
            format!("^{key}")
        } else {
            key
        }
    }
}

impl Default for KeyMap {
    fn default() -> Self {
        KeyMap::with_overrides(&HashMap::new()).expect("default keys parse")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_and_keys() {
        let c: Config =
            toml::from_str("theme = \"mono\"\nconcurrency = 2\n[keys]\naccept = \"ctrl+a\"\n")
                .unwrap();
        assert_eq!(c.theme, Theme::Mono);
        let km = c.keymap().unwrap();
        assert!(km.is(
            Action::Accept,
            &KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)
        ));
        assert!(km.is(
            Action::Bottom,
            &KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)
        ));
    }

    #[test]
    fn default_keys_and_unknown_actions() {
        let key = |c| KeyEvent::from(KeyCode::Char(c));
        let km = KeyMap::default();
        assert!(km.is(Action::Investigate, &key('i')));
        assert!(km.is(Action::Flagged, &key('d')));
        assert!(km.is(Action::Up, &key('k')));
        let c: Config = toml::from_str("[keys]\nreject = \"y\"\n").unwrap();
        assert!(c.keymap().is_err());
    }

    #[test]
    fn prepare_model_defaults_to_haiku_low_and_is_checked() {
        let p = Config::default().prepare_model().unwrap();
        assert_eq!(
            (p.model.as_deref(), p.effort.as_deref()),
            (Some("haiku"), Some("low"))
        );
        let c: Config =
            toml::from_str("prepare_model = \"claude-sonnet-5-5\"\nprepare_effort = \"High\"\n")
                .unwrap();
        let p = c.prepare_model().unwrap();
        assert_eq!(
            (p.model.as_deref(), p.effort.as_deref()),
            (Some("claude-sonnet-5-5"), Some("high"))
        );
        let c: Config =
            toml::from_str("prepare_model = \"default\"\nprepare_effort = \"\"\n").unwrap();
        assert_eq!(
            c.prepare_model().unwrap(),
            crate::agent::PrepareModel::default()
        );
        for bad in [
            "prepare_model = \"--dangerously-skip-permissions\"",
            "prepare_model = \"haiku; rm -rf /\"",
            "prepare_effort = \"turbo\"",
        ] {
            let c: Config = toml::from_str(bad).unwrap();
            assert!(c.prepare_model().is_err(), "{bad}");
        }
    }

    #[test]
    fn models_default_per_provider_and_deep_questions_follow_it() {
        let m = |c: &Config, r| {
            let m = c.model(r).unwrap();
            (m.model, m.effort)
        };
        let s = |v: &str| Some(v.to_string());
        let claude = Config::default();
        assert_eq!(m(&claude, Role::Fast), (s("haiku"), s("low")));
        assert_eq!(m(&claude, Role::Second), (s("sonnet"), s("low")));
        // The Flagged questions keep the CLI's own model.
        assert_eq!(m(&claude, Role::Deep), (None, None));
        let codex: Config =
            toml::from_str("provider = \"codex\"\nsecond_effort = \"minimal\"\n").unwrap();
        assert_eq!(m(&codex, Role::Fast), (None, s("low")));
        assert_eq!(m(&codex, Role::Second), (None, s("minimal")));
        let anthropic: Config = toml::from_str("provider = \"api\"\n").unwrap();
        assert_eq!(m(&anthropic, Role::Fast), (s("claude-haiku-5-5"), s("low")));
        // With an API the second model answers them, at its own effort.
        assert_eq!(m(&anthropic, Role::Deep), (s("claude-sonnet-5-5"), None));
        let openai: Config =
            toml::from_str("provider = \"api\"\n[api]\nformat = \"openai\"\n").unwrap();
        assert!(
            openai.prepare_model().is_err(),
            "an OpenAI-compatible API has no default model"
        );
        let openai: Config = toml::from_str(
            "provider = \"api\"\nprepare_model = \"qwen3:8b\"\nsecond_model = \"vendor/big\"\n[api]\nformat = \"openai\"\nbase_url = \"http://localhost:11434/v1/\"\n",
        )
        .unwrap();
        assert_eq!(m(&openai, Role::Fast), (s("qwen3:8b"), None));
        assert_eq!(openai.api.url(), "http://localhost:11434/v1");
        assert_eq!(openai.model_label(Role::Deep), "vendor/big");
        assert_eq!(Config::default().api.url(), "https://api.anthropic.com");
    }

    #[test]
    fn efforts_are_checked_against_the_providers_levels() {
        let bad = |t: &str| {
            toml::from_str::<Config>(t)
                .unwrap()
                .prepare_model()
                .is_err()
        };
        assert!(
            bad("prepare_effort = \"minimal\"\n"),
            "Claude Code has no minimal"
        );
        assert!(!bad("provider = \"codex\"\nprepare_effort = \"minimal\"\n"));
        assert!(bad("provider = \"codex\"\nprepare_effort = \"ultra\"\n"));
        assert!(bad("provider = \"api\"\nprepare_effort = \"none\"\n"));
        assert!(!bad(
            "provider = \"api\"\nprepare_model = \"m\"\nprepare_effort = \"none\"\n[api]\nformat = \"openai\"\n"
        ));
        assert!(toml::from_str::<Config>("provider = \"gemini\"\n").is_err());
    }

    #[test]
    fn images_default_on_and_can_be_disabled() {
        assert!(Config::default().images);
        let c: Config = toml::from_str("images = false\n").unwrap();
        assert!(!c.images);
    }

    #[test]
    fn dive_web_defaults_on_and_can_be_disabled() {
        assert!(Config::default().dive_web);
        let c: Config = toml::from_str("dive_web = false\n").unwrap();
        assert!(!c.dive_web);
        let c: Config = toml::from_str("theme = \"dark\"\n").unwrap();
        assert!(c.dive_web);
    }

    #[test]
    fn dive_keys_have_defaults() {
        let key = |c| KeyEvent::from(KeyCode::Char(c));
        let km = KeyMap::default();
        assert!(km.is(Action::DiveRerun, &key('r')));
        assert!(km.is(Action::ResolveOk, &key('o')));
        assert!(km.is(Action::ResolveFix, &key('f')));
        assert!(km.is(Action::EditNote, &key('m')));
        assert!(km.is(Action::NextTile, &KeyEvent::from(KeyCode::Right)));
        assert!(km.is(Action::DiveVisualize, &key('v')));
        assert!(km.is(Action::OpenVisual, &key('b')));
    }

    #[test]
    fn no_color_means_mono() {
        use std::ffi::OsStr;
        let dark = || toml::from_str::<Config>("theme = \"dark\"\n").unwrap();
        assert_eq!(
            dark().with_no_color(Some(OsStr::new("1"))).theme,
            Theme::Mono
        );
        assert_eq!(
            dark().with_no_color(Some(OsStr::new(""))).theme,
            Theme::Dark
        );
        assert_eq!(dark().with_no_color(None).theme, Theme::Dark);
    }

    #[test]
    fn preview_scroll_keys_default_to_capital_j_and_k() {
        let km = KeyMap::default();
        let shifted = KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT);
        assert!(km.is(Action::PreviewDown, &shifted));
        assert!(km.is(Action::PreviewUp, &KeyEvent::from(KeyCode::Char('K'))));
        assert!(!km.is(Action::PreviewDown, &KeyEvent::from(KeyCode::Char('j'))));
        assert_eq!(km.label(Action::PreviewDown), "J");
    }

    #[test]
    fn rejects_unknown_actions() {
        let c: Config = toml::from_str("[keys]\nfly = \"x\"\n").unwrap();
        assert!(c.keymap().is_err());
    }
}
