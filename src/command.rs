use crate::config::{CommandDef, CommandKind, Config};
use crate::progress::{ProgressReporter, STDOUT_PROGRESS_REPORTER};
use crate::run::ArtifactDir;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

/// Result of executing a short-lived command.
#[derive(Debug)]
pub struct CommandResult {
    pub name: String,
    pub exit_code: Option<i32>,
    pub stdout_path: String,
    pub stderr_path: String,
}

/// Error type for command execution.
#[derive(Debug)]
pub enum CommandError {
    /// A required short-lived command exited non-zero.
    NonZeroExit {
        name: String,
        exit_code: Option<i32>,
        stderr_path: String,
    },
    /// Failed to spawn the command process.
    SpawnFailed {
        name: String,
        cmd: String,
        source: std::io::Error,
    },
    /// Failed to write captured output to the log file.
    OutputWriteFailed {
        path: String,
        source: std::io::Error,
    },
    /// Readiness check failed for a long-lived command.
    ReadinessFailed {
        name: String,
        url: String,
        message: String,
    },
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::NonZeroExit {
                name,
                exit_code,
                stderr_path,
            } => {
                let code = exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                write!(
                    f,
                    "command '{name}' failed with exit code {code} (stderr: {stderr_path})"
                )
            }
            CommandError::SpawnFailed { name, cmd, source } => {
                write!(f, "failed to spawn command '{name}' (`{cmd}`): {source}")
            }
            CommandError::OutputWriteFailed { path, source } => {
                write!(f, "failed to write command output to '{path}': {source}")
            }
            CommandError::ReadinessFailed { name, url, message } => {
                write!(
                    f,
                    "readiness check for '{name}' at '{url}' failed: {message}"
                )
            }
        }
    }
}

impl std::error::Error for CommandError {}

/// Error type for skip-command validation.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("unknown command(s) in --skip-cmd: {unknown}. Known commands: {known}")]
    UnknownSkipCmds { unknown: String, known: String },
    #[error("invalid --skip-readiness: {0}")]
    InvalidSkipReadiness(String),
}

/// Validate that all skip-cmd names refer to known command names in the config.
/// Returns a typed error listing invalid names if any are unknown.
pub fn validate_skip_cmds(config: &Config, skip_cmds: &[String]) -> Result<(), ValidationError> {
    let unknown: Vec<&String> = skip_cmds
        .iter()
        .filter(|name| !config.commands.contains_key(name.as_str()))
        .collect();

    if unknown.is_empty() {
        Ok(())
    } else {
        let known: Vec<&String> = config.commands.keys().collect();
        Err(ValidationError::UnknownSkipCmds {
            unknown: unknown
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            known: known
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        })
    }
}

/// Validate that all skip-readiness names are known commands that are also in skip-cmds.
pub fn validate_skip_readiness(
    config: &Config,
    skip_cmds: &[String],
    skip_readiness: &[String],
) -> Result<(), ValidationError> {
    let mut errors = Vec::new();
    for name in skip_readiness {
        if !config.commands.contains_key(name.as_str()) {
            errors.push(format!("'{name}' is not a known command"));
        } else if !skip_cmds.contains(name) {
            errors.push(format!("'{name}' is not in --skip-cmd (readiness can only be skipped for skipped commands)"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ValidationError::InvalidSkipReadiness(errors.join("; ")))
    }
}

/// Execute all short-lived commands from the config during the setup phase.
///
/// Commands are executed in declaration order (the order they appear in bugatti.config.toml).
/// stdout and stderr are captured and stored under the run's logs/ directory.
/// If any command exits non-zero, execution stops and an error is returned.
///
/// Returns a list of successful command results.
pub async fn run_short_lived_commands(
    config: &Config,
    artifact_dir: &ArtifactDir,
    skip_cmds: &[String],
) -> Result<Vec<CommandResult>, CommandError> {
    run_short_lived_commands_with_reporter(
        config,
        artifact_dir,
        skip_cmds,
        &STDOUT_PROGRESS_REPORTER,
    )
    .await
}

pub(crate) async fn run_short_lived_commands_with_reporter(
    config: &Config,
    artifact_dir: &ArtifactDir,
    skip_cmds: &[String],
    reporter: &dyn ProgressReporter,
) -> Result<Vec<CommandResult>, CommandError> {
    let mut results = Vec::new();

    for (name, def) in &config.commands {
        if def.kind != CommandKind::ShortLived {
            continue;
        }
        if skip_cmds.contains(name) {
            reporter.line(&format!("SKIP ....... {name}"));
            continue;
        }

        let result = execute_short_lived(name, def, &artifact_dir.logs, reporter).await?;
        reporter.line(&format!(
            "OK ......... {name} (exit {})",
            result.exit_code.unwrap_or(-1)
        ));
        results.push(result);
    }

    Ok(results)
}

/// Execute a single short-lived command, capturing stdout and stderr to log files.
async fn execute_short_lived(
    name: &str,
    def: &CommandDef,
    logs_dir: &Path,
    reporter: &dyn ProgressReporter,
) -> Result<CommandResult, CommandError> {
    tracing::info!(command = name, cmd = %def.cmd, "executing short-lived command");
    reporter.line(&format!("RUN ........ {name}: {}", def.cmd));

    let output = Command::new("sh")
        .arg("-c")
        .arg(&def.cmd)
        .output()
        .await
        .map_err(|e| CommandError::SpawnFailed {
            name: name.to_string(),
            cmd: def.cmd.clone(),
            source: e,
        })?;

    let stdout_path = logs_dir.join(format!("{name}.stdout.log"));
    let stderr_path = logs_dir.join(format!("{name}.stderr.log"));

    std::fs::write(&stdout_path, &output.stdout).map_err(|e| CommandError::OutputWriteFailed {
        path: stdout_path.display().to_string(),
        source: e,
    })?;
    std::fs::write(&stderr_path, &output.stderr).map_err(|e| CommandError::OutputWriteFailed {
        path: stderr_path.display().to_string(),
        source: e,
    })?;

    let exit_code = output.status.code();

    if !output.status.success() {
        tracing::error!(command = name, exit_code = ?exit_code, "short-lived command failed");
        // Print last lines of output so the user can see what went wrong
        print_output_tail(reporter, "stderr", &output.stderr);
        print_output_tail(reporter, "stdout", &output.stdout);
        return Err(CommandError::NonZeroExit {
            name: name.to_string(),
            exit_code,
            stderr_path: stderr_path.display().to_string(),
        });
    }

    tracing::info!(command = name, exit_code = ?exit_code, "short-lived command succeeded");
    Ok(CommandResult {
        name: name.to_string(),
        exit_code,
        stdout_path: stdout_path.display().to_string(),
        stderr_path: stderr_path.display().to_string(),
    })
}

/// Print the last non-empty lines of command output, prefixed with a label.
fn print_output_tail(reporter: &dyn ProgressReporter, label: &str, output: &[u8]) {
    let text = String::from_utf8_lossy(output);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return;
    }
    let tail: Vec<&str> = lines
        .into_iter()
        .rev()
        .take(10)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    reporter.line(&format!("  {label}:"));
    for line in tail {
        reporter.line(&format!("    {line}"));
    }
}

/// Compute the checkpoint directory path for a given ID.
pub fn checkpoint_path(project_root: &Path, checkpoint_id: &str) -> PathBuf {
    project_root
        .join(".bugatti")
        .join("checkpoints")
        .join(checkpoint_id)
}

/// Default checkpoint command timeout (120 seconds).
const DEFAULT_CHECKPOINT_TIMEOUT: Duration = Duration::from_secs(120);

/// Run a checkpoint save or restore command with BUGATTI_CHECKPOINT_ID and BUGATTI_CHECKPOINT_PATH.
pub async fn run_checkpoint_command(
    cmd: &str,
    checkpoint_id: &str,
    project_root: &Path,
    timeout_secs: Option<u64>,
) -> Result<(), String> {
    run_checkpoint_command_with_reporter(
        cmd,
        checkpoint_id,
        project_root,
        timeout_secs,
        &STDOUT_PROGRESS_REPORTER,
    )
    .await
}

pub(crate) async fn run_checkpoint_command_with_reporter(
    cmd: &str,
    checkpoint_id: &str,
    project_root: &Path,
    timeout_secs: Option<u64>,
    reporter: &dyn ProgressReporter,
) -> Result<(), String> {
    let cp_path = checkpoint_path(project_root, checkpoint_id);
    std::fs::create_dir_all(&cp_path).map_err(|e| {
        format!(
            "failed to create checkpoint dir '{}': {e}",
            cp_path.display()
        )
    })?;

    let timeout = timeout_secs
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_CHECKPOINT_TIMEOUT);

    let child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .env("BUGATTI_CHECKPOINT_ID", checkpoint_id)
        .env("BUGATTI_CHECKPOINT_PATH", cp_path.display().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Ensure the process is killed if we abandon it on timeout.
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("failed to spawn checkpoint command: {e}"))?;

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            if !output.status.success() {
                print_output_tail(reporter, "stderr", &output.stderr);
                print_output_tail(reporter, "stdout", &output.stdout);
                return Err(format!(
                    "exited with code {}",
                    output.status.code().unwrap_or(-1)
                ));
            }
            Ok(())
        }
        Ok(Err(e)) => Err(format!("failed to wait for checkpoint command: {e}")),
        Err(_) => Err(format!("timed out after {}s", timeout.as_secs())),
    }
}

/// Default timeout for readiness checks (30 seconds).
const DEFAULT_READINESS_TIMEOUT: Duration = Duration::from_secs(30);

/// Default interval between readiness poll attempts (500ms).
const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// A tracked long-lived background process.
#[derive(Debug)]
pub struct TrackedProcess {
    pub name: String,
    pub child: Child,
    pub stdout_path: String,
    pub stderr_path: String,
}

impl TrackedProcess {
    /// Check if the process has exited unexpectedly.
    /// Returns Some(exit_code) if exited, None if still running.
    pub fn check_exited(&mut self) -> Option<Option<i32>> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.code()),
            Ok(None) => None,
            Err(_) => Some(None),
        }
    }
}

/// Result of tearing down long-lived processes.
#[derive(Debug)]
pub struct TeardownResult {
    pub name: String,
    pub success: bool,
    pub message: String,
}

/// Spawn all long-lived commands as background processes, capturing output to log files.
///
/// After spawning, if a readiness_url is configured, polls it until ready or timeout.
/// Returns a list of tracked processes that must be torn down later.
pub async fn spawn_long_lived_commands(
    config: &Config,
    artifact_dir: &ArtifactDir,
    skip_cmds: &[String],
    skip_readiness: &[String],
) -> Result<Vec<TrackedProcess>, CommandError> {
    spawn_long_lived_commands_with_reporter(
        config,
        artifact_dir,
        skip_cmds,
        skip_readiness,
        &STDOUT_PROGRESS_REPORTER,
    )
    .await
}

pub(crate) async fn spawn_long_lived_commands_with_reporter(
    config: &Config,
    artifact_dir: &ArtifactDir,
    skip_cmds: &[String],
    skip_readiness: &[String],
    reporter: &dyn ProgressReporter,
) -> Result<Vec<TrackedProcess>, CommandError> {
    let mut tracked = Vec::new();

    for (name, def) in &config.commands {
        if def.kind != CommandKind::LongLived {
            continue;
        }
        if skip_cmds.contains(name) {
            reporter.line(&format!("SKIP ....... {name} (long-lived)"));
            // Readiness checks still run for skipped commands unless explicitly disabled
            let urls = def.effective_readiness_urls();
            if !urls.is_empty() {
                if skip_readiness.contains(name) {
                    reporter.line(&format!(
                        "SKIP ....... {name} readiness check (--skip-readiness)"
                    ));
                } else {
                    let timeout = readiness_timeout(def);
                    for url in &urls {
                        reporter.line(&format!("WAIT ....... {name} (skipped): polling {url}"));
                        if let Err(e) = poll_readiness(url, timeout).await {
                            reporter.line(&format!(
                                "FAIL ....... {name} (skipped): readiness check failed"
                            ));
                            teardown_processes(&mut tracked).await;
                            return Err(CommandError::ReadinessFailed {
                                name: name.to_string(),
                                url: url.to_string(),
                                message: e,
                            });
                        }
                    }
                    reporter.line(&format!("READY ...... {name} (skipped)"));
                }
            }
            continue;
        }

        tracing::info!(command = name.as_str(), cmd = %def.cmd, "spawning long-lived command");
        reporter.line(&format!("START ...... {name}: {}", def.cmd));

        let stdout_path = artifact_dir.logs.join(format!("{name}.stdout.log"));
        let stderr_path = artifact_dir.logs.join(format!("{name}.stderr.log"));

        let stdout_file =
            std::fs::File::create(&stdout_path).map_err(|e| CommandError::OutputWriteFailed {
                path: stdout_path.display().to_string(),
                source: e,
            })?;
        let stderr_file =
            std::fs::File::create(&stderr_path).map_err(|e| CommandError::OutputWriteFailed {
                path: stderr_path.display().to_string(),
                source: e,
            })?;

        let child = Command::new("sh")
            .arg("-c")
            .arg(&def.cmd)
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file))
            .spawn()
            .map_err(|e| CommandError::SpawnFailed {
                name: name.to_string(),
                cmd: def.cmd.clone(),
                source: e,
            })?;

        let process = TrackedProcess {
            name: name.clone(),
            child,
            stdout_path: stdout_path.display().to_string(),
            stderr_path: stderr_path.display().to_string(),
        };

        tracked.push(process);

        // Check readiness if configured
        let urls = def.effective_readiness_urls();
        if !urls.is_empty() {
            let timeout = readiness_timeout(def);
            for url in &urls {
                reporter.line(&format!("WAIT ....... {name}: polling {url}"));
                if let Err(e) = poll_readiness(url, timeout).await {
                    // Readiness failed - tear down what we've started
                    reporter.line(&format!("FAIL ....... {name}: readiness check failed"));
                    teardown_processes(&mut tracked).await;
                    return Err(CommandError::ReadinessFailed {
                        name: name.to_string(),
                        url: url.to_string(),
                        message: e,
                    });
                }
            }
            reporter.line(&format!("READY ...... {name}"));
        }
    }

    Ok(tracked)
}

/// Compute the readiness timeout for a command, using the per-command override or the default.
fn readiness_timeout(def: &CommandDef) -> Duration {
    def.readiness_timeout_secs
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_READINESS_TIMEOUT)
}

/// Poll a readiness URL until it responds with a success status or timeout.
async fn poll_readiness(url: &str, timeout: Duration) -> Result<(), String> {
    tracing::info!(
        url = url,
        timeout_secs = timeout.as_secs(),
        "starting readiness poll"
    );
    let start = Instant::now();

    while start.elapsed() < timeout {
        // Try a simple TCP connection check by parsing the URL and connecting
        if check_url(url).await {
            tracing::info!(
                url = url,
                elapsed_ms = start.elapsed().as_millis() as u64,
                "readiness check passed"
            );
            return Ok(());
        }
        tokio::time::sleep(READINESS_POLL_INTERVAL).await;
    }

    tracing::error!(
        url = url,
        timeout_secs = timeout.as_secs(),
        "readiness check timed out"
    );
    Err(format!(
        "timed out after {}s waiting for {url}",
        timeout.as_secs()
    ))
}

/// Check if an HTTP(S) URL is reachable by attempting a GET request.
async fn check_url(url: &str) -> bool {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();

    let Some(client) = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .inspect_err(|e| {
                // Cached forever by the OnceLock: without this log every
                // readiness poll would silently fail until timeout.
                tracing::warn!(error = %e, "failed to build readiness HTTP client; readiness checks will fail");
            })
            .ok()
    }) else {
        return false;
    };

    client
        .get(url)
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

/// Check if any tracked processes have exited unexpectedly.
/// Returns the name and exit code of the first process found to have exited.
pub fn check_for_unexpected_exits(
    processes: &mut [TrackedProcess],
) -> Option<(String, Option<i32>)> {
    for process in processes.iter_mut() {
        if let Some(exit_code) = process.check_exited() {
            return Some((process.name.clone(), exit_code));
        }
    }
    None
}

/// Tear down all tracked long-lived processes by sending SIGTERM.
/// Returns results describing the outcome for each process.
pub async fn teardown_processes(processes: &mut [TrackedProcess]) -> Vec<TeardownResult> {
    tracing::info!(count = processes.len(), "tearing down long-lived processes");
    let mut results = Vec::new();

    for process in processes.iter_mut() {
        let result = teardown_single(process).await;
        tracing::info!(
            command = result.name.as_str(),
            success = result.success,
            message = result.message.as_str(),
            "teardown result"
        );
        results.push(result);
    }

    results
}

/// Tear down a single process with SIGTERM, waiting briefly for exit.
async fn teardown_single(process: &mut TrackedProcess) -> TeardownResult {
    let name = process.name.clone();

    // Check if already exited
    match process.child.try_wait() {
        Ok(Some(status)) => {
            return TeardownResult {
                name,
                success: true,
                message: format!("already exited with code {}", status.code().unwrap_or(-1)),
            };
        }
        Ok(None) => {} // Still running, proceed with SIGTERM
        Err(e) => {
            return TeardownResult {
                name,
                success: false,
                message: format!("failed to check process status: {e}"),
            };
        }
    }

    // Send SIGTERM
    #[cfg(unix)]
    {
        if let Some(pid) = process.child.id() {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = process.child.start_kill();
    }

    // Wait briefly for orderly shutdown
    match tokio::time::timeout(Duration::from_secs(5), process.child.wait()).await {
        Ok(Ok(status)) => TeardownResult {
            name,
            success: true,
            message: format!("terminated with code {}", status.code().unwrap_or(-1)),
        },
        Ok(Err(e)) => TeardownResult {
            name,
            success: false,
            message: format!("error waiting for process: {e}"),
        },
        Err(_) => {
            // Force kill after timeout
            let _ = process.child.start_kill();
            let _ = process.child.wait().await;
            TeardownResult {
                name,
                success: false,
                message: "did not exit after SIGTERM; force killed".to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CommandDef, CommandKind, Config, ProviderConfig};
    use crate::test_support as common;
    use indexmap::IndexMap;
    use std::sync::Mutex;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    #[derive(Default)]
    struct RecordingReporter {
        lines: Mutex<Vec<String>>,
    }

    impl RecordingReporter {
        fn lines(&self) -> Vec<String> {
            self.lines.lock().unwrap().clone()
        }
    }

    impl ProgressReporter for RecordingReporter {
        fn line(&self, line: &str) {
            self.lines.lock().unwrap().push(line.to_string());
        }
    }

    fn make_config(commands: Vec<(&str, CommandKind, &str)>) -> Config {
        make_config_with_readiness(
            commands
                .into_iter()
                .map(|(n, k, c)| (n, k, c, None))
                .collect(),
        )
    }

    fn make_config_with_readiness(
        commands: Vec<(&str, CommandKind, &str, Option<&str>)>,
    ) -> Config {
        let mut map = IndexMap::new();
        for (name, kind, cmd, readiness_url) in commands {
            map.insert(
                name.to_string(),
                CommandDef {
                    kind,
                    cmd: cmd.to_string(),
                    readiness_url: readiness_url.map(String::from),
                    readiness_urls: Vec::new(),
                    readiness_timeout_secs: None,
                },
            );
        }
        Config {
            provider: ProviderConfig::default(),
            commands: map,
            checkpoint: None,
        }
    }

    async fn one_shot_http_server(status_line: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let response =
                format!("HTTP/1.1 {status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        format!("http://{addr}/ready")
    }

    #[tokio::test]
    async fn check_url_returns_true_for_200() {
        let url = one_shot_http_server("200 OK").await;

        assert!(check_url(&url).await);
    }

    #[tokio::test]
    async fn check_url_returns_false_for_500() {
        let url = one_shot_http_server("500 Internal Server Error").await;

        assert!(!check_url(&url).await);
    }

    #[tokio::test]
    async fn check_url_returns_false_for_unreachable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        assert!(!check_url(&format!("http://{addr}/ready")).await);
    }

    #[tokio::test]
    async fn successful_short_lived_command() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        let config = make_config(vec![("echo_test", CommandKind::ShortLived, "echo hello")]);

        let results = run_short_lived_commands(&config, artifact_dir, &[])
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "echo_test");
        assert_eq!(results[0].exit_code, Some(0));

        // Verify stdout was captured
        let stdout = std::fs::read_to_string(&results[0].stdout_path).unwrap();
        assert_eq!(stdout.trim(), "hello");

        // Verify stderr file exists (empty is fine)
        assert!(std::path::Path::new(&results[0].stderr_path).exists());
    }

    #[tokio::test]
    async fn failed_short_lived_command() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        let config = make_config(vec![("fail_cmd", CommandKind::ShortLived, "exit 42")]);

        let err = run_short_lived_commands(&config, artifact_dir, &[])
            .await
            .unwrap_err();
        match err {
            CommandError::NonZeroExit {
                name, exit_code, ..
            } => {
                assert_eq!(name, "fail_cmd");
                assert_eq!(exit_code, Some(42));
            }
            other => panic!("expected NonZeroExit, got: {other}"),
        }
    }

    #[tokio::test]
    async fn output_capture_to_log_files() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        let config = make_config(vec![(
            "mixed_output",
            CommandKind::ShortLived,
            "echo stdout_msg && echo stderr_msg >&2",
        )]);

        let results = run_short_lived_commands(&config, artifact_dir, &[])
            .await
            .unwrap();
        assert_eq!(results.len(), 1);

        let stdout = std::fs::read_to_string(&results[0].stdout_path).unwrap();
        assert!(stdout.contains("stdout_msg"));

        let stderr = std::fs::read_to_string(&results[0].stderr_path).unwrap();
        assert!(stderr.contains("stderr_msg"));
    }

    #[tokio::test]
    async fn long_lived_commands_are_skipped() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        let config = make_config(vec![
            ("setup", CommandKind::ShortLived, "echo setup"),
            ("server", CommandKind::LongLived, "echo server"),
        ]);

        let results = run_short_lived_commands(&config, artifact_dir, &[])
            .await
            .unwrap();
        // Only the short-lived command should have run
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "setup");
    }

    #[tokio::test]
    async fn skip_cmd_flag_excludes_command() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        let config = make_config(vec![
            ("migrate", CommandKind::ShortLived, "echo migrate"),
            ("seed", CommandKind::ShortLived, "echo seed"),
        ]);

        let results = run_short_lived_commands(&config, artifact_dir, &["migrate".to_string()])
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "seed");
    }

    #[tokio::test]
    async fn progress_reporter_is_injectable() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        let config = make_config(vec![("echo_test", CommandKind::ShortLived, "echo hello")]);
        let reporter = RecordingReporter::default();

        let results = run_short_lived_commands_with_reporter(&config, artifact_dir, &[], &reporter)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            reporter.lines(),
            vec![
                "RUN ........ echo_test: echo hello".to_string(),
                "OK ......... echo_test (exit 0)".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn failed_command_stops_execution() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        // Insertion ordering: "a_first" was inserted before "b_second"
        let config = make_config(vec![
            ("a_first", CommandKind::ShortLived, "exit 1"),
            ("b_second", CommandKind::ShortLived, "echo should_not_run"),
        ]);

        let err = run_short_lived_commands(&config, artifact_dir, &[])
            .await
            .unwrap_err();
        match err {
            CommandError::NonZeroExit { name, .. } => {
                assert_eq!(name, "a_first");
            }
            other => panic!("expected NonZeroExit, got: {other}"),
        }

        // Verify second command's log files don't exist (it never ran)
        let second_stdout = artifact_dir.logs.join("b_second.stdout.log");
        assert!(!second_stdout.exists());
    }

    // --- Long-lived command tests ---

    #[tokio::test]
    async fn spawn_long_lived_captures_output() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        // Use a command that writes output then sleeps briefly
        let config = make_config(vec![(
            "worker",
            CommandKind::LongLived,
            "echo long_lived_output && sleep 60",
        )]);

        let mut tracked = spawn_long_lived_commands(&config, artifact_dir, &[], &[])
            .await
            .unwrap();
        assert_eq!(tracked.len(), 1);
        assert_eq!(tracked[0].name, "worker");

        // Give it a moment to write output
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Verify log files exist
        assert!(std::path::Path::new(&tracked[0].stdout_path).exists());
        assert!(std::path::Path::new(&tracked[0].stderr_path).exists());

        // Verify output was captured
        let stdout = std::fs::read_to_string(&tracked[0].stdout_path).unwrap();
        assert!(stdout.contains("long_lived_output"), "stdout: {stdout}");

        // Clean up
        teardown_processes(&mut tracked).await;
    }

    #[tokio::test]
    async fn spawn_long_lived_skip() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        let config = make_config(vec![
            ("server", CommandKind::LongLived, "sleep 60"),
            ("worker", CommandKind::LongLived, "sleep 60"),
        ]);

        let mut tracked =
            spawn_long_lived_commands(&config, artifact_dir, &["server".to_string()], &[])
                .await
                .unwrap();
        assert_eq!(tracked.len(), 1);
        assert_eq!(tracked[0].name, "worker");

        teardown_processes(&mut tracked).await;
    }

    #[tokio::test]
    async fn teardown_stops_running_processes() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        let config = make_config(vec![("sleeper", CommandKind::LongLived, "sleep 600")]);

        let mut tracked = spawn_long_lived_commands(&config, artifact_dir, &[], &[])
            .await
            .unwrap();
        assert_eq!(tracked.len(), 1);

        let results = teardown_processes(&mut tracked).await;
        assert_eq!(results.len(), 1);
        assert!(
            results[0].success,
            "teardown should succeed: {}",
            results[0].message
        );
    }

    #[tokio::test]
    async fn detect_unexpected_exit() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        // Command that exits immediately
        let config = make_config(vec![("quick_exit", CommandKind::LongLived, "exit 1")]);

        let mut tracked = spawn_long_lived_commands(&config, artifact_dir, &[], &[])
            .await
            .unwrap();

        // Wait for it to exit
        tokio::time::sleep(Duration::from_millis(200)).await;

        let result = check_for_unexpected_exits(&mut tracked);
        assert!(result.is_some(), "should detect unexpected exit");
        let (name, exit_code) = result.unwrap();
        assert_eq!(name, "quick_exit");
        assert_eq!(exit_code, Some(1));
    }

    #[test]
    fn validate_skip_cmds_valid_names() {
        let config = make_config(vec![
            ("migrate", CommandKind::ShortLived, "echo migrate"),
            ("server", CommandKind::LongLived, "sleep 60"),
        ]);

        assert!(validate_skip_cmds(&config, &["migrate".to_string()]).is_ok());
        assert!(validate_skip_cmds(&config, &["server".to_string()]).is_ok());
        assert!(
            validate_skip_cmds(&config, &["migrate".to_string(), "server".to_string()]).is_ok()
        );
        assert!(validate_skip_cmds(&config, &[]).is_ok());
    }

    #[test]
    fn validate_skip_cmds_invalid_names() {
        let config = make_config(vec![("migrate", CommandKind::ShortLived, "echo migrate")]);

        let err = validate_skip_cmds(&config, &["nonexistent".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("nonexistent"), "error: {err}");
        assert!(
            err.contains("migrate"),
            "error should list known commands: {err}"
        );
    }

    #[test]
    fn validate_skip_cmds_mixed_valid_invalid() {
        let config = make_config(vec![
            ("migrate", CommandKind::ShortLived, "echo migrate"),
            ("server", CommandKind::LongLived, "sleep 60"),
        ]);

        let err = validate_skip_cmds(&config, &["migrate".to_string(), "bogus".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("bogus"), "error: {err}");
        assert!(
            !err.contains("migrate") || err.contains("Known commands"),
            "should not list migrate as unknown"
        );
    }

    #[test]
    fn validate_skip_readiness_valid() {
        let config = make_config(vec![("server", CommandKind::LongLived, "sleep 60")]);
        assert!(
            validate_skip_readiness(&config, &["server".to_string()], &["server".to_string()])
                .is_ok()
        );
        assert!(validate_skip_readiness(&config, &["server".to_string()], &[]).is_ok());
    }

    #[test]
    fn validate_skip_readiness_must_be_skipped() {
        let config = make_config(vec![("server", CommandKind::LongLived, "sleep 60")]);
        let err = validate_skip_readiness(&config, &[], &["server".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("not in --skip-cmd"), "error: {err}");
    }

    #[test]
    fn validate_skip_readiness_unknown_command() {
        let config = make_config(vec![("server", CommandKind::LongLived, "sleep 60")]);
        let err = validate_skip_readiness(&config, &["server".to_string()], &["bogus".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a known command"), "error: {err}");
    }

    #[tokio::test]
    async fn spawn_long_lived_skip_readiness() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        // Readiness URL points to unreachable host — would fail without skip_readiness
        let config = make_config_with_readiness(vec![(
            "server",
            CommandKind::LongLived,
            "sleep 60",
            Some("http://127.0.0.1:1/nonexistent"),
        )]);

        // Skip both the command and its readiness check
        let tracked = spawn_long_lived_commands(
            &config,
            artifact_dir,
            &["server".to_string()],
            &["server".to_string()],
        )
        .await
        .unwrap();

        // No processes spawned (command was skipped), and no readiness error
        assert!(tracked.is_empty());
    }

    #[tokio::test]
    async fn short_lived_not_spawned_as_long_lived() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        let config = make_config(vec![
            ("setup", CommandKind::ShortLived, "echo setup"),
            ("server", CommandKind::LongLived, "sleep 60"),
        ]);

        let mut tracked = spawn_long_lived_commands(&config, artifact_dir, &[], &[])
            .await
            .unwrap();
        // Only long-lived should be spawned
        assert_eq!(tracked.len(), 1);
        assert_eq!(tracked[0].name, "server");

        teardown_processes(&mut tracked).await;
    }

    #[tokio::test]
    async fn commands_execute_in_declaration_order() {
        let artifact_case = common::ArtifactCase::new();
        let artifact_dir = &artifact_case.artifact_dir;

        // Insert in reverse-alpha order: z_last first, a_first second.
        // With BTreeMap this would have executed a_first then z_last.
        // With IndexMap it must execute z_last then a_first.
        let config = make_config(vec![
            ("z_last", CommandKind::ShortLived, "echo z_last"),
            ("a_first", CommandKind::ShortLived, "echo a_first"),
        ]);

        let results = run_short_lived_commands(&config, artifact_dir, &[])
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "z_last");
        assert_eq!(results[1].name, "a_first");
    }
}
