//! Integration tests for the end-to-end pipeline.
//!
//! These tests verify that all components are wired correctly by exercising
//! the pipeline with test fixtures and a mock provider.

use std::time::Duration;

use bugatti::command;
use bugatti::config;
use bugatti::diagnostics;
use bugatti::executor::{self, RunOutcome, StepOutcome, StepResult, StepVerdict};
use bugatti::exit_code;
use bugatti::expand;
use bugatti::provider::{AgentSession, OutputChunk};
use bugatti::report::{self, ReportInput};
use bugatti::run::{self, EffectiveConfigSummary};
use bugatti::test_file;

use bugatti::test_support as common;

/// Test the full pipeline with a mock provider: config -> parse -> expand ->
/// artifacts -> execution -> report -> exit code.
#[tokio::test]
async fn full_pipeline_with_mock_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();

    // Write a test file
    let test_content = r#"
name = "integration-test"

[[steps]]
instruction = "Check that the homepage loads correctly"

[[steps]]
instruction = "Verify the login form is present"
"#;
    let test_path = project_root.join("integration.test.toml");
    std::fs::write(&test_path, test_content).unwrap();

    // Phase 1: Load config (no config file -> defaults)
    let global_config = config::load_config(project_root).unwrap();

    // Phase 2: Parse test file
    let test_file = test_file::parse_test_file(&test_path).unwrap();
    assert_eq!(test_file.name, "integration-test");
    assert_eq!(test_file.steps.len(), 2);

    // Phase 3: Effective config
    let effective = config::effective_config(&global_config, &test_file);

    // Phase 4: Validate skip commands (none)
    command::validate_skip_cmds(&effective, &[]).unwrap();

    // Phase 5: Expand steps
    let steps = expand::expand_steps(&test_path, &test_file).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[0].instruction,
        "Check that the homepage loads correctly"
    );
    assert_eq!(steps[1].instruction, "Verify the login form is present");

    // Phase 6: Initialize run
    let (run_id, session_id, artifact_dir) =
        run::initialize_run(project_root, &test_path, &effective).unwrap();
    assert!(artifact_dir.root.is_dir());
    assert!(artifact_dir.transcripts.is_dir());
    assert!(artifact_dir.logs.is_dir());
    assert!(artifact_dir.diagnostics.is_dir());

    // Phase 7: Init tracing
    let _guard = diagnostics::init_tracing(&artifact_dir).unwrap();

    // Phase 8-9: Skip command setup (no commands configured)

    // Phase 10: Mock provider
    let mut session = common::MockSession::with_ok_responses(2);
    session.start().await.unwrap();

    // Phase 11: Execute steps
    let outcome = executor::execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        Some(Duration::from_secs(30)),
        None,
        None,
        std::path::Path::new("."),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(outcome.all_passed);
    assert_eq!(outcome.steps.len(), 2);
    for step in &outcome.steps {
        assert!(step.result.is_pass());
    }

    // Phase 12: Close session
    session.close().await.unwrap();

    // Phase 13: Write report
    let start_time = chrono::Utc::now();
    let end_time = chrono::Utc::now();
    let config_summary = EffectiveConfigSummary::from_config(&effective);

    let report_input = ReportInput {
        run_id: &run_id,
        session_id: &session_id,
        root_test_file: &test_path.display().to_string(),
        provider_name: &effective.provider.name,
        start_time: &start_time.to_rfc3339(),
        end_time: &end_time.to_rfc3339(),
        skipped_commands: &[],
        config_summary: &config_summary,
        outcome: &outcome,
        artifact_dir: &artifact_dir,
        artifact_errors: &outcome.artifact_errors,
    };
    report::write_report(&report_input, &artifact_dir).unwrap();

    // Verify report exists
    let report_path = report::report_path(&artifact_dir);
    assert!(report_path.is_file());

    let report_content = std::fs::read_to_string(&report_path).unwrap();
    assert!(
        report_content.contains("integration") || report_content.contains("test"),
        "report should reference the test: {}",
        &report_content[..500.min(report_content.len())]
    );
    assert!(report_content.contains(&run_id.0));
    assert!(report_content.contains("OK"));

    // Phase 14: Exit code
    let exit_code = exit_code::exit_code_for_run(&outcome);
    assert_eq!(exit_code, 0, "all-OK run should exit 0");
}

/// Test that config errors fail cleanly before execution.
#[test]
fn pipeline_fails_on_invalid_config() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("bugatti.config.toml");
    std::fs::write(&config_path, "invalid = {{{").unwrap();

    let result = config::load_config(tmp.path());
    assert!(result.is_err());
}

/// Test that parse errors fail cleanly.
#[test]
fn pipeline_fails_on_invalid_test_file() {
    let tmp = tempfile::tempdir().unwrap();
    let test_path = tmp.path().join("bad.test.toml");
    std::fs::write(&test_path, "this is not valid toml {{{").unwrap();

    let result = test_file::parse_test_file(&test_path);
    assert!(result.is_err());
}

/// Test that cycle detection fails before execution.
#[test]
fn pipeline_fails_on_cycle() {
    let tmp = tempfile::tempdir().unwrap();

    // Create a test file that includes itself
    let test_content = format!(
        r#"
name = "cycle-test"

[[steps]]
include_path = "{}"
"#,
        tmp.path().join("cycle.test.toml").display()
    );
    let test_path = tmp.path().join("cycle.test.toml");
    std::fs::write(&test_path, &test_content).unwrap();

    let test_file = test_file::parse_test_file(&test_path).unwrap();
    let result = expand::expand_steps(&test_path, &test_file);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("cycle"), "should report cycle, got: {err}");
}

/// Test pipeline with a step that returns ERROR.
#[tokio::test]
async fn pipeline_with_error_step_exits_nonzero() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();

    let test_content = r#"
name = "error-test"

[[steps]]
instruction = "This step will fail"
"#;
    let test_path = project_root.join("error.test.toml");
    std::fs::write(&test_path, test_content).unwrap();

    let global_config = config::load_config(project_root).unwrap();
    let test_file = test_file::parse_test_file(&test_path).unwrap();
    let effective = config::effective_config(&global_config, &test_file);
    let steps = expand::expand_steps(&test_path, &test_file).unwrap();
    let (run_id, session_id, artifact_dir) =
        run::initialize_run(project_root, &test_path, &effective).unwrap();
    let _guard = diagnostics::init_tracing(&artifact_dir).unwrap();

    // Mock provider that returns ERROR
    let mut session = common::MockSession::new(vec![vec![
        Ok(OutputChunk::Text(
            "Something went wrong\nRESULT ERROR: database connection failed\n".to_string(),
        )),
        Ok(OutputChunk::Done),
    ]]);
    session.start().await.unwrap();

    let outcome = executor::execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        Some(Duration::from_secs(30)),
        None,
        None,
        std::path::Path::new("."),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(!outcome.all_passed);
    assert_eq!(outcome.steps.len(), 1);
    assert!(outcome.steps[0].result.is_failure());

    let exit_code = exit_code::exit_code_for_run(&outcome);
    assert_ne!(exit_code, 0, "ERROR step should exit non-zero");
}

/// Test that report is generated for both successful and failed runs.
#[test]
fn report_generated_for_failed_run() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();

    let test_content = r#"
name = "fail-report-test"

[[steps]]
instruction = "This will fail"
"#;
    let test_path = project_root.join("fail.test.toml");
    std::fs::write(&test_path, test_content).unwrap();

    let global_config = config::load_config(project_root).unwrap();
    let test_file = test_file::parse_test_file(&test_path).unwrap();
    let effective = config::effective_config(&global_config, &test_file);
    let _steps = expand::expand_steps(&test_path, &test_file).unwrap();
    let (run_id, session_id, artifact_dir) =
        run::initialize_run(project_root, &test_path, &effective).unwrap();

    // Create a failed outcome
    let outcome = RunOutcome {
        steps: vec![StepOutcome {
            step_id: 0,
            instruction: "This will fail".to_string(),
            source_file: test_path.clone(),
            setup: false,
            result: StepResult::Verdict(StepVerdict::Error("something broke".to_string())),
            transcript: "RESULT ERROR: something broke".to_string(),
            log_events: vec![],
            evidence_refs: vec![],
            duration: Duration::from_secs(1),
        }],
        all_passed: false,
        total_duration: Duration::from_secs(1),
        artifact_errors: vec![],
    };

    let start_time = chrono::Utc::now();
    let end_time = chrono::Utc::now();
    let config_summary = EffectiveConfigSummary::from_config(&effective);

    let report_input = ReportInput {
        run_id: &run_id,
        session_id: &session_id,
        root_test_file: &test_path.display().to_string(),
        provider_name: &effective.provider.name,
        start_time: &start_time.to_rfc3339(),
        end_time: &end_time.to_rfc3339(),
        skipped_commands: &[],
        config_summary: &config_summary,
        outcome: &outcome,
        artifact_dir: &artifact_dir,
        artifact_errors: &[],
    };
    report::write_report(&report_input, &artifact_dir).unwrap();

    let report_path = report::report_path(&artifact_dir);
    assert!(report_path.is_file());

    let content = std::fs::read_to_string(&report_path).unwrap();
    assert!(content.contains("ERROR"));
    assert!(content.contains("something broke"));
}

#[test]
fn per_test_command_overrides_apply_to_effective_config() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();

    std::fs::write(
        project_root.join("bugatti.config.toml"),
        r#"
[commands.server]
kind = "long_lived"
cmd = "cargo run --bin server"
readiness_url = "http://localhost:3000/ready"
"#,
    )
    .unwrap();

    let test_path = project_root.join("override.test.toml");
    std::fs::write(
        &test_path,
        r#"
[overrides.commands.server]
cmd = "cargo run --bin alternate-server"
readiness_timeout_secs = 3

[[steps]]
instruction = "Check the server"
"#,
    )
    .unwrap();

    let global_config = config::load_config(project_root).unwrap();
    let test_file = test_file::parse_test_file(&test_path).unwrap();
    let effective = config::effective_config(&global_config, &test_file);
    let server = effective.commands.get("server").unwrap();

    assert_eq!(server.kind, config::CommandKind::LongLived);
    assert_eq!(server.cmd, "cargo run --bin alternate-server");
    assert_eq!(server.readiness_timeout_secs, Some(3));
    assert_eq!(
        server.readiness_url.as_deref(),
        Some("http://localhost:3000/ready")
    );
}

#[test]
fn layered_config_and_per_test_overrides_stack() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();
    let home = tempfile::tempdir().unwrap();
    let global_dir = home.path().join(".bugatti");
    std::fs::create_dir_all(&global_dir).unwrap();
    let global_path = global_dir.join("config.toml");

    std::fs::write(
        &global_path,
        r#"
[provider]
base_url = "http://global.example"

[commands.server]
kind = "long_lived"
cmd = "cargo run --bin global-server"
readiness_url = "http://localhost:3000/ready"
"#,
    )
    .unwrap();

    std::fs::write(
        project_root.join("bugatti.config.toml"),
        r#"
[provider]
agent_args = ["--project"]

[commands.server]
kind = "long_lived"
cmd = "cargo run --bin project-server"
readiness_url = "http://localhost:4000/ready"
"#,
    )
    .unwrap();

    let test_path = project_root.join("layered.test.toml");
    std::fs::write(
        &test_path,
        r#"
[overrides.commands.server]
cmd = "cargo run --bin test-server"

[[steps]]
instruction = "Check layered config"
"#,
    )
    .unwrap();

    let layered =
        config::load_layered_config_with_options(project_root, None, Some(&global_path), |_| None)
            .unwrap();
    let test_file = test_file::parse_test_file(&test_path).unwrap();
    let effective = config::effective_config(&layered, &test_file);
    let server = effective.commands.get("server").unwrap();

    assert_eq!(
        effective.provider.base_url.as_deref(),
        Some("http://global.example")
    );
    assert_eq!(effective.provider.agent_args, vec!["--project"]);
    assert_eq!(server.kind, config::CommandKind::LongLived);
    assert_eq!(server.cmd, "cargo run --bin test-server");
    assert_eq!(
        server.readiness_url.as_deref(),
        Some("http://localhost:4000/ready")
    );
}
