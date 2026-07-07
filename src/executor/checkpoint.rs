//! Checkpoint restore/save orchestration around step execution.

use std::path::Path;

use crate::config::CheckpointConfig;
use crate::expand::ExpandedStep;
use crate::progress::ProgressReporter;

use super::types::ExecutorError;

/// Restore the last checkpoint among leading skipped steps, if any, before
/// executing non-skipped steps.
pub(super) async fn restore_leading_checkpoint(
    steps: &[ExpandedStep],
    cp_config: &CheckpointConfig,
    project_root: &Path,
    reporter: &dyn ProgressReporter,
) -> Result<(), ExecutorError> {
    let first_non_skipped = steps.iter().position(|s| !s.skip && !s.setup);
    let Some(boundary) = first_non_skipped else {
        return Ok(());
    };
    if boundary == 0 {
        return Ok(());
    }

    // Find the last checkpoint among skipped steps before the boundary
    let last_cp = steps[..boundary]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, s)| s.checkpoint.as_deref().map(|cp| (i, cp)));

    let Some((cp_step_idx, cp_id)) = last_cp else {
        return Ok(());
    };

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
    Ok(())
}

/// Save the named checkpoint after a passing step.
pub(super) async fn save_step_checkpoint(
    cp_config: &CheckpointConfig,
    cp_id: &str,
    project_root: &Path,
    reporter: &dyn ProgressReporter,
) -> Result<(), ExecutorError> {
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
    Ok(())
}
