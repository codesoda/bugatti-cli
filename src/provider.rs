use crate::config::Config;
use crate::run::{RunId, SessionId};
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

/// Provider-agnostic trait representing one long-lived agent session.
///
/// The harness uses this trait to communicate with any provider (Claude Code, etc.)
/// without coupling to provider-specific details. One session spans the entire test run.
pub trait AgentSession {
    /// Initialize a new session from the effective config.
    ///
    /// The config has already been resolved (global + per-test overrides merged).
    /// Provider-specific options (agent_args, extra_system_prompt) are available
    /// in `config.provider`.
    ///
    /// `artifact_dir` points to the run's artifact directory for transcript/log storage.
    fn initialize(config: &Config, artifact_dir: &Path) -> Result<Self, ProviderError>
    where
        Self: Sized;

    /// Start the conversation / underlying process.
    ///
    /// Called once after initialization, before any messages are sent.
    fn start(&mut self) -> Result<(), ProviderError>;

    /// Send a bootstrap message at the beginning of the session.
    ///
    /// This delivers harness instructions, extra system prompt, and context
    /// before any test steps execute. The provider streams its response
    /// through the returned iterator.
    fn send_bootstrap(
        &mut self,
        message: BootstrapMessage,
    ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>;

    /// Send a step message and receive streamed output.
    ///
    /// The message includes run ID, session ID, and step ID for traceability.
    /// Returns an iterator of output chunks for live display and transcript capture.
    fn send_step(
        &mut self,
        message: StepMessage,
    ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>;

    /// Close the session and clean up resources.
    ///
    /// Called at the end of a run (success or failure) to shut down the provider process.
    fn close(&mut self) -> Result<(), ProviderError>;
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
