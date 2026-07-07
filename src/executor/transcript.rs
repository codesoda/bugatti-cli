//! Incremental transcript artifact writing for a run.
//!
//! Transcripts are captured as execution progresses, not reconstructed after
//! the fact. Write failures are non-fatal: they are recorded as artifact
//! errors and surfaced in the run outcome.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use crate::run::ArtifactDir;

use super::types::{StepOutcome, StepResult};

/// Writes transcript artifacts incrementally during step execution and
/// collects non-fatal artifact errors along the way.
pub(super) struct TranscriptWriter {
    full_transcript_file: Option<std::fs::File>,
    errors: Vec<String>,
}

impl TranscriptWriter {
    /// Open `full_transcript.txt` for incremental writing. A create failure
    /// is recorded as an artifact error and disables full-transcript output.
    pub(super) fn new(artifact_dir: &ArtifactDir) -> Self {
        let full_transcript_path = artifact_dir.transcripts.join("full_transcript.txt");
        let mut errors = Vec::new();
        let full_transcript_file = match std::fs::File::create(&full_transcript_path) {
            Ok(f) => Some(f),
            Err(e) => {
                let msg = format!(
                    "failed to create full transcript file '{}': {e}",
                    full_transcript_path.display()
                );
                tracing::error!("{msg}");
                errors.push(msg);
                None
            }
        };
        Self {
            full_transcript_file,
            errors,
        }
    }

    /// Record a non-fatal artifact error.
    fn record_error(&mut self, msg: String) {
        tracing::error!("{msg}");
        self.errors.push(msg);
    }

    /// Write the bootstrap transcript artifact and full-transcript section.
    pub(super) fn write_bootstrap(&mut self, artifact_dir: &ArtifactDir, transcript: &str) {
        let bootstrap_path = artifact_dir.transcripts.join("bootstrap.txt");
        if let Err(e) = std::fs::write(&bootstrap_path, transcript) {
            self.record_error(format!("failed to write bootstrap transcript: {e}"));
        }
        if let Some(ref mut f) = self.full_transcript_file {
            let _ = writeln!(f, "=== Bootstrap ===");
            let _ = writeln!(f, "{}", transcript);
            let _ = writeln!(f);
        }
    }

    /// Write the per-step transcript artifact, returning its path.
    pub(super) fn write_step_transcript(
        &mut self,
        artifact_dir: &ArtifactDir,
        step_id: usize,
        transcript: &str,
    ) -> PathBuf {
        let transcript_path = artifact_dir
            .transcripts
            .join(format!("step_{:04}.txt", step_id));
        if let Err(e) = std::fs::write(&transcript_path, transcript) {
            self.record_error(format!(
                "failed to write transcript for step {} to '{}': {e}",
                step_id,
                transcript_path.display()
            ));
        }
        transcript_path
    }

    /// Append a step section to the full transcript.
    pub(super) fn append_step_section(
        &mut self,
        step_id: usize,
        instruction: &str,
        result: &StepResult,
        duration: Duration,
        transcript: &str,
    ) {
        if let Some(ref mut f) = self.full_transcript_file {
            let write_result = (|| -> std::io::Result<()> {
                writeln!(f, "=== Step {} ===", step_id)?;
                writeln!(f, "Instruction: {}", instruction)?;
                writeln!(f, "Result: {}", result)?;
                writeln!(f, "Duration: {:.1}s", duration.as_secs_f64())?;
                writeln!(f, "---")?;
                writeln!(f, "{}", transcript)?;
                writeln!(f)?;
                Ok(())
            })();
            if let Err(e) = write_result {
                self.record_error(format!(
                    "failed to append step {} to full transcript: {e}",
                    step_id
                ));
            }
        }
    }

    /// Append the teardown section to the full transcript.
    pub(super) fn append_teardown_section(&mut self, transcript: &str) {
        if let Some(ref mut f) = self.full_transcript_file {
            let _ = writeln!(f, "=== Teardown ===");
            let _ = writeln!(f, "{}", transcript);
            let _ = writeln!(f);
        }
    }

    /// Close the full transcript and return collected artifact errors.
    pub(super) fn finish(self) -> Vec<String> {
        self.errors
    }
}

/// Write parsed BUGATTI_LOG events to a dedicated artifact file
/// (distinct from transcripts and diagnostics).
pub(super) fn write_log_events(artifact_dir: &ArtifactDir, outcomes: &[StepOutcome]) {
    let log_events_path = artifact_dir.logs.join("bugatti_log_events.txt");
    let all_log_events: Vec<_> = outcomes.iter().flat_map(|o| &o.log_events).collect();
    if !all_log_events.is_empty() {
        if let Ok(mut f) = std::fs::File::create(&log_events_path) {
            for event in &all_log_events {
                let _ = writeln!(f, "[step {}] {}", event.step_id, event.message);
            }
        }
    }
}
