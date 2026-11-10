//! The user's security settings: `security.toml` next to teer's config
//! (`$TEER_CONFIG`'s folder, else `~/.config/teer`). Never read from the
//! repository under review: a pull request could otherwise switch off its
//! own rules.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

/// `security.toml`. Missing means the defaults.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// The lens on or off: facts, the attack-surface card, the Security tile.
    pub enabled: bool,
    /// What the agent leaves out by default that it should check too:
    /// `dos`, `rate_limit`, `resource`, `validation`, `open_redirect`.
    pub include: Vec<String>,
    /// The project's own names, per language (`php`, `typescript`, `go`,
    /// `javascript`, `python`, `rust`): see [`Extra`].
    #[serde(flatten)]
    pub langs: BTreeMap<String, Extra>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        SecurityConfig {
            enabled: true,
            include: Vec::new(),
            langs: BTreeMap::new(),
        }
    }
}

/// A language's additions to the built-in catalog: names of calls,
/// attributes or decorators, matched like the built-in ones.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Extra {
    /// Authorization checks: `isGranted`, `MyVoter::vote`.
    pub checks: Vec<String>,
    /// What makes a function an entry point: `Route`, `AsMessageHandler`.
    pub entry: Vec<String>,
    /// Sinks by class id (`injection`, `exposure`, `llm`…).
    pub sinks: BTreeMap<String, Vec<String>>,
}

/// Where `security.toml` is read from.
pub fn path() -> PathBuf {
    let config = crate::review::store::config_path();
    config
        .parent()
        .map(|d| d.join("security.toml"))
        .unwrap_or_else(|| PathBuf::from("security.toml"))
}

impl SecurityConfig {
    /// Read [`path`]; missing or unreadable means the defaults (a broken
    /// file is logged, not fatal).
    pub fn load() -> SecurityConfig {
        let p = path();
        match std::fs::read_to_string(&p) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                crate::log::write(&format!("{}: {e}; using the defaults", p.display()));
                SecurityConfig::default()
            }),
            Err(_) => SecurityConfig::default(),
        }
    }

    /// The additions for language `config_name` (see `Lang::config_name`).
    pub fn extra(&self, config_name: &str) -> Extra {
        self.langs.get(config_name).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_include_list_and_each_language() {
        let c: SecurityConfig = toml::from_str(
            r#"
include = ["dos"]
[php]
checks = ["isGranted", "MyVoter::vote"]
sinks.injection = ["Db::raw"]
"#,
        )
        .unwrap();
        assert!(c.enabled);
        assert_eq!(c.include, ["dos"]);
        let php = c.extra("php");
        assert_eq!(php.checks, ["isGranted", "MyVoter::vote"]);
        assert_eq!(php.sinks["injection"], ["Db::raw"]);
        assert_eq!(c.extra("go"), Extra::default());
    }
}
