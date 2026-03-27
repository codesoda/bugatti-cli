use crate::executor::{RunOutcome, StepOutcome, StepResult, StepVerdict};
use crate::run::{ArtifactDir, EffectiveConfigSummary, RunId, SessionId};
use std::fmt::Write as FmtWrite;
use std::time::Duration;

/// Error type for report compilation.
#[derive(Debug)]
pub enum ReportError {
    /// Failed to write report.md to disk.
    WriteError {
        path: String,
        source: std::io::Error,
    },
}

impl std::fmt::Display for ReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReportError::WriteError { path, source } => {
                write!(f, "failed to write report to '{path}': {source}")
            }
        }
    }
}

impl std::error::Error for ReportError {}

/// Input data for compiling a report.
pub struct ReportInput<'a> {
    pub run_id: &'a RunId,
    pub session_id: &'a SessionId,
    pub root_test_file: &'a str,
    pub provider_name: &'a str,
    pub start_time: &'a str,
    pub end_time: &'a str,
    pub skipped_commands: &'a [String],
    pub config_summary: &'a EffectiveConfigSummary,
    pub outcome: &'a RunOutcome,
    pub artifact_dir: &'a ArtifactDir,
    /// Artifact capture errors (e.g., failed transcript writes) to include in the report.
    pub artifact_errors: &'a [String],
}

/// Compile report.md content from the run model.
///
/// Returns the report as a String. If any section fails to compile,
/// best-effort content is still produced with a note about the failure.
pub fn compile_report(input: &ReportInput) -> String {
    let mut report = String::new();

    // Header
    let _ = writeln!(report, "# Bugatti Run Report");
    let _ = writeln!(report);

    // Run metadata
    let _ = writeln!(report, "## Run Summary");
    let _ = writeln!(report);
    let _ = writeln!(report, "| Field | Value |");
    let _ = writeln!(report, "|-------|-------|");
    let _ = writeln!(report, "| Run ID | `{}` |", input.run_id);
    let _ = writeln!(report, "| Session ID | `{}` |", input.session_id);
    let _ = writeln!(report, "| Root Test File | `{}` |", input.root_test_file);
    let _ = writeln!(report, "| Provider | `{}` |", input.provider_name);
    let _ = writeln!(report, "| Start Time | {} |", input.start_time);
    let _ = writeln!(report, "| End Time | {} |", input.end_time);
    let _ = writeln!(
        report,
        "| Duration | {} |",
        format_duration(input.outcome.total_duration)
    );
    let _ = writeln!(
        report,
        "| Status | **{}** |",
        if input.outcome.all_passed {
            "PASSED"
        } else {
            "FAILED"
        }
    );
    let _ = writeln!(report);

    // Skipped commands
    if !input.skipped_commands.is_empty() {
        let _ = writeln!(report, "## Skipped Commands");
        let _ = writeln!(report);
        for cmd in input.skipped_commands {
            let _ = writeln!(report, "- `{cmd}`");
        }
        let _ = writeln!(report);
    }

    // Step results
    let _ = writeln!(report, "## Step Results");
    let _ = writeln!(report);

    if input.outcome.steps.is_empty() {
        let _ = writeln!(report, "_No steps were executed._");
        let _ = writeln!(report);
    } else {
        for outcome in &input.outcome.steps {
            write_step_section(&mut report, outcome, input.artifact_dir);
        }
    }

    // Effective config summary
    let _ = writeln!(report, "## Effective Configuration");
    let _ = writeln!(report);
    write_config_summary(&mut report, input.config_summary);

    // Artifact paths
    let _ = writeln!(report, "## Artifacts");
    let _ = writeln!(report);
    let _ = writeln!(report, "| Artifact | Path |",);
    let _ = writeln!(report, "|----------|------|");
    let _ = writeln!(
        report,
        "| Run Directory | `{}` |",
        input.artifact_dir.root.display()
    );
    let full_transcript_path = input.artifact_dir.transcripts.join("full_transcript.txt");
    let _ = writeln!(
        report,
        "| Full Transcript | `{}` |",
        full_transcript_path.display()
    );
    let _ = writeln!(
        report,
        "| Transcripts | `{}` |",
        input.artifact_dir.transcripts.display()
    );
    let _ = writeln!(report, "| Logs | `{}` |", input.artifact_dir.logs.display());
    let _ = writeln!(
        report,
        "| Diagnostics | `{}` |",
        input.artifact_dir.diagnostics.display()
    );
    let _ = writeln!(
        report,
        "| Screenshots | `{}` |",
        input.artifact_dir.screenshots.display()
    );
    let _ = writeln!(report);

    // Artifact capture errors
    if !input.artifact_errors.is_empty() {
        let _ = writeln!(report, "## Artifact Capture Errors");
        let _ = writeln!(report);
        let _ = writeln!(
            report,
            "_The following errors occurred while capturing run artifacts:_"
        );
        let _ = writeln!(report);
        for err in input.artifact_errors {
            let _ = writeln!(report, "- {err}");
        }
        let _ = writeln!(report);
    }

    report
}

/// Write the report to disk at the standard location.
///
/// Both successful and failed runs produce report.md.
/// If compilation partially fails, best-effort content is still written.
pub fn write_report(input: &ReportInput, artifact_dir: &ArtifactDir) -> Result<(), ReportError> {
    tracing::info!("compiling report");
    let content = compile_report(input);
    let report_path = report_path(artifact_dir);

    tracing::info!(path = %report_path.display(), bytes = content.len(), "writing report");
    std::fs::write(&report_path, &content).map_err(|e| {
        tracing::error!(path = %report_path.display(), error = %e, "report write failed");
        ReportError::WriteError {
            path: report_path.display().to_string(),
            source: e,
        }
    })
}

/// The standard path for report.md within a run's artifact directory.
pub fn report_path(artifact_dir: &ArtifactDir) -> std::path::PathBuf {
    artifact_dir.root.join("report.md")
}

fn write_step_section(report: &mut String, outcome: &StepOutcome, artifact_dir: &ArtifactDir) {
    let status_icon = match &outcome.result {
        StepResult::Verdict(StepVerdict::Ok) => "OK",
        StepResult::Verdict(StepVerdict::Warn(_)) => "WARN",
        StepResult::Verdict(StepVerdict::Error(_)) => "ERROR",
        StepResult::ProtocolError(_) => "PROTOCOL ERROR",
        StepResult::Timeout => "TIMEOUT",
        StepResult::ProviderFailed(_) => "PROVIDER ERROR",
    };

    let _ = writeln!(
        report,
        "### Step {} - {} [{}]",
        outcome.step_id + 1,
        status_icon,
        format_duration(outcome.duration)
    );
    let _ = writeln!(report);
    let _ = writeln!(report, "- **Instruction:** {}", outcome.instruction);
    let _ = writeln!(report, "- **Source:** `{}`", outcome.source_file.display());
    let _ = writeln!(report, "- **Result:** {}", outcome.result);

    // Transcript path for this step
    let transcript_path = artifact_dir
        .transcripts
        .join(format!("step_{:04}.txt", outcome.step_id));
    let _ = writeln!(report, "- **Transcript:** `{}`", transcript_path.display());

    // Include log events for WARN/ERROR steps
    if !outcome.log_events.is_empty()
        && !matches!(outcome.result, StepResult::Verdict(StepVerdict::Ok))
    {
        let _ = writeln!(report);
        let _ = writeln!(report, "**BUGATTI_LOG events:**");
        let _ = writeln!(report);
        for event in &outcome.log_events {
            let _ = writeln!(report, "- {}", event.message);
        }
    }

    let _ = writeln!(report);
}

fn write_config_summary(report: &mut String, summary: &EffectiveConfigSummary) {
    let _ = writeln!(report, "| Setting | Value |");
    let _ = writeln!(report, "|---------|-------|");
    let _ = writeln!(report, "| Provider | `{}` |", summary.provider_name);
    let _ = writeln!(
        report,
        "| Extra System Prompt | {} |",
        if summary.has_extra_system_prompt {
            "configured"
        } else {
            "none"
        }
    );
    let _ = writeln!(
        report,
        "| Agent Args | {} |",
        if summary.agent_args.is_empty() {
            "none".to_string()
        } else {
            format!("`{}`", summary.agent_args.join(" "))
        }
    );
    let _ = writeln!(
        report,
        "| Commands | {} |",
        if summary.command_names.is_empty() {
            "none".to_string()
        } else {
            summary
                .command_names
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    let _ = writeln!(report);
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let mins = secs / 60.0;
        format!("{mins:.1}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{LogEvent, RunOutcome, StepOutcome, StepResult, StepVerdict};
    use crate::run::{ArtifactDir, EffectiveConfigSummary, RunId, SessionId};
    use std::path::PathBuf;
    use std::time::Duration;

    fn test_artifact_dir() -> (tempfile::TempDir, ArtifactDir) {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run-001".to_string());
        let dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        dir.create_all().unwrap();
        (tmp, dir)
    }

    fn test_config_summary() -> EffectiveConfigSummary {
        EffectiveConfigSummary {
            provider_name: "claude-code".to_string(),
            has_extra_system_prompt: true,
            agent_args: vec!["--verbose".to_string()],
            command_names: vec!["migrate".to_string(), "server".to_string()],
        }
    }

    fn make_ok_step(step_id: usize, instruction: &str) -> StepOutcome {
        StepOutcome {
            step_id,
            instruction: instruction.to_string(),
            source_file: PathBuf::from("tests/login.test.toml"),
            result: StepResult::Verdict(StepVerdict::Ok),
            transcript: format!("Checked.\nRESULT OK"),
            log_events: vec![],
            duration: Duration::from_millis(1500),
        }
    }

    fn make_warn_step(step_id: usize, msg: &str) -> StepOutcome {
        StepOutcome {
            step_id,
            instruction: "Check response time".to_string(),
            source_file: PathBuf::from("tests/perf.test.toml"),
            result: StepResult::Verdict(StepVerdict::Warn(msg.to_string())),
            transcript: format!("Checked.\nBUGATTI_LOG slow response\nRESULT WARN: {msg}"),
            log_events: vec![LogEvent {
                run_id: "test-run-001".to_string(),
                step_id,
                message: "slow response".to_string(),
            }],
            duration: Duration::from_millis(3200),
        }
    }

    fn make_error_step(step_id: usize, msg: &str) -> StepOutcome {
        StepOutcome {
            step_id,
            instruction: "Verify login works".to_string(),
            source_file: PathBuf::from("tests/login.test.toml"),
            result: StepResult::Verdict(StepVerdict::Error(msg.to_string())),
            transcript: format!(
                "Tried login.\nBUGATTI_LOG Auth failed\nBUGATTI_LOG Retried once\nRESULT ERROR: {msg}"
            ),
            log_events: vec![
                LogEvent {
                    run_id: "test-run-001".to_string(),
                    step_id,
                    message: "Auth failed".to_string(),
                },
                LogEvent {
                    run_id: "test-run-001".to_string(),
                    step_id,
                    message: "Retried once".to_string(),
                },
            ],
            duration: Duration::from_millis(5000),
        }
    }

    fn make_report_input<'a>(
        run_id: &'a RunId,
        session_id: &'a SessionId,
        skipped: &'a [String],
        summary: &'a EffectiveConfigSummary,
        outcome: &'a RunOutcome,
        artifact_dir: &'a ArtifactDir,
    ) -> ReportInput<'a> {
        ReportInput {
            run_id,
            session_id,
            root_test_file: "tests/login.test.toml",
            provider_name: "claude-code",
            start_time: "2026-03-27T10:00:00Z",
            end_time: "2026-03-27T10:01:30Z",
            skipped_commands: skipped,
            config_summary: summary,
            outcome,
            artifact_dir,
            artifact_errors: &[],
        }
    }

    #[test]
    fn compile_report_successful_run() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![
                make_ok_step(0, "Check homepage"),
                make_ok_step(1, "Check login"),
            ],
            all_passed: true,
            total_duration: Duration::from_millis(3000),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        let report = compile_report(&input);

        assert!(report.contains("# Bugatti Run Report"));
        assert!(report.contains("test-run-001"));
        assert!(report.contains("test-session-001"));
        assert!(report.contains("**PASSED**"));
        assert!(report.contains("tests/login.test.toml"));
        assert!(report.contains("claude-code"));
        assert!(report.contains("Step 1 - OK"));
        assert!(report.contains("Step 2 - OK"));
        assert!(report.contains("Check homepage"));
        assert!(report.contains("Check login"));
    }

    #[test]
    fn compile_report_failed_run_with_log_events() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![
                make_ok_step(0, "Check homepage"),
                make_error_step(1, "login button missing"),
            ],
            all_passed: false,
            total_duration: Duration::from_millis(6500),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        let report = compile_report(&input);

        assert!(report.contains("**FAILED**"));
        assert!(report.contains("Step 2 - ERROR"));
        assert!(report.contains("login button missing"));
        // Error step should have log events included
        assert!(report.contains("BUGATTI_LOG events:"));
        assert!(report.contains("Auth failed"));
        assert!(report.contains("Retried once"));
    }

    #[test]
    fn compile_report_warn_step_includes_log_events() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![make_warn_step(0, "response took 2.5s")],
            all_passed: true,
            total_duration: Duration::from_millis(3200),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        let report = compile_report(&input);

        assert!(report.contains("Step 1 - WARN"));
        assert!(report.contains("BUGATTI_LOG events:"));
        assert!(report.contains("slow response"));
    }

    #[test]
    fn compile_report_ok_step_excludes_log_events() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();

        // OK step with log events - they should NOT appear in the report
        let mut step = make_ok_step(0, "Check homepage");
        step.log_events = vec![LogEvent {
            run_id: "test-run-001".to_string(),
            step_id: 0,
            message: "debug info".to_string(),
        }];

        let outcome = RunOutcome {
            steps: vec![step],
            all_passed: true,
            total_duration: Duration::from_millis(1500),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        let report = compile_report(&input);

        assert!(!report.contains("BUGATTI_LOG events:"));
    }

    #[test]
    fn compile_report_with_skipped_commands() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![make_ok_step(0, "Check homepage")],
            all_passed: true,
            total_duration: Duration::from_millis(1500),
            artifact_errors: vec![],
        };
        let skipped = vec!["server".to_string(), "migrate".to_string()];

        let input = make_report_input(
            &run_id,
            &session_id,
            &skipped,
            &summary,
            &outcome,
            &artifact_dir,
        );
        let report = compile_report(&input);

        assert!(report.contains("## Skipped Commands"));
        assert!(report.contains("`server`"));
        assert!(report.contains("`migrate`"));
    }

    #[test]
    fn compile_report_no_skipped_commands_omits_section() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![make_ok_step(0, "Check homepage")],
            all_passed: true,
            total_duration: Duration::from_millis(1500),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        let report = compile_report(&input);

        assert!(!report.contains("## Skipped Commands"));
    }

    #[test]
    fn compile_report_includes_config_summary() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![],
            all_passed: true,
            total_duration: Duration::from_millis(0),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        let report = compile_report(&input);

        assert!(report.contains("## Effective Configuration"));
        assert!(report.contains("`claude-code`"));
        assert!(report.contains("configured")); // extra_system_prompt
        assert!(report.contains("`--verbose`"));
        assert!(report.contains("`migrate`"));
        assert!(report.contains("`server`"));
    }

    #[test]
    fn compile_report_includes_artifact_paths() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![],
            all_passed: true,
            total_duration: Duration::from_millis(0),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        let report = compile_report(&input);

        assert!(report.contains("## Artifacts"));
        assert!(report.contains("transcripts"));
        assert!(report.contains("diagnostics"));
    }

    #[test]
    fn compile_report_protocol_error_step() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![StepOutcome {
                step_id: 0,
                instruction: "Check something".to_string(),
                source_file: PathBuf::from("test.test.toml"),
                result: StepResult::ProtocolError("no RESULT marker".to_string()),
                transcript: "Some output without marker".to_string(),
                log_events: vec![],
                duration: Duration::from_millis(2000),
            }],
            all_passed: false,
            total_duration: Duration::from_millis(2000),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        let report = compile_report(&input);

        assert!(report.contains("PROTOCOL ERROR"));
        assert!(report.contains("**FAILED**"));
    }

    #[test]
    fn write_report_creates_file() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![make_ok_step(0, "Check homepage")],
            all_passed: true,
            total_duration: Duration::from_millis(1500),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        write_report(&input, &artifact_dir).unwrap();

        let path = report_path(&artifact_dir);
        assert!(path.is_file());
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("# Bugatti Run Report"));
        assert!(contents.contains("test-run-001"));
    }

    #[test]
    fn write_report_failed_run_still_writes() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![make_error_step(0, "server crashed")],
            all_passed: false,
            total_duration: Duration::from_millis(5000),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        write_report(&input, &artifact_dir).unwrap();

        let path = report_path(&artifact_dir);
        assert!(path.is_file());
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("**FAILED**"));
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.5s");
        assert_eq!(format_duration(Duration::from_millis(200)), "0.2s");
        assert_eq!(format_duration(Duration::from_secs(59)), "59.0s");
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(format_duration(Duration::from_secs(60)), "1.0m");
        assert_eq!(format_duration(Duration::from_secs(90)), "1.5m");
        assert_eq!(format_duration(Duration::from_secs(300)), "5.0m");
    }

    #[test]
    fn compile_report_includes_transcript_paths_per_step() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![
                make_ok_step(0, "Check homepage"),
                make_ok_step(1, "Check login"),
            ],
            all_passed: true,
            total_duration: Duration::from_millis(3000),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        let report = compile_report(&input);

        assert!(report.contains("step_0000.txt"));
        assert!(report.contains("step_0001.txt"));
    }

    #[test]
    fn compile_report_includes_full_transcript_path() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![make_ok_step(0, "Check homepage")],
            all_passed: true,
            total_duration: Duration::from_millis(1500),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        let report = compile_report(&input);

        assert!(report.contains("Full Transcript"));
        assert!(report.contains("full_transcript.txt"));
    }

    #[test]
    fn compile_report_includes_artifact_errors() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![make_ok_step(0, "Check homepage")],
            all_passed: true,
            total_duration: Duration::from_millis(1500),
            artifact_errors: vec![],
        };
        let errors = vec![
            "failed to write transcript for step 0".to_string(),
            "failed to append to full transcript".to_string(),
        ];

        let mut input =
            make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        input.artifact_errors = &errors;
        let report = compile_report(&input);

        assert!(report.contains("## Artifact Capture Errors"));
        assert!(report.contains("failed to write transcript for step 0"));
        assert!(report.contains("failed to append to full transcript"));
    }

    #[test]
    fn compile_report_no_artifact_errors_omits_section() {
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let (_tmp, artifact_dir) = test_artifact_dir();
        let summary = test_config_summary();
        let outcome = RunOutcome {
            steps: vec![make_ok_step(0, "Check homepage")],
            all_passed: true,
            total_duration: Duration::from_millis(1500),
            artifact_errors: vec![],
        };

        let input = make_report_input(&run_id, &session_id, &[], &summary, &outcome, &artifact_dir);
        let report = compile_report(&input);

        assert!(!report.contains("## Artifact Capture Errors"));
    }
}
