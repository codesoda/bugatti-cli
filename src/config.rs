use crate::test_file::ProviderOverrides;
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
    #[serde(default)]
    pub checkpoint: Option<CheckpointConfig>,
}

/// Checkpoint save/restore command configuration.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CheckpointConfig {
    /// Command to save a checkpoint (receives BUGATTI_CHECKPOINT_ID and BUGATTI_CHECKPOINT_PATH).
    pub save: String,
    /// Command to restore a checkpoint (receives BUGATTI_CHECKPOINT_ID and BUGATTI_CHECKPOINT_PATH).
    pub restore: String,
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
    #[serde(default)]
    pub step_timeout_secs: Option<u64>,
    #[serde(default)]
    pub strict_warnings: Option<bool>,
    #[serde(default)]
    pub base_url: Option<String>,
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
            step_timeout_secs: None,
            strict_warnings: None,
            base_url: None,
        }
    }
}

/// A harness command definition.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CommandDef {
    pub kind: CommandKind,
    pub cmd: String,
    /// Single readiness URL (convenience shorthand — mutually exclusive with `readiness_urls`).
    #[serde(default)]
    pub readiness_url: Option<String>,
    /// Multiple readiness URLs to poll before the command is considered ready.
    #[serde(default)]
    pub readiness_urls: Vec<String>,
    /// Timeout in seconds for readiness polling (default: 30).
    #[serde(default)]
    pub readiness_timeout_secs: Option<u64>,
}

impl CommandDef {
    /// Return the effective list of readiness URLs, merging `readiness_url` and `readiness_urls`.
    pub fn effective_readiness_urls(&self) -> Vec<&str> {
        let mut urls: Vec<&str> = self.readiness_urls.iter().map(|s| s.as_str()).collect();
        if let Some(ref url) = self.readiness_url {
            if !urls.iter().any(|u| *u == url.as_str()) {
                urls.insert(0, url.as_str());
            }
        }
        urls
    }
}

/// Whether a command is short-lived (run to completion) or long-lived (background process).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    ShortLived,
    LongLived,
}

impl ProviderConfig {
    /// Merge per-test provider overrides over this config.
    /// Override fields that are `Some` replace the global values;
    /// `None` fields preserve the global values.
    pub fn merge_overrides(&self, overrides: &ProviderOverrides) -> ProviderConfig {
        ProviderConfig {
            name: overrides.name.clone().unwrap_or_else(|| self.name.clone()),
            extra_system_prompt: overrides
                .extra_system_prompt
                .clone()
                .or_else(|| self.extra_system_prompt.clone()),
            agent_args: overrides
                .agent_args
                .clone()
                .unwrap_or_else(|| self.agent_args.clone()),
            step_timeout_secs: overrides
                .step_timeout_secs
                .or(self.step_timeout_secs),
            strict_warnings: self.strict_warnings,
            base_url: overrides
                .base_url
                .clone()
                .or_else(|| self.base_url.clone()),
        }
    }
}

/// Compute the effective config by merging test file overrides over the global config.
/// The resulting config has the same commands but provider settings may be overridden.
pub fn effective_config(global: &Config, test_file: &crate::test_file::TestFile) -> Config {
    let provider = match test_file
        .overrides
        .as_ref()
        .and_then(|o| o.provider.as_ref())
    {
        Some(overrides) => global.provider.merge_overrides(overrides),
        None => global.provider.clone(),
    };
    Config {
        provider,
        commands: global.commands.clone(),
        checkpoint: global.checkpoint.clone(),
    }
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
    tracing::info!(path = %path.display(), "loading config");
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let config: Config = toml::from_str(&contents).map_err(|e| {
                tracing::error!(path = %path.display(), error = %e, "config parse failed");
                ConfigError::ParseError(e)
            })?;
            tracing::info!(
                provider = %config.provider.name,
                commands = config.commands.len(),
                "config loaded"
            );
            Ok(config)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("no config file found, using defaults");
            Ok(Config::default())
        }
        Err(e) => {
            tracing::error!(path = %path.display(), error = %e, "config read failed");
            Err(ConfigError::ReadError(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_file::{ProviderOverrides, Step, TestFile, TestOverrides};
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
    fn merge_full_overrides() {
        let global = Config {
            provider: ProviderConfig {
                name: "claude-code".to_string(),
                extra_system_prompt: Some("Global prompt".to_string()),
                agent_args: vec!["--verbose".to_string()],
                step_timeout_secs: None,
                strict_warnings: None,
                base_url: None,
            },
            commands: BTreeMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides {
                provider: Some(ProviderOverrides {
                    name: Some("openai".to_string()),
                    extra_system_prompt: Some("Override prompt".to_string()),
                    agent_args: Some(vec!["--model".to_string(), "gpt-4".to_string()]),
                    step_timeout_secs: None,
                    base_url: None,
                }),
            }),
            steps: vec![],
        };

        let effective = effective_config(&global, &test_file);
        assert_eq!(effective.provider.name, "openai");
        assert_eq!(
            effective.provider.extra_system_prompt,
            Some("Override prompt".to_string())
        );
        assert_eq!(
            effective.provider.agent_args,
            vec!["--model".to_string(), "gpt-4".to_string()]
        );
        // Commands are preserved from global config
        assert_eq!(effective.commands, global.commands);
    }

    #[test]
    fn merge_partial_overrides() {
        let global = Config {
            provider: ProviderConfig {
                name: "claude-code".to_string(),
                extra_system_prompt: Some("Global prompt".to_string()),
                agent_args: vec!["--verbose".to_string()],
                step_timeout_secs: None,
                strict_warnings: None,
                base_url: None,
            },
            commands: BTreeMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides {
                provider: Some(ProviderOverrides {
                    name: Some("openai".to_string()),
                    extra_system_prompt: None,
                    agent_args: None,
                    step_timeout_secs: None,
                    base_url: None,
                }),
            }),
            steps: vec![],
        };

        let effective = effective_config(&global, &test_file);
        assert_eq!(effective.provider.name, "openai");
        // Unset override fields preserve global values
        assert_eq!(
            effective.provider.extra_system_prompt,
            Some("Global prompt".to_string())
        );
        assert_eq!(effective.provider.agent_args, vec!["--verbose".to_string()]);
    }

    #[test]
    fn merge_no_overrides() {
        let global = Config {
            provider: ProviderConfig {
                name: "claude-code".to_string(),
                extra_system_prompt: Some("Global prompt".to_string()),
                agent_args: vec!["--verbose".to_string()],
                step_timeout_secs: None,
                strict_warnings: None,
                base_url: None,
            },
            commands: BTreeMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: None,
            steps: vec![Step {
                instruction: Some("Do something".to_string()),
                include_path: None,
                include_glob: None,
                step_timeout_secs: None,
                skip: false,
                checkpoint: None,
            }],
        };

        let effective = effective_config(&global, &test_file);
        assert_eq!(effective.provider, global.provider);
    }

    #[test]
    fn merge_empty_overrides_section() {
        let global = Config::default();
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides { provider: None }),
            steps: vec![],
        };

        let effective = effective_config(&global, &test_file);
        assert_eq!(effective.provider, global.provider);
    }

    #[test]
    fn parse_config_with_step_timeout() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("bugatti.config.toml"),
            r#"
[provider]
step_timeout_secs = 600
"#,
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.provider.step_timeout_secs, Some(600));
    }

    #[test]
    fn merge_timeout_override() {
        let global = Config {
            provider: ProviderConfig {
                step_timeout_secs: Some(300),
                ..ProviderConfig::default()
            },
            commands: BTreeMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides {
                provider: Some(ProviderOverrides {
                    step_timeout_secs: Some(120),
                    ..ProviderOverrides::default()
                }),
            }),
            steps: vec![],
        };

        let effective = effective_config(&global, &test_file);
        assert_eq!(effective.provider.step_timeout_secs, Some(120));
    }

    #[test]
    fn merge_timeout_none_preserves_global() {
        let global = Config {
            provider: ProviderConfig {
                step_timeout_secs: Some(300),
                ..ProviderConfig::default()
            },
            commands: BTreeMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides {
                provider: Some(ProviderOverrides {
                    step_timeout_secs: None,
                    ..ProviderOverrides::default()
                }),
            }),
            steps: vec![],
        };

        let effective = effective_config(&global, &test_file);
        assert_eq!(effective.provider.step_timeout_secs, Some(300));
    }

    #[test]
    fn parse_config_with_strict_warnings() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("bugatti.config.toml"),
            r#"
[provider]
strict_warnings = true
"#,
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.provider.strict_warnings, Some(true));
    }

    #[test]
    fn merge_preserves_strict_warnings_from_global() {
        let global = Config {
            provider: ProviderConfig {
                strict_warnings: Some(true),
                ..ProviderConfig::default()
            },
            commands: BTreeMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides {
                provider: Some(ProviderOverrides {
                    name: Some("openai".to_string()),
                    ..ProviderOverrides::default()
                }),
            }),
            steps: vec![],
        };

        let effective = effective_config(&global, &test_file);
        assert_eq!(effective.provider.strict_warnings, Some(true));
        assert_eq!(effective.provider.name, "openai");
    }

    #[test]
    fn parse_config_with_base_url() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("bugatti.config.toml"),
            r#"
[provider]
base_url = "http://localhost:3000"
"#,
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        assert_eq!(
            config.provider.base_url,
            Some("http://localhost:3000".to_string())
        );
    }

    #[test]
    fn merge_base_url_override() {
        let global = Config {
            provider: ProviderConfig {
                base_url: Some("http://localhost:3000".to_string()),
                ..ProviderConfig::default()
            },
            commands: BTreeMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides {
                provider: Some(ProviderOverrides {
                    base_url: Some("http://localhost:5000".to_string()),
                    ..ProviderOverrides::default()
                }),
            }),
            steps: vec![],
        };

        let effective = effective_config(&global, &test_file);
        assert_eq!(
            effective.provider.base_url,
            Some("http://localhost:5000".to_string())
        );
    }

    #[test]
    fn merge_base_url_none_preserves_global() {
        let global = Config {
            provider: ProviderConfig {
                base_url: Some("http://localhost:3000".to_string()),
                ..ProviderConfig::default()
            },
            commands: BTreeMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides {
                provider: Some(ProviderOverrides {
                    name: Some("openai".to_string()),
                    ..ProviderOverrides::default()
                }),
            }),
            steps: vec![],
        };

        let effective = effective_config(&global, &test_file);
        assert_eq!(
            effective.provider.base_url,
            Some("http://localhost:3000".to_string())
        );
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
