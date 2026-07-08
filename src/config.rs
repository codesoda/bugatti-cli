use crate::test_file::{CommandOverrides, ProviderOverrides};
use indexmap::IndexMap;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Top-level project configuration loaded from bugatti.config.toml.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub provider: ProviderConfig,
    #[serde(default)]
    pub commands: IndexMap<String, CommandDef>,
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
    /// Timeout in seconds for save/restore commands (default: 120).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
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
            if !urls.contains(&url.as_str()) {
                urls.insert(0, url.as_str());
            }
        }
        urls
    }

    /// Merge per-test command overrides over this command definition.
    ///
    /// Command `kind` is deliberately not overridable.
    ///
    /// Note: optional fields use "set wins" semantics, so an override cannot
    /// *unset* a base value — e.g. a base `readiness_url` still contributes to
    /// [`CommandDef::effective_readiness_urls`] even when the override supplies
    /// `readiness_urls`. This matches the per-test override spec (issue #41).
    pub fn merge_overrides(&self, overrides: &CommandOverrides) -> CommandDef {
        CommandDef {
            kind: self.kind.clone(),
            cmd: overrides.cmd.clone().unwrap_or_else(|| self.cmd.clone()),
            readiness_url: overrides
                .readiness_url
                .clone()
                .or_else(|| self.readiness_url.clone()),
            readiness_urls: overrides
                .readiness_urls
                .clone()
                .unwrap_or_else(|| self.readiness_urls.clone()),
            readiness_timeout_secs: overrides
                .readiness_timeout_secs
                .or(self.readiness_timeout_secs),
        }
    }
}

/// Whether a command is short-lived (run to completion) or long-lived (background process).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    ShortLived,
    LongLived,
}

/// A configuration layer as written on disk, with field *presence* preserved.
///
/// Unlike [`Config`], no defaults are applied at parse time, so the layered
/// merge can distinguish "the user explicitly wrote `name = \"claude-code\"`"
/// (which must override a global layer) from "the field was omitted" (which
/// must fall through to the lower layer).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    provider: RawProviderConfig,
    #[serde(default)]
    commands: IndexMap<String, CommandDef>,
    #[serde(default)]
    checkpoint: Option<CheckpointConfig>,
}

/// Provider settings with field presence preserved (see [`RawConfig`]).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RawProviderConfig {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    extra_system_prompt: Option<String>,
    #[serde(default)]
    agent_args: Option<Vec<String>>,
    #[serde(default)]
    step_timeout_secs: Option<u64>,
    #[serde(default)]
    strict_warnings: Option<bool>,
    #[serde(default)]
    base_url: Option<String>,
}

impl RawConfig {
    /// Merge a higher-priority layer over this lower-priority layer.
    ///
    /// Any field the higher layer explicitly sets wins — including explicit
    /// values that happen to equal the defaults (e.g. `name = "claude-code"`
    /// or `agent_args = []`).
    fn merge_layer(self, higher: RawConfig) -> RawConfig {
        let mut commands = self.commands;
        commands.extend(higher.commands);

        RawConfig {
            provider: RawProviderConfig {
                name: higher.provider.name.or(self.provider.name),
                extra_system_prompt: higher
                    .provider
                    .extra_system_prompt
                    .or(self.provider.extra_system_prompt),
                agent_args: higher.provider.agent_args.or(self.provider.agent_args),
                step_timeout_secs: higher
                    .provider
                    .step_timeout_secs
                    .or(self.provider.step_timeout_secs),
                strict_warnings: higher
                    .provider
                    .strict_warnings
                    .or(self.provider.strict_warnings),
                base_url: higher.provider.base_url.or(self.provider.base_url),
            },
            commands,
            checkpoint: higher.checkpoint.or(self.checkpoint),
        }
    }

    /// Apply defaults to produce the final [`Config`].
    fn into_config(self) -> Config {
        Config {
            provider: ProviderConfig {
                name: self.provider.name.unwrap_or_else(default_provider_name),
                extra_system_prompt: self.provider.extra_system_prompt,
                agent_args: self.provider.agent_args.unwrap_or_default(),
                step_timeout_secs: self.provider.step_timeout_secs,
                strict_warnings: self.provider.strict_warnings,
                base_url: self.provider.base_url,
            },
            commands: self.commands,
            checkpoint: self.checkpoint,
        }
    }
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
            step_timeout_secs: overrides.step_timeout_secs.or(self.step_timeout_secs),
            strict_warnings: self.strict_warnings,
            base_url: overrides.base_url.clone().or_else(|| self.base_url.clone()),
        }
    }
}

/// Compute the effective config by merging test file overrides over the global config.
/// The resulting config preserves global commands unless a test supplies command overrides.
pub fn effective_config(global: &Config, test_file: &crate::test_file::TestFile) -> Config {
    let provider = match test_file
        .overrides
        .as_ref()
        .and_then(|o| o.provider.as_ref())
    {
        Some(overrides) => global.provider.merge_overrides(overrides),
        None => global.provider.clone(),
    };

    let mut commands = global.commands.clone();
    if let Some(command_overrides) = test_file
        .overrides
        .as_ref()
        .and_then(|o| o.commands.as_ref())
    {
        for (name, overrides) in command_overrides {
            if let Some(global_def) = global.commands.get(name) {
                commands.insert(name.clone(), global_def.merge_overrides(overrides));
            } else {
                tracing::warn!(command = %name, "unknown command override ignored");
                eprintln!("WARNING: unknown command override '{name}' ignored");
            }
        }
    }

    Config {
        provider,
        commands,
        checkpoint: global.checkpoint.clone(),
    }
}

/// Error type for config loading.
#[derive(Debug)]
pub enum ConfigError {
    /// Failed to read the config file.
    ReadError {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Failed to parse the TOML content.
    ParseError(toml::de::Error),
    /// An explicit --config path was provided but the file does not exist.
    ExplicitPathNotFound(PathBuf),
    /// An environment variable override could not be parsed.
    InvalidEnvVar { var: String, value: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::ReadError { path, source } => write!(
                f,
                "failed to read config file {}: {source}. Check that the file exists and is readable.",
                path.display()
            ),
            ConfigError::ParseError(e) => write!(
                f,
                "invalid bugatti.config.toml: {e}. See https://bugatti.dev/llms/cli-reference.txt for config format."
            ),
            ConfigError::ExplicitPathNotFound(p) => {
                write!(f, "config file not found: {}", p.display())
            }
            ConfigError::InvalidEnvVar { var, value } => {
                write!(f, "invalid environment variable {var}={value:?}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Parse TOML contents into a raw layer, emitting trace logs for success/failure.
fn parse_raw_config(path: &Path, contents: &str) -> Result<RawConfig, ConfigError> {
    let raw: RawConfig = toml::from_str(contents).map_err(|e| {
        tracing::error!(path = %path.display(), error = %e, "config parse failed");
        ConfigError::ParseError(e)
    })?;
    tracing::info!(
        path = %path.display(),
        provider = raw.provider.name.as_deref().unwrap_or("claude-code"),
        commands = raw.commands.len(),
        "config loaded"
    );
    Ok(raw)
}

/// Load a raw config layer from an explicit file path (missing file is an error).
fn load_raw_from_file(path: &Path) -> Result<RawConfig, ConfigError> {
    tracing::info!(path = %path.display(), "loading config from explicit path");
    match std::fs::read_to_string(path) {
        Ok(contents) => parse_raw_config(path, &contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::error!(path = %path.display(), "explicit config path not found");
            Err(ConfigError::ExplicitPathNotFound(path.to_path_buf()))
        }
        Err(e) => {
            tracing::error!(path = %path.display(), error = %e, "config read failed");
            Err(ConfigError::ReadError {
                path: path.to_path_buf(),
                source: e,
            })
        }
    }
}

/// Load configuration from an explicit file path.
///
/// Unlike [`load_config`], a missing file is an error — callers who pass
/// `--config` want to fail loudly if the path is wrong rather than silently
/// fall back to defaults.
pub fn load_config_from_file(path: &Path) -> Result<Config, ConfigError> {
    load_raw_from_file(path).map(RawConfig::into_config)
}

/// Return the default global config path (`$HOME/.bugatti/config.toml`).
///
/// If `BUGATTI_CONFIG_HOME` is set, it is treated as the config directory and
/// `config.toml` is read beneath it. This is primarily useful for tests and
/// sandboxed environments.
pub fn global_config_path() -> Option<PathBuf> {
    if let Ok(config_home) = std::env::var("BUGATTI_CONFIG_HOME") {
        return Some(PathBuf::from(config_home).join("config.toml"));
    }

    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".bugatti/config.toml"))
}

/// Load a raw global config layer (missing file is an empty layer).
fn load_raw_global(path: &Path) -> Result<RawConfig, ConfigError> {
    tracing::info!(path = %path.display(), "loading global config");
    match std::fs::read_to_string(path) {
        Ok(contents) => parse_raw_config(path, &contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(path = %path.display(), "no global config file found, using defaults");
            Ok(RawConfig::default())
        }
        Err(e) => {
            tracing::error!(path = %path.display(), error = %e, "global config read failed");
            Err(ConfigError::ReadError {
                path: path.to_path_buf(),
                source: e,
            })
        }
    }
}

/// Load global configuration from an optional global config file.
///
/// A missing global config file is silently treated as defaults because the
/// global layer is optional. Existing files that cannot be read or parsed still
/// fail loudly.
pub fn load_global_config(path: &Path) -> Result<Config, ConfigError> {
    load_raw_global(path).map(RawConfig::into_config)
}

/// Apply environment variable overrides using an injected environment lookup.
///
/// This keeps tests deterministic and avoids mutating process-wide environment
/// variables in parallel test runs.
///
/// Empty values are treated as unset (a common convention that lets users
/// neutralise a variable with `BUGATTI_PROVIDER= bugatti test`), and
/// `BUGATTI_STEP_TIMEOUT` must parse to a positive integer.
pub fn apply_env_overrides_from<F>(config: &mut Config, mut get_env: F) -> Result<(), ConfigError>
where
    F: FnMut(&str) -> Option<String>,
{
    let non_empty = |var: &str, get_env: &mut F| -> Option<String> {
        match get_env(var) {
            Some(v) if v.trim().is_empty() => {
                tracing::debug!(var, "ignoring empty environment override");
                None
            }
            other => other,
        }
    };

    if let Some(value) = non_empty("BUGATTI_PROVIDER", &mut get_env) {
        config.provider.name = value;
    }
    if let Some(value) = non_empty("BUGATTI_BASE_URL", &mut get_env) {
        config.provider.base_url = Some(value);
    }
    if let Some(value) = non_empty("BUGATTI_STEP_TIMEOUT", &mut get_env) {
        let parsed = match value.parse::<u64>() {
            Ok(n) if n > 0 => n,
            _ => {
                return Err(ConfigError::InvalidEnvVar {
                    var: "BUGATTI_STEP_TIMEOUT".to_string(),
                    value,
                })
            }
        };
        config.provider.step_timeout_secs = Some(parsed);
    }

    Ok(())
}

/// External sources consulted during layered config loading.
///
/// Production code uses [`ConfigSources::process`] (real global config path
/// and process environment). Tests use [`ConfigSources::hermetic`] so results
/// never depend on the developer's `$HOME` or shell environment.
#[derive(Debug, Clone)]
pub struct ConfigSources {
    /// Path to the global config file, if any.
    pub global_path: Option<PathBuf>,
    /// Environment variable lookup used for `BUGATTI_*` overrides.
    pub env: fn(&str) -> Option<String>,
}

impl ConfigSources {
    /// Sources backed by the real global config location and process env.
    pub fn process() -> Self {
        Self {
            global_path: global_config_path(),
            env: |var| std::env::var(var).ok(),
        }
    }

    /// Sources that consult no global config file and no environment.
    pub fn hermetic() -> Self {
        Self {
            global_path: None,
            env: |_| None,
        }
    }
}

/// Load global and project config layers, then apply environment overrides.
pub fn load_layered_config(
    project_root: &Path,
    explicit: Option<&Path>,
    sources: &ConfigSources,
) -> Result<Config, ConfigError> {
    load_layered_config_with_options(
        project_root,
        explicit,
        sources.global_path.as_deref(),
        sources.env,
    )
}

/// Load layered config with explicit global path and injected environment lookup.
///
/// Layers are applied in ascending precedence: global, project/explicit, env.
/// Field presence is tracked per layer, so a project config that explicitly
/// sets a field to its default value still overrides the global layer.
pub fn load_layered_config_with_options<F>(
    project_root: &Path,
    explicit: Option<&Path>,
    global_path: Option<&Path>,
    get_env: F,
) -> Result<Config, ConfigError>
where
    F: FnMut(&str) -> Option<String>,
{
    let global = match global_path {
        Some(path) => load_raw_global(path)?,
        None => RawConfig::default(),
    };

    let project = match explicit {
        Some(path) => load_raw_from_file(path)?,
        None => load_raw_project(project_root)?,
    };

    let mut layered = global.merge_layer(project).into_config();
    apply_env_overrides_from(&mut layered, get_env)?;
    Ok(layered)
}

/// Load configuration from `bugatti.config.toml` in the given directory.
///
/// Returns `Ok(Config::default())` if the file does not exist, after printing
/// a stderr warning so the fallback is visible in the terminal and run report
/// instead of only in the diagnostics trace.
/// Returns `Err` if the file exists but cannot be read or parsed.
pub fn load_config(dir: &Path) -> Result<Config, ConfigError> {
    load_raw_project(dir).map(RawConfig::into_config)
}

/// Load the raw project config layer from `bugatti.config.toml` in `dir`.
fn load_raw_project(dir: &Path) -> Result<RawConfig, ConfigError> {
    let path = dir.join("bugatti.config.toml");
    tracing::info!(path = %path.display(), "loading config");
    match std::fs::read_to_string(&path) {
        Ok(contents) => parse_raw_config(&path, &contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(path = %path.display(), "no project config file found");
            eprintln!(
                "WARNING: no bugatti.config.toml found in {} — using defaults plus any \
                 global config (~/.bugatti/config.toml) and BUGATTI_* environment overrides.\n\
                 Pass --config <path> to point at a project config file explicitly.",
                dir.display()
            );
            Ok(RawConfig::default())
        }
        Err(e) => {
            tracing::error!(path = %path.display(), error = %e, "config read failed");
            Err(ConfigError::ReadError {
                path: path.clone(),
                source: e,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_file::{CommandOverrides, ProviderOverrides, Step, TestFile, TestOverrides};
    use indexmap::IndexMap;
    use std::collections::BTreeMap;
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
    fn config_preserves_toml_declaration_order() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("bugatti.config.toml"),
            r#"
[commands.z_server]
kind = "long_lived"
cmd = "sleep 60"

[commands.a_migrate]
kind = "short_lived"
cmd = "echo migrate"
"#,
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        let names: Vec<&String> = config.commands.keys().collect();
        assert_eq!(names, vec!["z_server", "a_migrate"]);
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
        assert!(err_msg.contains("https://bugatti.dev/llms/cli-reference.txt"));
    }

    #[test]
    fn read_error_includes_actionable_hint() {
        let err_msg = ConfigError::ReadError {
            path: PathBuf::from("/proj/bugatti.config.toml"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied"),
        }
        .to_string();
        assert!(err_msg.contains("failed to read config file /proj/bugatti.config.toml"));
        assert!(err_msg.contains("Check that the file exists and is readable"));
    }

    #[test]
    fn load_from_explicit_path_reads_arbitrary_filename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.toml");
        fs::write(
            &path,
            r#"
[provider]
name = "openai"

[commands.migrate]
kind = "short_lived"
cmd = "cargo sqlx migrate run"
"#,
        )
        .unwrap();

        let config = load_config_from_file(&path).unwrap();
        assert_eq!(config.provider.name, "openai");
        assert_eq!(config.commands.len(), 1);
    }

    #[test]
    fn explicit_path_missing_is_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        let err = load_config_from_file(&path).unwrap_err();
        match err {
            ConfigError::ExplicitPathNotFound(p) => assert_eq!(p, path),
            other => panic!("expected ExplicitPathNotFound, got {other:?}"),
        }
    }

    #[test]
    fn explicit_path_invalid_toml_returns_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.toml");
        fs::write(&path, "not valid toml [[[").unwrap();

        let err = load_config_from_file(&path).unwrap_err();
        assert!(matches!(err, ConfigError::ParseError(_)));
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
            commands: IndexMap::new(),
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
                commands: None,
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
            commands: IndexMap::new(),
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
                commands: None,
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
            commands: IndexMap::new(),
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
                setup: false,
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

            overrides: Some(TestOverrides {
                provider: None,
                commands: None,
            }),
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
            commands: IndexMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides {
                provider: Some(ProviderOverrides {
                    step_timeout_secs: Some(120),
                    ..ProviderOverrides::default()
                }),
                commands: None,
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
            commands: IndexMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides {
                provider: Some(ProviderOverrides {
                    step_timeout_secs: None,
                    ..ProviderOverrides::default()
                }),
                commands: None,
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
            commands: IndexMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides {
                provider: Some(ProviderOverrides {
                    name: Some("openai".to_string()),
                    ..ProviderOverrides::default()
                }),
                commands: None,
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
            commands: IndexMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides {
                provider: Some(ProviderOverrides {
                    base_url: Some("http://localhost:5000".to_string()),
                    ..ProviderOverrides::default()
                }),
                commands: None,
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
            commands: IndexMap::new(),
            checkpoint: None,
        };
        let test_file = TestFile {
            name: "test".to_string(),

            overrides: Some(TestOverrides {
                provider: Some(ProviderOverrides {
                    name: Some("openai".to_string()),
                    ..ProviderOverrides::default()
                }),
                commands: None,
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
    fn command_override_full_merge_preserves_kind() {
        let mut commands = IndexMap::new();
        commands.insert(
            "server".to_string(),
            CommandDef {
                kind: CommandKind::LongLived,
                cmd: "cargo run --bin server".to_string(),
                readiness_url: Some("http://localhost:3000/ready".to_string()),
                readiness_urls: vec!["http://localhost:3000/health".to_string()],
                readiness_timeout_secs: Some(30),
            },
        );
        let global = Config {
            provider: ProviderConfig::default(),
            commands,
            checkpoint: None,
        };
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "server".to_string(),
            CommandOverrides {
                cmd: Some("cargo run --bin alt-server".to_string()),
                readiness_url: Some("http://localhost:4000/ready".to_string()),
                readiness_urls: Some(vec!["http://localhost:4000/health".to_string()]),
                readiness_timeout_secs: Some(5),
            },
        );
        let test_file = TestFile {
            name: "test".to_string(),
            overrides: Some(TestOverrides {
                provider: None,
                commands: Some(overrides),
            }),
            steps: vec![],
        };

        let effective = effective_config(&global, &test_file);
        let server = effective.commands.get("server").unwrap();
        assert_eq!(server.kind, CommandKind::LongLived);
        assert_eq!(server.cmd, "cargo run --bin alt-server");
        assert_eq!(
            server.readiness_url.as_deref(),
            Some("http://localhost:4000/ready")
        );
        assert_eq!(
            server.readiness_urls,
            vec!["http://localhost:4000/health".to_string()]
        );
        assert_eq!(server.readiness_timeout_secs, Some(5));
    }

    #[test]
    fn command_override_partial_preserves_existing_values() {
        let mut commands = IndexMap::new();
        commands.insert(
            "server".to_string(),
            CommandDef {
                kind: CommandKind::ShortLived,
                cmd: "npm start".to_string(),
                readiness_url: Some("http://localhost:3000/ready".to_string()),
                readiness_urls: vec!["http://localhost:3000/health".to_string()],
                readiness_timeout_secs: Some(30),
            },
        );
        let global = Config {
            provider: ProviderConfig::default(),
            commands,
            checkpoint: None,
        };
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "server".to_string(),
            CommandOverrides {
                readiness_timeout_secs: Some(2),
                ..CommandOverrides::default()
            },
        );
        let test_file = TestFile {
            name: "test".to_string(),
            overrides: Some(TestOverrides {
                provider: None,
                commands: Some(overrides),
            }),
            steps: vec![],
        };

        let effective = effective_config(&global, &test_file);
        let server = effective.commands.get("server").unwrap();
        assert_eq!(server.cmd, "npm start");
        assert_eq!(server.kind, CommandKind::ShortLived);
        assert_eq!(server.readiness_timeout_secs, Some(2));
        assert_eq!(
            server.readiness_url.as_deref(),
            Some("http://localhost:3000/ready")
        );
    }

    #[test]
    fn unknown_command_override_is_ignored() {
        let mut commands = IndexMap::new();
        commands.insert(
            "server".to_string(),
            CommandDef {
                kind: CommandKind::LongLived,
                cmd: "npm start".to_string(),
                readiness_url: None,
                readiness_urls: vec![],
                readiness_timeout_secs: None,
            },
        );
        let global = Config {
            provider: ProviderConfig::default(),
            commands,
            checkpoint: None,
        };
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "missing".to_string(),
            CommandOverrides {
                cmd: Some("echo ignored".to_string()),
                ..CommandOverrides::default()
            },
        );
        let test_file = TestFile {
            name: "test".to_string(),
            overrides: Some(TestOverrides {
                provider: None,
                commands: Some(overrides),
            }),
            steps: vec![],
        };

        let effective = effective_config(&global, &test_file);
        assert_eq!(effective.commands.len(), 1);
        assert_eq!(effective.commands["server"].cmd, "npm start");
        assert!(!effective.commands.contains_key("missing"));
    }

    #[test]
    fn command_override_one_of_two_commands_leaves_other_intact() {
        let mut commands = IndexMap::new();
        commands.insert(
            "server".to_string(),
            CommandDef {
                kind: CommandKind::LongLived,
                cmd: "npm start".to_string(),
                readiness_url: None,
                readiness_urls: vec![],
                readiness_timeout_secs: None,
            },
        );
        commands.insert(
            "worker".to_string(),
            CommandDef {
                kind: CommandKind::ShortLived,
                cmd: "cargo run --bin worker".to_string(),
                readiness_url: None,
                readiness_urls: vec![],
                readiness_timeout_secs: Some(9),
            },
        );
        let global = Config {
            provider: ProviderConfig::default(),
            commands,
            checkpoint: None,
        };
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "server".to_string(),
            CommandOverrides {
                cmd: Some("npm run dev".to_string()),
                ..CommandOverrides::default()
            },
        );
        let test_file = TestFile {
            name: "test".to_string(),
            overrides: Some(TestOverrides {
                provider: None,
                commands: Some(overrides),
            }),
            steps: vec![],
        };

        let effective = effective_config(&global, &test_file);
        assert_eq!(effective.commands["server"].cmd, "npm run dev");
        assert_eq!(effective.commands["worker"].cmd, "cargo run --bin worker");
        assert_eq!(effective.commands["worker"].readiness_timeout_secs, Some(9));
    }

    #[test]
    fn command_override_empty_table_no_op_and_empty_readiness_urls_clears() {
        let command = CommandDef {
            kind: CommandKind::LongLived,
            cmd: "npm start".to_string(),
            readiness_url: None,
            readiness_urls: vec!["http://localhost:3000/health".to_string()],
            readiness_timeout_secs: None,
        };
        assert_eq!(
            command.merge_overrides(&CommandOverrides::default()),
            command
        );

        let cleared = command.merge_overrides(&CommandOverrides {
            readiness_urls: Some(vec![]),
            ..CommandOverrides::default()
        });
        assert!(cleared.readiness_urls.is_empty());
    }

    #[test]
    fn load_global_config_missing_file_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_global_config(&dir.path().join(".bugatti/config.toml")).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn load_global_config_parse_error_is_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "invalid = {{{").unwrap();

        let err = load_global_config(&path).unwrap_err();
        assert!(matches!(err, ConfigError::ParseError(_)));
    }

    #[test]
    fn layered_config_uses_global_when_project_missing() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let global_path = home.path().join("config.toml");
        fs::write(
            &global_path,
            r#"
[provider]
name = "openai"
base_url = "http://global.example"
"#,
        )
        .unwrap();

        let config =
            load_layered_config_with_options(project.path(), None, Some(&global_path), |_| None)
                .unwrap();

        assert_eq!(config.provider.name, "openai");
        assert_eq!(
            config.provider.base_url.as_deref(),
            Some("http://global.example")
        );
    }

    #[test]
    fn project_layer_wins_over_global_provider_fields() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let global_path = home.path().join("config.toml");
        fs::write(
            &global_path,
            r#"
[provider]
name = "global-provider"
base_url = "http://global.example"
step_timeout_secs = 11
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("bugatti.config.toml"),
            r#"
[provider]
name = "project-provider"
base_url = "http://project.example"
"#,
        )
        .unwrap();

        let config =
            load_layered_config_with_options(dir.path(), None, Some(&global_path), |_| None)
                .unwrap();
        assert_eq!(config.provider.name, "project-provider");
        assert_eq!(
            config.provider.base_url.as_deref(),
            Some("http://project.example")
        );
        assert_eq!(config.provider.step_timeout_secs, Some(11));
    }

    #[test]
    fn project_explicit_default_values_override_global() {
        // A project that explicitly writes the default provider name (or an
        // empty agent_args) must override the global layer — field presence,
        // not value comparison, decides the merge.
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let global_path = home.path().join("config.toml");
        fs::write(
            &global_path,
            r#"
[provider]
name = "global-provider"
agent_args = ["--global-flag"]
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("bugatti.config.toml"),
            r#"
[provider]
name = "claude-code"
agent_args = []
"#,
        )
        .unwrap();

        let config =
            load_layered_config_with_options(dir.path(), None, Some(&global_path), |_| None)
                .unwrap();
        assert_eq!(config.provider.name, "claude-code");
        assert!(
            config.provider.agent_args.is_empty(),
            "explicit empty agent_args must clear the global value"
        );
    }

    #[test]
    fn omitted_project_fields_fall_through_to_global() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let global_path = home.path().join("config.toml");
        fs::write(
            &global_path,
            r#"
[provider]
name = "global-provider"
agent_args = ["--global-flag"]
"#,
        )
        .unwrap();
        fs::write(dir.path().join("bugatti.config.toml"), "[provider]\n").unwrap();

        let config =
            load_layered_config_with_options(dir.path(), None, Some(&global_path), |_| None)
                .unwrap();
        assert_eq!(config.provider.name, "global-provider");
        assert_eq!(config.provider.agent_args, vec!["--global-flag"]);
    }

    #[test]
    fn commands_merge_across_layers_with_project_winning() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let global_path = home.path().join("config.toml");
        fs::write(
            &global_path,
            r#"
[commands.server]
kind = "long_lived"
cmd = "npm start"

[commands.global-only]
kind = "short_lived"
cmd = "echo global"

[checkpoint]
save = "save-checkpoint"
restore = "restore-checkpoint"
timeout_secs = 60
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("bugatti.config.toml"),
            r#"
[commands.server]
kind = "short_lived"
cmd = "npm run project"

[commands.project-only]
kind = "long_lived"
cmd = "echo project"
"#,
        )
        .unwrap();

        let config =
            load_layered_config_with_options(dir.path(), None, Some(&global_path), |_| None)
                .unwrap();
        assert_eq!(config.commands["server"].cmd, "npm run project");
        assert_eq!(config.commands["global-only"].cmd, "echo global");
        assert_eq!(config.commands["project-only"].cmd, "echo project");
        assert_eq!(
            config
                .checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.save.as_str()),
            Some("save-checkpoint")
        );
    }

    #[test]
    fn env_overrides_beat_project_and_global() {
        let mut config = Config {
            provider: ProviderConfig {
                name: "project-provider".to_string(),
                base_url: Some("http://project.example".to_string()),
                step_timeout_secs: Some(9),
                ..ProviderConfig::default()
            },
            commands: IndexMap::new(),
            checkpoint: None,
        };

        apply_env_overrides_from(&mut config, |var| match var {
            "BUGATTI_PROVIDER" => Some("env-provider".to_string()),
            "BUGATTI_BASE_URL" => Some("http://env.example".to_string()),
            "BUGATTI_STEP_TIMEOUT" => Some("45".to_string()),
            _ => None,
        })
        .unwrap();

        assert_eq!(config.provider.name, "env-provider");
        assert_eq!(
            config.provider.base_url.as_deref(),
            Some("http://env.example")
        );
        assert_eq!(config.provider.step_timeout_secs, Some(45));
    }

    #[test]
    fn empty_env_values_are_treated_as_unset() {
        let mut config = Config {
            provider: ProviderConfig {
                name: "project-provider".to_string(),
                base_url: Some("http://project.example".to_string()),
                step_timeout_secs: Some(9),
                ..ProviderConfig::default()
            },
            commands: IndexMap::new(),
            checkpoint: None,
        };

        apply_env_overrides_from(&mut config, |_| Some("".to_string())).unwrap();

        assert_eq!(config.provider.name, "project-provider");
        assert_eq!(
            config.provider.base_url.as_deref(),
            Some("http://project.example")
        );
        assert_eq!(config.provider.step_timeout_secs, Some(9));
    }

    #[test]
    fn zero_step_timeout_env_is_rejected() {
        let mut config = Config::default();
        let err = apply_env_overrides_from(&mut config, |var| match var {
            "BUGATTI_STEP_TIMEOUT" => Some("0".to_string()),
            _ => None,
        })
        .unwrap_err();
        assert!(matches!(err, ConfigError::InvalidEnvVar { .. }));
    }

    #[test]
    fn layered_config_env_beats_global() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let global_path = home.path().join("config.toml");
        fs::write(
            &global_path,
            r#"
[provider]
name = "global-provider"
"#,
        )
        .unwrap();

        let config =
            load_layered_config_with_options(project.path(), None, Some(&global_path), |var| {
                (var == "BUGATTI_PROVIDER").then(|| "env-provider".to_string())
            })
            .unwrap();

        assert_eq!(config.provider.name, "env-provider");
    }

    #[test]
    fn invalid_step_timeout_env_errors() {
        let mut config = Config::default();
        let err = apply_env_overrides_from(&mut config, |var| {
            (var == "BUGATTI_STEP_TIMEOUT").then(|| "not-a-number".to_string())
        })
        .unwrap_err();

        assert!(matches!(
            err,
            ConfigError::InvalidEnvVar { var, value }
                if var == "BUGATTI_STEP_TIMEOUT" && value == "not-a-number"
        ));
    }

    #[test]
    fn layered_config_without_global_path_does_not_crash() {
        let project = tempfile::tempdir().unwrap();
        let config =
            load_layered_config_with_options(project.path(), None, None, |_| None).unwrap();
        assert_eq!(config.provider.name, default_provider_name());
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
