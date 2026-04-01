use crate::diagnostics::{EvidenceKind, EvidenceRef};
use crate::expand::ExpandedStep;
use crate::provider::{AgentSession, BootstrapMessage, OutputChunk, ProviderError, StepMessage};
use crate::run::{ArtifactDir, RunId, SessionId};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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
/// Scans from the end of the text for the last occurrence of a RESULT marker.
///
/// Matches:
///   RESULT OK
///   RESULT WARN: <message>
///   RESULT ERROR: <message>
///
/// The marker can appear on its own line or embedded at the end of a line
/// (agents sometimes omit the newline before the marker).
pub fn parse_result_marker(text: &str) -> Option<StepVerdict> {
    // First try line-based scan (most common case)
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if let Some(verdict) = try_parse_result_line(trimmed) {
            return Some(verdict);
        }
    }

    // Fallback: scan for RESULT marker embedded anywhere in text.
    // Find the last occurrence and parse from there.
    let mut pos = text.len();
    while pos > 0 {
        if let Some(idx) = text[..pos].rfind("RESULT ") {
            let from_marker = &text[idx..];
            // Take up to the next newline (or end of string)
            let end = from_marker.find('\n').unwrap_or(from_marker.len());
            let candidate = from_marker[..end].trim();
            if let Some(verdict) = try_parse_result_line(candidate) {
                return Some(verdict);
            }
            pos = idx;
        } else {
            break;
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

/// Configuration for the bootstrap message sent at session start.
pub struct BootstrapConfig<'a> {
    pub test_name: &'a str,
    pub test_file: &'a str,
    pub extra_system_prompt: Option<&'a str>,
    pub base_url: Option<&'a str>,
}

/// Build the bootstrap message content sent to the provider at session start.
///
/// Includes the result contract, BUGATTI_LOG format, test metadata, and
/// extra system prompt (if configured).
pub fn build_bootstrap_content(
    config: &BootstrapConfig,
    total_steps: usize,
    run_id: &RunId,
    session_id: &SessionId,
) -> String {
    let mut content = String::new();

    // Extra system prompt first (if provided)
    if let Some(prompt) = config.extra_system_prompt {
        content.push_str(prompt);
        content.push_str("\n\n");
    }

    // Harness instructions
    content.push_str("You are being driven by the Bugatti test harness. ");
    content.push_str("Follow these rules for every step:\n\n");

    // Result contract
    content.push_str("## Result Contract\n\n");
    content.push_str("After completing each step, you MUST emit exactly one result line as the final line of your response:\n");
    content.push_str("- `RESULT OK` — the step passed\n");
    content.push_str("- `RESULT WARN: <message>` — the step passed with a warning\n");
    content.push_str("- `RESULT ERROR: <message>` — the step failed\n\n");
    content.push_str("Free-form text before the result line is allowed and encouraged.\n\n");

    // BUGATTI_LOG format
    content.push_str("## Logging\n\n");
    content.push_str("To emit structured log events visible in the harness, output a line matching:\n");
    content.push_str("`BUGATTI_LOG <message>`\n\n");

    // Test metadata
    content.push_str("## Test Metadata\n\n");
    content.push_str(&format!("- Test: {}\n", config.test_name));
    content.push_str(&format!("- File: {}\n", config.test_file));
    content.push_str(&format!("- Steps: {}\n", total_steps));
    content.push_str(&format!("- Run ID: {}\n", run_id));
    content.push_str(&format!("- Session ID: {}\n", session_id));
    if let Some(base_url) = config.base_url {
        content.push_str(&format!("- Base URL: {}\n", base_url));
        content.push_str("\nAll URLs in step instructions are relative to the Base URL unless a full URL (with host) is provided.\n");
    }

    content
}

/// Execute all expanded steps sequentially within one provider session.
///
/// Returns the run outcome with all step results.
/// The provider session must already be initialized and started.
#[allow(clippy::too_many_arguments)]
pub fn execute_steps(
    session: &mut dyn AgentSession,
    steps: &[ExpandedStep],
    run_id: &RunId,
    session_id: &SessionId,
    artifact_dir: &ArtifactDir,
    step_timeout: Option<Duration>,
    bootstrap_config: Option<&BootstrapConfig>,
    interrupted: &AtomicBool,
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

    // Send bootstrap message before any steps
    if let Some(config) = bootstrap_config {
        let content = build_bootstrap_content(config, total_steps, run_id, session_id);
        tracing::info!("sending bootstrap message");
        println!("BOOTSTRAP .. sending harness instructions");

        let bootstrap_msg = BootstrapMessage {
            run_id: run_id.clone(),
            session_id: session_id.clone(),
            content,
        };

        let bootstrap_start = Instant::now();
        match session.send_bootstrap(bootstrap_msg) {
            Ok(stream) => {
                let mut bootstrap_transcript = String::new();
                for chunk in stream {
                    match chunk {
                        Ok(OutputChunk::Text(text)) => bootstrap_transcript.push_str(&text),
                        Ok(OutputChunk::Done) => break,
                        Err(e) => {
                            tracing::warn!(error = %e, "bootstrap stream error (non-fatal)");
                            break;
                        }
                    }
                }
                // Write bootstrap transcript
                let bootstrap_path = artifact_dir.transcripts.join("bootstrap.txt");
                if let Err(e) = std::fs::write(&bootstrap_path, &bootstrap_transcript) {
                    let msg = format!("failed to write bootstrap transcript: {e}");
                    tracing::error!("{msg}");
                    artifact_errors.push(msg);
                }
                if let Some(ref mut f) = full_transcript_file {
                    let _ = writeln!(f, "=== Bootstrap ===");
                    let _ = writeln!(f, "{}", bootstrap_transcript);
                    let _ = writeln!(f);
                }
                let bootstrap_duration = bootstrap_start.elapsed();
                tracing::info!(duration_ms = bootstrap_duration.as_millis() as u64, "bootstrap complete");
                println!("OK ......... bootstrap ({:.1}s)", bootstrap_duration.as_secs_f64());
            }
            Err(e) => {
                tracing::error!(error = %e, "bootstrap send failed");
                return Err(ExecutorError::Provider(e));
            }
        }
    }

    for step in steps {
        // Check for interruption between steps
        if interrupted.load(Ordering::Relaxed) {
            tracing::warn!("interrupted before step {}", step.step_id);
            break;
        }

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
            total_steps,
            source_file: step.source_file.display().to_string(),
            instruction: step.instruction.clone(),
        };

        // Per-step timeout overrides the test/config-level timeout
        let effective_timeout = step.step_timeout_secs
            .map(Duration::from_secs)
            .unwrap_or(timeout);

        let result = execute_single_step(session, message, &effective_timeout, interrupted);

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

        // Auto-attach evidence refs for non-OK steps
        let evidence_refs = if !step_result.is_pass() || matches!(&step_result, StepResult::Verdict(StepVerdict::Warn(_))) {
            vec![EvidenceRef {
                kind: EvidenceKind::CommandLog,
                path: transcript_path.clone(),
                description: format!("Step {} transcript", step.step_id),
                collection_error: if transcript_path.exists() { None } else { Some("transcript file not written".to_string()) },
            }]
        } else {
            vec![]
        };

        let outcome = StepOutcome {
            step_id: step.step_id,
            instruction: step.instruction.clone(),
            source_file: step.source_file.clone(),
            result: step_result,
            transcript,
            log_events,
            evidence_refs,
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

    // Send teardown message (best-effort, non-fatal)
    if !interrupted.load(Ordering::Relaxed) {
        println!("TEARDOWN .. cleaning up");
        let teardown_msg = StepMessage {
            run_id: run_id.clone(),
            session_id: session_id.clone(),
            step_id: total_steps,
            total_steps,
            source_file: "teardown".to_string(),
            instruction: "All test steps are complete. Close any browsers, tools, or resources you opened during this session.".to_string(),
        };
        match session.send_step(teardown_msg) {
            Ok(stream) => {
                let teardown_start = Instant::now();
                let teardown_timeout = Duration::from_secs(30);
                let mut teardown_transcript = String::new();
                for chunk in stream {
                    if teardown_start.elapsed() > teardown_timeout {
                        tracing::warn!("teardown timed out");
                        break;
                    }
                    match chunk {
                        Ok(OutputChunk::Text(text)) => teardown_transcript.push_str(&text),
                        Ok(OutputChunk::Done) => break,
                        Err(e) => {
                            tracing::warn!(error = %e, "teardown stream error");
                            break;
                        }
                    }
                }
                if let Some(ref mut f) = full_transcript_file {
                    let _ = writeln!(f, "=== Teardown ===");
                    let _ = writeln!(f, "{}", teardown_transcript);
                    let _ = writeln!(f);
                }
                println!("OK ......... teardown");
            }
            Err(e) => {
                tracing::warn!(error = %e, "teardown send failed (non-fatal)");
                println!("WARN ....... teardown failed: {e}");
            }
        }
    }

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
    interrupted: &AtomicBool,
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

        // Check for Ctrl+C interruption
        if interrupted.load(Ordering::Relaxed) {
            tracing::warn!("step interrupted by Ctrl+C during streaming");
            return Err((
                transcript,
                StepResult::ProviderFailed("interrupted by Ctrl+C".to_string()),
            ));
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
    fn parse_result_embedded_in_line() {
        // Agent omits newline before RESULT marker
        let text = "Success message confirmed visible.RESULT OK";
        assert_eq!(parse_result_marker(text), Some(StepVerdict::Ok));
    }

    #[test]
    fn parse_result_embedded_with_duplicate() {
        // Agent emits RESULT OK twice without newlines
        let text = "Log line.RESULT OKRESULT OK";
        assert_eq!(parse_result_marker(text), Some(StepVerdict::Ok));
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
        fn initialize(_config: &Config, _artifact_dir: &Path, _verbose: bool) -> Result<Self, ProviderError>
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
                step_timeout_secs: None,
            },
            ExpandedStep {
                step_id: 1,
                instruction: "Verify login form exists".to_string(),
                source_file: PathBuf::from("/test/root.test.toml"),
                source_step_index: 1,
                parent_chain: vec![],
                step_timeout_secs: None,
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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
            None,
            &AtomicBool::new(false),
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

    // --- Evidence ref tests ---

    #[test]
    fn execute_steps_evidence_refs_for_error() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text("RESULT ERROR: page not found".to_string())),
            Ok(OutputChunk::Done),
        ]]);

        let outcome = execute_steps(
            &mut session, &steps, &run_id, &session_id, &artifact_dir, None, None, &AtomicBool::new(false),
        ).unwrap();

        assert_eq!(outcome.steps[0].evidence_refs.len(), 1);
        assert_eq!(outcome.steps[0].evidence_refs[0].kind, crate::diagnostics::EvidenceKind::CommandLog);
        assert!(outcome.steps[0].evidence_refs[0].collection_error.is_none());
    }

    #[test]
    fn execute_steps_evidence_refs_for_warn() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text("RESULT WARN: slow response".to_string())),
            Ok(OutputChunk::Done),
        ]]);

        let outcome = execute_steps(
            &mut session, &steps, &run_id, &session_id, &artifact_dir, None, None, &AtomicBool::new(false),
        ).unwrap();

        assert_eq!(outcome.steps[0].evidence_refs.len(), 1);
    }

    #[test]
    fn execute_steps_evidence_refs_empty_for_ok() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text("RESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ]]);

        let outcome = execute_steps(
            &mut session, &steps, &run_id, &session_id, &artifact_dir, None, None, &AtomicBool::new(false),
        ).unwrap();

        assert!(outcome.steps[0].evidence_refs.is_empty());
    }

    // --- Bootstrap tests ---

    #[test]
    fn build_bootstrap_content_includes_result_contract() {
        let config = BootstrapConfig {
            test_name: "Login test",
            test_file: "tests/login.test.toml",
            extra_system_prompt: None,
            base_url: None,
        };
        let run_id = RunId("run-1".to_string());
        let session_id = SessionId("sess-1".to_string());
        let content = build_bootstrap_content(&config, 3, &run_id, &session_id);

        assert!(content.contains("RESULT OK"));
        assert!(content.contains("RESULT WARN:"));
        assert!(content.contains("RESULT ERROR:"));
        assert!(content.contains("BUGATTI_LOG"));
    }

    #[test]
    fn build_bootstrap_content_includes_test_metadata() {
        let config = BootstrapConfig {
            test_name: "Login test",
            test_file: "tests/login.test.toml",
            extra_system_prompt: None,
            base_url: None,
        };
        let run_id = RunId("run-abc".to_string());
        let session_id = SessionId("sess-xyz".to_string());
        let content = build_bootstrap_content(&config, 5, &run_id, &session_id);

        assert!(content.contains("Login test"));
        assert!(content.contains("tests/login.test.toml"));
        assert!(content.contains("Steps: 5"));
        assert!(content.contains("run-abc"));
        assert!(content.contains("sess-xyz"));
    }

    #[test]
    fn build_bootstrap_content_includes_extra_system_prompt() {
        let config = BootstrapConfig {
            test_name: "Test",
            test_file: "test.test.toml",
            extra_system_prompt: Some("Be concise and thorough"),
            base_url: None,
        };
        let run_id = RunId("run-1".to_string());
        let session_id = SessionId("sess-1".to_string());
        let content = build_bootstrap_content(&config, 1, &run_id, &session_id);

        assert!(content.contains("Be concise and thorough"));
        // Extra system prompt should appear before the harness instructions
        let prompt_pos = content.find("Be concise").unwrap();
        let contract_pos = content.find("Result Contract").unwrap();
        assert!(prompt_pos < contract_pos);
    }

    #[test]
    fn build_bootstrap_content_omits_prompt_when_none() {
        let config = BootstrapConfig {
            test_name: "Test",
            test_file: "test.test.toml",
            extra_system_prompt: None,
            base_url: None,
        };
        let run_id = RunId("run-1".to_string());
        let session_id = SessionId("sess-1".to_string());
        let content = build_bootstrap_content(&config, 1, &run_id, &session_id);

        // Should start with harness instructions, not a blank line
        assert!(content.starts_with("You are being driven"));
    }

    #[test]
    fn build_bootstrap_content_includes_base_url() {
        let config = BootstrapConfig {
            test_name: "Test",
            test_file: "test.test.toml",
            extra_system_prompt: None,
            base_url: Some("http://localhost:3000"),
        };
        let run_id = RunId("run-1".to_string());
        let session_id = SessionId("sess-1".to_string());
        let content = build_bootstrap_content(&config, 1, &run_id, &session_id);
        assert!(content.contains("- Base URL: http://localhost:3000"));
    }

    #[test]
    fn build_bootstrap_content_omits_base_url_when_none() {
        let config = BootstrapConfig {
            test_name: "Test",
            test_file: "test.test.toml",
            extra_system_prompt: None,
            base_url: None,
        };
        let run_id = RunId("run-1".to_string());
        let session_id = SessionId("sess-1".to_string());
        let content = build_bootstrap_content(&config, 1, &run_id, &session_id);
        assert!(!content.contains("Base URL"));
    }

    #[test]
    fn execute_steps_with_bootstrap_writes_transcript() {
        let steps = vec![test_steps().remove(0)];
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text("RESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ]]);

        let bootstrap = BootstrapConfig {
            test_name: "Test",
            test_file: "test.test.toml",
            extra_system_prompt: None,
            base_url: None,
        };

        let _outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
            Some(&bootstrap),
            &AtomicBool::new(false),
        )
        .unwrap();

        // Bootstrap transcript file should exist
        let bootstrap_path = artifact_dir.transcripts.join("bootstrap.txt");
        assert!(bootstrap_path.is_file());

        // Full transcript should contain bootstrap section
        let full = std::fs::read_to_string(artifact_dir.transcripts.join("full_transcript.txt")).unwrap();
        assert!(full.contains("=== Bootstrap ==="));
    }

    // --- Interrupt tests ---

    #[test]
    fn execute_steps_interrupted_between_steps() {
        let steps = test_steps(); // 2 steps
        let (run_id, session_id) = test_run_ids();
        let (_tmp, artifact_dir) = test_artifact_dir();

        let mut session = MockSession::new(vec![
            vec![
                Ok(OutputChunk::Text("RESULT OK".to_string())),
                Ok(OutputChunk::Done),
            ],
            vec![
                Ok(OutputChunk::Text("RESULT OK".to_string())),
                Ok(OutputChunk::Done),
            ],
        ]);

        // Set interrupted flag — will be checked before step 2
        let flag = AtomicBool::new(false);

        // We can't set the flag between steps in this test easily,
        // so set it before execution starts — step 1 won't even run
        flag.store(true, Ordering::Relaxed);

        let outcome = execute_steps(
            &mut session,
            &steps,
            &run_id,
            &session_id,
            &artifact_dir,
            None,
            None,
            &flag,
        )
        .unwrap();

        // No steps should have executed
        assert_eq!(outcome.steps.len(), 0);
    }
}
