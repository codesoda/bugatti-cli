use crate::config::Config;
use crate::provider::{
    format_step_message, AgentSession, BootstrapMessage, OutputChunk, OutputStream, ProviderError,
    StepMessage, VecOutputStream,
};
use async_trait::async_trait;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};

/// Compact base system prompt for the driving `pi` agent.
///
/// Kept deliberately small so the harness bootstrap appended after it (via
/// `--append-system-prompt`) stays prominent and its RESULT-marker protocol is
/// reliably followed. Replacing pi's verbose default coding-assistant prompt
/// does not remove tool access — tools are registered independently of the
/// prompt text.
const PI_BASE_SYSTEM_PROMPT: &str = "You are an expert coding assistant operating inside pi, driven by the Bugatti automated test harness. You can read files, run shell commands, edit code, and use any other available tools to carry out each step. Follow the harness protocol in the instructions appended below exactly — in particular, emit the required RESULT marker as the final line of every response.";

/// Pi CLI provider adapter.
///
/// Pi is invoked one turn at a time via `pi -p --mode json`. Conversation
/// continuity across steps is preserved with a fixed `--session-id` plus a
/// per-run `--session-dir`, so each turn resumes the same session that the
/// previous turn created. The harness bootstrap (test instructions, extra
/// system prompt) is passed on every turn via a compact base `--system-prompt`
/// plus the bootstrap content appended inline with `--append-system-prompt`.
///
/// Why both flags: `pi` only reliably honors `--append-system-prompt` when an
/// explicit base `--system-prompt` is also set. Appended onto pi's large
/// default coding-assistant prompt, the harness RESULT-marker protocol gets
/// buried and the model silently drops it (see issue #47). Supplying a small
/// purpose-built base prompt keeps the appended protocol prominent. The
/// bootstrap is also passed as its literal *content* (not a file path) so it
/// works regardless of whether the installed `pi` build auto-reads path args.
pub struct PiAdapter {
    /// Path to the `pi` CLI binary.
    binary_path: PathBuf,
    /// Extra agent arguments from config.
    agent_args: Vec<String>,
    /// Artifact directory for transcript/log storage.
    artifact_dir: PathBuf,
    /// Directory used to persist the pi session between turns.
    session_dir: PathBuf,
    /// Stable session id reused across every turn.
    session_id: String,
    /// Whether verbose output is enabled.
    verbose: bool,
    /// Bootstrap content passed inline via `--append-system-prompt` on each turn.
    bootstrap_content: Option<String>,
    /// Path to the persisted bootstrap prompt file (written lazily).
    bootstrap_path: Option<PathBuf>,
    /// Number of turns spawned so far.
    turn_index: usize,
}

/// A single event from the Pi CLI `--mode json` output stream.
#[derive(Debug, Deserialize)]
struct PiEvent {
    #[serde(rename = "type")]
    event_type: String,
    /// Present on "message_update" events — incremental assistant output.
    #[serde(default, rename = "assistantMessageEvent")]
    assistant_event: Option<AssistantEvent>,
    /// Present on "turn_end" / "message_end" / "agent_end"-adjacent events.
    #[serde(default)]
    message: Option<PiMessage>,
}

/// The incremental assistant event payload inside a "message_update".
#[derive(Debug, Deserialize)]
struct AssistantEvent {
    #[serde(rename = "type")]
    kind: String,
    /// Streamed text delta (present on "text_delta").
    #[serde(default)]
    delta: Option<String>,
    /// Completed tool call (present on "toolcall_end").
    #[serde(default, rename = "toolCall")]
    tool_call: Option<ToolCall>,
}

/// A completed tool call.
#[derive(Debug, Deserialize)]
struct ToolCall {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<serde_json::Value>,
}

/// An assistant/user message envelope. Used to surface provider errors.
#[derive(Debug, Deserialize)]
struct PiMessage {
    #[serde(default, rename = "stopReason")]
    stop_reason: Option<String>,
    #[serde(default, rename = "errorMessage")]
    error_message: Option<String>,
}

impl PiAdapter {
    /// Resolve the path to the `pi` binary.
    fn find_binary() -> Result<PathBuf, ProviderError> {
        which::which("pi").map_err(|e| {
            ProviderError::InitializationFailed(format!("pi CLI binary not found in PATH: {e}"))
        })
    }

    /// Write the bootstrap prompt file once and return its path, if any.
    fn ensure_bootstrap_file(&mut self) -> Result<Option<PathBuf>, ProviderError> {
        if self.bootstrap_path.is_some() {
            return Ok(self.bootstrap_path.clone());
        }
        let Some(content) = self.bootstrap_content.clone() else {
            return Ok(None);
        };
        let path = self.artifact_dir.join("pi_bootstrap_prompt.txt");
        std::fs::write(&path, content).map_err(|e| {
            ProviderError::StartFailed(format!("failed to write pi bootstrap prompt file: {e}"))
        })?;
        self.bootstrap_path = Some(path.clone());
        Ok(Some(path))
    }

    /// Spawn a single pi turn, sending `prompt` on stdin and streaming output.
    async fn spawn_turn(
        &mut self,
        prompt: &str,
    ) -> Result<Box<dyn OutputStream + '_>, ProviderError> {
        // Persist the bootstrap as an artifact for debugging. The return value
        // is intentionally unused for launching: pi receives the bootstrap as
        // inline content below, not as a path argument.
        let _bootstrap_path = self.ensure_bootstrap_file()?;

        let mut cmd = Command::new(&self.binary_path);
        // If the stream is abandoned mid-turn (e.g. step timeout or Ctrl+C),
        // make sure the pi subprocess doesn't linger.
        cmd.kill_on_drop(true);
        cmd.arg("--print")
            .arg("--mode")
            .arg("json")
            .arg("--session-id")
            .arg(&self.session_id)
            .arg("--session-dir")
            .arg(&self.session_dir);

        if let Some(ref content) = self.bootstrap_content {
            // Set a compact base system prompt so the appended bootstrap (and
            // its RESULT-marker protocol) is actually honored, then append the
            // bootstrap *content* inline. See the type-level docs for the
            // rationale (issue #47).
            cmd.arg("--system-prompt").arg(PI_BASE_SYSTEM_PROMPT);
            cmd.arg("--append-system-prompt").arg(content);
        }

        for arg in &self.agent_args {
            cmd.arg(arg);
        }

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        if self.verbose {
            let args: Vec<_> = cmd
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().to_string())
                .collect();
            eprintln!(
                "\x1b[38;5;243m[verbose]\x1b[0m \x1b[38;5;243mlaunch:\x1b[0m \x1b[38;5;152m{} {}\x1b[0m",
                cmd.as_std().get_program().to_string_lossy(),
                args.join(" ")
            );
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| ProviderError::StartFailed(format!("failed to spawn pi CLI: {e}")))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::StartFailed("failed to capture stdin".to_string()))?;
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|e| ProviderError::SendFailed(format!("failed to write to stdin: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| ProviderError::SendFailed(format!("failed to flush stdin: {e}")))?;
        drop(stdin);

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::StartFailed("failed to capture stdout".to_string()))?;

        self.turn_index += 1;

        Ok(Box::new(PiTurnStream {
            child,
            reader: BufReader::new(stdout),
            verbose: self.verbose,
            done: false,
            latest_error: None,
        }))
    }
}

#[async_trait]
impl AgentSession for PiAdapter {
    fn initialize(
        config: &Config,
        artifact_dir: &Path,
        verbose: bool,
    ) -> Result<Self, ProviderError>
    where
        Self: Sized,
    {
        tracing::info!(provider = "pi", "initializing provider");
        let binary_path = Self::find_binary()?;

        let session_dir = artifact_dir.join("pi-session");
        std::fs::create_dir_all(&session_dir).map_err(|e| {
            ProviderError::InitializationFailed(format!(
                "failed to create pi session dir '{}': {e}",
                session_dir.display()
            ))
        })?;

        let session_id = uuid::Uuid::new_v4().to_string();
        tracing::info!(
            binary = %binary_path.display(),
            session_id = %session_id,
            "pi provider initialized"
        );

        Ok(Self {
            binary_path,
            agent_args: config.provider.agent_args.clone(),
            artifact_dir: artifact_dir.to_path_buf(),
            session_dir,
            session_id,
            verbose,
            bootstrap_content: None,
            bootstrap_path: None,
            turn_index: 0,
        })
    }

    async fn start(&mut self) -> Result<(), ProviderError> {
        // Pi is spawned lazily per turn; nothing to start up front.
        Ok(())
    }

    async fn send_bootstrap(
        &mut self,
        message: BootstrapMessage,
    ) -> Result<Box<dyn OutputStream + '_>, ProviderError> {
        // Store bootstrap content — it is passed inline as --append-system-prompt on
        // every turn. No model call is made here.
        self.bootstrap_content = Some(message.content);
        Ok(Box::new(VecOutputStream::done()))
    }

    async fn send_step(
        &mut self,
        message: StepMessage,
    ) -> Result<Box<dyn OutputStream + '_>, ProviderError> {
        self.spawn_turn(&format_step_message(&message)).await
    }

    async fn close(&mut self) -> Result<(), ProviderError> {
        // Each turn is a short-lived process that has already exited; nothing
        // long-lived to tear down.
        tracing::info!("closing pi session");
        Ok(())
    }
}

/// Stream that reads one turn of streamed output from a pi subprocess.
///
/// Parses JSONL from stdout, emitting `OutputChunk::Text` for streamed text
/// deltas and `OutputChunk::Done` once the turn finishes. Provider errors are
/// surfaced via the `errorMessage`/`stopReason` fields on turn/message events.
/// The subprocess is spawned with `kill_on_drop`, so abandoning the stream
/// mid-turn (step timeout, Ctrl+C) cannot leak a lingering pi process.
struct PiTurnStream {
    child: Child,
    reader: BufReader<ChildStdout>,
    verbose: bool,
    done: bool,
    latest_error: Option<String>,
}

/// How long to let the pi subprocess exit on its own after the turn has
/// logically completed, before force-killing it.
const TURN_EXIT_GRACE: Duration = Duration::from_secs(2);

impl PiTurnStream {
    /// Reap the turn subprocess after the turn has logically completed.
    ///
    /// pi 0.80.x keeps the `--print` process alive after emitting the terminal
    /// `agent_end` event (issue #48), so waiting without a bound here hangs
    /// until the step timeout fires. The turn is semantically finished once
    /// `agent_end` arrives (the session transcript is already flushed), so
    /// give the process a short grace period to exit cleanly, then kill it.
    ///
    /// Returns the exit status if the process exited on its own, or `None` if
    /// it had to be killed.
    async fn reap_child(&mut self) -> Option<ExitStatus> {
        if let Ok(Ok(status)) = tokio::time::timeout(TURN_EXIT_GRACE, self.child.wait()).await {
            return Some(status);
        }
        tracing::debug!("pi turn process still alive after turn completion; killing it");
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        None
    }
}

#[async_trait]
impl OutputStream for PiTurnStream {
    async fn next_chunk(&mut self) -> Option<Result<OutputChunk, ProviderError>> {
        if self.done {
            return None;
        }

        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line).await {
                Ok(0) => {
                    // EOF — the turn's output stream is finished. The process
                    // has usually exited, but don't block forever if it hasn't
                    // (pi 0.80.x can keep running after closing stdout).
                    self.done = true;

                    let status = self.reap_child().await;

                    if let Some(message) = self.latest_error.take() {
                        return Some(Err(ProviderError::StreamError(message)));
                    }

                    if let Some(status) = status {
                        if !status.success() {
                            return Some(Err(ProviderError::StreamError(format!(
                                "pi command exited with status {status}"
                            ))));
                        }
                    }

                    return Some(Ok(OutputChunk::Done));
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    let event = match serde_json::from_str::<PiEvent>(trimmed) {
                        Ok(event) => event,
                        Err(_) => continue,
                    };

                    // Capture provider errors from any message-bearing event.
                    if let Some(message) = &event.message {
                        if message.stop_reason.as_deref() == Some("error") {
                            if let Some(err) = &message.error_message {
                                self.latest_error = Some(err.clone());
                            }
                        }
                    }

                    match event.event_type.as_str() {
                        "message_update" => {
                            if let Some(ev) = &event.assistant_event {
                                match ev.kind.as_str() {
                                    "text_delta" => {
                                        if let Some(delta) = &ev.delta {
                                            if !delta.is_empty() {
                                                return Some(Ok(OutputChunk::Text(delta.clone())));
                                            }
                                        }
                                    }
                                    "toolcall_end" if self.verbose => {
                                        if let Some(tc) = &ev.tool_call {
                                            let name = tc.name.as_deref().unwrap_or("unknown");
                                            let args_preview = tc
                                                .arguments
                                                .as_ref()
                                                .map(|v| {
                                                    if let Some(cmd) =
                                                        v.get("command").and_then(|c| c.as_str())
                                                    {
                                                        format!("$ {cmd}")
                                                    } else if let Some(path) =
                                                        v.get("path").and_then(|p| p.as_str())
                                                    {
                                                        path.to_string()
                                                    } else {
                                                        v.to_string()
                                                    }
                                                })
                                                .unwrap_or_default();
                                            eprintln!(
                                                "\x1b[38;5;243m[verbose]\x1b[0m \x1b[38;5;243mtool:\x1b[0m \x1b[38;5;111m{name}\x1b[0m \x1b[38;5;250m{args_preview}\x1b[0m"
                                            );
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            continue;
                        }
                        "agent_end" => {
                            // Terminal event: the turn is complete. Do NOT
                            // block on `wait()` here — pi 0.80.x keeps the
                            // --print process alive after emitting agent_end,
                            // which previously stalled every step until the
                            // step timeout (issue #48).
                            self.done = true;
                            if let Some(message) = self.latest_error.take() {
                                return Some(Err(ProviderError::StreamError(message)));
                            }
                            let _ = self.reap_child().await;
                            return Some(Ok(OutputChunk::Done));
                        }
                        _ => continue,
                    }
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(ProviderError::StreamError(format!(
                        "failed to read from pi CLI stdout: {e}"
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
    use std::time::Instant;

    fn test_config() -> Config {
        Config {
            provider: ProviderConfig {
                name: "pi".to_string(),
                extra_system_prompt: Some("Be concise".to_string()),
                agent_args: vec![
                    "--model".to_string(),
                    "anthropic/claude-sonnet-4-5".to_string(),
                ],
                step_timeout_secs: None,
                strict_warnings: None,
                base_url: None,
            },
            commands: IndexMap::new(),
            checkpoint: None,
        }
    }

    /// Write a fake `pi` binary that emits a deterministic JSON event stream.
    fn write_fake_pi(dir: &Path) -> PathBuf {
        let script_path = dir.join("pi");
        fs::write(
            &script_path,
            r#"#!/bin/sh
# Drain stdin (the prompt) so the writer doesn't get SIGPIPE.
cat >/dev/null
printf '{"type":"session","id":"abc"}\n'
printf '{"type":"agent_start"}\n'
printf '{"type":"turn_start"}\n'
printf '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"RESULT"}}\n'
printf '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":": OK"}}\n'
printf '{"type":"agent_end"}\n'
exit 0
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
        script_path
    }

    /// Write a fake `pi` binary that records its argv to `pi_args.txt` (one
    /// base64-encoded argument per line, so arguments containing newlines stay
    /// intact) before emitting a deterministic success stream.
    fn write_arg_recording_pi(dir: &Path, args_out: &Path) -> PathBuf {
        let script_path = dir.join("pi");
        fs::write(
            &script_path,
            format!(
                r#"#!/bin/sh
: >"{out}"
for a in "$@"; do
  printf '%s' "$a" | base64 | tr -d '\n' >>"{out}"
  printf '\n' >>"{out}"
done
cat >/dev/null
printf '{{"type":"message_update","assistantMessageEvent":{{"type":"text_delta","delta":"RESULT OK"}}}}\n'
printf '{{"type":"agent_end"}}\n'
exit 0
"#,
                out = args_out.display()
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
        script_path
    }

    /// Write a fake `pi` binary that emits `agent_end` but then never exits,
    /// mimicking pi 0.80.x's lingering `--print` process (issue #48).
    fn write_lingering_pi(dir: &Path) -> PathBuf {
        let script_path = dir.join("pi");
        fs::write(
            &script_path,
            r#"#!/bin/sh
cat >/dev/null
printf '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"RESULT OK"}}\n'
printf '{"type":"agent_end"}\n'
# Simulate pi 0.80.x: the process stays alive after the turn completes.
sleep 300
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
        script_path
    }

    /// Write a fake `pi` binary that emits an error event.
    fn write_failing_pi(dir: &Path) -> PathBuf {
        let script_path = dir.join("pi");
        fs::write(
            &script_path,
            r#"#!/bin/sh
cat >/dev/null
printf '{"type":"turn_end","message":{"stopReason":"error","errorMessage":"model not found"}}\n'
printf '{"type":"agent_end"}\n'
exit 0
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
        script_path
    }

    fn make_adapter(binary: PathBuf, artifact_dir: PathBuf) -> PiAdapter {
        let session_dir = artifact_dir.join("pi-session");
        fs::create_dir_all(&session_dir).unwrap();
        PiAdapter {
            binary_path: binary,
            agent_args: Vec::new(),
            artifact_dir,
            session_dir,
            session_id: "test-session".to_string(),
            verbose: false,
            bootstrap_content: None,
            bootstrap_path: None,
            turn_index: 0,
        }
    }

    async fn collect_output(
        mut stream: Box<dyn OutputStream + '_>,
    ) -> Result<String, ProviderError> {
        let mut output = String::new();
        while let Some(item) = stream.next_chunk().await {
            match item? {
                OutputChunk::Text(text) => output.push_str(&text),
                OutputChunk::Done => break,
            }
        }
        Ok(output)
    }

    #[tokio::test]
    async fn send_step_streams_text_deltas() {
        let tmp = tempfile::tempdir().unwrap();
        let artifact_dir = tmp.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();
        let mut adapter = make_adapter(write_fake_pi(tmp.path()), artifact_dir);

        let output = collect_output(
            adapter
                .send_step(StepMessage {
                    run_id: RunId("run-1".to_string()),
                    session_id: SessionId("sess-1".to_string()),
                    step_id: 0,
                    total_steps: 1,
                    source_file: "tests/login.test.toml".to_string(),
                    instruction: "Verify the login form works".to_string(),
                })
                .await
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(output, "RESULT: OK");
        assert_eq!(adapter.turn_index, 1);
    }

    #[tokio::test]
    async fn send_step_completes_when_pi_process_lingers_after_agent_end() {
        // Regression test for issue #48: pi 0.80.x keeps the --print process
        // alive after emitting agent_end. The adapter must still deliver Done
        // promptly instead of blocking in wait() until the step timeout.
        let tmp = tempfile::tempdir().unwrap();
        let artifact_dir = tmp.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();
        let mut adapter = make_adapter(write_lingering_pi(tmp.path()), artifact_dir);

        let start = Instant::now();
        let output = collect_output(
            adapter
                .send_step(StepMessage {
                    run_id: RunId("run-1".to_string()),
                    session_id: SessionId("sess-1".to_string()),
                    step_id: 0,
                    total_steps: 1,
                    source_file: "tests/login.test.toml".to_string(),
                    instruction: "Verify the login form works".to_string(),
                })
                .await
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(output, "RESULT OK");
        // Grace period is 2s; anything close to the fake's 300s sleep means
        // we blocked on the lingering process again.
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "send_step took {:?}, adapter blocked on lingering pi process",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn send_bootstrap_stores_content_without_calling_model() {
        let tmp = tempfile::tempdir().unwrap();
        let artifact_dir = tmp.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();
        let mut adapter = make_adapter(write_fake_pi(tmp.path()), artifact_dir);

        let output = collect_output(
            adapter
                .send_bootstrap(BootstrapMessage {
                    run_id: RunId("run-1".to_string()),
                    session_id: SessionId("sess-1".to_string()),
                    content: "Harness instructions".to_string(),
                })
                .await
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(output, "");
        assert_eq!(
            adapter.bootstrap_content.as_deref(),
            Some("Harness instructions")
        );
        assert_eq!(adapter.turn_index, 0);
    }

    #[tokio::test]
    async fn bootstrap_file_written_on_first_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let artifact_dir = tmp.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();
        let mut adapter = make_adapter(write_fake_pi(tmp.path()), artifact_dir.clone());

        adapter.bootstrap_content = Some("system prompt content".to_string());
        let _ = collect_output(
            adapter
                .send_step(StepMessage {
                    run_id: RunId("run-1".to_string()),
                    session_id: SessionId("sess-1".to_string()),
                    step_id: 0,
                    total_steps: 1,
                    source_file: "tests/x.test.toml".to_string(),
                    instruction: "do it".to_string(),
                })
                .await
                .unwrap(),
        )
        .await
        .unwrap();

        let bootstrap_file = artifact_dir.join("pi_bootstrap_prompt.txt");
        assert!(bootstrap_file.exists());
        assert_eq!(
            fs::read_to_string(&bootstrap_file).unwrap(),
            "system prompt content"
        );
    }

    #[tokio::test]
    async fn bootstrap_passed_as_content_with_base_system_prompt() {
        // Regression for issue #47: pi must receive the bootstrap *content*
        // (carrying the RESULT-marker protocol) via --append-system-prompt,
        // alongside an explicit base --system-prompt so the appended protocol
        // is actually honored. Passing a file *path* (or appending onto pi's
        // default prompt) silently drops the protocol.
        let tmp = tempfile::tempdir().unwrap();
        let artifact_dir = tmp.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();
        let args_out = tmp.path().join("pi_args.txt");
        let mut adapter = make_adapter(write_arg_recording_pi(tmp.path(), &args_out), artifact_dir);

        let bootstrap = "## Result Contract\nEmit `RESULT OK` as the final line.";
        adapter.bootstrap_content = Some(bootstrap.to_string());

        let _ = collect_output(
            adapter
                .send_step(StepMessage {
                    run_id: RunId("run-1".to_string()),
                    session_id: SessionId("sess-1".to_string()),
                    step_id: 0,
                    total_steps: 1,
                    source_file: "tests/x.test.toml".to_string(),
                    instruction: "do it".to_string(),
                })
                .await
                .unwrap(),
        )
        .await
        .unwrap();

        let recorded = fs::read_to_string(&args_out).unwrap();
        let args: Vec<String> = recorded
            .lines()
            .map(|line| {
                let decoded = std::process::Command::new("base64")
                    .arg("-d")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .and_then(|mut child| {
                        use std::io::Write as _;
                        child
                            .stdin
                            .take()
                            .unwrap()
                            .write_all(line.as_bytes())
                            .unwrap();
                        child.wait_with_output()
                    })
                    .unwrap();
                String::from_utf8(decoded.stdout).unwrap()
            })
            .collect();
        let args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        // A base system prompt is supplied.
        let sys_idx = args
            .iter()
            .position(|a| *a == "--system-prompt")
            .expect("--system-prompt must be passed");
        assert_eq!(args[sys_idx + 1], PI_BASE_SYSTEM_PROMPT);

        // The bootstrap is appended as literal content, not a file path.
        let append_idx = args
            .iter()
            .position(|a| *a == "--append-system-prompt")
            .expect("--append-system-prompt must be passed");
        assert_eq!(args[append_idx + 1], bootstrap);
        assert!(
            !args.iter().any(|a| a.ends_with("pi_bootstrap_prompt.txt")),
            "bootstrap must be passed as content, not a file path: {args:?}"
        );
    }

    #[tokio::test]
    async fn error_event_surfaces_stream_error() {
        let tmp = tempfile::tempdir().unwrap();
        let artifact_dir = tmp.path().join("artifacts");
        fs::create_dir_all(&artifact_dir).unwrap();
        let mut adapter = make_adapter(write_failing_pi(tmp.path()), artifact_dir);

        let result = collect_output(
            adapter
                .send_step(StepMessage {
                    run_id: RunId("run-1".to_string()),
                    session_id: SessionId("sess-1".to_string()),
                    step_id: 0,
                    total_steps: 1,
                    source_file: "tests/x.test.toml".to_string(),
                    instruction: "do it".to_string(),
                })
                .await
                .unwrap(),
        )
        .await;

        assert!(
            matches!(result, Err(ProviderError::StreamError(ref msg)) if msg == "model not found"),
            "expected StreamError, got: {result:?}"
        );
    }

    #[test]
    fn initialize_finds_binary() {
        let config = test_config();
        let tmp = tempfile::tempdir().unwrap();

        let result = PiAdapter::initialize(&config, tmp.path(), false);
        match result {
            Ok(adapter) => {
                assert_eq!(
                    adapter.agent_args,
                    vec!["--model", "anthropic/claude-sonnet-4-5"]
                );
                assert!(!adapter.session_id.is_empty());
                assert!(adapter.session_dir.ends_with("pi-session"));
            }
            Err(ProviderError::InitializationFailed(msg)) => {
                assert!(msg.contains("pi CLI binary not found"));
            }
            Err(e) => panic!("unexpected error type: {e:?}"),
        }
    }

    #[test]
    fn parse_text_delta_event() {
        let json = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"hello"}}"#;
        let event: PiEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "message_update");
        let ev = event.assistant_event.unwrap();
        assert_eq!(ev.kind, "text_delta");
        assert_eq!(ev.delta.as_deref(), Some("hello"));
    }

    #[test]
    fn parse_error_message_event() {
        let json = r#"{"type":"turn_end","message":{"stopReason":"error","errorMessage":"boom"}}"#;
        let event: PiEvent = serde_json::from_str(json).unwrap();
        let message = event.message.unwrap();
        assert_eq!(message.stop_reason.as_deref(), Some("error"));
        assert_eq!(message.error_message.as_deref(), Some("boom"));
    }
}
