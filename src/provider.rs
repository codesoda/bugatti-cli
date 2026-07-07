use crate::config::Config;
use crate::run::{RunId, SessionId};
use async_trait::async_trait;
use std::path::Path;

/// A message sent to the provider for a single step.
#[derive(Debug, Clone)]
pub struct StepMessage {
    /// The run this step belongs to.
    pub run_id: RunId,
    /// The session this step belongs to.
    pub session_id: SessionId,
    /// Sequential step identifier within the run.
    pub step_id: usize,
    /// Total number of steps in the run.
    pub total_steps: usize,
    /// Source file this step originated from.
    pub source_file: String,
    /// The instruction text for this step.
    pub instruction: String,
}

/// A bootstrap message sent at session start before any steps.
#[derive(Debug, Clone)]
pub struct BootstrapMessage {
    /// The run this session belongs to.
    pub run_id: RunId,
    /// The session identifier.
    pub session_id: SessionId,
    /// Bootstrap prompt content (e.g., system prompt additions, harness instructions).
    pub content: String,
}

/// A chunk of streamed output from the provider.
#[derive(Debug, Clone)]
pub enum OutputChunk {
    /// A piece of text output from the assistant.
    Text(String),
    /// The provider has finished its response for the current message.
    Done,
}

/// An asynchronous stream of output chunks from the provider for one turn.
///
/// This is the async replacement for the previous `Iterator<Item = Result<OutputChunk, _>>`
/// streaming interface: callers repeatedly await `next_chunk` until it returns `None`
/// (stream exhausted) or an `OutputChunk::Done` is yielded (turn complete).
#[async_trait]
pub trait OutputStream: Send {
    /// Await the next chunk of output. Returns `None` when the stream is exhausted.
    async fn next_chunk(&mut self) -> Option<Result<OutputChunk, ProviderError>>;
}

/// An `OutputStream` backed by a pre-collected list of chunks.
///
/// Useful for providers that complete a turn without streaming (e.g. bootstrap
/// handling that makes no model call) and for tests.
pub struct VecOutputStream {
    chunks: std::vec::IntoIter<Result<OutputChunk, ProviderError>>,
}

impl VecOutputStream {
    /// Create a stream that yields the given chunks in order.
    pub fn new(chunks: Vec<Result<OutputChunk, ProviderError>>) -> Self {
        Self {
            chunks: chunks.into_iter(),
        }
    }

    /// Create a stream that yields a single `OutputChunk::Done`.
    pub fn done() -> Self {
        Self::new(vec![Ok(OutputChunk::Done)])
    }

    /// Create an empty stream.
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }
}

#[async_trait]
impl OutputStream for VecOutputStream {
    async fn next_chunk(&mut self) -> Option<Result<OutputChunk, ProviderError>> {
        self.chunks.next()
    }
}

/// Provider-agnostic trait representing one long-lived agent session.
///
/// The harness uses this trait to communicate with any provider (Claude Code, etc.)
/// without coupling to provider-specific details. One session spans the entire test run.
#[async_trait]
pub trait AgentSession: Send {
    /// Initialize a new session from the effective config.
    ///
    /// The config has already been resolved (global + per-test overrides merged).
    /// Provider-specific options (agent_args, extra_system_prompt) are available
    /// in `config.provider`.
    ///
    /// `artifact_dir` points to the run's artifact directory for transcript/log storage.
    fn initialize(
        config: &Config,
        artifact_dir: &Path,
        verbose: bool,
    ) -> Result<Self, ProviderError>
    where
        Self: Sized;

    /// Start the conversation / underlying process.
    ///
    /// Called once after initialization, before any messages are sent.
    async fn start(&mut self) -> Result<(), ProviderError>;

    /// Send a bootstrap message at the beginning of the session.
    ///
    /// This delivers harness instructions, extra system prompt, and context
    /// before any test steps execute. The provider streams its response
    /// through the returned stream.
    async fn send_bootstrap(
        &mut self,
        message: BootstrapMessage,
    ) -> Result<Box<dyn OutputStream + '_>, ProviderError>;

    /// Send a step message and receive streamed output.
    ///
    /// The message includes run ID, session ID, and step ID for traceability.
    /// Returns a stream of output chunks for live display and transcript capture.
    async fn send_step(
        &mut self,
        message: StepMessage,
    ) -> Result<Box<dyn OutputStream + '_>, ProviderError>;

    /// Close the session and clean up resources.
    ///
    /// Called at the end of a run (success or failure) to shut down the provider process.
    async fn close(&mut self) -> Result<(), ProviderError>;
}

/// Format a step message with metadata prefix for sending to the provider.
pub fn format_step_message(message: &StepMessage) -> String {
    format!(
        "[run_id={} session_id={} step={}/{} source={}]\n\n{}",
        message.run_id,
        message.session_id,
        message.step_id + 1,
        message.total_steps,
        message.source_file,
        message.instruction
    )
}

/// Initialize the configured provider adapter for a test run.
pub fn initialize_session(
    config: &Config,
    artifact_dir: &Path,
    verbose: bool,
) -> Result<Box<dyn AgentSession>, ProviderError> {
    match config.provider.name.as_str() {
        "claude-code" => Ok(Box::new(crate::claude_code::ClaudeCodeAdapter::initialize(
            config,
            artifact_dir,
            verbose,
        )?)),
        "codex" => Ok(Box::new(crate::codex::CodexAdapter::initialize(
            config,
            artifact_dir,
            verbose,
        )?)),
        "pi" => Ok(Box::new(crate::pi::PiAdapter::initialize(
            config,
            artifact_dir,
            verbose,
        )?)),
        other => Err(ProviderError::InitializationFailed(format!(
            "unknown provider '{other}' (supported: claude-code, codex, pi)"
        ))),
    }
}

/// Errors from provider operations.
#[derive(Debug, Clone)]
pub enum ProviderError {
    /// Provider failed to initialize (e.g., binary not found, invalid config).
    InitializationFailed(String),
    /// Provider failed to start the conversation process.
    StartFailed(String),
    /// Provider session crashed or exited unexpectedly mid-run.
    SessionCrashed(String),
    /// Failed to send a message to the provider.
    SendFailed(String),
    /// Error during output streaming.
    StreamError(String),
    /// Provider shutdown failed.
    ShutdownFailed(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::InitializationFailed(msg) => {
                write!(f, "provider initialization failed: {msg}")
            }
            ProviderError::StartFailed(msg) => {
                write!(f, "provider failed to start: {msg}")
            }
            ProviderError::SessionCrashed(msg) => {
                write!(f, "provider session crashed: {msg}")
            }
            ProviderError::SendFailed(msg) => {
                write!(f, "failed to send message to provider: {msg}")
            }
            ProviderError::StreamError(msg) => {
                write!(f, "provider stream error: {msg}")
            }
            ProviderError::ShutdownFailed(msg) => {
                write!(f, "provider shutdown failed: {msg}")
            }
        }
    }
}

impl std::error::Error for ProviderError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use indexmap::IndexMap;

    fn test_config(provider_name: &str) -> Config {
        Config {
            provider: ProviderConfig {
                name: provider_name.to_string(),
                extra_system_prompt: None,
                agent_args: Vec::new(),
                step_timeout_secs: None,
                strict_warnings: None,
                base_url: None,
            },
            commands: IndexMap::new(),
            checkpoint: None,
        }
    }

    #[test]
    fn format_step_message_includes_metadata() {
        let message = StepMessage {
            run_id: RunId("run-123".to_string()),
            session_id: SessionId("sess-456".to_string()),
            step_id: 1,
            total_steps: 3,
            source_file: "tests/login.test.toml".to_string(),
            instruction: "Verify the login form loads".to_string(),
        };

        let formatted = format_step_message(&message);

        assert!(formatted.contains("run_id=run-123"));
        assert!(formatted.contains("session_id=sess-456"));
        assert!(formatted.contains("step=2/3"));
        assert!(formatted.contains("source=tests/login.test.toml"));
        assert!(formatted.contains("Verify the login form loads"));
    }

    #[test]
    fn initialize_session_rejects_unknown_provider() {
        let config = test_config("unknown");
        let artifact_dir = tempfile::tempdir().unwrap();

        let result = initialize_session(&config, artifact_dir.path(), false);

        assert!(
            matches!(result, Err(ProviderError::InitializationFailed(ref msg)) if msg.contains("unknown provider 'unknown'"))
        );
    }
}
