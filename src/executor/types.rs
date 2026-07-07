//! Core executor data types: step verdicts, outcomes, and errors.

use std::path::PathBuf;
use std::time::Duration;

use crate::diagnostics::EvidenceRef;
use crate::provider::ProviderError;

use super::markers::LogEvent;

/// The parsed result from the RESULT contract marker.
#[derive(Debug, Clone, PartialEq)]
pub enum StepVerdict {
    Ok,
    Warn(String),
    Error(String),
}

impl std::fmt::Display for StepVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepVerdict::Ok => write!(f, "OK"),
            StepVerdict::Warn(msg) => write!(f, "WARN: {msg}"),
            StepVerdict::Error(msg) => write!(f, "ERROR: {msg}"),
        }
    }
}

/// The outcome of a single step after execution.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    /// The step ID from the expanded step.
    pub step_id: usize,
    /// The instruction text that was sent.
    pub instruction: String,
    /// Source file for provenance.
    pub source_file: PathBuf,
    /// Whether this was a setup step (not counted in test pass/fail).
    pub setup: bool,
    /// The result: either a parsed verdict, a protocol error, or a timeout.
    pub result: StepResult,
    /// Full transcript text captured from the provider.
    pub transcript: String,
    /// BUGATTI_LOG events parsed from provider output, separate from transcript.
    pub log_events: Vec<LogEvent>,
    /// Evidence references collected during this step (screenshots, command logs, etc.).
    /// Missing or failed evidence collection is noted via `EvidenceRef::collection_error`.
    pub evidence_refs: Vec<EvidenceRef>,
    /// How long the step took.
    pub duration: Duration,
}

/// The result of a step execution.
#[derive(Debug, Clone, PartialEq)]
pub enum StepResult {
    /// Successfully parsed a RESULT marker.
    Verdict(StepVerdict),
    /// Output ended without a valid RESULT marker.
    ProtocolError(String),
    /// Step exceeded the timeout.
    Timeout,
    /// Provider error during execution.
    ProviderFailed(String),
}

impl StepResult {
    /// Whether this result represents a passing step.
    pub fn is_pass(&self) -> bool {
        matches!(
            self,
            StepResult::Verdict(StepVerdict::Ok) | StepResult::Verdict(StepVerdict::Warn(_))
        )
    }

    /// Whether this result is a hard failure.
    pub fn is_failure(&self) -> bool {
        !self.is_pass()
    }
}

impl std::fmt::Display for StepResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepResult::Verdict(v) => write!(f, "{v}"),
            StepResult::ProtocolError(msg) => write!(f, "PROTOCOL ERROR: {msg}"),
            StepResult::Timeout => write!(f, "TIMEOUT"),
            StepResult::ProviderFailed(msg) => write!(f, "PROVIDER ERROR: {msg}"),
        }
    }
}

/// The outcome of the full run after all steps have executed.
#[derive(Debug)]
pub struct RunOutcome {
    /// Ordered list of step outcomes.
    pub steps: Vec<StepOutcome>,
    /// Whether all steps passed.
    pub all_passed: bool,
    /// Total duration of step execution.
    pub total_duration: Duration,
    /// Artifact capture errors encountered during the run (e.g., failed transcript writes).
    pub artifact_errors: Vec<String>,
}

/// Error from the executor.
#[derive(Debug)]
pub enum ExecutorError {
    /// Provider error during session lifecycle.
    Provider(ProviderError),
    /// Failed to write transcript artifact.
    TranscriptWrite {
        path: String,
        source: std::io::Error,
    },
    /// Checkpoint save or restore failed.
    CheckpointFailed(String),
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutorError::Provider(e) => write!(f, "provider error: {e}"),
            ExecutorError::TranscriptWrite { path, source } => {
                write!(f, "failed to write transcript to '{path}': {source}")
            }
            ExecutorError::CheckpointFailed(msg) => write!(f, "checkpoint failed: {msg}"),
        }
    }
}

impl std::error::Error for ExecutorError {}

impl From<ProviderError> for ExecutorError {
    fn from(e: ProviderError) -> Self {
        ExecutorError::Provider(e)
    }
}
