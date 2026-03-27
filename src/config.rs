use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Top-level project configuration loaded from bugatti.config.toml.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub provider: ProviderConfig,
    #[serde(default)]
    pub commands: BTreeMap<String, CommandDef>,
}

/// Provider-level settings.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    #[serde(default = "default_provider_name")]
    pub name: String,
    #[serde(default)]
    pub extra_system_prompt: Option<String>,
    #[serde(default)]
    pub agent_args: Vec<String>,
}

fn default_provider_name() -> String {
    "claude-code".to_string()
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: default_provider_name(),
            extra_system_prompt: None,
            agent_args: Vec::new(),
        }
    }
}

/// A harness command definition.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CommandDef {
    pub kind: CommandKind,
    pub cmd: String,
    #[serde(default)]
    pub readiness_url: Option<String>,
}

/// Whether a command is short-lived (run to completion) or long-lived (background process).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    ShortLived,
    LongLived,
}

/// Error type for config loading.
#[derive(Debug)]
pub enum ConfigError {
    /// Failed to read the config file.
    ReadError(std::io::Error),
    /// Failed to parse the TOML content.
    ParseError(toml::de::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::ReadError(e) => write!(f, "failed to read bugatti.config.toml: {e}"),
            ConfigError::ParseError(e) => write!(f, "invalid bugatti.config.toml: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Load configuration from `bugatti.config.toml` in the given directory.
///
/// Returns `Ok(Config::default())` if the file does not exist.
/// Returns `Err` if the file exists but cannot be read or parsed.
pub fn load_config(dir: &Path) -> Result<Config, ConfigError> {
    let path = dir.join("bugatti.config.toml");
    match std::fs::read_to_string(&path) {
        Ok(contents) => toml::from_str(&contents).map_err(ConfigError::ParseError),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(ConfigError::ReadError(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parse_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("bugatti.config.toml"),
            r#"
[provider]
name = "openai"
extra_system_prompt = "Be concise"
agent_args = ["--model", "gpt-4"]

[commands.migrate]
kind = "short_lived"
cmd = "cargo sqlx migrate run"

[commands.server]
kind = "long_lived"
cmd = "cargo run --bin server"
readiness_url = "http://localhost:3000/health"
"#,
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.provider.name, "openai");
        assert_eq!(
            config.provider.extra_system_prompt,
            Some("Be concise".to_string())
        );
        assert_eq!(config.provider.agent_args, vec!["--model", "gpt-4"]);

        let migrate = &config.commands["migrate"];
        assert_eq!(migrate.kind, CommandKind::ShortLived);
        assert_eq!(migrate.cmd, "cargo sqlx migrate run");
        assert!(migrate.readiness_url.is_none());

        let server = &config.commands["server"];
        assert_eq!(server.kind, CommandKind::LongLived);
        assert_eq!(server.cmd, "cargo run --bin server");
        assert_eq!(
            server.readiness_url,
            Some("http://localhost:3000/health".to_string())
        );
    }

    #[test]
    fn missing_config_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_config(dir.path()).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.provider.name, "claude-code");
        assert!(config.commands.is_empty());
    }

    #[test]
    fn invalid_toml_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("bugatti.config.toml"),
            "this is not valid toml [[[",
        )
        .unwrap();

        let result = load_config(dir.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("invalid bugatti.config.toml"));
    }

    #[test]
    fn unknown_fields_produce_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("bugatti.config.toml"),
            r#"
[provider]
name = "claude-code"
unknown_field = true
"#,
        )
        .unwrap();

        let result = load_config(dir.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("invalid bugatti.config.toml"));
    }
}
