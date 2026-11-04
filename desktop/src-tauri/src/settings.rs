//! The settings screen's side in Rust: `teer setup`'s own presets, checks,
//! model lists, test answer and saving ([`teer::setup`]), for the window
//! to show. A review open in the window takes the saved settings at once.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{State, WebviewWindow};

use teer::config::{ApiConfig, Config, Provider};
use teer::review::store;
use teer::setup::{Choice, Found, PRESETS, Preset, Tool};

use crate::commands::Windows;
use crate::host::{Input, Returned};

/// An agent setup offers.
#[derive(Debug, Clone, Serialize)]
pub struct PresetView {
    pub name: &'static str,
    pub about: &'static str,
    pub api: bool,
    /// Asks for its base URL.
    pub custom: bool,
    pub base_url: &'static str,
    pub key_env: &'static str,
    /// The fast and second model to suggest first.
    pub suggest: (&'static str, &'static str),
    /// Installed (a CLI) or its key set (an API); `None` when nothing is
    /// to be found (a local server).
    pub found: Option<bool>,
}

/// What the settings start from.
#[derive(Debug, Clone, Serialize)]
pub struct SetupState {
    pub presets: Vec<PresetView>,
    /// The preset the config has now.
    pub current: usize,
    pub base_url: String,
    pub key_env: String,
    pub fast: Option<String>,
    pub second: Option<String>,
    pub second_opinion: bool,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolView {
    pub name: String,
    pub needed: bool,
    /// `Ready`, `Broken` or `Missing`.
    pub state: &'static str,
    pub found: String,
    pub about: String,
}

impl From<Tool> for ToolView {
    fn from(t: Tool) -> Self {
        let (state, found) = match t.found {
            Found::Ready(s) => ("Ready", s),
            Found::Broken(s) => ("Broken", s),
            Found::Missing(s) => ("Missing", s),
        };
        ToolView {
            name: t.name,
            needed: t.needed,
            state,
            found,
            about: t.about,
        }
    }
}

/// What the window chose.
#[derive(Debug, Clone, Deserialize)]
pub struct ChoiceIn {
    pub preset: usize,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub key_env: String,
    pub fast: String,
    /// `None`: no second opinion.
    pub second: Option<String>,
}

fn preset(i: usize) -> Result<Preset, String> {
    PRESETS
        .get(i)
        .copied()
        .ok_or_else(|| "No such agent.".to_string())
}

impl ChoiceIn {
    fn api(&self) -> Result<ApiConfig, String> {
        let p = preset(self.preset)?;
        Ok(ApiConfig {
            format: p.format,
            base_url: self.base_url.trim().into(),
            key_env: self.key_env.trim().into(),
        })
    }

    fn choice(&self) -> Result<Choice, String> {
        Ok(Choice {
            provider: preset(self.preset)?.provider,
            api: self.api()?,
            fast: self.fast.trim().into(),
            second: self.second.clone().filter(|s| !s.trim().is_empty()),
        })
    }
}

fn config() -> Result<Config, String> {
    Config::load(&store::config_path()).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn setup_state() -> Result<SetupState, String> {
    let cfg = config()?;
    let found = teer::setup::detect();
    let current = Preset::of(&cfg);
    let api = cfg.provider() == Provider::Api;
    Ok(SetupState {
        presets: PRESETS
            .iter()
            .zip(found)
            .map(|(p, found)| PresetView {
                name: p.name,
                about: p.about(),
                api: p.provider == Provider::Api,
                custom: p.custom,
                base_url: p.base_url,
                key_env: p.key_env,
                suggest: p.suggest,
                found,
            })
            .collect(),
        current,
        base_url: if api {
            cfg.api.base_url.clone()
        } else {
            String::new()
        },
        key_env: if api {
            cfg.api.key_env.clone()
        } else {
            String::new()
        },
        fast: cfg.prepare_model.clone(),
        second: cfg.second_model.clone(),
        second_opinion: cfg.second_opinion,
        config_path: store::config_path(),
    })
}

/// git, gh, the editor, the sandbox and, where commits are signed, the
/// pinentry the window needs.
#[tauri::command]
pub async fn setup_tools(repo: Option<String>) -> Result<Vec<ToolView>, String> {
    let cfg = config()?;
    let keys = cfg.keymap().map(|(k, _)| k).unwrap_or_default();
    let mut tools: Vec<ToolView> = teer::setup::tools(&cfg, &keys)
        .await
        .into_iter()
        .map(ToolView::from)
        .collect();
    if let Some(repo) = repo.map(PathBuf::from)
        && let Some(t) =
            tauri::async_runtime::spawn_blocking(move || teer::setup::pinentry_tool(&repo))
                .await
                .map_err(|e| e.to_string())?
    {
        tools.push(t.into());
    }
    Ok(tools)
}

/// The models agent `preset` offers.
#[tauri::command]
pub async fn setup_models(choice: ChoiceIn) -> Result<Vec<String>, String> {
    let p = preset(choice.preset)?;
    teer::setup::models(p.provider, &choice.api()?, p.suggest.0)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Ask the chosen fast model for a tiny answer: milliseconds it took.
#[tauri::command]
pub async fn setup_test(choice: ChoiceIn) -> Result<u128, String> {
    let cfg = choice.choice()?.apply(&config()?);
    teer::setup::test_answer(cfg).await.map(|d| d.as_millis())
}

/// Save the choice into the config, keeping the rest of it; a review open
/// in this window takes it.
#[tauri::command]
pub fn setup_save(
    window: WebviewWindow,
    windows: State<'_, Windows>,
    choice: ChoiceIn,
) -> Result<(), String> {
    let path = store::config_path();
    let cfg = config()?;
    teer::setup::save(&path, &cfg, &choice.choice()?).map_err(|e| format!("{e:#}"))?;
    let saved = Config::load(&path).map_err(|e| format!("{e:#}"))?;
    if let Some(host) = windows.host(window.label()) {
        let _ = host
            .tx
            .send(Input::Returned(Returned::Settings(Box::new(saved))));
    }
    Ok(())
}
