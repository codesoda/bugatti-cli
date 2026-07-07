//! Console progress rendering for step execution.

use std::time::Duration;

use crate::progress::ProgressReporter;

use super::types::{StepOutcome, StepResult, StepVerdict};

/// Truncate an instruction string to a maximum length, appending "..." if truncated.
pub(super) fn truncate_instruction(instruction: &str, max_len: usize) -> String {
    // Take first line only for the summary
    let first_line = instruction.lines().next().unwrap_or(instruction);
    if first_line.len() <= max_len {
        first_line.to_string()
    } else {
        format!("{}...", &first_line[..max_len])
    }
}

/// Print the result of a step to the console.
pub(super) fn print_step_result(
    reporter: &dyn ProgressReporter,
    result: &StepResult,
    duration: Duration,
) {
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
pub(super) fn print_run_summary(
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
