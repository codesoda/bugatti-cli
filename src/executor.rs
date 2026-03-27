use crate::diagnostics::EvidenceRef;
use crate::expand::ExpandedStep;
use crate::provider::{AgentSession, OutputChunk, ProviderError, StepMessage};
use crate::run::{ArtifactDir, RunId, SessionId};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The prefix that marks a log event line in provider output.
const BUGATTI_LOG_PREFIX: &str = "BUGATTI_LOG ";

/// A log event parsed from provider output during step execution.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEvent {
    /// The run this log event belongs to.
    pub run_id: String,
    /// The step that produced this log event.
    pub step_id: usize,
    /// The log message text.
    pub message: String,
}

/// Parse BUGATTI_LOG lines from text, returning extracted log events.
///
/// Lines matching 'BUGATTI_LOG <message>' are recognized.
/// Each matching line produces a LogEvent with the given run_id and step_id.
pub fn parse_log_events(text: &str, run_id: &str, step_id: usize) -> Vec<LogEvent> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            trimmed
                .strip_prefix(BUGATTI_LOG_PREFIX)
                .map(|msg| LogEvent {
                    run_id: run_id.to_string(),
                    step_id,
                    message: msg.to_string(),
                })
        })
        .collect()
}

/// Default step timeout in seconds.
const DEFAULT_STEP_TIMEOUT_SECS: u64 = 300;

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
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutorError::Provider(e) => write!(f, "provider error: {e}"),
            ExecutorError::TranscriptWrite { path, source } => {
                write!(f, "failed to write transcript to '{path}': {source}")
            }
        }
    }
}

impl std::error::Error for ExecutorError {}

impl From<ProviderError> for ExecutorError {
    fn from(e: ProviderError) -> Self {
        ExecutorError::Provider(e)
    }
}

/// Parse the RESULT contract marker from accumulated output text.
///
/// Scans from the end of the text for the last line matching:
///   RESULT OK
///   RESULT WARN: <message>
///   RESULT ERROR: <message>
///
/// Free-form text before the result marker is allowed.
pub fn parse_result_marker(text: &str) -> Option<StepVerdict> {
    // Scan lines in reverse to find the last RESULT marker
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if let Some(verdict) = try_parse_result_line(trimmed) {
            return Some(verdict);
        }
    }
    None
}

fn try_parse_result_line(line: &str) -> Option<StepVerdict> {
    let rest = line.strip_prefix("RESULT")?;

    if rest.is_empty() {
        return None;
    }

    let rest = rest.trim_start();

    if rest == "OK" {
        return Some(StepVerdict::Ok);
    }

    if let Some(msg) = rest.strip_prefix("WARN:") {
        return Some(StepVerdict::Warn(msg.trim().to_string()));
    }

    if let Some(msg) = rest.strip_prefix("ERROR:") {
        return Some(StepVerdict::Error(msg.trim().to_string()));
    }

    None
}

/// Execute all expanded steps sequentially within one provider session.
///
/// Returns the run outcome with all step results.
/// The provider session must already be initialized and started.
pub fn execute_steps(
    session: &mut dyn AgentSession,
    steps: &[ExpandedStep],
    run_id: &RunId,
    session_id: &SessionId,
    artifact_dir: &ArtifactDir,
    step_timeout: Option<Duration>,
) -> Result<RunOutcome, ExecutorError> {
    let timeout = step_timeout.unwrap_or(Duration::from_secs(DEFAULT_STEP_TIMEOUT_SECS));
    let mut outcomes = Vec::with_capacity(steps.len());
    let mut artifact_errors: Vec<String> = Vec::new();
    let run_start = Instant::now();

    let total_steps = steps.len();

    // Open full_transcript.txt for incremental writing during streaming.
    // This ensures transcript is captured as execution progresses, not reconstructed after the fact.
    let full_transcript_path = artifact_dir.transcripts.join("full_transcript.txt");
    let mut full_transcript_file = match std::fs::File::create(&full_transcript_path) {
        Ok(f) => Some(f),
        Err(e) => {
            let msg = format!(
                "failed to create full transcript file '{}': {e}",
                full_transcript_path.display()
            );
            tracing::error!("{msg}");
            artifact_errors.push(msg);
            None
        }
    };

    for step in steps {
        // Print step begin
        let instruction_summary = truncate_instruction(&step.instruction, 60);
        tracing::info!(
            step_id = step.step_id,
            total = total_steps,
            source = %step.source_file.display(),
            "step execution begin"
        );
        println!(
            "STEP {}/{} ... {} (from {})",
            step.step_id + 1,
            total_steps,
            instruction_summary,
            step.source_file.display()
        );

        let step_start = Instant::now();

        let message = StepMessage {
            run_id: run_id.clone(),
            session_id: session_id.clone(),
            step_id: step.step_id,
            instruction: step.instruction.clone(),
        };

        let result = execute_single_step(session, message, &timeout);

        let duration = step_start.elapsed();

        let (step_result, transcript) = match result {
            Ok((transcript_text, verdict)) => (verdict, transcript_text),
            Err((transcript_text, err_result)) => (err_result, transcript_text),
        };

        // Parse BUGATTI_LOG events from the transcript
        let log_events = parse_log_events(&transcript, &run_id.0, step.step_id);

        // Render log events to console
        for event in &log_events {
            println!("LOG ........ {}", event.message);
        }

        // Log step result to tracing
        tracing::info!(
            step_id = step.step_id,
            result = %step_result,
            duration_ms = duration.as_millis() as u64,
            log_event_count = log_events.len(),
            "step execution complete"
        );

        // Print step result
        print_step_result(&step_result, duration);

        // Write per-step transcript artifact
        let transcript_path = artifact_dir
            .transcripts
            .join(format!("step_{:04}.txt", step.step_id));
        if let Err(e) = std::fs::write(&transcript_path, &transcript) {
            let msg = format!(
                "failed to write transcript for step {} to '{}': {e}",
                step.step_id,
                transcript_path.display()
            );
            tracing::error!("{msg}");
            artifact_errors.push(msg);
        }

        // Append to full transcript incrementally during execution
        if let Some(ref mut f) = full_transcript_file {
            let write_result = (|| -> std::io::Result<()> {
                writeln!(f, "=== Step {} ===", step.step_id)?;
                writeln!(f, "Instruction: {}", step.instruction)?;
                writeln!(f, "Result: {}", step_result)?;
                writeln!(f, "Duration: {:.1}s", duration.as_secs_f64())?;
                writeln!(f, "---")?;
                writeln!(f, "{}", transcript)?;
                writeln!(f)?;
                Ok(())
            })();
            if let Err(e) = write_result {
                let msg = format!(
                    "failed to append step {} to full transcript: {e}",
                    step.step_id
                );
                tracing::error!("{msg}");
                artifact_errors.push(msg);
            }
        }

        let outcome = StepOutcome {
            step_id: step.step_id,
            instruction: step.instruction.clone(),
            source_file: step.source_file.clone(),
            result: step_result,
            transcript,
            log_events,
            evidence_refs: vec![],
            duration,
        };

        let is_failure = outcome.result.is_failure();
        outcomes.push(outcome);

        // Stop on failure
        if is_failure {
            tracing::warn!(
                step_id = step.step_id,
                "stopping execution due to step failure"
            );
            break;
        }
    }

    let all_passed = outcomes.iter().all(|o| o.result.is_pass());
    let total_duration = run_start.elapsed();

    // Print final run status
    print_run_summary(&outcomes, total_duration, total_steps);

    // Flush/close the full transcript file
    drop(full_transcript_file);

    // Write log events to a separate file (distinct from transcript and diagnostics)
    let log_events_path = artifact_dir.logs.join("bugatti_log_events.txt");
    let all_log_events: Vec<&LogEvent> = outcomes.iter().flat_map(|o| &o.log_events).collect();
    if !all_log_events.is_empty() {
        if let Ok(mut f) = std::fs::File::create(&log_events_path) {
            for event in &all_log_events {
                let _ = writeln!(f, "[step {}] {}", event.step_id, event.message);
            }
        }
    }

    Ok(RunOutcome {
        steps: outcomes,
        all_passed,
        total_duration,
        artifact_errors,
    })
}

/// Truncate an instruction string to a maximum length, appending "..." if truncated.
fn truncate_instruction(instruction: &str, max_len: usize) -> String {
    // Take first line only for the summary
    let first_line = instruction.lines().next().unwrap_or(instruction);
    if first_line.len() <= max_len {
        first_line.to_string()
    } else {
        format!("{}...", &first_line[..max_len])
    }
}

/// Print the result of a step to the console.
fn print_step_result(result: &StepResult, duration: Duration) {
    let duration_str = format!("{:.1}s", duration.as_secs_f64());
    match result {
        StepResult::Verdict(StepVerdict::Ok) => {
            println!("OK ......... ({duration_str})");
        }
        StepResult::Verdict(StepVerdict::Warn(msg)) => {
            println!("WARN ....... {msg} ({duration_str})");
        }
        StepResult::Verdict(StepVerdict::Error(msg)) => {
            println!("ERROR ...... {msg} ({duration_str})");
        }
        StepResult::ProtocolError(msg) => {
            println!("FAIL ....... protocol error: {msg} ({duration_str})");
        }
        StepResult::Timeout => {
            println!("FAIL ....... step timed out ({duration_str})");
        }
        StepResult::ProviderFailed(msg) => {
            println!("FAIL ....... provider error: {msg} ({duration_str})");
        }
    }
}

/// Print a summary of the full run after all steps have completed.
fn print_run_summary(outcomes: &[StepOutcome], total_duration: Duration, total_steps: usize) {
    println!();
    println!("═══════════════════════════════════════════════════");

    let completed = outcomes.len();
    let ok_count = outcomes
        .iter()
        .filter(|o| matches!(o.result, StepResult::Verdict(StepVerdict::Ok)))
        .count();
    let warn_count = outcomes
        .iter()
        .filter(|o| matches!(o.result, StepResult::Verdict(StepVerdict::Warn(_))))
        .count();
    let fail_count = outcomes.iter().filter(|o| o.result.is_failure()).count();
    let skipped = total_steps - completed;

    let all_passed = outcomes.iter().all(|o| o.result.is_pass());
    let status = if all_passed { "PASSED" } else { "FAILED" };

    println!(
        "Run {status}: {ok_count} ok, {warn_count} warn, {fail_count} failed, {skipped} skipped ({:.1}s)",
        total_duration.as_secs_f64()
    );
    println!("═══════════════════════════════════════════════════");
}

/// Execute a single step, collecting transcript and parsing the result.
///
/// Returns Ok((transcript, StepResult)) on successful completion,
/// or Err((transcript, StepResult)) on failure.
fn execute_single_step(
    session: &mut dyn AgentSession,
    message: StepMessage,
    timeout: &Duration,
) -> Result<(String, StepResult), (String, StepResult)> {
    let start = Instant::now();
    let mut transcript = String::new();

    let stream = match session.send_step(message) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "provider send_step failed");
            return Err((transcript, StepResult::ProviderFailed(e.to_string())));
        }
    };

    for chunk_result in stream {
        // Check timeout
        if start.elapsed() > *timeout {
            tracing::error!(
                elapsed_ms = start.elapsed().as_millis() as u64,
                "step timed out during streaming"
            );
            return Err((transcript, StepResult::Timeout));
        }

        match chunk_result {
            Ok(OutputChunk::Text(text)) => {
                transcript.push_str(&text);
            }
            Ok(OutputChunk::Done) => {
                break;
            }
            Err(e) => {
                return Err((transcript, StepResult::ProviderFailed(e.to_string())));
            }
        }
    }

    // Check timeout one more time after stream ends
    if start.elapsed() > *timeout {
        tracing::error!(
            elapsed_ms = start.elapsed().as_millis() as u64,
            "step timed out after streaming"
        );
        return Err((transcript, StepResult::Timeout));
    }

    // Parse result contract
    match parse_result_marker(&transcript) {
        Some(verdict) => Ok((transcript, StepResult::Verdict(verdict))),
        None => Err((
            transcript,
            StepResult::ProtocolError(
                "output ended without a valid RESULT marker (expected RESULT OK, RESULT WARN: ..., or RESULT ERROR: ...)".to_string(),
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::provider::{
        AgentSession, BootstrapMessage, OutputChunk, ProviderError, StepMessage,
    };
    use std::path::Path;

    // --- Result marker parsing tests ---

    #[test]
    fn parse_result_ok() {
        assert_eq!(parse_result_marker("RESULT OK"), Some(StepVerdict::Ok));
    }

    #[test]
    fn parse_result_warn() {
        assert_eq!(
            parse_result_marker("RESULT WARN: slow response"),
            Some(StepVerdict::Warn("slow response".to_string()))
        );
    }

    #[test]
    fn parse_result_error() {
        assert_eq!(
            parse_result_marker("RESULT ERROR: page not found"),
            Some(StepVerdict::Error("page not found".to_string()))
        );
    }

    #[test]
    fn parse_result_with_freeform_text_before() {
        let text =
            "I checked the page and everything looks fine.\nThe login form works.\nRESULT OK";
        assert_eq!(parse_result_marker(text), Some(StepVerdict::Ok));
    }

    #[test]
    fn parse_result_with_freeform_text_after_marker_ignored() {
        // Last RESULT line wins
        let text = "RESULT OK\nsome trailing text\nRESULT ERROR: actually failed";
        assert_eq!(
            parse_result_marker(text),
            Some(StepVerdict::Error("actually failed".to_string()))
        );
    }

    #[test]
    fn parse_result_missing_returns_none() {
        assert_eq!(parse_result_marker("No result here"), None);
    }

    #[test]
    fn parse_result_partial_returns_none() {
        assert_eq!(parse_result_marker("RESULT"), None);
        assert_eq!(parse_result_marker("RESULT UNKNOWN"), None);
    }

    #[test]
    fn parse_result_whitespace_trimmed() {
        assert_eq!(parse_result_marker("  RESULT OK  "), Some(StepVerdict::Ok));
    }

    #[test]
    fn parse_result_warn_with_extra_whitespace() {
        assert_eq!(
            parse_result_marker("RESULT WARN:   extra spaces  "),
            Some(StepVerdict::Warn("extra spaces".to_string()))
        );
    }

    // --- Mock provider for execution tests ---

    struct MockSession {
        responses: Vec<Vec<Result<OutputChunk, ProviderError>>>,
        call_count: usize,
    }

    impl MockSession {
        fn new(responses: Vec<Vec<Result<OutputChunk, ProviderError>>>) -> Self {
            Self {
                responses,
                call_count: 0,
            }
        }
    }

    impl AgentSession for MockSession {
        fn initialize(_config: &Config, _artifact_dir: &Path) -> Result<Self, ProviderError>
        where
            Self: Sized,
        {
            Ok(Self::new(vec![]))
        }

        fn start(&mut self) -> Result<(), ProviderError> {
            Ok(())
        }

        fn send_bootstrap(
            &mut self,
            _message: BootstrapMessage,
        ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>
        {
            Ok(Box::new(std::iter::empty()))
        }

        fn send_step(
            &mut self,
            _message: StepMessage,
        ) -> Result<Box<dyn Iterator<Item = Result<OutputChunk, ProviderError>> + '_>, ProviderError>
        {
            if self.call_count < self.responses.len() {
                let idx = self.call_count;
                self.call_count += 1;
                Ok(Box::new(self.responses[idx].clone().into_iter()))
            } else {
                Err(ProviderError::SendFailed("no more responses".to_string()))
            }
        }

        fn close(&mut self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    fn test_steps() -> Vec<ExpandedStep> {
        vec![
            ExpandedStep {
                step_id: 0,
                instruction: "Check the homepage loads".to_string(),
                source_file: PathBuf::from("/test/root.test.toml"),
                source_step_index: 0,
                parent_chain: vec![],
            },
            ExpandedStep {
                step_id: 1,
                instruction: "Verify login form exists".to_string(),
                source_file: PathBuf::from("/test/root.test.toml"),
                source_step_index: 1,
                parent_chain: vec![],
            },
        ]
    }

    fn test_run_ids() -> (RunId, SessionId) {
        (
            RunId("test-run-001".to_string()),
            SessionId("test-session-001".to_string()),
        )
    }

    fn test_artifact_dir() -> (tempfile::TempDir, ArtifactDir) {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run-001".to_string());
        let dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        dir.create_all().unwrap();
        (tmp, dir)
    }

    #[test]
    fn execute_steps_all_ok() {
        let steps = test_steps();
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![
            vec![
                Ok(OutputChunk::Text(
                    "Page loaded successfully.\nRESULT OK".to_string(),
                )),
                Ok(OutputChunk::Done),
            ],
            vec![
                Ok(OutputChunk::Text(
                    "Login form found.\nRESULT OK".to_string(),
                )),
                Ok(OutputChunk::Done),
            ],
        ]);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        assert!(outcome.all_passed);
        assert_eq!(outcome.steps.len(), 2);
        assert_eq!(
            outcome.steps[0].result,
            StepResult::Verdict(StepVerdict::Ok)
        );
        assert_eq!(
            outcome.steps[1].result,
            StepResult::Verdict(StepVerdict::Ok)
        );
    }

    #[test]
    fn execute_steps_stops_on_error() {
        let steps = test_steps();
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![
            vec![
                Ok(OutputChunk::Text(
                    "RESULT ERROR: page returned 500".to_string(),
                )),
                Ok(OutputChunk::Done),
            ],
            // Second step should never execute
            vec![
                Ok(OutputChunk::Text("RESULT OK".to_string())),
                Ok(OutputChunk::Done),
            ],
        ]);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        assert!(!outcome.all_passed);
        assert_eq!(outcome.steps.len(), 1); // Only first step executed
        assert_eq!(
            outcome.steps[0].result,
            StepResult::Verdict(StepVerdict::Error("page returned 500".to_string()))
        );
    }

    #[test]
    fn execute_steps_warn_continues() {
        let steps = test_steps();
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![
            vec![
                Ok(OutputChunk::Text("RESULT WARN: slow response".to_string())),
                Ok(OutputChunk::Done),
            ],
            vec![
                Ok(OutputChunk::Text("RESULT OK".to_string())),
                Ok(OutputChunk::Done),
            ],
        ]);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        assert!(outcome.all_passed);
        assert_eq!(outcome.steps.len(), 2);
        assert_eq!(
            outcome.steps[0].result,
            StepResult::Verdict(StepVerdict::Warn("slow response".to_string()))
        );
    }

    #[test]
    fn execute_steps_missing_result_marker() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text(
                "I checked the page and it looks fine.".to_string(),
            )),
            Ok(OutputChunk::Done),
        ]]);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        assert!(!outcome.all_passed);
        assert!(matches!(
            &outcome.steps[0].result,
            StepResult::ProtocolError(_)
        ));
    }

    #[test]
    fn execute_steps_provider_send_failure() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        // Empty responses means send_step returns error
        let mut session = MockSession::new(vec![]);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        assert!(!outcome.all_passed);
        assert!(matches!(
            &outcome.steps[0].result,
            StepResult::ProviderFailed(_)
        ));
    }

    #[test]
    fn execute_steps_provider_stream_error() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text("partial output".to_string())),
            Err(ProviderError::SessionCrashed("process died".to_string())),
        ]]);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        assert!(!outcome.all_passed);
        assert!(matches!(
            &outcome.steps[0].result,
            StepResult::ProviderFailed(_)
        ));
        // Partial transcript is preserved
        assert!(outcome.steps[0].transcript.contains("partial output"));
    }

    #[test]
    fn execute_steps_writes_transcript_artifacts() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text("Check complete.\nRESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ]]);

        let _outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        // Per-step transcript
        let step_transcript = artifact_dir.transcripts.join("step_0000.txt");
        assert!(step_transcript.is_file());
        let contents = std::fs::read_to_string(&step_transcript).unwrap();
        assert!(contents.contains("Check complete."));
        assert!(contents.contains("RESULT OK"));

        // Combined transcript
        let full_transcript = artifact_dir.transcripts.join("full_transcript.txt");
        assert!(full_transcript.is_file());
    }

    #[test]
    fn execute_steps_full_transcript_written_incrementally() {
        let steps = test_steps();
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![
            vec![
                Ok(OutputChunk::Text(
                    "First step output.\nRESULT OK".to_string(),
                )),
                Ok(OutputChunk::Done),
            ],
            vec![
                Ok(OutputChunk::Text(
                    "Second step output.\nRESULT OK".to_string(),
                )),
                Ok(OutputChunk::Done),
            ],
        ]);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        assert!(outcome.artifact_errors.is_empty());

        // Full transcript should exist and contain both steps
        let full_transcript_path = artifact_dir.transcripts.join("full_transcript.txt");
        assert!(full_transcript_path.is_file());
        let contents = std::fs::read_to_string(&full_transcript_path).unwrap();
        assert!(contents.contains("=== Step 0 ==="));
        assert!(contents.contains("First step output."));
        assert!(contents.contains("=== Step 1 ==="));
        assert!(contents.contains("Second step output."));
    }

    #[test]
    fn execute_steps_full_transcript_captures_partial_on_failure() {
        let steps = test_steps();
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![
            vec![
                Ok(OutputChunk::Text(
                    "RESULT ERROR: something broke".to_string(),
                )),
                Ok(OutputChunk::Done),
            ],
            // Second step will not execute
            vec![
                Ok(OutputChunk::Text("RESULT OK".to_string())),
                Ok(OutputChunk::Done),
            ],
        ]);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        // Full transcript should contain the first (failed) step but not the second
        let full_transcript_path = artifact_dir.transcripts.join("full_transcript.txt");
        let contents = std::fs::read_to_string(&full_transcript_path).unwrap();
        assert!(contents.contains("=== Step 0 ==="));
        assert!(contents.contains("something broke"));
        assert!(!contents.contains("=== Step 1 ==="));
        assert!(outcome.artifact_errors.is_empty());
    }

    #[test]
    fn execute_steps_records_duration() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text("RESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ]]);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        // Duration should be very small but non-zero
        assert!(outcome.steps[0].duration.as_nanos() > 0);
        assert!(outcome.total_duration.as_nanos() > 0);
    }

    #[test]
    fn step_result_is_pass() {
        assert!(StepResult::Verdict(StepVerdict::Ok).is_pass());
        assert!(StepResult::Verdict(StepVerdict::Warn("x".to_string())).is_pass());
        assert!(!StepResult::Verdict(StepVerdict::Error("x".to_string())).is_pass());
        assert!(!StepResult::ProtocolError("x".to_string()).is_pass());
        assert!(!StepResult::Timeout.is_pass());
        assert!(!StepResult::ProviderFailed("x".to_string()).is_pass());
    }

    #[test]
    fn execute_steps_multiple_text_chunks_concatenated() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text("First chunk. ".to_string())),
            Ok(OutputChunk::Text("Second chunk.\n".to_string())),
            Ok(OutputChunk::Text("RESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ]]);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        assert!(outcome.all_passed);
        assert!(outcome.steps[0].transcript.contains("First chunk."));
        assert!(outcome.steps[0].transcript.contains("Second chunk."));
    }

    // --- BUGATTI_LOG parsing tests ---

    #[test]
    fn parse_log_events_single_line() {
        let events = parse_log_events("BUGATTI_LOG Server started on port 3000", "run-1", 0);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].run_id, "run-1");
        assert_eq!(events[0].step_id, 0);
        assert_eq!(events[0].message, "Server started on port 3000");
    }

    #[test]
    fn parse_log_events_multiple_lines() {
        let text = "Some output\nBUGATTI_LOG first event\nMore output\nBUGATTI_LOG second event\nRESULT OK";
        let events = parse_log_events(text, "run-2", 3);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].message, "first event");
        assert_eq!(events[0].step_id, 3);
        assert_eq!(events[1].message, "second event");
    }

    #[test]
    fn parse_log_events_none_found() {
        let events = parse_log_events("No log events here\nRESULT OK", "run-1", 0);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_log_events_whitespace_trimmed() {
        let events = parse_log_events("  BUGATTI_LOG trimmed message  ", "run-1", 0);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message, "trimmed message  ");
    }

    #[test]
    fn parse_log_events_prefix_only_no_message() {
        // "BUGATTI_LOG " with trailing space but no content - still valid, empty message
        let events = parse_log_events("BUGATTI_LOG ", "run-1", 0);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message, "");
    }

    #[test]
    fn parse_log_events_no_space_after_prefix_not_matched() {
        // "BUGATTI_LOG" without trailing space is not matched (prefix is "BUGATTI_LOG ")
        let events = parse_log_events("BUGATTI_LOG", "run-1", 0);
        assert!(events.is_empty());
    }

    // --- BUGATTI_LOG in execution tests ---

    #[test]
    fn execute_steps_captures_log_events() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text(
                "Starting check\nBUGATTI_LOG Database connected\nBUGATTI_LOG Schema validated\nRESULT OK".to_string(),
            )),
            Ok(OutputChunk::Done),
        ]]);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        assert!(outcome.all_passed);
        assert_eq!(outcome.steps[0].log_events.len(), 2);
        assert_eq!(outcome.steps[0].log_events[0].message, "Database connected");
        assert_eq!(outcome.steps[0].log_events[0].run_id, "test-run-001");
        assert_eq!(outcome.steps[0].log_events[0].step_id, 0);
        assert_eq!(outcome.steps[0].log_events[1].message, "Schema validated");
    }

    #[test]
    fn execute_steps_no_log_events() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text("No logs here.\nRESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ]]);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        assert!(outcome.steps[0].log_events.is_empty());
        // No log events file should be created when there are no events
        let log_events_path = artifact_dir.logs.join("bugatti_log_events.txt");
        assert!(!log_events_path.exists());
    }

    #[test]
    fn execute_steps_writes_log_events_file() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text(
                "BUGATTI_LOG Migration complete\nRESULT OK".to_string(),
            )),
            Ok(OutputChunk::Done),
        ]]);

        let _outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
        )
        .unwrap();

        // Log events file should exist and be separate from transcript
        let log_events_path = artifact_dir.logs.join("bugatti_log_events.txt");
        assert!(log_events_path.is_file());
        let contents = std::fs::read_to_string(&log_events_path).unwrap();
        assert!(contents.contains("[step 0] Migration complete"));

        // Transcript should still contain the BUGATTI_LOG line (raw transcript is unfiltered)
        let transcript_path = artifact_dir.transcripts.join("step_0000.txt");
        let transcript = std::fs::read_to_string(&transcript_path).unwrap();
        assert!(transcript.contains("BUGATTI_LOG Migration complete"));
    }
}
