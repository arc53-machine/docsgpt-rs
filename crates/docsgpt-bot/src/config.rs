//! Configuration building blocks shared by the bots.
//!
//! Each bot defines its own `#[serde(deny_unknown_fields)]` config struct and
//! embeds these parts as fields (not `#[serde(flatten)]`, which disables
//! `deny_unknown_fields`). [`load_toml`] reads a file with `${VAR}` references.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

/// The DocsGPT cloud, used when no `api_base` is configured.
pub const DEFAULT_API_BASE: &str = "https://gptcloud.arc53.com";

/// One DocsGPT agent a bot can talk to.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// Short name users type (`#sales`); lowercase letters, digits, `-` and `_`.
    pub name: String,
    /// The agent's DocsGPT API key.
    pub api_key: String,
    /// One line shown in agent pickers.
    #[serde(default)]
    pub description: Option<String>,
    /// Use this agent when the user hasn't picked one. At most one per bot.
    #[serde(default)]
    pub default: bool,
}

impl AgentConfig {
    /// An agent with no description that is not the default.
    pub fn new(name: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            api_key: api_key.into(),
            description: None,
            default: false,
        }
    }
}

/// Where conversation state is kept.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// A single SQLite file (the default).
    #[default]
    Sqlite,
    /// Process memory; lost on restart.
    Memory,
}

/// The `[storage]` table.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    /// Backend; SQLite by default.
    #[serde(default)]
    pub backend: Backend,
    /// SQLite file; see [`StorageConfig::sqlite_path`].
    #[serde(default)]
    pub path: Option<String>,
}

impl StorageConfig {
    /// The configured SQLite path, or the bot's default.
    pub fn sqlite_path<'a>(&'a self, default: &'a str) -> &'a str {
        self.path.as_deref().filter(|p| !p.trim().is_empty()).unwrap_or(default)
    }
}

/// The `[server]` table, for bots that receive webhooks or serve `/healthz`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Address to listen on. Default `0.0.0.0:8080`.
    #[serde(default = "default_bind")]
    pub bind: String,
    /// Public HTTPS URL that reaches `bind`, for platforms that push events.
    #[serde(default)]
    pub public_url: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            public_url: None,
        }
    }
}

fn default_bind() -> String {
    "0.0.0.0:8080".into()
}

static ENV_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}").expect("valid regex"));

/// Replace `${VAR}` and `${VAR:-default}` with values from the environment.
///
/// Fails listing every variable that is unset and has no default.
pub fn expand_env(input: &str) -> Result<String> {
    expand_with(input, |k| std::env::var(k).ok())
}

fn expand_with(input: &str, lookup: impl Fn(&str) -> Option<String>) -> Result<String> {
    let mut missing = Vec::new();
    let out = ENV_REF.replace_all(input, |caps: &regex::Captures| {
        let var = &caps[1];
        match (lookup(var), caps.get(2)) {
            (Some(v), _) => v,
            (None, Some(d)) => d.as_str().to_string(),
            (None, None) => {
                if !missing.contains(&var.to_string()) {
                    missing.push(var.to_string());
                }
                String::new()
            }
        }
    });
    if !missing.is_empty() {
        return Err(Error::config(format!(
            "missing environment variables referenced in config: {}",
            missing.join(", ")
        )));
    }
    Ok(out.into_owned())
}

/// Read a TOML file, expand `${VAR}` references, and deserialize it.
pub fn load_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = std::fs::read_to_string(path).map_err(|e| Error::config(format!("reading {}: {e}", path.display())))?;
    parse_toml(&raw).map_err(|e| Error::config(format!("{}: {e}", path.display())))
}

/// Expand `${VAR}` references in TOML text and deserialize it.
pub fn parse_toml<T: DeserializeOwned>(raw: &str) -> Result<T> {
    let expanded = expand_env(raw)?;
    toml::from_str(&expanded).map_err(|e| Error::config(e.to_string()))
}

/// Pick the config file: `explicit`, else the path in `env_var`, else
/// `default_file` if it exists in the working directory. `None` means the bot
/// should fall back to its environment-variable layout.
pub fn find_config(explicit: Option<&Path>, env_var: &str, default_file: &str) -> Option<PathBuf> {
    explicit
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os(env_var).filter(|v| !v.is_empty()).map(PathBuf::from))
        .or_else(|| {
            let p = Path::new(default_file);
            p.exists().then(|| p.to_path_buf())
        })
}

/// Lowercase and check a bot or agent name: letters, digits, `-` and `_`.
pub fn normalize_name(kind: &str, name: &str) -> Result<String> {
    let n = name.trim().to_ascii_lowercase();
    if n.is_empty() || !n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(Error::config(format!(
            "{kind} name {name:?} must be letters, digits, '-' or '_'"
        )));
    }
    Ok(n)
}

/// Agents from the single-bot environment layout: `API_KEY` becomes the
/// `default` agent and each `API_KEY_<NAME>` an agent named `<name>`.
///
/// `vars` is usually `std::env::vars()`; entries with empty values are skipped.
/// Sorted by name after the default, so the order is stable.
pub fn agents_from_env(vars: impl IntoIterator<Item = (String, String)>) -> Vec<AgentConfig> {
    let mut default = None;
    let mut extra = Vec::new();
    for (k, v) in vars {
        if v.trim().is_empty() {
            continue;
        }
        if k == "API_KEY" {
            default = Some(AgentConfig {
                default: true,
                ..AgentConfig::new("default", v)
            });
        } else if let Some(name) = k.strip_prefix("API_KEY_")
            && !name.is_empty()
        {
            extra.push(AgentConfig::new(name.to_ascii_lowercase(), v));
        }
    }
    extra.sort_by(|a, b| a.name.cmp(&b.name));
    default.into_iter().chain(extra).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(k: &str) -> Option<String> {
        (k == "SET").then(|| "abc".to_string())
    }

    #[test]
    fn expands_env_with_defaults() {
        assert_eq!(expand_with("x ${SET} y", lookup).unwrap(), "x abc y");
        assert_eq!(expand_with("${UNSET:-fallback}", lookup).unwrap(), "fallback");
        assert_eq!(expand_with("${UNSET:-}", lookup).unwrap(), "");
        assert_eq!(expand_with("$SET {SET} $${SET}", lookup).unwrap(), "$SET {SET} $abc");
    }

    #[test]
    fn reports_every_missing_variable_once() {
        let err = expand_with("${A} ${B} ${A}", lookup).unwrap_err().to_string();
        assert!(err.ends_with("A, B"), "{err}");
    }

    #[test]
    fn parses_parts_inside_a_bot_config() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Bot {
            #[serde(default)]
            storage: StorageConfig,
            #[serde(default)]
            server: ServerConfig,
            agents: Vec<AgentConfig>,
        }
        let bot: Bot = parse_toml(
            r#"
            [storage]
            backend = "memory"
            [[agents]]
            name = "Sales"
            api_key = "k"
            description = "Pricing"
            "#,
        )
        .unwrap();
        assert_eq!(bot.storage.backend, Backend::Memory);
        assert_eq!(bot.storage.sqlite_path("data/x.db"), "data/x.db");
        assert_eq!(bot.server.bind, "0.0.0.0:8080");
        assert_eq!(bot.agents[0].description.as_deref(), Some("Pricing"));

        let err = parse_toml::<Bot>("agents = []\n[storage]\nbackend = \"mongodb\"").unwrap_err();
        assert!(err.to_string().contains("mongodb"), "{err}");
        assert!(parse_toml::<Bot>("agents = []\nsurprise = 1").is_err());
    }

    #[test]
    fn normalizes_names() {
        assert_eq!(normalize_name("agent", " Sales_2 ").unwrap(), "sales_2");
        assert!(normalize_name("agent", "two words").is_err());
        assert!(normalize_name("agent", "").is_err());
    }

    #[test]
    fn agents_from_env_layout() {
        let vars = [
            ("API_KEY_SUPPORT", "k2"),
            ("API_KEY", "k1"),
            ("API_KEY_ALPHA", "k3"),
            ("API_KEY_EMPTY", " "),
            ("OTHER", "x"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()));
        let agents = agents_from_env(vars);
        let names: Vec<_> = agents.iter().map(|a| (a.name.as_str(), a.default)).collect();
        assert_eq!(names, vec![("default", true), ("alpha", false), ("support", false)]);
    }
}
