//! `teer setup`, without the screen: the agents it offers, what is found
//! of them on this machine, the models to pick from, and writing the choice
//! into the config (comments and other settings stay as they are).

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::config::{ApiConfig, ApiFormat, Config, Provider};

/// One of the agents setup offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    pub provider: Provider,
    pub format: ApiFormat,
    /// Empty: the format's own.
    pub base_url: &'static str,
    pub key_env: &'static str,
    /// Asks for its base URL (a server only the user knows).
    pub custom: bool,
    /// The fast and second model to suggest first.
    pub suggest: (&'static str, &'static str),
}

const fn api(
    name: &'static str,
    format: ApiFormat,
    base_url: &'static str,
    key_env: &'static str,
    suggest: (&'static str, &'static str),
) -> Preset {
    Preset {
        name,
        provider: Provider::Api,
        format,
        base_url,
        key_env,
        custom: false,
        suggest,
    }
}

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "Claude Code",
        provider: Provider::Claude,
        format: ApiFormat::Anthropic,
        base_url: "",
        key_env: "",
        custom: false,
        suggest: ("haiku", "sonnet"),
    },
    Preset {
        name: "Codex",
        provider: Provider::Codex,
        format: ApiFormat::Openai,
        base_url: "",
        key_env: "",
        custom: false,
        suggest: ("", ""),
    },
    api(
        "Anthropic API",
        ApiFormat::Anthropic,
        "",
        "ANTHROPIC_API_KEY",
        ("claude-haiku-5-5", "claude-sonnet-5-5"),
    ),
    api(
        "OpenAI API",
        ApiFormat::Openai,
        "",
        "OPENAI_API_KEY",
        ("", ""),
    ),
    api(
        "OpenRouter",
        ApiFormat::Openai,
        "https://openrouter.ai/api/v1",
        "OPENROUTER_API_KEY",
        ("", ""),
    ),
    api(
        "Ollama",
        ApiFormat::Openai,
        "http://localhost:11434/v1",
        "",
        ("", ""),
    ),
    Preset {
        custom: true,
        ..api("Other API", ApiFormat::Openai, "", "", ("", ""))
    },
];

impl Preset {
    /// What setup says about it before it is picked.
    pub fn about(self) -> &'static str {
        match (self.provider, self.custom, self.key_env.is_empty()) {
            (Provider::Claude, ..) => "claude -p, headless",
            (Provider::Codex, ..) => "codex exec, read-only",
            (_, true, _) => "any server that speaks OpenAI's chat completions",
            (_, _, true) => "local, no key",
            _ => "your API key, teer runs the agent",
        }
    }

    /// The preset that matches what `cfg` has now, to start from.
    pub fn of(cfg: &Config) -> usize {
        let p = cfg.provider();
        PRESETS
            .iter()
            .position(|s| match p {
                Provider::Api => {
                    s.provider == p
                        && s.format == cfg.api.format
                        && (s.custom
                            || s.base_url == cfg.api.url()
                            || (s.base_url.is_empty() && cfg.api.base_url.is_empty()))
                }
                _ => s.provider == p,
            })
            .unwrap_or(0)
    }
}

/// What is found of each preset on this machine, in its order: the CLI
/// installed, the key's variable set.
pub fn detect() -> Vec<Option<bool>> {
    PRESETS
        .iter()
        .map(|p| match p.provider {
            Provider::Api if p.key_env.is_empty() => None,
            Provider::Api => Some(std::env::var(p.key_env).is_ok_and(|k| !k.trim().is_empty())),
            cli => Some(
                cli.program()
                    .is_some_and(crate::agent::claude::is_installed),
            ),
        })
        .collect()
}

/// A program teer runs besides the agent, and what is found of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    /// teer doesn't work without it.
    pub needed: bool,
    pub found: Found,
    /// What it is for, or what to do about it.
    pub about: String,
}

/// What is found of a [`Tool`]; each says so in a few words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// Usable as it is: "found", "logged in as …".
    Ready(String),
    /// There, but not usable as it is.
    Broken(String),
    /// Not on this machine.
    Missing(String),
}

/// How long `gh auth status` may take: it asks GitHub.
const GH_TIMEOUT: Duration = Duration::from_secs(10);

/// The programs [`tools`] looks for, in its order: the editor by the name
/// `editor` in `cfg` gives it.
pub fn tool_names(cfg: &Config) -> Vec<String> {
    let editor = cfg.editor().map_or("editor".into(), |e| e.name());
    vec!["git".into(), "gh".into(), editor, "sandbox-exec".into()]
}

/// What is found of each of [`tool_names`]: git teer needs, gh for pull
/// requests (installed and logged in), the editor of `cfg` for `e`, the
/// sandbox Behavior runs code in.
pub async fn tools(cfg: &Config) -> Vec<Tool> {
    let names = tool_names(cfg);
    let gh_program = crate::approve::Tools::default().gh;
    let git = tokio::task::spawn_blocking(|| crate::agent::claude::is_installed("git"));
    let gh = gh_tool(&gh_program).await;
    let git = git.await.unwrap_or(false);
    let sandbox = crate::sandbox::available();
    let tool = |name: &String, needed, found: bool, missing: &str, about: &str| Tool {
        name: name.clone(),
        needed,
        found: if found {
            Found::Ready("found".into())
        } else {
            Found::Missing(missing.into())
        },
        about: about.into(),
    };
    // Looked up, not run: `--version` may open an IDE's window.
    let editor = match cfg.editor() {
        Ok(e) => tool(
            &names[2],
            false,
            on_path(e.program()),
            "not installed",
            "`e` opens a unit in it (`editor` in the config)",
        ),
        Err(e) => Tool {
            name: names[2].clone(),
            needed: false,
            found: Found::Broken("invalid".into()),
            about: format!("{e:#}"),
        },
    };
    vec![
        tool(
            &names[0],
            true,
            git,
            "not installed",
            if git {
                "reads the changes"
            } else {
                "needed: teer reads every change with it"
            },
        ),
        gh,
        editor,
        tool(
            &names[3],
            false,
            sandbox,
            "not on this system",
            if sandbox {
                "Behavior runs code in it"
            } else {
                "Behavior runs code only on macOS"
            },
        ),
    ]
}

/// Is `program` an executable file: as given when it has a `/`, else in a
/// folder on PATH?
fn on_path(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let runs = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        return runs(Path::new(program));
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| runs(&dir.join(program))))
}

/// Is `gh` installed and logged in? `gh auth status` says, with the account.
pub async fn gh_tool(gh: &OsStr) -> Tool {
    let tool = |found, about: &str| Tool {
        name: "gh".into(),
        needed: false,
        found,
        about: about.into(),
    };
    let what_for = "pull request titles, `teer pr list`, posting reviews";
    let login = "run `gh auth login`";
    let cwd = std::env::temp_dir();
    let run = crate::gh::run(gh, &cwd, &["auth", "status"], None);
    let out = match tokio::time::timeout(GH_TIMEOUT, run).await {
        Err(_) => {
            let why = format!("`gh auth status` ran over {}s", GH_TIMEOUT.as_secs());
            return tool(Found::Broken("no answer".into()), &why);
        }
        Ok(Err(crate::gh::Error::NotInstalled)) => {
            return tool(Found::Missing("not installed".into()), what_for);
        }
        Ok(Err(crate::gh::Error::Failed(why))) => {
            return tool(Found::Broken("doesn't run".into()), &why);
        }
        Ok(Ok(out)) => out,
    };
    match gh_account(&format!("{}\n{}", out.stdout, out.stderr)) {
        Some((host, user)) if host == "github.com" => {
            tool(Found::Ready(format!("logged in as {user}")), what_for)
        }
        Some((host, user)) => tool(
            Found::Ready(format!("logged in as {user} on {host}")),
            what_for,
        ),
        None if out.ok => tool(Found::Ready("logged in".into()), what_for),
        None => tool(Found::Broken("not logged in".into()), login),
    }
}

/// The host and account of the first "Logged in to <host> account <user>"
/// line of `gh auth status` (older gh: "… to <host> as <user>").
fn gh_account(text: &str) -> Option<(String, String)> {
    text.lines().find_map(|l| {
        let (_, rest) = l.split_once("Logged in to ")?;
        match rest.split_whitespace().collect::<Vec<_>>()[..] {
            [host, "account" | "as", user, ..] => Some((host.into(), user.into())),
            _ => None,
        }
    })
}

/// The models to pick from: the provider's list, its suggestion first.
pub async fn models(provider: Provider, api: &ApiConfig, suggest: &str) -> Result<Vec<String>> {
    let mut list = match provider {
        Provider::Claude => ["haiku", "sonnet", "opus"].map(String::from).to_vec(),
        Provider::Codex => {
            let bin = crate::agent::commands::CODEX_BIN;
            tokio::task::spawn_blocking(move || crate::agent::codex::list_models(bin)).await??
        }
        Provider::Api => {
            let key = crate::cli::api_key(api).map_err(anyhow::Error::msg)?;
            crate::agent::api::list_models(api.format, &api.url(), key.as_deref()).await?
        }
    };
    if let Some(i) = list.iter().position(|m| m == suggest) {
        let m = list.remove(i);
        list.insert(0, m);
    }
    Ok(list)
}

/// What setup decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub provider: Provider,
    /// For [`Provider::Api`].
    pub api: ApiConfig,
    pub fast: String,
    /// `None`: no second opinion.
    pub second: Option<String>,
}

impl Choice {
    /// `cfg` with the choice in it, as teer will read it.
    pub fn apply(&self, cfg: &Config) -> Config {
        let mut c = cfg.clone();
        c.provider = Some(self.provider);
        if self.provider == Provider::Api {
            c.api = self.api.clone();
        }
        c.prepare_model = Some(self.fast.clone());
        c.second_opinion = self.second.is_some();
        if let Some(s) = &self.second {
            c.second_model = Some(s.clone());
        }
        // Efforts the new provider doesn't take go back to its default.
        if c.prepare_model().is_err() {
            c.prepare_effort = None;
        }
        if c.second_model().is_err() {
            c.second_effort = None;
        }
        c
    }
}

/// Write `choice` into the config at `path`, keeping everything else in it.
pub fn save(path: &Path, cfg: &Config, choice: &Choice) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("can't read {}", path.display())),
    };
    let mut doc: DocumentMut = text
        .parse()
        .with_context(|| format!("invalid config at {}", path.display()))?;
    let applied = choice.apply(cfg);
    doc["provider"] = value(choice.provider.name());
    doc["prepare_model"] = value(&choice.fast);
    doc["second_opinion"] = value(choice.second.is_some());
    if let Some(s) = &choice.second {
        doc["second_model"] = value(s);
    }
    for (key, kept) in [
        ("prepare_effort", &applied.prepare_effort),
        ("second_effort", &applied.second_effort),
    ] {
        if kept.is_none() {
            doc.remove(key);
        }
    }
    if choice.provider == Provider::Api {
        if !doc.contains_table("api") {
            doc["api"] = Item::Table(Table::new());
        }
        let a = &mut doc["api"];
        a["format"] = value(choice.api.format.name());
        a["base_url"] = value(choice.api.base_url.trim());
        a["key_env"] = value(choice.api.key_env.trim());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("can't create {}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())
        .with_context(|| format!("can't write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("can't write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_keeps_comments_and_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teer/config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# mine\ntheme = \"light\"\nprepare_effort = \"max\"\n\n[keys]\naccept = \"y\"\n",
        )
        .unwrap();
        let cfg = Config::load(&path).unwrap();
        let choice = Choice {
            provider: Provider::Api,
            api: ApiConfig {
                format: ApiFormat::Openai,
                base_url: "https://openrouter.ai/api/v1".into(),
                key_env: "OPENROUTER_API_KEY".into(),
            },
            fast: "vendor/small:free".into(),
            second: Some("vendor/big".into()),
        };
        save(&path, &cfg, &choice).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# mine\ntheme = \"light\"\n"), "{text}");
        // OpenAI-compatible APIs don't take `max`: back to the default.
        assert!(!text.contains("prepare_effort"), "{text}");
        let saved = Config::load(&path).unwrap();
        assert_eq!(saved.provider, Some(Provider::Api));
        assert_eq!(saved.api, choice.api);
        assert_eq!(
            saved.prepare_model().unwrap().model.as_deref(),
            Some("vendor/small:free")
        );
        assert_eq!(
            saved.second_model().unwrap().model.as_deref(),
            Some("vendor/big")
        );
        assert!(saved.second_opinion);
        assert_eq!(saved.keys.get("accept").map(String::as_str), Some("y"));
        assert_eq!(saved.theme, crate::config::Theme::Light);
    }

    #[test]
    fn no_second_model_turns_the_second_opinion_off() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let choice = Choice {
            provider: Provider::Claude,
            api: ApiConfig::default(),
            fast: "haiku".into(),
            second: None,
        };
        save(&path, &Config::default(), &choice).unwrap();
        let saved = Config::load(&path).unwrap();
        assert_eq!(saved.provider, Some(Provider::Claude));
        assert!(!saved.second_opinion);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("[api]"));
    }

    #[test]
    fn the_account_gh_is_logged_in_with() {
        let now = "github.com\n  ✓ Logged in to github.com account octo (keyring)\n  - Active account: true\n";
        assert_eq!(gh_account(now), Some(("github.com".into(), "octo".into())));
        let older = "ghe.corp\n  ✓ Logged in to ghe.corp as octo (oauth_token)\n";
        assert_eq!(gh_account(older), Some(("ghe.corp".into(), "octo".into())));
        let failed = "github.com\n  X Failed to log in to github.com account octo (keyring)\n";
        assert_eq!(gh_account(failed), None);
    }

    /// A `gh` that prints `out` and exits with `code`.
    fn fake_gh(dir: &Path, out: &str, code: i32) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("gh");
        std::fs::write(&p, format!("#!/bin/sh\nprintf '{out}'\nexit {code}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[tokio::test]
    async fn gh_installed_logged_in_or_not() {
        let dir = tempfile::tempdir().unwrap();
        let ok = fake_gh(
            dir.path(),
            "github.com\\n  ✓ Logged in to github.com account octo (keyring)\\n",
            0,
        );
        let t = gh_tool(ok.as_os_str()).await;
        assert_eq!(t.found, Found::Ready("logged in as octo".into()));

        let out = fake_gh(
            dir.path(),
            "You are not logged into any GitHub hosts. To log in, run: gh auth login\\n",
            1,
        );
        let t = gh_tool(out.as_os_str()).await;
        assert_eq!(t.found, Found::Broken("not logged in".into()));
        assert_eq!(t.about, "run `gh auth login`");

        let none = gh_tool(dir.path().join("no-gh").as_os_str()).await;
        assert_eq!(none.found, Found::Missing("not installed".into()));
        assert!(!none.needed);
    }

    #[test]
    fn the_preset_of_a_config() {
        let mut cfg = Config::default();
        assert_eq!(PRESETS[Preset::of(&cfg)].name, "Claude Code");
        cfg.provider = Some(Provider::Api);
        cfg.api.format = ApiFormat::Openai;
        cfg.api.base_url = "https://openrouter.ai/api/v1".into();
        assert_eq!(PRESETS[Preset::of(&cfg)].name, "OpenRouter");
        cfg.api.base_url = "http://gpu-box:8000/v1".into();
        assert_eq!(PRESETS[Preset::of(&cfg)].name, "Other API");
    }
}
