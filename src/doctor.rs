use crate::config::{self, Config};
use crate::discovery;
use crate::exit_code::{EXIT_CONFIG_ERROR, EXIT_OK};
use crate::expand;
use crate::output;
use crate::test_file;
use crate::update;
use semver::Version;
use std::cmp::Ordering;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

const CLAUDE_INSTALL_URL: &str = "https://docs.anthropic.com/en/docs/claude-code";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub status: CheckStatus,
    pub message: String,
}

impl CheckResult {
    fn ok(message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Ok,
            message: message.into(),
        }
    }

    fn warn(message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Warn,
            message: message.into(),
        }
    }

    fn fail(message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Fail,
            message: message.into(),
        }
    }
}

/// Run diagnostic checks for the current project.
pub async fn run_doctor(dir: &Path) -> i32 {
    let mut checks = Vec::new();

    let version_check = check_version_target();
    print_check(&version_check);
    checks.push(version_check);

    let (config, config_check) = check_config(dir);
    print_check(&config_check);
    checks.push(config_check);

    let provider_check = check_provider_binary(&config).await;
    print_check(&provider_check);
    checks.push(provider_check);

    let commands_check = check_commands(&config);
    print_check(&commands_check);
    checks.push(commands_check);

    for check in check_test_files(dir) {
        print_check(&check);
        checks.push(check);
    }

    let update_check = check_update_status().await;
    print_check(&update_check);
    checks.push(update_check);

    let code = exit_code_for_checks(&checks);
    if code == EXIT_OK {
        println!("\nDoctor completed: no failing checks.");
    } else {
        println!("\nDoctor completed: one or more checks failed.");
    }
    code
}

pub fn exit_code_for_checks(checks: &[CheckResult]) -> i32 {
    if checks.iter().any(|check| check.status == CheckStatus::Fail) {
        EXIT_CONFIG_ERROR
    } else {
        EXIT_OK
    }
}

fn print_check(result: &CheckResult) {
    let c = output::stdout_colors();
    let red = if c.enabled { "\x1b[31m" } else { "" };
    let (symbol, color) = match result.status {
        CheckStatus::Ok => ("✓", c.result),
        CheckStatus::Warn => ("!", c.prompt),
        CheckStatus::Fail => ("✗", red),
    };
    println!("{}{}{} {}", color, symbol, c.reset, result.message);
}

fn check_version_target() -> CheckResult {
    CheckResult::ok(format!(
        "bugatti v{} ({}/{})",
        update::current_version(),
        std::env::consts::ARCH,
        std::env::consts::OS
    ))
}

fn check_config(dir: &Path) -> (Config, CheckResult) {
    let config_path = dir.join("bugatti.config.toml");
    if !config_path.exists() {
        return (
            Config::default(),
            CheckResult::warn(
                "bugatti.config.toml not found; using defaults. Run `bugatti init` to scaffold one.",
            ),
        );
    }

    match config::load_config(dir) {
        Ok(config) => (config, CheckResult::ok("bugatti.config.toml loaded")),
        Err(e) => (
            Config::default(),
            CheckResult::fail(format!("config error: {e}")),
        ),
    }
}

pub fn provider_binary(provider_name: &str) -> Result<&'static str, String> {
    match provider_name {
        "claude-code" => Ok("claude"),
        "codex" => Ok("codex"),
        "pi" => Ok("pi"),
        other => Err(format!(
            "unknown provider '{other}' (supported: claude-code, codex, pi)"
        )),
    }
}

async fn check_provider_binary(config: &Config) -> CheckResult {
    let binary = match provider_binary(&config.provider.name) {
        Ok(binary) => binary,
        Err(message) => return CheckResult::fail(format!("provider: {message}")),
    };

    let path = match which::which(binary) {
        Ok(path) => path,
        Err(e) => {
            let mut message = format!(
                "provider binary `{binary}` for `{}` not found on PATH: {e}",
                config.provider.name
            );
            if config.provider.name == "claude-code" {
                message.push_str(&format!(". Install Claude Code: {CLAUDE_INSTALL_URL}"));
            }
            return CheckResult::fail(message);
        }
    };

    match binary_version(binary).await {
        Some(version) => CheckResult::ok(format!(
            "provider `{}` found at {} ({})",
            config.provider.name,
            path.display(),
            version
        )),
        None => CheckResult::ok(format!(
            "provider `{}` found at {}",
            config.provider.name,
            path.display()
        )),
    }
}

async fn binary_version(binary: &str) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(2),
        Command::new(binary).arg("--version").output(),
    )
    .await
    .ok()?
    .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).to_string()
    } else {
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    let version = text.lines().next()?.trim();
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

pub fn command_binary_token(cmd: &str) -> Option<&str> {
    cmd.split_whitespace().next()
}

fn check_commands(config: &Config) -> CheckResult {
    if config.commands.is_empty() {
        return CheckResult::ok("0 commands configured");
    }

    let mut missing = Vec::new();
    for (name, def) in &config.commands {
        let Some(token) = command_binary_token(&def.cmd) else {
            missing.push(format!("{name}: empty command"));
            continue;
        };
        if which::which(token).is_err() {
            missing.push(format!("{name}: `{token}` from `{}`", def.cmd));
        }
    }

    if missing.is_empty() {
        CheckResult::ok(format!(
            "{} commands configured, all found on PATH",
            config.commands.len()
        ))
    } else {
        CheckResult::warn(format!(
            "{} command binary checks may be missing (shell builtins/compound commands can be false positives): {}",
            missing.len(),
            missing.join("; ")
        ))
    }
}

fn check_test_files(dir: &Path) -> Vec<CheckResult> {
    let discovery = match discovery::discover_root_tests(dir) {
        Ok(discovery) => discovery,
        Err(e) => return vec![CheckResult::fail(format!("test discovery failed: {e}"))],
    };

    let mut checks = Vec::new();
    if discovery.tests.is_empty() {
        checks.push(CheckResult::warn(
            "no root *.test.toml files found; run `bugatti init` to create an example",
        ));
    } else {
        checks.push(CheckResult::ok(format!(
            "{} root test file(s) discovered",
            discovery.tests.len()
        )));
    }

    if !discovery.errors.is_empty() {
        let details = discovery
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        checks.push(CheckResult::fail(format!(
            "test file parse errors: {details}"
        )));
    }

    let mut expand_errors = Vec::new();
    for discovered in &discovery.tests {
        match test_file::parse_test_file(&discovered.path)
            .map_err(|e| e.to_string())
            .and_then(|tf| {
                expand::expand_steps(&discovered.path, &tf)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }) {
            Ok(()) => {}
            Err(e) => expand_errors.push(format!("{}: {e}", discovered.path.display())),
        }
    }

    if expand_errors.is_empty() {
        if !discovery.tests.is_empty() {
            checks.push(CheckResult::ok("test includes expanded successfully"));
        }
    } else {
        checks.push(CheckResult::fail(format!(
            "test include/expand errors: {}",
            expand_errors.join("; ")
        )));
    }

    checks
}

async fn check_update_status() -> CheckResult {
    let Some(latest) = update::latest_version_tag().await else {
        return CheckResult::warn("unable to check for updates");
    };

    let current = update::current_version();
    match compare_versions(current, &latest) {
        Some(Ordering::Less) => CheckResult::warn(format!(
            "update available: v{current} → v{latest}; run `bugatti update` to install"
        )),
        Some(_) => CheckResult::ok(format!("bugatti v{current} is up to date")),
        None => CheckResult::warn(format!(
            "unable to compare local version `{current}` with latest `{latest}`"
        )),
    }
}

fn compare_versions(current: &str, latest: &str) -> Option<Ordering> {
    let current = Version::parse(current.trim_start_matches(['v', 'V'])).ok()?;
    let latest = Version::parse(latest.trim_start_matches(['v', 'V'])).ok()?;
    Some(current.cmp(&latest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CommandDef, CommandKind, ProviderConfig};
    use indexmap::IndexMap;
    use std::fs;

    #[test]
    fn check_aggregation_returns_failure_when_any_check_fails() {
        let checks = vec![
            CheckResult::ok("ok"),
            CheckResult::warn("warn"),
            CheckResult::fail("fail"),
        ];
        assert_eq!(exit_code_for_checks(&checks), EXIT_CONFIG_ERROR);

        let checks = vec![CheckResult::ok("ok"), CheckResult::warn("warn")];
        assert_eq!(exit_code_for_checks(&checks), EXIT_OK);
    }

    #[test]
    fn command_token_extracts_first_whitespace_token() {
        assert_eq!(command_binary_token("npm run dev"), Some("npm"));
        assert_eq!(command_binary_token("  cargo run"), Some("cargo"));
        assert_eq!(command_binary_token(""), None);
    }

    #[test]
    fn provider_mapping_handles_known_and_unknown_providers() {
        assert_eq!(provider_binary("claude-code").unwrap(), "claude");
        assert_eq!(provider_binary("codex").unwrap(), "codex");
        assert_eq!(provider_binary("pi").unwrap(), "pi");
        assert!(provider_binary("other")
            .unwrap_err()
            .contains("unknown provider 'other'"));
    }

    #[test]
    fn broken_config_is_a_failing_check() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("bugatti.config.toml"), "invalid [[[toml").unwrap();

        let (_config, check) = check_config(dir.path());

        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.message.contains("config error"));
    }

    #[test]
    fn cyclic_include_is_a_failing_test_check() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("root.test.toml"),
            r#"
name = "root"

[[steps]]
include_path = "child.test.toml"
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("child.test.toml"),
            r#"
name = "child"

[[steps]]
include_path = "root.test.toml"
"#,
        )
        .unwrap();

        let checks = check_test_files(dir.path());

        assert!(checks.iter().any(|check| {
            check.status == CheckStatus::Fail && check.message.contains("include cycle detected")
        }));
    }

    #[test]
    fn command_check_warns_on_missing_binary() {
        let mut commands = IndexMap::new();
        commands.insert(
            "dev".to_string(),
            CommandDef {
                kind: CommandKind::LongLived,
                cmd: "definitely-not-a-bugatti-test-binary --serve".to_string(),
                readiness_url: None,
                readiness_urls: Vec::new(),
                readiness_timeout_secs: None,
            },
        );
        let config = Config {
            provider: ProviderConfig::default(),
            commands,
            checkpoint: None,
        };

        let check = check_commands(&config);

        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check
            .message
            .contains("definitely-not-a-bugatti-test-binary"));
    }
}
