use crate::config::{CommandDef, CommandKind, Config};
use crate::run::ArtifactDir;
use std::path::Path;
use std::process::Command;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CommandDef, CommandKind, Config, ProviderConfig};
    use crate::run::{ArtifactDir, RunId};
    use std::collections::BTreeMap;

    fn make_config(commands: Vec<(&str, CommandKind, &str)>) -> Config {
        let mut map = BTreeMap::new();
        for (name, kind, cmd) in commands {
            map.insert(
                name.to_string(),
                CommandDef {
                    kind,
                    cmd: cmd.to_string(),
                    readiness_url: None,
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
}
