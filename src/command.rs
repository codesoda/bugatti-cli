use crate::config::{CommandDef, CommandKind, Config};
use crate::run::ArtifactDir;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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

/// Execute all short-lived commands from the config during the setup phase.
///
/// Commands are executed in BTreeMap order (alphabetical by name).
/// stdout and stderr are captured and stored under the run's logs/ directory.
/// If any command exits non-zero, execution stops and an error is returned.
///
/// Returns a list of successful command results.
pub fn run_short_lived_commands(
    config: &Config,
    artifact_dir: &ArtifactDir,
    skip_cmds: &[String],
) -> Result<Vec<CommandResult>, CommandError> {
    let mut results = Vec::new();

    for (name, def) in &config.commands {
        if def.kind != CommandKind::ShortLived {
            continue;
        }
        if skip_cmds.contains(name) {
            println!("SKIP ....... {name}");
            continue;
        }

        let result = execute_short_lived(name, def, &artifact_dir.logs)?;
        println!(
            "OK ......... {name} (exit {})",
            result.exit_code.unwrap_or(-1)
        );
        results.push(result);
    }

    Ok(results)
}

/// Execute a single short-lived command, capturing stdout and stderr to log files.
fn execute_short_lived(
    name: &str,
    def: &CommandDef,
    logs_dir: &Path,
) -> Result<CommandResult, CommandError> {
    println!("RUN ........ {name}: {}", def.cmd);

    let output = Command::new("sh")
        .arg("-c")
        .arg(&def.cmd)
        .output()
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
        return Err(CommandError::NonZeroExit {
            name: name.to_string(),
            exit_code,
            stderr_path: stderr_path.display().to_string(),
        });
    }

    Ok(CommandResult {
        name: name.to_string(),
        exit_code,
        stdout_path: stdout_path.display().to_string(),
        stderr_path: stderr_path.display().to_string(),
    })
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
pub fn spawn_long_lived_commands(
    config: &Config,
    artifact_dir: &ArtifactDir,
    skip_cmds: &[String],
) -> Result<Vec<TrackedProcess>, CommandError> {
    let mut tracked = Vec::new();

    for (name, def) in &config.commands {
        if def.kind != CommandKind::LongLived {
            continue;
        }
        if skip_cmds.contains(name) {
            println!("SKIP ....... {name} (long-lived)");
            continue;
        }

        println!("START ...... {name}: {}", def.cmd);

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
        if let Some(ref readiness_url) = def.readiness_url {
            println!("WAIT ....... {name}: polling {readiness_url}");
            if let Err(e) = poll_readiness(readiness_url, DEFAULT_READINESS_TIMEOUT) {
                // Readiness failed - tear down what we've started
                println!("FAIL ....... {name}: readiness check failed");
                teardown_processes(&mut tracked);
                return Err(CommandError::ReadinessFailed {
                    name: name.to_string(),
                    url: readiness_url.clone(),
                    message: e,
                });
            }
            println!("READY ...... {name}");
        }
    }

    Ok(tracked)
}

/// Poll a readiness URL until it responds with a success status or timeout.
fn poll_readiness(url: &str, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();

    while start.elapsed() < timeout {
        // Try a simple TCP connection check by parsing the URL and connecting
        if check_url(url) {
            return Ok(());
        }
        std::thread::sleep(READINESS_POLL_INTERVAL);
    }

    Err(format!(
        "timed out after {}s waiting for {url}",
        timeout.as_secs()
    ))
}

/// Check if a URL is reachable by attempting a simple HTTP GET via a spawned curl process.
fn check_url(url: &str) -> bool {
    Command::new("curl")
        .args(["-sf", "--max-time", "2", "-o", "/dev/null", url])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
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
pub fn teardown_processes(processes: &mut [TrackedProcess]) -> Vec<TeardownResult> {
    let mut results = Vec::new();

    for process in processes.iter_mut() {
        let result = teardown_single(process);
        results.push(result);
    }

    results
}

/// Tear down a single process with SIGTERM, waiting briefly for exit.
fn teardown_single(process: &mut TrackedProcess) -> TeardownResult {
    let name = process.name.clone();

    // Check if already exited
    match process.child.try_wait() {
        Ok(Some(status)) => {
            return TeardownResult {
                name,
                success: true,
                message: format!(
                    "already exited with code {}",
                    status.code().unwrap_or(-1)
                ),
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
        unsafe {
            libc::kill(process.child.id() as libc::pid_t, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = process.child.kill();
    }

    // Wait briefly for orderly shutdown
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match process.child.try_wait() {
            Ok(Some(status)) => {
                return TeardownResult {
                    name,
                    success: true,
                    message: format!(
                        "terminated with code {}",
                        status.code().unwrap_or(-1)
                    ),
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    // Force kill after timeout
                    let _ = process.child.kill();
                    let _ = process.child.wait();
                    return TeardownResult {
                        name,
                        success: false,
                        message: "did not exit after SIGTERM; force killed".to_string(),
                    };
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return TeardownResult {
                    name,
                    success: false,
                    message: format!("error waiting for process: {e}"),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CommandDef, CommandKind, Config, ProviderConfig};
    use crate::run::{ArtifactDir, RunId};
    use std::collections::BTreeMap;

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
        let mut map = BTreeMap::new();
        for (name, kind, cmd, readiness_url) in commands {
            map.insert(
                name.to_string(),
                CommandDef {
                    kind,
                    cmd: cmd.to_string(),
                    readiness_url: readiness_url.map(String::from),
                },
            );
        }
        Config {
            provider: ProviderConfig::default(),
            commands: map,
        }
    }

    #[test]
    fn successful_short_lived_command() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();

        let config = make_config(vec![("echo_test", CommandKind::ShortLived, "echo hello")]);

        let results = run_short_lived_commands(&config, &artifact_dir, &[]).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "echo_test");
        assert_eq!(results[0].exit_code, Some(0));

        // Verify stdout was captured
        let stdout = std::fs::read_to_string(&results[0].stdout_path).unwrap();
        assert_eq!(stdout.trim(), "hello");

        // Verify stderr file exists (empty is fine)
        assert!(std::path::Path::new(&results[0].stderr_path).exists());
    }

    #[test]
    fn failed_short_lived_command() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();

        let config = make_config(vec![("fail_cmd", CommandKind::ShortLived, "exit 42")]);

        let err = run_short_lived_commands(&config, &artifact_dir, &[]).unwrap_err();
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

    #[test]
    fn output_capture_to_log_files() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();

        let config = make_config(vec![(
            "mixed_output",
            CommandKind::ShortLived,
            "echo stdout_msg && echo stderr_msg >&2",
        )]);

        let results = run_short_lived_commands(&config, &artifact_dir, &[]).unwrap();
        assert_eq!(results.len(), 1);

        let stdout = std::fs::read_to_string(&results[0].stdout_path).unwrap();
        assert!(stdout.contains("stdout_msg"));

        let stderr = std::fs::read_to_string(&results[0].stderr_path).unwrap();
        assert!(stderr.contains("stderr_msg"));
    }

    #[test]
    fn long_lived_commands_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();

        let config = make_config(vec![
            ("setup", CommandKind::ShortLived, "echo setup"),
            ("server", CommandKind::LongLived, "echo server"),
        ]);

        let results = run_short_lived_commands(&config, &artifact_dir, &[]).unwrap();
        // Only the short-lived command should have run
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "setup");
    }

    #[test]
    fn skip_cmd_flag_excludes_command() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();

        let config = make_config(vec![
            ("migrate", CommandKind::ShortLived, "echo migrate"),
            ("seed", CommandKind::ShortLived, "echo seed"),
        ]);

        let results =
            run_short_lived_commands(&config, &artifact_dir, &["migrate".to_string()]).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "seed");
    }

    #[test]
    fn failed_command_stops_execution() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();

        // BTreeMap ordering: "a_first" comes before "b_second"
        let config = make_config(vec![
            ("a_first", CommandKind::ShortLived, "exit 1"),
            ("b_second", CommandKind::ShortLived, "echo should_not_run"),
        ]);

        let err = run_short_lived_commands(&config, &artifact_dir, &[]).unwrap_err();
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

    #[test]
    fn spawn_long_lived_captures_output() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();

        // Use a command that writes output then sleeps briefly
        let config = make_config(vec![(
            "worker",
            CommandKind::LongLived,
            "echo long_lived_output && sleep 60",
        )]);

        let mut tracked = spawn_long_lived_commands(&config, &artifact_dir, &[]).unwrap();
        assert_eq!(tracked.len(), 1);
        assert_eq!(tracked[0].name, "worker");

        // Give it a moment to write output
        std::thread::sleep(Duration::from_millis(200));

        // Verify log files exist
        assert!(std::path::Path::new(&tracked[0].stdout_path).exists());
        assert!(std::path::Path::new(&tracked[0].stderr_path).exists());

        // Verify output was captured
        let stdout = std::fs::read_to_string(&tracked[0].stdout_path).unwrap();
        assert!(
            stdout.contains("long_lived_output"),
            "stdout: {stdout}"
        );

        // Clean up
        teardown_processes(&mut tracked);
    }

    #[test]
    fn spawn_long_lived_skip() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();

        let config = make_config(vec![
            ("server", CommandKind::LongLived, "sleep 60"),
            ("worker", CommandKind::LongLived, "sleep 60"),
        ]);

        let mut tracked =
            spawn_long_lived_commands(&config, &artifact_dir, &["server".to_string()]).unwrap();
        assert_eq!(tracked.len(), 1);
        assert_eq!(tracked[0].name, "worker");

        teardown_processes(&mut tracked);
    }

    #[test]
    fn teardown_stops_running_processes() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();

        let config = make_config(vec![("sleeper", CommandKind::LongLived, "sleep 600")]);

        let mut tracked = spawn_long_lived_commands(&config, &artifact_dir, &[]).unwrap();
        assert_eq!(tracked.len(), 1);

        let results = teardown_processes(&mut tracked);
        assert_eq!(results.len(), 1);
        assert!(results[0].success, "teardown should succeed: {}", results[0].message);
    }

    #[test]
    fn detect_unexpected_exit() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();

        // Command that exits immediately
        let config = make_config(vec![("quick_exit", CommandKind::LongLived, "exit 1")]);

        let mut tracked = spawn_long_lived_commands(&config, &artifact_dir, &[]).unwrap();

        // Wait for it to exit
        std::thread::sleep(Duration::from_millis(200));

        let result = check_for_unexpected_exits(&mut tracked);
        assert!(result.is_some(), "should detect unexpected exit");
        let (name, exit_code) = result.unwrap();
        assert_eq!(name, "quick_exit");
        assert_eq!(exit_code, Some(1));
    }

    #[test]
    fn short_lived_not_spawned_as_long_lived() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();

        let config = make_config(vec![
            ("setup", CommandKind::ShortLived, "echo setup"),
            ("server", CommandKind::LongLived, "sleep 60"),
        ]);

        let mut tracked = spawn_long_lived_commands(&config, &artifact_dir, &[]).unwrap();
        // Only long-lived should be spawned
        assert_eq!(tracked.len(), 1);
        assert_eq!(tracked[0].name, "server");

        teardown_processes(&mut tracked);
    }
}
