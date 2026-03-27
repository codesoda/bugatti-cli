use crate::config::Config;
use crate::provider::{AgentSession, BootstrapMessage, OutputChunk, ProviderError, StepMessage};
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Claude Code CLI provider adapter.
///
/// Implements the `AgentSession` trait by driving the `claude` CLI subprocess.
/// Each message is sent as a separate `claude -p <message>` invocation that
/// shares the same `--session-id` for conversation continuity across the
/// entire test run.
pub struct ClaudeCodeAdapter {
    /// Path to the claude CLI binary.
    binary_path: PathBuf,
    /// Session ID for conversation continuity across messages.
    session_id: String,
    /// Extra agent arguments from config.
    agent_args: Vec<String>,
    /// Extra system prompt from config.
    extra_system_prompt: Option<String>,
    /// Artifact directory for transcript storage (used in future stories for transcript capture).
    #[allow(dead_code)]
    artifact_dir: PathBuf,
    /// Whether the session has been started.
    started: bool,
}

/// A single event from the Claude Code CLI stream-json output.
#[derive(Debug, Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

impl ClaudeCodeAdapter {
    /// Resolve the path to the `claude` binary.
    fn find_binary() -> Result<PathBuf, ProviderError> {
        which::which("claude").map_err(|e| {
            ProviderError::InitializationFailed(format!("claude CLI binary not found in PATH: {e}"))
        })
    }

    /// Build the command for sending a message to the Claude Code CLI.
    fn build_command(&self, message: &str) -> Command {
        let mut cmd = Command::new(&self.binary_path);
        cmd.arg("-p")
            .arg(message)
            .arg("--session-id")
            .arg(&self.session_id)
            .arg("--output-format")
            .arg("stream-json");

        if let Some(ref prompt) = self.extra_system_prompt {
            cmd.arg("--system-prompt").arg(prompt);
        }

        for arg in &self.agent_args {
            cmd.arg(arg);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd
    }

    /// Spawn a claude subprocess for a message and return a streaming iterator.
    fn send_message(
        &mut self,
        message: &str,
    ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>
    {
        if !self.started {
            return Err(ProviderError::SendFailed("session not started".to_string()));
        }

        let mut cmd = self.build_command(message);
        let child = cmd
            .spawn()
            .map_err(|e| ProviderError::SendFailed(format!("failed to spawn claude CLI: {e}")))?;

        Ok(Box::new(ClaudeCodeStreamIterator::new(child)))
    }
}

impl AgentSession for ClaudeCodeAdapter {
    fn initialize(config: &Config, artifact_dir: &Path) -> Result<Self, ProviderError>
    where
        Self: Sized,
    {
        let binary_path = Self::find_binary()?;
        let session_id = uuid::Uuid::new_v4().to_string();

        Ok(Self {
            binary_path,
            session_id,
            agent_args: config.provider.agent_args.clone(),
            extra_system_prompt: config.provider.extra_system_prompt.clone(),
            artifact_dir: artifact_dir.to_path_buf(),
            started: false,
        })
    }

    fn start(&mut self) -> Result<(), ProviderError> {
        self.started = true;
        Ok(())
    }

    fn send_bootstrap(
        &mut self,
        message: BootstrapMessage,
    ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>
    {
        self.send_message(&message.content)
    }

    fn send_step(
        &mut self,
        message: StepMessage,
    ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>
    {
        self.send_message(&message.instruction)
    }

    fn close(&mut self) -> Result<(), ProviderError> {
        self.started = false;
        Ok(())
    }
}

/// Iterator that streams output from a Claude Code CLI subprocess.
///
/// Reads JSONL (stream-json) from the child's stdout, parsing each line
/// into `OutputChunk` values. Yields `OutputChunk::Done` when the process
/// exits, and reports errors if the process fails.
struct ClaudeCodeStreamIterator {
    reader: BufReader<std::process::ChildStdout>,
    child: Child,
    done: bool,
}

impl ClaudeCodeStreamIterator {
    fn new(mut child: Child) -> Self {
        let stdout = child.stdout.take().expect("stdout was piped");
        Self {
            reader: BufReader::new(stdout),
            child,
            done: false,
        }
    }
}

impl Iterator for ClaudeCodeStreamIterator {
    type Item = Result<OutputChunk, ProviderError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => {
                    // EOF — process finished, check exit status
                    self.done = true;
                    match self.child.wait() {
                        Ok(status) if status.success() => {
                            return Some(Ok(OutputChunk::Done));
                        }
                        Ok(status) => {
                            // Collect stderr for the error message
                            let stderr_msg = self
                                .child
                                .stderr
                                .take()
                                .map(|s| {
                                    let mut buf = String::new();
                                    BufReader::new(s).read_to_string(&mut buf).ok();
                                    buf
                                })
                                .unwrap_or_default();
                            return Some(Err(ProviderError::SessionCrashed(format!(
                                "claude CLI exited with {status}: {stderr_msg}"
                            ))));
                        }
                        Err(e) => {
                            return Some(Err(ProviderError::SessionCrashed(format!(
                                "failed to wait for claude CLI: {e}"
                            ))));
                        }
                    }
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    match serde_json::from_str::<StreamEvent>(trimmed) {
                        Ok(event) => match event.event_type.as_str() {
                            "assistant" => {
                                if let Some(text) = event.content {
                                    if !text.is_empty() {
                                        return Some(Ok(OutputChunk::Text(text)));
                                    }
                                }
                                // Empty assistant text or no content — skip
                                continue;
                            }
                            "error" => {
                                let msg = event
                                    .message
                                    .or(event.content)
                                    .unwrap_or_else(|| "unknown error".to_string());
                                return Some(Err(ProviderError::StreamError(msg)));
                            }
                            "result" => {
                                if let Some(text) = event.content {
                                    if !text.is_empty() {
                                        return Some(Ok(OutputChunk::Text(text)));
                                    }
                                }
                                continue;
                            }
                            // Skip system, tool_use, and other event types
                            _ => continue,
                        },
                        Err(_) => {
                            // Non-JSON line; skip
                            continue;
                        }
                    }
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(ProviderError::StreamError(format!(
                        "failed to read from claude CLI stdout: {e}"
                    ))));
                }
            }
        }
    }
}

/// Use `std::io::Read::read_to_string` for reading stderr.
use std::io::Read;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ProviderConfig};
    use std::collections::BTreeMap;

    fn test_config() -> Config {
        Config {
            provider: ProviderConfig {
                name: "claude-code".to_string(),
                extra_system_prompt: Some("Be concise".to_string()),
                agent_args: vec!["--verbose".to_string()],
            },
            commands: BTreeMap::new(),
        }
    }

    #[test]
    fn build_command_includes_session_id_and_output_format() {
        let adapter = ClaudeCodeAdapter {
            binary_path: PathBuf::from("/usr/bin/claude"),
            session_id: "test-session-123".to_string(),
            agent_args: vec![],
            extra_system_prompt: None,
            artifact_dir: PathBuf::from("/tmp/artifacts"),
            started: true,
        };

        let cmd = adapter.build_command("hello world");
        let args: Vec<_> = cmd.get_args().collect();

        assert_eq!(cmd.get_program(), "/usr/bin/claude");
        assert!(args.contains(&std::ffi::OsStr::new("-p")));
        assert!(args.contains(&std::ffi::OsStr::new("hello world")));
        assert!(args.contains(&std::ffi::OsStr::new("--session-id")));
        assert!(args.contains(&std::ffi::OsStr::new("test-session-123")));
        assert!(args.contains(&std::ffi::OsStr::new("--output-format")));
        assert!(args.contains(&std::ffi::OsStr::new("stream-json")));
    }

    #[test]
    fn build_command_includes_system_prompt_when_configured() {
        let adapter = ClaudeCodeAdapter {
            binary_path: PathBuf::from("/usr/bin/claude"),
            session_id: "test-session".to_string(),
            agent_args: vec![],
            extra_system_prompt: Some("Be concise".to_string()),
            artifact_dir: PathBuf::from("/tmp/artifacts"),
            started: true,
        };

        let cmd = adapter.build_command("test");
        let args: Vec<_> = cmd.get_args().collect();

        assert!(args.contains(&std::ffi::OsStr::new("--system-prompt")));
        assert!(args.contains(&std::ffi::OsStr::new("Be concise")));
    }

    #[test]
    fn build_command_includes_agent_args() {
        let adapter = ClaudeCodeAdapter {
            binary_path: PathBuf::from("/usr/bin/claude"),
            session_id: "test-session".to_string(),
            agent_args: vec!["--model".to_string(), "opus".to_string()],
            extra_system_prompt: None,
            artifact_dir: PathBuf::from("/tmp/artifacts"),
            started: true,
        };

        let cmd = adapter.build_command("test");
        let args: Vec<_> = cmd.get_args().collect();

        assert!(args.contains(&std::ffi::OsStr::new("--model")));
        assert!(args.contains(&std::ffi::OsStr::new("opus")));
    }

    #[test]
    fn build_command_omits_system_prompt_when_none() {
        let adapter = ClaudeCodeAdapter {
            binary_path: PathBuf::from("/usr/bin/claude"),
            session_id: "test-session".to_string(),
            agent_args: vec![],
            extra_system_prompt: None,
            artifact_dir: PathBuf::from("/tmp/artifacts"),
            started: true,
        };

        let cmd = adapter.build_command("test");
        let args: Vec<_> = cmd.get_args().collect();

        assert!(!args.contains(&std::ffi::OsStr::new("--system-prompt")));
    }

    #[test]
    fn send_message_fails_before_start() {
        let mut adapter = ClaudeCodeAdapter {
            binary_path: PathBuf::from("/usr/bin/claude"),
            session_id: "test-session".to_string(),
            agent_args: vec![],
            extra_system_prompt: None,
            artifact_dir: PathBuf::from("/tmp/artifacts"),
            started: false,
        };

        let result = adapter.send_message("hello");
        match result {
            Err(ProviderError::SendFailed(msg)) => {
                assert!(msg.contains("session not started"));
            }
            Err(e) => panic!("expected SendFailed, got: {e:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn close_resets_started_flag() {
        let mut adapter = ClaudeCodeAdapter {
            binary_path: PathBuf::from("/usr/bin/claude"),
            session_id: "test-session".to_string(),
            agent_args: vec![],
            extra_system_prompt: None,
            artifact_dir: PathBuf::from("/tmp/artifacts"),
            started: true,
        };

        adapter.close().unwrap();
        assert!(!adapter.started);
    }

    #[test]
    fn parse_assistant_stream_event() {
        let json = r#"{"type":"assistant","content":"Hello, world!"}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "assistant");
        assert_eq!(event.content.unwrap(), "Hello, world!");
    }

    #[test]
    fn parse_error_stream_event() {
        let json = r#"{"type":"error","message":"rate limited"}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "error");
        assert_eq!(event.message.unwrap(), "rate limited");
    }

    #[test]
    fn parse_result_stream_event() {
        let json = r#"{"type":"result","content":"Final answer"}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "result");
        assert_eq!(event.content.unwrap(), "Final answer");
    }

    #[test]
    fn stream_iterator_with_mock_process() {
        // Test the iterator with a subprocess that echoes JSON lines
        let child = Command::new("sh")
            .arg("-c")
            .arg(r#"echo '{"type":"assistant","content":"Hello"}'; echo '{"type":"assistant","content":" world"}'; echo '{"type":"result","content":"Done"}';"#)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let mut iter = ClaudeCodeStreamIterator::new(child);
        let mut collected = Vec::new();

        for item in &mut iter {
            match item {
                Ok(OutputChunk::Text(text)) => collected.push(text),
                Ok(OutputChunk::Done) => break,
                Err(e) => panic!("unexpected error: {e}"),
            }
        }

        assert_eq!(collected, vec!["Hello", " world", "Done"]);
    }

    #[test]
    fn stream_iterator_handles_process_failure() {
        let child = Command::new("sh")
            .arg("-c")
            .arg("exit 1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let mut iter = ClaudeCodeStreamIterator::new(child);
        let result = iter.next();

        assert!(result.is_some());
        let item = result.unwrap();
        assert!(
            matches!(item, Err(ProviderError::SessionCrashed(_))),
            "expected SessionCrashed, got: {item:?}"
        );
    }

    #[test]
    fn stream_iterator_skips_non_json_lines() {
        let child = Command::new("sh")
            .arg("-c")
            .arg(r#"echo 'not json'; echo '{"type":"assistant","content":"text"}'; echo 'also not json'"#)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let mut iter = ClaudeCodeStreamIterator::new(child);
        let mut texts = Vec::new();

        for item in &mut iter {
            match item {
                Ok(OutputChunk::Text(t)) => texts.push(t),
                Ok(OutputChunk::Done) => break,
                Err(e) => panic!("unexpected error: {e}"),
            }
        }

        assert_eq!(texts, vec!["text"]);
    }

    #[test]
    fn stream_iterator_reports_error_events() {
        let child = Command::new("sh")
            .arg("-c")
            .arg(r#"echo '{"type":"error","message":"something went wrong"}'"#)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let mut iter = ClaudeCodeStreamIterator::new(child);
        let result = iter.next();

        assert!(result.is_some());
        let item = result.unwrap();
        assert!(
            matches!(item, Err(ProviderError::StreamError(ref msg)) if msg == "something went wrong"),
            "expected StreamError, got: {item:?}"
        );
    }

    #[test]
    fn initialize_fails_without_claude_binary() {
        // Use a config with a nonexistent provider — initialization should still
        // attempt to find the `claude` binary. If it's not in PATH, this fails.
        // We can't guarantee `claude` is NOT in PATH, so we test the struct directly.
        let config = test_config();
        let tmp = tempfile::tempdir().unwrap();

        // This test verifies the initialize path works when `claude` is in PATH.
        // If it's not in PATH, we verify the error message.
        let result = ClaudeCodeAdapter::initialize(&config, tmp.path());
        match result {
            Ok(adapter) => {
                assert!(!adapter.started);
                assert_eq!(adapter.agent_args, vec!["--verbose"]);
                assert_eq!(adapter.extra_system_prompt, Some("Be concise".to_string()));
            }
            Err(ProviderError::InitializationFailed(msg)) => {
                assert!(msg.contains("claude CLI binary not found"));
            }
            Err(e) => panic!("unexpected error type: {e:?}"),
        }
    }

    #[test]
    fn start_enables_messaging() {
        let mut adapter = ClaudeCodeAdapter {
            binary_path: PathBuf::from("/usr/bin/claude"),
            session_id: "test-session".to_string(),
            agent_args: vec![],
            extra_system_prompt: None,
            artifact_dir: PathBuf::from("/tmp/artifacts"),
            started: false,
        };

        assert!(!adapter.started);
        adapter.start().unwrap();
        assert!(adapter.started);
    }
}
