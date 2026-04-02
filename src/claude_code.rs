use crate::config::Config;
use crate::provider::{AgentSession, BootstrapMessage, OutputChunk, ProviderError, StepMessage};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

/// ANSI color codes for verbose output.
mod color {
    // Label prefix — dim grey
    pub const DIM: &str = "\x1b[38;5;243m";
    // Content text — lighter grey
    pub const LIGHT: &str = "\x1b[38;5;250m";
    // Tool names — soft blue
    pub const TOOL: &str = "\x1b[38;5;111m";
    // Thinking — soft purple
    pub const THINKING: &str = "\x1b[38;5;183m";
    // Tool result — soft green
    pub const RESULT: &str = "\x1b[38;5;151m";
    // Message/prompt — soft yellow
    pub const PROMPT: &str = "\x1b[38;5;223m";
    // Launch command — soft cyan
    pub const CMD: &str = "\x1b[38;5;152m";
    // Separator — very dim
    pub const SEP: &str = "\x1b[38;5;238m";
    // Reset
    pub const RESET: &str = "\x1b[0m";
}

/// Claude Code CLI provider adapter.
///
/// Implements the `AgentSession` trait by driving a single long-lived `claude`
/// CLI subprocess. Messages are sent as JSON to stdin and responses are streamed
/// as JSON from stdout, eliminating per-step process spawn overhead.
pub struct ClaudeCodeAdapter {
    /// Path to the claude CLI binary.
    binary_path: PathBuf,
    /// Extra agent arguments from config.
    agent_args: Vec<String>,
    /// Artifact directory for transcript storage.
    #[allow(dead_code)]
    artifact_dir: PathBuf,
    /// Whether verbose output is enabled.
    verbose: bool,
    /// Bootstrap content to pass as --append-system-prompt at launch.
    bootstrap_content: Option<String>,
    /// The long-lived claude subprocess.
    child: Option<Child>,
    /// Stdin handle for sending messages.
    stdin: Option<ChildStdin>,
    /// Buffered reader for stdout.
    reader: Option<BufReader<std::process::ChildStdout>>,
}

/// Format a step message with metadata prefix for sending to the provider.
pub(crate) fn format_step_message(message: &StepMessage) -> String {
    format!(
        "[run_id={} session_id={} step={}/{} source={}]\n\n{}",
        message.run_id, message.session_id,
        message.step_id + 1, message.total_steps,
        message.source_file,
        message.instruction
    )
}

/// A single event from the Claude Code CLI stream-json output.
#[derive(Debug, Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    /// Present on "assistant" and "user" (tool_result) events.
    #[serde(default)]
    message: Option<AssistantMessage>,
    /// Present on "result" events — the final text result.
    #[serde(default)]
    result: Option<String>,
    /// Present on "error" events.
    #[serde(default)]
    error: Option<String>,
}

/// The message payload inside an "assistant" stream event.
#[derive(Debug, Deserialize)]
struct AssistantMessage {
    #[serde(default)]
    content: Vec<ContentBlock>,
}

/// A single content block inside an assistant message.
#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: Option<String>,
    /// Tool use fields
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    id: Option<String>,
    /// Thinking content
    #[serde(default)]
    thinking: Option<String>,
    /// Tool result content (string form)
    #[serde(default)]
    content: Option<serde_json::Value>,
    /// Tool use input arguments
    #[serde(default)]
    input: Option<serde_json::Value>,
    /// Tool result: links back to the tool_use id
    #[serde(default)]
    tool_use_id: Option<String>,
}

/// Format a user message as stream-json input for the Claude CLI.
fn format_stream_input(content: &str) -> String {
    // Escape the content for JSON embedding
    let escaped = serde_json::to_string(content).unwrap_or_else(|_| format!("\"{}\"", content));
    format!(
        r#"{{"type":"user","message":{{"role":"user","content":{}}}}}"#,
        escaped
    )
}

impl ClaudeCodeAdapter {
    /// Resolve the path to the `claude` binary.
    fn find_binary() -> Result<PathBuf, ProviderError> {
        which::which("claude").map_err(|e| {
            ProviderError::InitializationFailed(format!("claude CLI binary not found in PATH: {e}"))
        })
    }

    /// Spawn the claude process if not already running.
    fn ensure_started(&mut self) -> Result<(), ProviderError> {
        if self.child.is_some() {
            return Ok(());
        }

        tracing::info!("spawning long-lived claude process");

        let mut cmd = Command::new(&self.binary_path);
        cmd.arg("-p")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--no-session-persistence");

        for arg in &self.agent_args {
            cmd.arg(arg);
        }

        // Write bootstrap to a temp file and pass via --append-system-prompt-file
        // This avoids issues with multi-line content in CLI arguments.
        if let Some(ref bootstrap) = self.bootstrap_content {
            let prompt_path = self.artifact_dir.join("bootstrap_prompt.txt");
            std::fs::write(&prompt_path, bootstrap).map_err(|e| {
                ProviderError::StartFailed(format!("failed to write bootstrap prompt file: {e}"))
            })?;
            cmd.arg("--append-system-prompt-file").arg(&prompt_path);
        }

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if self.verbose {
            let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
            eprintln!("{}[verbose]{} {}launch:{} {} {}{}", color::DIM, color::RESET, color::DIM, color::RESET, color::CMD, args.join(" "), color::RESET);
            eprintln!("{}         binary: {}{}", color::DIM, cmd.get_program().to_string_lossy(), color::RESET);
        }

        let mut child = cmd.spawn().map_err(|e| {
            ProviderError::StartFailed(format!("failed to spawn claude CLI: {e}"))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            ProviderError::StartFailed("failed to capture stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ProviderError::StartFailed("failed to capture stdout".to_string())
        })?;

        self.child = Some(child);
        self.stdin = Some(stdin);
        self.reader = Some(BufReader::new(stdout));

        tracing::info!("claude-code long-lived process started");
        Ok(())
    }

    /// Send a message to the long-lived process and return a streaming iterator.
    fn send_message(
        &mut self,
        message: &str,
    ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>
    {
        self.ensure_started()?;

        let stdin = self.stdin.as_mut().ok_or_else(|| {
            ProviderError::SendFailed("session not started".to_string())
        })?;

        let input_line = format_stream_input(message);

        if self.verbose {
            eprintln!("{}[verbose]{} {}prompt ({} bytes):{}", color::DIM, color::RESET, color::DIM, message.len(), color::RESET);
            eprintln!("{}{}{}", color::PROMPT, message, color::RESET);
            eprintln!("{}───{}", color::SEP, color::RESET);
        }

        stdin
            .write_all(input_line.as_bytes())
            .map_err(|e| ProviderError::SendFailed(format!("failed to write to stdin: {e}")))?;
        stdin
            .write_all(b"\n")
            .map_err(|e| ProviderError::SendFailed(format!("failed to write newline: {e}")))?;
        stdin
            .flush()
            .map_err(|e| ProviderError::SendFailed(format!("failed to flush stdin: {e}")))?;

        let reader = self.reader.as_mut().ok_or_else(|| {
            ProviderError::SendFailed("stdout reader not available".to_string())
        })?;

        Ok(Box::new(StreamTurnIterator { reader, done: false, verbose: self.verbose }))
    }
}

impl AgentSession for ClaudeCodeAdapter {
    fn initialize(config: &Config, artifact_dir: &Path, verbose: bool) -> Result<Self, ProviderError>
    where
        Self: Sized,
    {
        tracing::info!(provider = "claude-code", "initializing provider");
        let binary_path = Self::find_binary()?;
        tracing::info!(
            binary = %binary_path.display(),
            "claude-code provider initialized"
        );

        Ok(Self {
            binary_path,
            agent_args: config.provider.agent_args.clone(),
            artifact_dir: artifact_dir.to_path_buf(),
            verbose,
            bootstrap_content: None,
            child: None,
            stdin: None,
            reader: None,
        })
    }

    fn start(&mut self) -> Result<(), ProviderError> {
        // Process is spawned lazily on first send_step, after bootstrap content is available.
        tracing::info!("claude-code session ready (process will launch on first message)");
        Ok(())
    }

    fn send_bootstrap(
        &mut self,
        message: BootstrapMessage,
    ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>
    {
        // Store bootstrap content — it will be passed as --append-system-prompt at launch.
        // If the process is already running, send as a regular message (shouldn't happen in normal flow).
        if self.child.is_some() {
            return self.send_message(&message.content);
        }
        self.bootstrap_content = Some(message.content);
        // Return an empty iterator since no API call is made
        Ok(Box::new(std::iter::once(Ok(OutputChunk::Done))))
    }

    fn send_step(
        &mut self,
        message: StepMessage,
    ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>
    {
        let formatted = format_step_message(&message);
        self.send_message(&formatted)
    }

    fn close(&mut self) -> Result<(), ProviderError> {
        tracing::info!("closing claude-code session");

        // Drop stdin to signal EOF — the process will exit
        self.stdin.take();
        self.reader.take();

        if let Some(mut child) = self.child.take() {
            match child.wait() {
                Ok(status) => {
                    tracing::info!(exit_status = %status, "claude process exited");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to wait for claude process");
                }
            }
        }

        Ok(())
    }
}

/// Iterator that reads one turn of streamed output from the long-lived process.
///
/// Reads JSONL from stdout, parsing each line into `OutputChunk` values.
/// Yields `OutputChunk::Done` when a `result` event is received (turn complete).
/// The process stays alive for the next message.
struct StreamTurnIterator<'a> {
    reader: &'a mut BufReader<std::process::ChildStdout>,
    done: bool,
    verbose: bool,
}

impl<'a> Iterator for StreamTurnIterator<'a> {
    type Item = Result<OutputChunk, ProviderError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => {
                    // EOF — process exited unexpectedly
                    self.done = true;
                    return Some(Err(ProviderError::SessionCrashed(
                        "claude process exited unexpectedly".to_string(),
                    )));
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    match serde_json::from_str::<StreamEvent>(trimmed) {
                        Ok(event) => match event.event_type.as_str() {
                            "assistant" => {
                                if let Some(msg) = &event.message {
                                    for block in &msg.content {
                                        match block.block_type.as_str() {
                                            "text" => {
                                                if let Some(text) = &block.text {
                                                    if !text.is_empty() {
                                                        return Some(Ok(OutputChunk::Text(
                                                            text.clone(),
                                                        )));
                                                    }
                                                }
                                            }
                                            "tool_use" => {
                                                if self.verbose {
                                                    let name = block.name.as_deref().unwrap_or("unknown");
                                                    let input_preview = block.input.as_ref().map(|v| {
                                                        // For Bash, show the command directly
                                                        if let Some(cmd) = v.get("command").and_then(|c| c.as_str()) {
                                                            format!("$ {cmd}")
                                                        } else if let Some(path) = v.get("file_path").and_then(|p| p.as_str()) {
                                                            path.to_string()
                                                        } else if let Some(pattern) = v.get("pattern").and_then(|p| p.as_str()) {
                                                            format!("/{pattern}/")
                                                        } else {
                                                            v.to_string()
                                                        }
                                                    }).unwrap_or_default();
                                                    let id_short = block.id.as_deref().unwrap_or("").chars().take(12).collect::<String>();
                                                    eprintln!("{}[verbose]{} {}tool:{} {}{}{} {}{}{} {}({}){}", color::DIM, color::RESET, color::DIM, color::RESET, color::TOOL, name, color::RESET, color::LIGHT, input_preview, color::RESET, color::DIM, id_short, color::RESET);
                                                }
                                            }
                                            "thinking" => {
                                                if self.verbose {
                                                    if let Some(thinking) = &block.thinking {
                                                        eprintln!("{}[verbose]{} {}thinking:{}", color::DIM, color::RESET, color::DIM, color::RESET);
                                                        eprintln!("{}{}{}", color::THINKING, thinking, color::RESET);
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                continue;
                            }
                            "user" => {
                                // Tool results — log in verbose mode
                                if self.verbose {
                                    if let Some(msg) = &event.message {
                                        for block in &msg.content {
                                            if block.block_type == "tool_result" {
                                                let result_text = block.content
                                                    .as_ref()
                                                    .map(|v| match v {
                                                        serde_json::Value::String(s) => s.clone(),
                                                        other => other.to_string(),
                                                    })
                                                    .or_else(|| block.text.clone())
                                                    .unwrap_or_default();
                                                let id_short = block.tool_use_id.as_deref().unwrap_or("").chars().take(12).collect::<String>();
                                                eprintln!("{}[verbose]{} {}result:{} {}({}){}", color::DIM, color::RESET, color::DIM, color::RESET, color::DIM, id_short, color::RESET);
                                                eprintln!("{}{}{}", color::RESULT, result_text, color::RESET);
                                            }
                                        }
                                    }
                                }
                                continue;
                            }
                            "error" => {
                                let msg = event
                                    .error
                                    .or(event.result)
                                    .unwrap_or_else(|| "unknown error".to_string());
                                return Some(Err(ProviderError::StreamError(msg)));
                            }
                            "result" => {
                                // Turn complete — text was already streamed via assistant events
                                self.done = true;
                                return Some(Ok(OutputChunk::Done));
                            }
                            // Skip system (inter-turn init), rate_limit_event, etc.
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
                step_timeout_secs: None,
                strict_warnings: None,
                base_url: None,
            },
            commands: BTreeMap::new(),
            checkpoint: None,
        }
    }

    #[test]
    fn send_message_fails_with_bad_binary() {
        let mut adapter = ClaudeCodeAdapter {
            binary_path: PathBuf::from("/nonexistent/claude"),
            agent_args: vec![],
            artifact_dir: PathBuf::from("/tmp/artifacts"),
            child: None,
            stdin: None,
            reader: None,
            verbose: false,
            bootstrap_content: None,
        };

        let result = adapter.send_message("hello");
        assert!(result.is_err(), "expected error with nonexistent binary");
    }

    #[test]
    fn close_cleans_up_handles() {
        let mut adapter = ClaudeCodeAdapter {
            binary_path: PathBuf::from("/usr/bin/claude"),
            agent_args: vec![],
            artifact_dir: PathBuf::from("/tmp/artifacts"),
            child: None,
            stdin: None,
            reader: None,
            verbose: false,
            bootstrap_content: None,
        };

        // Close with no process should succeed
        adapter.close().unwrap();
        assert!(adapter.child.is_none());
        assert!(adapter.stdin.is_none());
        assert!(adapter.reader.is_none());
    }

    #[test]
    fn parse_assistant_stream_event() {
        let json = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello, world!"}]}}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "assistant");
        let msg = event.message.unwrap();
        assert_eq!(msg.content[0].text.as_deref(), Some("Hello, world!"));
    }

    #[test]
    fn parse_error_stream_event() {
        let json = r#"{"type":"error","error":"rate limited"}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "error");
        assert_eq!(event.error.unwrap(), "rate limited");
    }

    #[test]
    fn parse_result_stream_event() {
        let json = r#"{"type":"result","result":"Final answer"}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "result");
        assert_eq!(event.result.unwrap(), "Final answer");
    }

    #[test]
    fn format_stream_input_escapes_json() {
        let input = format_stream_input("hello \"world\"");
        assert!(input.contains(r#""hello \"world\"""#));
        assert!(input.contains(r#""type":"user""#));
        assert!(input.contains(r#""role":"user""#));
    }

    #[test]
    fn format_stream_input_handles_newlines() {
        let input = format_stream_input("line1\nline2");
        // Should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&input).unwrap();
        assert_eq!(parsed["type"], "user");
        assert_eq!(parsed["message"]["role"], "user");
        assert_eq!(parsed["message"]["content"], "line1\nline2");
    }

    #[test]
    fn stream_turn_iterator_with_mock_process() {
        // Simulate stdout with assistant + result events
        let child = Command::new("sh")
            .arg("-c")
            .arg(r#"echo '{"type":"system","subtype":"init"}'; echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Hello"}]}}'; echo '{"type":"result","result":"Hello","subtype":"success"}';"#)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let stdout = child.stdout.unwrap();
        let mut reader = BufReader::new(stdout);

        // Skip system init
        let mut init_line = String::new();
        reader.read_line(&mut init_line).unwrap();

        let mut iter = StreamTurnIterator {
            reader: unsafe { &mut *(&mut reader as *mut BufReader<std::process::ChildStdout>) },
            done: false,
            verbose: false,
        };

        let mut collected = Vec::new();
        for item in &mut iter {
            match item {
                Ok(OutputChunk::Text(text)) => collected.push(text),
                Ok(OutputChunk::Done) => break,
                Err(e) => panic!("unexpected error: {e}"),
            }
        }

        assert_eq!(collected, vec!["Hello"]);
    }

    #[test]
    fn stream_turn_iterator_reports_error_events() {
        let child = Command::new("sh")
            .arg("-c")
            .arg(r#"echo '{"type":"error","error":"something went wrong"}'"#)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let stdout = child.stdout.unwrap();
        let mut reader = BufReader::new(stdout);

        let mut iter = StreamTurnIterator {
            reader: unsafe { &mut *(&mut reader as *mut BufReader<std::process::ChildStdout>) },
            done: false,
            verbose: false,
        };

        let result = iter.next();
        assert!(result.is_some());
        let item = result.unwrap();
        assert!(
            matches!(item, Err(ProviderError::StreamError(ref msg)) if msg == "something went wrong"),
            "expected StreamError, got: {item:?}"
        );
    }

    #[test]
    fn stream_turn_iterator_skips_non_json_lines() {
        let child = Command::new("sh")
            .arg("-c")
            .arg(r#"echo 'not json'; echo '{"type":"assistant","message":{"content":[{"type":"text","text":"text"}]}}'; echo '{"type":"result","result":"text","subtype":"success"}';"#)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let stdout = child.stdout.unwrap();
        let mut reader = BufReader::new(stdout);

        let mut iter = StreamTurnIterator {
            reader: unsafe { &mut *(&mut reader as *mut BufReader<std::process::ChildStdout>) },
            done: false,
            verbose: false,
        };

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
    fn initialize_finds_binary() {
        let config = test_config();
        let tmp = tempfile::tempdir().unwrap();

        let result = ClaudeCodeAdapter::initialize(&config, tmp.path(), false);
        match result {
            Ok(adapter) => {
                assert!(adapter.child.is_none()); // Not started yet
                assert_eq!(adapter.agent_args, vec!["--verbose"]);
            }
            Err(ProviderError::InitializationFailed(msg)) => {
                assert!(msg.contains("claude CLI binary not found"));
            }
            Err(e) => panic!("unexpected error type: {e:?}"),
        }
    }

    #[test]
    fn format_step_message_includes_metadata() {
        use crate::run::{RunId, SessionId};

        let msg = StepMessage {
            run_id: RunId("run-abc".to_string()),
            session_id: SessionId("sess-xyz".to_string()),
            step_id: 2,
            total_steps: 5,
            source_file: "tests/login.test.toml".to_string(),
            instruction: "Check the login page".to_string(),
        };
        let formatted = format_step_message(&msg);

        assert!(formatted.contains("run_id=run-abc"));
        assert!(formatted.contains("session_id=sess-xyz"));
        assert!(formatted.contains("step=3/5"));
        assert!(formatted.contains("source=tests/login.test.toml"));
        assert!(formatted.contains("Check the login page"));
        let meta_pos = formatted.find("[run_id=").unwrap();
        let instruction_pos = formatted.find("Check the login page").unwrap();
        assert!(meta_pos < instruction_pos);
    }
}
