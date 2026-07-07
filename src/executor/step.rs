//! Single-step execution: streaming provider output with timeout and
//! interruption handling, then parsing the RESULT contract.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::provider::{AgentSession, OutputChunk, StepMessage};

use super::markers::parse_result_marker;
use super::types::StepResult;

/// Execute a single step, collecting transcript and parsing the result.
///
/// Returns Ok((transcript, StepResult)) on successful completion,
/// or Err((transcript, StepResult)) on failure.
///
/// The timeout is enforced with `tokio::time::timeout` around each chunk read,
/// so a provider that hangs mid-stream is detected as soon as the deadline passes
/// (not only after the next chunk arrives).
pub(super) async fn execute_single_step(
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
