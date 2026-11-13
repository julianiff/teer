//! `~/.config/teer/config.toml`.
//!
//! ```toml
//! theme = "dark"            # dark | light | mono
//! concurrency = 4           # agent requests in flight
//! languages = ["rust", "python", "typescript", "javascript", "php", "go"]
//! large_review = 150        # above this many units, prepare lazily per file
//! dive_web = true           # a question in the unit view may search the web (asks once per session)
//! provider = "claude"       # claude | codex | api; `teer setup` picks it (asked on first start)
//! prepare_model = "haiku"   # fast model that describes units; "default" = the CLI's own
//! prepare_effort = "low"    # low | medium | high | xhigh | max | default
//! order = "story"           # inside a file, story: units that call each other together, caller first | line
//! auto_accept = "no_behavior_change"  # accept units with no behavior change for you (za shows them) | none
//! second_opinion = true     # a second model checks each description; disagreements are reasons to look
//! second_model = "sonnet"   # model of that check, also of the Flagged questions with an API
//! second_effort = "low"     # low | medium | high | xhigh | max | default
//! behavior_timeout = 120    # seconds one Behavior probe run may take (each side)
//! timeoutlen = 1000         # milliseconds a key prefix (g, ], [, z, space) waits for the next key
//!
//! [api]                     # provider = "api" only
//! format = "anthropic"      # anthropic (Messages API) | openai (chat completions: OpenAI, OpenRouter, Ollama, ...)
//! base_url = ""             # empty: https://api.anthropic.com or https://api.openai.com/v1
//! key_env = "ANTHROPIC_API_KEY"  # the environment variable holding the key; empty for none
//!
//! [fix_loop]                # after the notes were copied (`y`, or for the model API)
//! quiet_seconds = 5         # re-run once the files changed and then stayed the same this long
//!
//! [keys]                    # action id = key or keys, in Neovim notation (see `crate::keymap`)
//! "decide.accept" = "a"
//! "move.down" = ["j", "<down>"]
//! ```

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::keymap::{KeySpec, Keymap};

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

/// Which units teer accepts for the reviewer before the review opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoAccept {
    /// None: every unit waits for the reviewer.
    None,
    /// Units with no behavior change (formatting, comments, renames) that no
    /// second model disputes.
    #[default]
    NoBehaviorChange,
}

/// When the agent proposes a change on its own (`<space>e` always asks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProposeFixes {
    /// When a high security finding stands (its path verified, not
    /// disputed): something very important has to change.
    #[default]
    High,
    /// Only when asked.
    Never,
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
    /// The API key from the variable `key_env` names; `None` when it
    /// names none (a local server).
    pub fn key(&self) -> std::result::Result<Option<String>, String> {
        let var = self.key_env.trim();
        if var.is_empty() {
            return Ok(None);
        }
        match std::env::var(var) {
            Ok(k) if !k.trim().is_empty() => Ok(Some(k.trim().to_string())),
            _ => Err(format!(
                "${var} isn't set: export your API key in it, or pick another agent with `teer setup`."
            )),
        }
    }

    pub fn url(&self) -> String {
        let u = self.base_url.trim().trim_end_matches('/');
        if u.is_empty() {
            self.format.default_url().into()
        } else {
            u.into()
        }
    }
}

/// `[fix_loop]`: how teer waits for the changes to notes it copied out
/// (`y` on the `:w` sheet, or for the model API), watching the working tree.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct FixLoop {
    /// Seconds the files must stay the same, after they changed, before
    /// teer re-runs the review on its own. Fractions work.
    pub quiet_seconds: f64,
}

impl Default for FixLoop {
    fn default() -> Self {
        FixLoop { quiet_seconds: 5.0 }
    }
}

impl FixLoop {
    /// `quiet_seconds` as a duration; a negative or unreadable one is none.
    pub fn quiet(&self) -> std::time::Duration {
        let s = self.quiet_seconds;
        std::time::Duration::from_secs_f64(if s.is_finite() {
            s.clamp(0.0, 86_400.0)
        } else {
            0.0
        })
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
    /// Whether the unit view's questions that can use web search may do so.
    pub dive_web: bool,
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
    /// Which units are accepted for the reviewer (`za` shows them in the list).
    pub auto_accept: AutoAccept,
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
    /// Milliseconds a key prefix waits for the next key (Neovim's name).
    pub timeoutlen: u64,
    /// Waiting for the changes to copied notes.
    pub fix_loop: FixLoop,
    /// When the agent proposes a change without being asked.
    pub propose_fixes: ProposeFixes,
    /// The mode a review opens in when the command line names none and
    /// the reviewer chose none for it; unset, a pull request opens in
    /// `review` and your own changes in `check` (see [`crate::mode`]).
    pub mode: Option<crate::mode::Mode>,
    /// Action id to its keys (see [`crate::keymap`]).
    pub keys: HashMap<String, KeySpec>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: Theme::Dark,
            concurrency: 4,
            languages: None,
            large_review: 150,
            dive_web: true,
            provider: None,
            api: ApiConfig::default(),
            prepare_model: None,
            prepare_effort: None,
            order: Order::Story,
            auto_accept: AutoAccept::NoBehaviorChange,
            second_opinion: true,
            second_model: None,
            second_effort: None,
            behavior_timeout: 120,
            editor: crate::editor::DEFAULT.into(),
            timeoutlen: 1000,
            fix_loop: FixLoop::default(),
            propose_fixes: ProposeFixes::default(),
            mode: None,
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

    /// The keymap with `[keys]` applied, and warnings for the user.
    pub fn keymap(&self) -> Result<(Keymap, Vec<String>)> {
        Keymap::new(&self.keys, self.timeoutlen)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_and_keys() {
        let c: Config = toml::from_str(
            "theme = \"mono\"\nconcurrency = 2\ntimeoutlen = 500\n[keys]\n\"decide.accept\" = \"<c-a>\"\n\"move.down\" = [\"j\", \"<c-n>\"]\nreject = \"y\"\n",
        )
        .unwrap();
        assert_eq!(c.theme, Theme::Mono);
        assert_eq!(c.timeoutlen, 500);
        let (km, warnings) = c.keymap().unwrap();
        assert_eq!(km.label(crate::keymap::Action::DecideAccept), "^a");
        assert_eq!(km.labels(crate::keymap::Action::MoveDown), "j/^n");
        assert_eq!(
            warnings,
            vec!["[keys]: unknown action 'reject', ignored".to_string()]
        );
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
    fn auto_accept_defaults_to_no_behavior_change_and_can_be_off() {
        assert_eq!(Config::default().auto_accept, AutoAccept::NoBehaviorChange);
        let c: Config = toml::from_str("theme = \"dark\"\n").unwrap();
        assert_eq!(c.auto_accept, AutoAccept::NoBehaviorChange);
        let c: Config = toml::from_str("auto_accept = \"none\"\n").unwrap();
        assert_eq!(c.auto_accept, AutoAccept::None);
        let c: Config = toml::from_str("auto_accept = \"no_behavior_change\"\n").unwrap();
        assert_eq!(c.auto_accept, AutoAccept::NoBehaviorChange);
        assert!(toml::from_str::<Config>("auto_accept = \"all\"\n").is_err());
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
    fn the_fix_loop_waits_five_quiet_seconds_unless_set() {
        use std::time::Duration;
        assert_eq!(Config::default().fix_loop.quiet(), Duration::from_secs(5));
        let c: Config = toml::from_str("[fix_loop]\nquiet_seconds = 2\n").unwrap();
        assert_eq!(c.fix_loop.quiet(), Duration::from_secs(2));
        let c: Config = toml::from_str("[fix_loop]\nquiet_seconds = 0.25\n").unwrap();
        assert_eq!(c.fix_loop.quiet(), Duration::from_millis(250));
        let c: Config = toml::from_str("[fix_loop]\nquiet_seconds = -3\n").unwrap();
        assert_eq!(c.fix_loop.quiet(), Duration::ZERO);
    }
}
