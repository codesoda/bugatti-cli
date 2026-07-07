//! Step execution engine.
//!
//! Drives a provider session through the expanded steps of a test run:
//! bootstrap, per-step streaming with timeouts, checkpoint restore/save,
//! transcript artifact capture, teardown, and the final run summary.
//!
//! Module layout:
//! - [`types`]: step verdicts, outcomes, and executor errors
//! - [`markers`]: RESULT / BUGATTI_LOG contract parsing
//! - [`bootstrap`]: bootstrap message content
//! - [`step`]: single-step streaming execution
//! - [`lifecycle`]: bootstrap/teardown session exchanges
//! - [`checkpoint`]: checkpoint restore/save orchestration
//! - [`transcript`]: incremental transcript artifact writing
//! - [`console`]: progress/summary rendering

use crate::diagnostics::{EvidenceKind, EvidenceRef};
use crate::expand::ExpandedStep;
use crate::provider::{AgentSession, StepMessage};
use crate::run::{ArtifactDir, RunId, SessionId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

mod bootstrap;
mod checkpoint;
mod console;
mod lifecycle;
mod markers;
mod step;
mod transcript;
mod types;

#[cfg(test)]
mod tests;

pub use bootstrap::{build_bootstrap_content, BootstrapConfig};
pub use markers::{parse_log_events, parse_result_marker, LogEvent};
pub use types::{ExecutorError, RunOutcome, StepOutcome, StepResult, StepVerdict};

use crate::progress::{ProgressReporter, STDOUT_PROGRESS_REPORTER};
use console::{print_run_summary, print_step_result, truncate_instruction};
use step::execute_single_step;
use transcript::TranscriptWriter;

/// Default step timeout in seconds.
const DEFAULT_STEP_TIMEOUT_SECS: u64 = 300;

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
    let run_start = Instant::now();

    let total_steps = steps.len();

    let mut transcript_writer = TranscriptWriter::new(artifact_dir);

    // Send bootstrap message before any steps
    if let Some(config) = bootstrap_config {
        lifecycle::run_bootstrap(
            session,
            config,
            total_steps,
            run_id,
            session_id,
            artifact_dir,
            &mut transcript_writer,
            reporter,
        )
        .await?;
    }

    // Checkpoint restore: find the last checkpoint among leading skipped steps
    // and restore it before executing non-skipped steps.
    if let Some(cp_config) = checkpoint_config {
        checkpoint::restore_leading_checkpoint(steps, cp_config, project_root, reporter).await?;
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

        // Write transcript artifacts (per-step file and full-transcript section)
        let transcript_path =
            transcript_writer.write_step_transcript(artifact_dir, step.step_id, &transcript);
        transcript_writer.append_step_section(
            step.step_id,
            &step.instruction,
            &step_result,
            duration,
            &transcript,
        );

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
                checkpoint::save_step_checkpoint(cp_config, cp_id, project_root, reporter).await?;
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
        lifecycle::run_teardown(
            session,
            run_id,
            session_id,
            total_steps,
            &mut transcript_writer,
            reporter,
        )
        .await;
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

    // Flush/close the full transcript file and collect artifact errors
    let artifact_errors = transcript_writer.finish();

    // Write log events to a separate file (distinct from transcript and diagnostics)
    transcript::write_log_events(artifact_dir, &outcomes);

    Ok(RunOutcome {
        steps: outcomes,
        all_passed,
        total_duration,
        artifact_errors,
    })
}
