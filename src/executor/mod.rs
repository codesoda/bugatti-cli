use crate::diagnostics::{EvidenceKind, EvidenceRef};
use crate::expand::ExpandedStep;
use crate::provider::{AgentSession, BootstrapMessage, OutputChunk, ProviderError, StepMessage};
use crate::run::{ArtifactDir, RunId, SessionId};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

mod bootstrap;
mod markers;

#[cfg(test)]
mod tests;

pub use bootstrap::{build_bootstrap_content, BootstrapConfig};
pub use markers::{parse_log_events, parse_result_marker, LogEvent};

use crate::progress::{ProgressReporter, STDOUT_PROGRESS_REPORTER};

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

/// Execute all expanded steps sequentially within one provider session.
///
/// Returns the run outcome with all step results.
/// The provider session must already be initialized and started.
#[allow(clippy::too_many_arguments)]
pub async fn execute_steps(
    session: &mut dyn AgentSession,
    steps: &[ExpandedStep],
    run_id: &RunId,
    session_id: &SessionId,
    artifact_dir: &ArtifactDir,
    step_timeout: Option<Duration>,
    bootstrap_config: Option<&BootstrapConfig<'_>>,
    checkpoint_config: Option<&crate::config::CheckpointConfig>,
    project_root: &std::path::Path,
    interrupted: &AtomicBool,
) -> Result<RunOutcome, ExecutorError> {
    execute_steps_with_reporter(
        session,
        steps,
        run_id,
        session_id,
        artifact_dir,
        step_timeout,
        bootstrap_config,
        checkpoint_config,
        &STDOUT_PROGRESS_REPORTER,
        project_root,
        interrupted,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_steps_with_reporter(
    session: &mut dyn AgentSession,
    steps: &[ExpandedStep],
    run_id: &RunId,
    session_id: &SessionId,
    artifact_dir: &ArtifactDir,
    step_timeout: Option<Duration>,
    bootstrap_config: Option<&BootstrapConfig<'_>>,
    checkpoint_config: Option<&crate::config::CheckpointConfig>,
    reporter: &dyn ProgressReporter,
    project_root: &std::path::Path,
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
        reporter.line("BOOTSTRAP .. sending harness instructions");

        let bootstrap_msg = BootstrapMessage {
            run_id: run_id.clone(),
            session_id: session_id.clone(),
            content,
        };

        let bootstrap_start = Instant::now();
        match session.send_bootstrap(bootstrap_msg).await {
            Ok(mut stream) => {
                let mut bootstrap_transcript = String::new();
                while let Some(chunk) = stream.next_chunk().await {
                    match chunk {
                        Ok(OutputChunk::Text(text)) => bootstrap_transcript.push_str(&text),
                        Ok(OutputChunk::Done) => break,
                        Err(e) => {
                            tracing::warn!(error = %e, "bootstrap stream error (non-fatal)");
                            break;
                        }
                    }
                }
                drop(stream);
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
                tracing::info!(
                    duration_ms = bootstrap_duration.as_millis() as u64,
                    "bootstrap complete"
                );
                reporter.line(&format!(
                    "OK ......... bootstrap ({:.1}s)",
                    bootstrap_duration.as_secs_f64()
                ));
            }
            Err(e) => {
                tracing::error!(error = %e, "bootstrap send failed");
                return Err(ExecutorError::Provider(e));
            }
        }
    }

    // Checkpoint restore: find the last checkpoint among leading skipped steps
    // and restore it before executing non-skipped steps.
    if let Some(cp_config) = checkpoint_config {
        let first_non_skipped = steps.iter().position(|s| !s.skip && !s.setup);
        if let Some(boundary) = first_non_skipped {
            if boundary > 0 {
                // Find the last checkpoint among skipped steps before the boundary
                let last_cp = steps[..boundary]
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(i, s)| s.checkpoint.as_deref().map(|cp| (i, cp)));

                if let Some((cp_step_idx, cp_id)) = last_cp {
                    let skipped_after = boundary - cp_step_idx - 1;
                    if skipped_after > 0 {
                        reporter.line(&format!(
                            "WARN ....... restoring checkpoint \"{}\" from step {}, but {} step(s) after it were also skipped without checkpoints",
                            cp_id, cp_step_idx + 1, skipped_after
                        ));
                    }

                    reporter.line(&format!("RESTORE .... checkpoint \"{cp_id}\""));
                    if let Err(e) = crate::command::run_checkpoint_command_with_reporter(
                        &cp_config.restore,
                        cp_id,
                        project_root,
                        cp_config.timeout_secs,
                        reporter,
                    )
                    .await
                    {
                        reporter.line(&format!("FAIL ....... checkpoint restore: {e}"));
                        return Err(ExecutorError::CheckpointFailed(format!(
                            "restore \"{cp_id}\" failed: {e}"
                        )));
                    }
                    reporter.line(&format!("OK ......... checkpoint \"{cp_id}\" restored"));
                }
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
        let display_source = std::env::current_dir()
            .ok()
            .and_then(|cwd| {
                step.source_file
                    .strip_prefix(&cwd)
                    .ok()
                    .map(|p| p.display().to_string())
            })
            .unwrap_or_else(|| step.source_file.display().to_string());

        // Handle skipped steps (setup steps bypass skip)
        if step.skip && !step.setup {
            reporter.line(&format!(
                "SKIP {}/{} ... {} (from {})",
                step.step_id + 1,
                total_steps,
                instruction_summary,
                display_source
            ));
            outcomes.push(StepOutcome {
                step_id: step.step_id,
                instruction: step.instruction.clone(),
                source_file: step.source_file.clone(),
                setup: false,
                result: StepResult::Verdict(StepVerdict::Ok),
                transcript: String::new(),
                log_events: vec![],
                evidence_refs: vec![],
                duration: Duration::ZERO,
            });
            continue;
        }

        let step_label = if step.setup { "SETUP" } else { "STEP" };
        tracing::info!(
            step_id = step.step_id,
            total = total_steps,
            setup = step.setup,
            source = %step.source_file.display(),
            "step execution begin"
        );
        reporter.line(&format!(
            "{step_label} {}/{} ... {} (from {})",
            step.step_id + 1,
            total_steps,
            instruction_summary,
            display_source
        ));

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
        let effective_timeout = step
            .step_timeout_secs
            .map(Duration::from_secs)
            .unwrap_or(timeout);

        let result = execute_single_step(session, message, &effective_timeout, interrupted).await;

        let duration = step_start.elapsed();

        let (step_result, transcript) = match result {
            Ok((transcript_text, verdict)) => (verdict, transcript_text),
            Err((transcript_text, err_result)) => {
                // Setup steps tolerate missing RESULT markers — the agent just runs the command
                if step.setup && matches!(err_result, StepResult::ProtocolError(_)) {
                    (StepResult::Verdict(StepVerdict::Ok), transcript_text)
                } else {
                    (err_result, transcript_text)
                }
            }
        };

        // Parse BUGATTI_LOG events from the transcript
        let log_events = parse_log_events(&transcript, &run_id.0, step.step_id);

        // Render log events to console
        for event in &log_events {
            reporter.line(&format!("LOG ........ {}", event.message));
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
        print_step_result(reporter, &step_result, duration);

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
        let evidence_refs = if !step_result.is_pass()
            || matches!(&step_result, StepResult::Verdict(StepVerdict::Warn(_)))
        {
            vec![EvidenceRef {
                kind: EvidenceKind::CommandLog,
                path: transcript_path.clone(),
                description: format!("Step {} transcript", step.step_id),
                collection_error: if transcript_path.exists() {
                    None
                } else {
                    Some("transcript file not written".to_string())
                },
            }]
        } else {
            vec![]
        };

        let outcome = StepOutcome {
            step_id: step.step_id,
            instruction: step.instruction.clone(),
            source_file: step.source_file.clone(),
            setup: step.setup,
            result: step_result,
            transcript,
            log_events,
            evidence_refs,
            duration,
        };

        let is_failure = outcome.result.is_failure();
        outcomes.push(outcome);

        // Save checkpoint after a passing step (not on failure)
        if !is_failure {
            if let (Some(cp_config), Some(cp_id)) = (checkpoint_config, step.checkpoint.as_deref())
            {
                reporter.line(&format!("SAVE ....... checkpoint \"{cp_id}\""));
                if let Err(e) = crate::command::run_checkpoint_command_with_reporter(
                    &cp_config.save,
                    cp_id,
                    project_root,
                    cp_config.timeout_secs,
                    reporter,
                )
                .await
                {
                    reporter.line(&format!("FAIL ....... checkpoint save: {e}"));
                    return Err(ExecutorError::CheckpointFailed(format!(
                        "save \"{cp_id}\" failed: {e}"
                    )));
                }
                reporter.line(&format!("OK ......... checkpoint \"{cp_id}\" saved"));
            }
        }

        // Stop on failure
        if is_failure {
            tracing::warn!(
                step_id = step.step_id,
                "stopping execution due to step failure"
            );
            break;
        }
    }

    // Send teardown message before summary (best-effort, non-fatal)
    if !interrupted.load(Ordering::Relaxed) {
        reporter.line("TEARDOWN .. cleaning up");
        let teardown_msg = StepMessage {
            run_id: run_id.clone(),
            session_id: session_id.clone(),
            step_id: total_steps,
            total_steps,
            source_file: "teardown".to_string(),
            instruction: "All test steps are complete. Close any browsers, tools, or resources you opened during this session.".to_string(),
        };
        match session.send_step(teardown_msg).await {
            Ok(mut stream) => {
                let teardown_start = Instant::now();
                let teardown_timeout = Duration::from_secs(30);
                let mut teardown_transcript = String::new();
                loop {
                    let remaining = match teardown_timeout.checked_sub(teardown_start.elapsed()) {
                        Some(r) => r,
                        None => {
                            tracing::warn!("teardown timed out");
                            break;
                        }
                    };
                    let chunk = match tokio::time::timeout(remaining, stream.next_chunk()).await {
                        Ok(Some(chunk)) => chunk,
                        Ok(None) => break,
                        Err(_) => {
                            tracing::warn!("teardown timed out");
                            break;
                        }
                    };
                    match chunk {
                        Ok(OutputChunk::Text(text)) => teardown_transcript.push_str(&text),
                        Ok(OutputChunk::Done) => break,
                        Err(e) => {
                            tracing::warn!(error = %e, "teardown stream error");
                            break;
                        }
                    }
                }
                drop(stream);
                if let Some(ref mut f) = full_transcript_file {
                    let _ = writeln!(f, "=== Teardown ===");
                    let _ = writeln!(f, "{}", teardown_transcript);
                    let _ = writeln!(f);
                }
                reporter.line("OK ......... teardown");
            }
            Err(e) => {
                tracing::warn!(error = %e, "teardown send failed (non-fatal)");
                reporter.line(&format!("WARN ....... teardown failed: {e}"));
            }
        }
    }

    // Setup steps don't count toward pass/fail — only test steps determine the verdict.
    // A failed setup step aborts the run (handled above), so if we reach here,
    // all setup steps succeeded.
    // If the run was interrupted, force all_passed to false — partial runs are not passing.
    // Load once to avoid TOCTOU race between all_passed and print_run_summary.
    let was_interrupted = interrupted.load(Ordering::Relaxed);
    let all_passed = !was_interrupted
        && outcomes
            .iter()
            .filter(|o| !o.setup)
            .all(|o| o.result.is_pass());
    let total_duration = run_start.elapsed();

    // Print final run status (after teardown)
    print_run_summary(
        reporter,
        &outcomes,
        total_duration,
        total_steps,
        was_interrupted,
    );

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
fn print_step_result(reporter: &dyn ProgressReporter, result: &StepResult, duration: Duration) {
    let duration_str = format!("{:.1}s", duration.as_secs_f64());
    match result {
        StepResult::Verdict(StepVerdict::Ok) => {
            reporter.line(&format!("OK ......... ({duration_str})"));
        }
        StepResult::Verdict(StepVerdict::Warn(msg)) => {
            reporter.line(&format!("WARN ....... {msg} ({duration_str})"));
        }
        StepResult::Verdict(StepVerdict::Error(msg)) => {
            reporter.line(&format!("ERROR ...... {msg} ({duration_str})"));
        }
        StepResult::ProtocolError(msg) => {
            reporter.line(&format!(
                "FAIL ....... protocol error: {msg} ({duration_str})"
            ));
        }
        StepResult::Timeout => {
            reporter.line(&format!("FAIL ....... step timed out ({duration_str})"));
        }
        StepResult::ProviderFailed(msg) => {
            reporter.line(&format!(
                "FAIL ....... provider error: {msg} ({duration_str})"
            ));
        }
    }
}

/// Print a summary of the full run after all steps have completed.
fn print_run_summary(
    reporter: &dyn ProgressReporter,
    outcomes: &[StepOutcome],
    total_duration: Duration,
    total_steps: usize,
    was_interrupted: bool,
) {
    reporter.line("");
    reporter.line("═══════════════════════════════════════════════════");

    let test_outcomes: Vec<_> = outcomes.iter().filter(|o| !o.setup).collect();
    let setup_count = outcomes.iter().filter(|o| o.setup).count();
    let completed = test_outcomes.len();
    let ok_count = test_outcomes
        .iter()
        .filter(|o| matches!(o.result, StepResult::Verdict(StepVerdict::Ok)))
        .count();
    let warn_count = test_outcomes
        .iter()
        .filter(|o| matches!(o.result, StepResult::Verdict(StepVerdict::Warn(_))))
        .count();
    let fail_count = test_outcomes
        .iter()
        .filter(|o| o.result.is_failure())
        .count();
    let skipped = total_steps - completed - setup_count;

    let all_passed = !was_interrupted && test_outcomes.iter().all(|o| o.result.is_pass());
    let status = if was_interrupted {
        "INTERRUPTED"
    } else if all_passed {
        "PASSED"
    } else {
        "FAILED"
    };

    let setup_part = if setup_count > 0 {
        format!(", {setup_count} setup")
    } else {
        String::new()
    };
    reporter.line(&format!(
        "Run {status}: {ok_count} ok, {warn_count} warn, {fail_count} failed, {skipped} skipped{setup_part} ({:.1}s)",
        total_duration.as_secs_f64()
    ));
    reporter.line("═══════════════════════════════════════════════════");
}

/// Execute a single step, collecting transcript and parsing the result.
///
/// Returns Ok((transcript, StepResult)) on successful completion,
/// or Err((transcript, StepResult)) on failure.
///
/// The timeout is enforced with `tokio::time::timeout` around each chunk read,
/// so a provider that hangs mid-stream is detected as soon as the deadline passes
/// (not only after the next chunk arrives).
async fn execute_single_step(
    session: &mut dyn AgentSession,
    message: StepMessage,
    timeout: &Duration,
    interrupted: &AtomicBool,
) -> Result<(String, StepResult), (String, StepResult)> {
    let start = Instant::now();
    let mut transcript = String::new();

    let mut stream = match session.send_step(message).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "provider send_step failed");
            return Err((transcript, StepResult::ProviderFailed(e.to_string())));
        }
    };

    loop {
        // Check timeout and compute the remaining budget for the next chunk
        let remaining = match timeout.checked_sub(start.elapsed()) {
            Some(r) => r,
            None => {
                tracing::error!(
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "step timed out during streaming"
                );
                drop(stream);
                return Err((transcript, StepResult::Timeout));
            }
        };

        // Check for Ctrl+C interruption
        if interrupted.load(Ordering::Relaxed) {
            tracing::warn!("step interrupted by Ctrl+C during streaming");
            drop(stream);
            return Err((
                transcript,
                StepResult::ProviderFailed("interrupted by Ctrl+C".to_string()),
            ));
        }

        let chunk_result = match tokio::time::timeout(remaining, stream.next_chunk()).await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => {
                tracing::error!(
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "step timed out waiting for provider output"
                );
                drop(stream);
                return Err((transcript, StepResult::Timeout));
            }
        };

        match chunk_result {
            Ok(OutputChunk::Text(text)) => {
                transcript.push_str(&text);
            }
            Ok(OutputChunk::Done) => {
                break;
            }
            Err(e) => {
                drop(stream);
                return Err((transcript, StepResult::ProviderFailed(e.to_string())));
            }
        }
    }
    drop(stream);

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
