use crate::config::Config;
use crate::provider::{
    format_step_message, AgentSession, BootstrapMessage, OutputChunk, ProviderError, StepMessage,
};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};

/// OpenAI Codex CLI provider adapter.
///
/// Codex CLI is invoked one turn at a time via `codex exec --json`, then resumed
/// for subsequent steps with `codex exec resume <thread_id> --json`.
pub struct CodexAdapter {
    binary_path: PathBuf,
    agent_args: Vec<String>,
    artifact_dir: PathBuf,
    verbose: bool,
    thread_id: Option<String>,
    turn_index: usize,
}

#[derive(Debug, Deserialize)]
struct CodexEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

impl CodexAdapter {
    fn find_binary() -> Result<PathBuf, ProviderError> {
        which::which("codex").map_err(|e| {
            ProviderError::InitializationFailed(format!("codex CLI binary not found in PATH: {e}"))
        })
    }

    fn next_output_path(&mut self) -> PathBuf {
        let path = self
            .artifact_dir
            .join(format!("codex-last-message-{:03}.txt", self.turn_index));
        self.turn_index += 1;
        path
    }

    fn spawn_turn(
        &mut self,
        prompt: &str,
    ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>
    {
        let output_path = self.next_output_path();
        let requires_thread_id = self.thread_id.is_none();

        let mut cmd = Command::new(&self.binary_path);
        cmd.arg("exec");
        if let Some(thread_id) = self.thread_id.as_deref() {
            cmd.arg("resume").arg(thread_id);
        }
        cmd.arg("--json")
            .arg("--color")
            .arg("never")
            .arg("--skip-git-repo-check")
            .arg("-o")
            .arg(&output_path);

        for arg in &self.agent_args {
            cmd.arg(arg);
        }

        cmd.arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        if self.verbose {
            let args: Vec<_> = cmd
                .get_args()
                .map(|arg| arg.to_string_lossy().to_string())
                .collect();
            eprintln!(
                "\x1b[38;5;243m[verbose]\x1b[0m \x1b[38;5;243mlaunch:\x1b[0m \x1b[38;5;152m{} {}\x1b[0m",
                cmd.get_program().to_string_lossy(),
                args.join(" ")
            );
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| ProviderError::StartFailed(format!("failed to spawn codex CLI: {e}")))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::StartFailed("failed to capture stdin".to_string()))?;
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| ProviderError::SendFailed(format!("failed to write to stdin: {e}")))?;
        stdin
            .flush()
            .map_err(|e| ProviderError::SendFailed(format!("failed to flush stdin: {e}")))?;
        drop(stdin);

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::StartFailed("failed to capture stdout".to_string()))?;

        Ok(Box::new(CodexTurnIterator {
            child,
            reader: BufReader::new(stdout),
            output_path,
            thread_id: &mut self.thread_id,
            pending_output: None,
            pending_done: false,
            complete: false,
            latest_error: None,
            requires_thread_id,
        }))
    }
}

impl AgentSession for CodexAdapter {
    fn initialize(
        config: &Config,
        artifact_dir: &Path,
        verbose: bool,
    ) -> Result<Self, ProviderError>
    where
        Self: Sized,
    {
        let binary_path = Self::find_binary()?;
        Ok(Self {
            binary_path,
            agent_args: config.provider.agent_args.clone(),
            artifact_dir: artifact_dir.to_path_buf(),
            verbose,
            thread_id: None,
            turn_index: 0,
        })
    }

    fn start(&mut self) -> Result<(), ProviderError> {
        Ok(())
    }

    fn send_bootstrap(
        &mut self,
        message: BootstrapMessage,
    ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>
    {
        self.spawn_turn(&message.content)
    }

    fn send_step(
        &mut self,
        message: StepMessage,
    ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>
    {
        self.spawn_turn(&format_step_message(&message))
    }

    fn close(&mut self) -> Result<(), ProviderError> {
        Ok(())
    }
}

struct CodexTurnIterator<'a> {
    child: Child,
    reader: BufReader<ChildStdout>,
    output_path: PathBuf,
    thread_id: &'a mut Option<String>,
    pending_output: Option<String>,
    pending_done: bool,
    complete: bool,
    latest_error: Option<String>,
    requires_thread_id: bool,
}

impl<'a> Iterator for CodexTurnIterator<'a> {
    type Item = Result<OutputChunk, ProviderError>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(output) = self.pending_output.take() {
            self.pending_done = true;
            return Some(Ok(OutputChunk::Text(output)));
        }

        if self.pending_done {
            self.pending_done = false;
            self.complete = true;
            return Some(Ok(OutputChunk::Done));
        }

        if self.complete {
            return None;
        }

        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => {
                    let status = match self.child.wait() {
                        Ok(status) => status,
                        Err(e) => {
                            self.complete = true;
                            return Some(Err(ProviderError::ShutdownFailed(format!(
                                "failed to wait for codex CLI: {e}"
                            ))));
                        }
                    };

                    if self.requires_thread_id && self.thread_id.is_none() {
                        self.complete = true;
                        return Some(Err(ProviderError::StreamError(
                            "codex did not report a thread id for the new session".to_string(),
                        )));
                    }

                    if !status.success() {
                        self.complete = true;
                        let message = self.latest_error.clone().unwrap_or_else(|| {
                            format!("codex command exited with status {status}")
                        });
                        return Some(Err(ProviderError::StreamError(message)));
                    }

                    let output = match std::fs::read_to_string(&self.output_path) {
                        Ok(contents) => contents,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                        Err(e) => {
                            self.complete = true;
                            return Some(Err(ProviderError::StreamError(format!(
                                "failed to read codex output file '{}': {e}",
                                self.output_path.display()
                            ))));
                        }
                    };

                    if output.is_empty() {
                        self.complete = true;
                        return Some(Ok(OutputChunk::Done));
                    }

                    self.pending_output = Some(output);
                    return self.next();
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    let event = match serde_json::from_str::<CodexEvent>(trimmed) {
                        Ok(event) => event,
                        Err(_) => continue,
                    };

                    match event.event_type.as_str() {
                        "thread.started" => {
                            if let Some(thread_id) = event.thread_id {
                                *self.thread_id = Some(thread_id);
                            }
                        }
                        "error" => {
                            if let Some(message) = event.message {
                                self.latest_error = Some(message);
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    self.complete = true;
                    return Some(Err(ProviderError::StreamError(format!(
                        "failed to read from codex CLI stdout: {e}"
                    ))));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ProviderConfig};
    use crate::run::{RunId, SessionId};
    use indexmap::IndexMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn test_config() -> Config {
        Config {
            provider: ProviderConfig {
                name: "codex".to_string(),
                extra_system_prompt: Some("Be concise".to_string()),
                agent_args: vec!["--model".to_string(), "o3".to_string()],
                step_timeout_secs: None,
                strict_warnings: None,
                base_url: None,
            },
            commands: IndexMap::new(),
            checkpoint: None,
        }
    }

    fn write_fake_codex(dir: &Path) -> PathBuf {
        let script_path = dir.join("codex");
        fs::write(
            &script_path,
            r#"#!/bin/sh
output_file=""
mode=""
thread_id=""
while [ $# -gt 0 ]; do
  case "$1" in
    exec)
      mode="exec"
      shift
      ;;
    resume)
      mode="resume"
      thread_id="$2"
      shift 2
      ;;
    -o|--output-last-message)
      output_file="$2"
      shift 2
      ;;
    --color)
      shift 2
      ;;
    --json|--skip-git-repo-check|-|--model)
      shift
      ;;
    *)
      shift
      ;;
  esac
done

cat >/dev/null

if [ "$mode" = "exec" ]; then
  printf '{"type":"thread.started","thread_id":"thread-123"}\n'
  printf '{"type":"turn.started"}\n'
  printf 'Bootstrap acknowledged.\n' > "$output_file"
  exit 0
fi

if [ "$mode" = "resume" ] && [ "$thread_id" = "thread-123" ]; then
  printf '{"type":"turn.started"}\n'
  printf 'RESULT OK\n' > "$output_file"
  exit 0
fi

printf '{"type":"error","message":"unexpected resume thread id"}\n'
exit 1
"#,
        )
        .unwrap();

        let mut permissions = fs::metadata(&script_path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).unwrap();
        script_path
    }

    fn collect_output(
        stream: Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>,
    ) -> Result<String, ProviderError> {
        let mut output = String::new();
        for item in stream {
            match item? {
                OutputChunk::Text(text) => output.push_str(&text),
                OutputChunk::Done => break,
            }
        }
        Ok(output)
    }

    #[test]
    fn send_bootstrap_starts_new_codex_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let artifact_dir = tmp.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let mut adapter = CodexAdapter {
            binary_path: write_fake_codex(tmp.path()),
            agent_args: vec!["--model".to_string(), "o3".to_string()],
            artifact_dir,
            verbose: false,
            thread_id: None,
            turn_index: 0,
        };

        let output = collect_output(
            adapter
                .send_bootstrap(BootstrapMessage {
                    run_id: RunId("run-123".to_string()),
                    session_id: SessionId("sess-123".to_string()),
                    content: "Bootstrap prompt".to_string(),
                })
                .unwrap(),
        )
        .unwrap();

        assert_eq!(adapter.thread_id.as_deref(), Some("thread-123"));
        assert_eq!(output, "Bootstrap acknowledged.\n");
    }

    #[test]
    fn send_step_resumes_existing_codex_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let artifact_dir = tmp.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();

        let mut adapter = CodexAdapter {
            binary_path: write_fake_codex(tmp.path()),
            agent_args: Vec::new(),
            artifact_dir,
            verbose: false,
            thread_id: Some("thread-123".to_string()),
            turn_index: 0,
        };

        let output = collect_output(
            adapter
                .send_step(StepMessage {
                    run_id: RunId("run-456".to_string()),
                    session_id: SessionId("sess-456".to_string()),
                    step_id: 0,
                    total_steps: 1,
                    source_file: "tests/flow.test.toml".to_string(),
                    instruction: "Verify the login form works".to_string(),
                })
                .unwrap(),
        )
        .unwrap();

        assert_eq!(output, "RESULT OK\n");
        assert_eq!(adapter.thread_id.as_deref(), Some("thread-123"));
    }

    #[test]
    fn initialize_finds_binary() {
        let config = test_config();
        let tmp = tempfile::tempdir().unwrap();

        let result = CodexAdapter::initialize(&config, tmp.path(), false);
        match result {
            Ok(adapter) => {
                assert_eq!(adapter.agent_args, vec!["--model", "o3"]);
                assert!(adapter.thread_id.is_none());
            }
            Err(ProviderError::InitializationFailed(msg)) => {
                assert!(msg.contains("codex CLI binary not found"));
            }
            Err(e) => panic!("unexpected error type: {e:?}"),
        }
    }
}
