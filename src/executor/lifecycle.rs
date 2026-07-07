//! Session lifecycle exchanges: the bootstrap message sent before any steps
//! and the best-effort teardown message sent after the last step.

use std::time::{Duration, Instant};

use crate::progress::ProgressReporter;
use crate::provider::{AgentSession, BootstrapMessage, OutputChunk, StepMessage};
use crate::run::{ArtifactDir, RunId, SessionId};

use super::bootstrap::{build_bootstrap_content, BootstrapConfig};
use super::transcript::TranscriptWriter;
use super::types::ExecutorError;

/// Send the bootstrap message and capture its transcript.
///
/// Stream errors are non-fatal (the transcript is simply truncated); a send
/// failure aborts the run.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_bootstrap(
    session: &mut dyn AgentSession,
    config: &BootstrapConfig<'_>,
    total_steps: usize,
    run_id: &RunId,
    session_id: &SessionId,
    artifact_dir: &ArtifactDir,
    transcript_writer: &mut TranscriptWriter,
    reporter: &dyn ProgressReporter,
) -> Result<(), ExecutorError> {
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
            transcript_writer.write_bootstrap(artifact_dir, &bootstrap_transcript);
            let bootstrap_duration = bootstrap_start.elapsed();
            tracing::info!(
                duration_ms = bootstrap_duration.as_millis() as u64,
                "bootstrap complete"
            );
            reporter.line(&format!(
                "OK ......... bootstrap ({:.1}s)",
                bootstrap_duration.as_secs_f64()
            ));
            Ok(())
        }
        Err(e) => {
            tracing::error!(error = %e, "bootstrap send failed");
            Err(ExecutorError::Provider(e))
        }
    }
}

/// Send the teardown message after all steps (best-effort, non-fatal).
pub(super) async fn run_teardown(
    session: &mut dyn AgentSession,
    run_id: &RunId,
    session_id: &SessionId,
    total_steps: usize,
    transcript_writer: &mut TranscriptWriter,
    reporter: &dyn ProgressReporter,
) {
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
            transcript_writer.append_teardown_section(&teardown_transcript);
            reporter.line("OK ......... teardown");
        }
        Err(e) => {
            tracing::warn!(error = %e, "teardown send failed (non-fatal)");
            reporter.line(&format!("WARN ....... teardown failed: {e}"));
        }
    }
}
