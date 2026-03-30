//! Stable exit codes for the Bugatti CLI.
//!
//! Exit codes are designed for script and CI consumption:
//!
//! | Code | Meaning |
//! |------|---------|
//! | 0    | All steps passed (OK or WARN) |
//! | 1    | One or more steps had ERROR results |
//! | 2    | Configuration, parse, or cycle error before execution |
//! | 3    | Provider startup or readiness check failure |
//! | 4    | Timeout during step execution |
//! | 5    | Run was interrupted (Ctrl+C / SIGINT) |

/// All steps passed (OK or WARN only).
pub const EXIT_OK: i32 = 0;

/// One or more steps had ERROR, protocol error, or provider failure.
pub const EXIT_STEP_ERROR: i32 = 1;

/// Configuration, parse, cycle, or validation error before execution.
pub const EXIT_CONFIG_ERROR: i32 = 2;

/// Provider startup or readiness check failure.
pub const EXIT_PROVIDER_ERROR: i32 = 3;

/// Step execution timeout.
pub const EXIT_TIMEOUT: i32 = 4;

/// Run was interrupted (Ctrl+C / SIGINT).
pub const EXIT_INTERRUPTED: i32 = 5;

use crate::executor::{RunOutcome, StepResult, StepVerdict};

/// Compute the exit code for a single completed run.
///
/// - All OK/WARN -> 0
/// - Any ERROR verdict -> 1
/// - Any timeout -> 4
/// - Any provider/protocol error -> 1
pub fn exit_code_for_run(outcome: &RunOutcome) -> i32 {
    if outcome.all_passed {
        return EXIT_OK;
    }

    // Find the most specific failure type
    let mut has_timeout = false;
    for step in &outcome.steps {
        match &step.result {
            StepResult::Timeout => has_timeout = true,
            StepResult::Verdict(_)
            | StepResult::ProtocolError(_)
            | StepResult::ProviderFailed(_) => {}
        }
    }

    if has_timeout {
        EXIT_TIMEOUT
    } else {
        EXIT_STEP_ERROR
    }
}

/// Compute the exit code for a run, optionally treating WARN as failure.
///
/// When `strict_warnings` is true, any WARN verdict produces EXIT_STEP_ERROR
/// instead of EXIT_OK.
pub fn exit_code_for_run_strict(outcome: &RunOutcome, strict_warnings: bool) -> i32 {
    if !strict_warnings {
        return exit_code_for_run(outcome);
    }
    // With strict warnings: only pure OK is passing
    let has_timeout = outcome.steps.iter().any(|s| matches!(&s.result, StepResult::Timeout));
    if has_timeout {
        return EXIT_TIMEOUT;
    }
    let all_ok = outcome.steps.iter().all(|s| matches!(&s.result, StepResult::Verdict(StepVerdict::Ok)));
    if all_ok {
        EXIT_OK
    } else {
        EXIT_STEP_ERROR
    }
}

/// Compute aggregate exit code from multiple run results.
///
/// Returns the highest (most severe) exit code among all runs.
/// Parse errors before execution contribute EXIT_CONFIG_ERROR.
pub fn aggregate_exit_code(run_exit_codes: &[i32], has_parse_errors: bool) -> i32 {
    let max_run = run_exit_codes.iter().copied().max().unwrap_or(EXIT_OK);
    if has_parse_errors {
        max_run.max(EXIT_CONFIG_ERROR)
    } else {
        max_run
    }
}

/// Format exit code as a human-readable description.
pub fn describe_exit_code(code: i32) -> &'static str {
    match code {
        EXIT_OK => "all tests passed",
        EXIT_STEP_ERROR => "one or more steps failed",
        EXIT_CONFIG_ERROR => "configuration or parse error",
        EXIT_PROVIDER_ERROR => "provider or readiness failure",
        EXIT_TIMEOUT => "step execution timeout",
        EXIT_INTERRUPTED => "run interrupted",
        _ => "unknown exit code",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{StepOutcome, StepResult, StepVerdict};
    use std::path::PathBuf;
    use std::time::Duration;

    fn make_outcome(results: Vec<StepResult>) -> RunOutcome {
        let all_passed = results.iter().all(|r| r.is_pass());
        let steps = results
            .into_iter()
            .enumerate()
            .map(|(i, result)| StepOutcome {
                step_id: i,
                instruction: format!("step {i}"),
                source_file: PathBuf::from("test.test.toml"),
                result,
                transcript: String::new(),
                log_events: vec![],
                evidence_refs: vec![],
                duration: Duration::from_millis(100),
            })
            .collect();
        RunOutcome {
            steps,
            all_passed,
            total_duration: Duration::from_secs(1),
            artifact_errors: vec![],
        }
    }

    #[test]
    fn exit_code_all_ok() {
        let outcome = make_outcome(vec![
            StepResult::Verdict(StepVerdict::Ok),
            StepResult::Verdict(StepVerdict::Ok),
        ]);
        assert_eq!(exit_code_for_run(&outcome), EXIT_OK);
    }

    #[test]
    fn exit_code_warn_is_pass() {
        let outcome = make_outcome(vec![
            StepResult::Verdict(StepVerdict::Ok),
            StepResult::Verdict(StepVerdict::Warn("slow".to_string())),
        ]);
        assert_eq!(exit_code_for_run(&outcome), EXIT_OK);
    }

    #[test]
    fn exit_code_error_step() {
        let outcome = make_outcome(vec![StepResult::Verdict(StepVerdict::Error(
            "page not found".to_string(),
        ))]);
        assert_eq!(exit_code_for_run(&outcome), EXIT_STEP_ERROR);
    }

    #[test]
    fn exit_code_timeout() {
        let outcome = make_outcome(vec![StepResult::Timeout]);
        assert_eq!(exit_code_for_run(&outcome), EXIT_TIMEOUT);
    }

    #[test]
    fn exit_code_protocol_error() {
        let outcome = make_outcome(vec![StepResult::ProtocolError("no marker".to_string())]);
        assert_eq!(exit_code_for_run(&outcome), EXIT_STEP_ERROR);
    }

    #[test]
    fn exit_code_provider_failed() {
        let outcome = make_outcome(vec![StepResult::ProviderFailed("crash".to_string())]);
        assert_eq!(exit_code_for_run(&outcome), EXIT_STEP_ERROR);
    }

    #[test]
    fn aggregate_no_runs() {
        assert_eq!(aggregate_exit_code(&[], false), EXIT_OK);
    }

    #[test]
    fn aggregate_all_ok() {
        assert_eq!(aggregate_exit_code(&[0, 0, 0], false), EXIT_OK);
    }

    #[test]
    fn aggregate_mixed() {
        assert_eq!(aggregate_exit_code(&[0, 1, 4], false), EXIT_TIMEOUT);
    }

    #[test]
    fn aggregate_with_parse_errors() {
        assert_eq!(aggregate_exit_code(&[0], true), EXIT_CONFIG_ERROR);
    }

    #[test]
    fn aggregate_parse_errors_with_worse_run() {
        assert_eq!(aggregate_exit_code(&[4], true), EXIT_TIMEOUT);
    }

    #[test]
    fn exit_code_strict_warns_fail() {
        let outcome = make_outcome(vec![
            StepResult::Verdict(StepVerdict::Ok),
            StepResult::Verdict(StepVerdict::Warn("slow".to_string())),
        ]);
        assert_eq!(exit_code_for_run_strict(&outcome, true), EXIT_STEP_ERROR);
    }

    #[test]
    fn exit_code_strict_ok_still_passes() {
        let outcome = make_outcome(vec![
            StepResult::Verdict(StepVerdict::Ok),
            StepResult::Verdict(StepVerdict::Ok),
        ]);
        assert_eq!(exit_code_for_run_strict(&outcome, true), EXIT_OK);
    }

    #[test]
    fn exit_code_strict_false_preserves_existing() {
        let outcome = make_outcome(vec![
            StepResult::Verdict(StepVerdict::Ok),
            StepResult::Verdict(StepVerdict::Warn("slow".to_string())),
        ]);
        assert_eq!(exit_code_for_run_strict(&outcome, false), EXIT_OK);
    }

    #[test]
    fn describe_known_codes() {
        assert_eq!(describe_exit_code(EXIT_OK), "all tests passed");
        assert_eq!(
            describe_exit_code(EXIT_STEP_ERROR),
            "one or more steps failed"
        );
        assert_eq!(describe_exit_code(EXIT_INTERRUPTED), "run interrupted");
    }

    #[test]
    fn describe_unknown_code() {
        assert_eq!(describe_exit_code(99), "unknown exit code");
    }
}
