use clap::Parser;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bugatti::claude_code::ClaudeCodeAdapter;
use bugatti::cli::{Cli, Commands};
use bugatti::command::{self, TrackedProcess};
use bugatti::config;
use bugatti::diagnostics;
use bugatti::discovery::{discover_root_tests, DiscoveredTest};
use bugatti::executor;
use bugatti::exit_code::{
    self, EXIT_CONFIG_ERROR, EXIT_INTERRUPTED, EXIT_OK, EXIT_PROVIDER_ERROR, EXIT_STEP_ERROR,
};
use bugatti::expand;
use bugatti::provider::AgentSession;
use bugatti::report::{self, ReportInput};
use bugatti::run::{self, ArtifactDir, EffectiveConfigSummary};
use bugatti::test_file;

/// Global flag set by the Ctrl+C handler.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Check whether the run has been interrupted by Ctrl+C.
pub fn is_interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

/// Outcome of running a single root test file.
#[derive(Debug)]
struct TestRunResult {
    /// Path to the test file.
    path: PathBuf,
    /// Name from the test file.
    name: String,
    /// Exit code for this individual run.
    exit_code: i32,
    /// Run ID (if execution started).
    run_id: Option<String>,
    /// Report path (if a report was written).
    report_path: Option<String>,
    /// Error message if the test failed before execution.
    error: Option<String>,
}

fn main() {
    // Install Ctrl+C handler for graceful interruption.
    // The handler sets a flag; the run loop checks it between steps.
    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let interrupted = interrupted.clone();
        let _ = ctrlc::set_handler(move || {
            eprintln!("\nInterrupted (Ctrl+C). Attempting best-effort cleanup...");
            interrupted.store(true, Ordering::Relaxed);
            INTERRUPTED.store(true, Ordering::Relaxed);
        });
    }

    let cli = Cli::parse();

    let code = match cli.command {
        Commands::Test { path, skip_cmds } => {
            let project_root = std::env::current_dir().unwrap_or_else(|e| {
                eprintln!("ERROR: failed to determine current directory: {e}");
                std::process::exit(EXIT_CONFIG_ERROR);
            });

            if !skip_cmds.is_empty() {
                println!("Skipping commands: {}", skip_cmds.join(", "));
            }
            match path {
                Some(p) => {
                    let test_path = PathBuf::from(&p);
                    if !test_path.exists() {
                        eprintln!("ERROR: test file not found: {p}");
                        EXIT_CONFIG_ERROR
                    } else {
                        let result = run_test_pipeline(&project_root, &test_path, &skip_cmds);
                        // Print run reference for single-file mode
                        if let Some(run_id) = &result.run_id {
                            println!("\nRun ID: {run_id}");
                        }
                        if let Some(rp) = &result.report_path {
                            println!("Report: {rp}");
                        }
                        println!(
                            "\nExit code: {} ({})",
                            result.exit_code,
                            exit_code::describe_exit_code(result.exit_code)
                        );
                        result.exit_code
                    }
                }
                None => run_discovery(&project_root, &skip_cmds),
            }
        }
    };

    std::process::exit(code);
}

/// Run the full test pipeline for a single test file.
///
/// Pipeline order: config load -> parse -> expand -> artifact setup -> command setup
/// -> provider init -> step execution -> report -> teardown -> exit
fn run_test_pipeline(project_root: &Path, test_path: &Path, skip_cmds: &[String]) -> TestRunResult {
    let test_name_fallback = test_path.display().to_string();

    // Phase 1: Load config
    let global_config = match config::load_config(project_root) {
        Ok(c) => c,
        Err(e) => {
            return TestRunResult {
                path: test_path.to_path_buf(),
                name: test_name_fallback,
                exit_code: EXIT_CONFIG_ERROR,
                run_id: None,
                report_path: None,
                error: Some(format!("config error: {e}")),
            };
        }
    };

    // Phase 2: Parse test file
    let test_file = match test_file::parse_test_file(test_path) {
        Ok(tf) => tf,
        Err(e) => {
            return TestRunResult {
                path: test_path.to_path_buf(),
                name: test_name_fallback,
                exit_code: EXIT_CONFIG_ERROR,
                run_id: None,
                report_path: None,
                error: Some(format!("parse error: {e}")),
            };
        }
    };

    let test_name = test_file.name.clone();

    // Phase 3: Compute effective config (merge overrides)
    let effective = config::effective_config(&global_config, &test_file);

    // Phase 4: Validate skip commands
    if let Err(msg) = command::validate_skip_cmds(&effective, skip_cmds) {
        return TestRunResult {
            path: test_path.to_path_buf(),
            name: test_name,
            exit_code: EXIT_CONFIG_ERROR,
            run_id: None,
            report_path: None,
            error: Some(msg),
        };
    }

    // Phase 5: Expand include steps
    let steps = match expand::expand_steps(test_path, &test_file) {
        Ok(s) => s,
        Err(e) => {
            return TestRunResult {
                path: test_path.to_path_buf(),
                name: test_name,
                exit_code: EXIT_CONFIG_ERROR,
                run_id: None,
                report_path: None,
                error: Some(format!("expand error: {e}")),
            };
        }
    };

    // Phase 6: Initialize run (create artifact directories, write metadata)
    let (run_id, session_id, artifact_dir) =
        match run::initialize_run(project_root, test_path, &effective) {
            Ok(r) => r,
            Err(e) => {
                return TestRunResult {
                    path: test_path.to_path_buf(),
                    name: test_name,
                    exit_code: EXIT_CONFIG_ERROR,
                    run_id: None,
                    report_path: None,
                    error: Some(format!("artifact setup error: {e}")),
                };
            }
        };

    // From here on, we have a run_id and artifact_dir — always try best-effort reporting.
    run_test_with_artifacts(
        project_root,
        test_path,
        &test_name,
        skip_cmds,
        &effective,
        steps,
        run_id,
        session_id,
        artifact_dir,
    )
}

/// Continue the pipeline after artifact setup. Ensures best-effort report writing
/// and subprocess teardown even on failure.
#[allow(clippy::too_many_arguments)]
fn run_test_with_artifacts(
    _project_root: &Path,
    test_path: &Path,
    test_name: &str,
    skip_cmds: &[String],
    effective: &bugatti::config::Config,
    steps: Vec<bugatti::expand::ExpandedStep>,
    run_id: bugatti::run::RunId,
    session_id: bugatti::run::SessionId,
    artifact_dir: ArtifactDir,
) -> TestRunResult {
    let start_time = chrono::Utc::now();
    let config_summary = EffectiveConfigSummary::from_config(effective);

    // Phase 7: Initialize tracing
    let _tracing_guard = match diagnostics::init_tracing(&artifact_dir) {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("WARNING: failed to initialize tracing: {e}");
            None
        }
    };

    tracing::info!(
        run_id = %run_id,
        test_file = %test_path.display(),
        "starting test pipeline"
    );

    // Phase 8: Run short-lived setup commands
    if let Err(e) = command::run_short_lived_commands(effective, &artifact_dir, skip_cmds) {
        tracing::error!(error = %e, "short-lived command failed");
        let end_time = chrono::Utc::now();
        // Best-effort report for command failure
        let empty_outcome = executor::RunOutcome {
            steps: vec![],
            all_passed: false,
            total_duration: std::time::Duration::ZERO,
            artifact_errors: vec![],
        };
        let _ = write_best_effort_report(
            &run_id,
            &session_id,
            test_path,
            effective,
            skip_cmds,
            &config_summary,
            &empty_outcome,
            &artifact_dir,
            &start_time,
            &end_time,
        );
        return TestRunResult {
            path: test_path.to_path_buf(),
            name: test_name.to_string(),
            exit_code: EXIT_CONFIG_ERROR,
            run_id: Some(run_id.0.clone()),
            report_path: Some(report::report_path(&artifact_dir).display().to_string()),
            error: Some(format!("setup command failed: {e}")),
        };
    }

    // Phase 9: Spawn long-lived commands
    let mut tracked_processes: Vec<TrackedProcess> =
        match command::spawn_long_lived_commands(effective, &artifact_dir, skip_cmds) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "long-lived command failed");
                let end_time = chrono::Utc::now();
                let empty_outcome = executor::RunOutcome {
                    steps: vec![],
                    all_passed: false,
                    total_duration: std::time::Duration::ZERO,
                    artifact_errors: vec![],
                };
                let _ = write_best_effort_report(
                    &run_id,
                    &session_id,
                    test_path,
                    effective,
                    skip_cmds,
                    &config_summary,
                    &empty_outcome,
                    &artifact_dir,
                    &start_time,
                    &end_time,
                );
                return TestRunResult {
                    path: test_path.to_path_buf(),
                    name: test_name.to_string(),
                    exit_code: EXIT_PROVIDER_ERROR,
                    run_id: Some(run_id.0.clone()),
                    report_path: Some(report::report_path(&artifact_dir).display().to_string()),
                    error: Some(format!("long-lived command failed: {e}")),
                };
            }
        };

    // Phase 10: Initialize provider session
    let mut session = match ClaudeCodeAdapter::initialize(effective, &artifact_dir.root) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "provider initialization failed");
            command::teardown_processes(&mut tracked_processes);
            let end_time = chrono::Utc::now();
            let empty_outcome = executor::RunOutcome {
                steps: vec![],
                all_passed: false,
                total_duration: std::time::Duration::ZERO,
                artifact_errors: vec![],
            };
            let _ = write_best_effort_report(
                &run_id,
                &session_id,
                test_path,
                effective,
                skip_cmds,
                &config_summary,
                &empty_outcome,
                &artifact_dir,
                &start_time,
                &end_time,
            );
            return TestRunResult {
                path: test_path.to_path_buf(),
                name: test_name.to_string(),
                exit_code: EXIT_PROVIDER_ERROR,
                run_id: Some(run_id.0.clone()),
                report_path: Some(report::report_path(&artifact_dir).display().to_string()),
                error: Some(format!("provider initialization failed: {e}")),
            };
        }
    };

    if let Err(e) = session.start() {
        tracing::error!(error = %e, "provider start failed");
        command::teardown_processes(&mut tracked_processes);
        let end_time = chrono::Utc::now();
        let empty_outcome = executor::RunOutcome {
            steps: vec![],
            all_passed: false,
            total_duration: std::time::Duration::ZERO,
            artifact_errors: vec![],
        };
        let _ = write_best_effort_report(
            &run_id,
            &session_id,
            test_path,
            effective,
            skip_cmds,
            &config_summary,
            &empty_outcome,
            &artifact_dir,
            &start_time,
            &end_time,
        );
        return TestRunResult {
            path: test_path.to_path_buf(),
            name: test_name.to_string(),
            exit_code: EXIT_PROVIDER_ERROR,
            run_id: Some(run_id.0.clone()),
            report_path: Some(report::report_path(&artifact_dir).display().to_string()),
            error: Some(format!("provider start failed: {e}")),
        };
    }

    // Phase 11: Check for unexpected exits before step execution
    if let Some((name, code)) = command::check_for_unexpected_exits(&mut tracked_processes) {
        tracing::error!(command = %name, exit_code = ?code, "long-lived process exited unexpectedly");
        let _ = session.close();
        command::teardown_processes(&mut tracked_processes);
        let end_time = chrono::Utc::now();
        let empty_outcome = executor::RunOutcome {
            steps: vec![],
            all_passed: false,
            total_duration: std::time::Duration::ZERO,
            artifact_errors: vec![],
        };
        let _ = write_best_effort_report(
            &run_id,
            &session_id,
            test_path,
            effective,
            skip_cmds,
            &config_summary,
            &empty_outcome,
            &artifact_dir,
            &start_time,
            &end_time,
        );
        let code_str = code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        return TestRunResult {
            path: test_path.to_path_buf(),
            name: test_name.to_string(),
            exit_code: EXIT_PROVIDER_ERROR,
            run_id: Some(run_id.0.clone()),
            report_path: Some(report::report_path(&artifact_dir).display().to_string()),
            error: Some(format!(
                "long-lived process '{name}' exited unexpectedly (code: {code_str})"
            )),
        };
    }

    // Phase 12: Execute steps
    let outcome = match executor::execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
    ) {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::error!(error = %e, "step execution failed");
            let _ = session.close();
            command::teardown_processes(&mut tracked_processes);
            let end_time = chrono::Utc::now();
            let empty_outcome = executor::RunOutcome {
                steps: vec![],
                all_passed: false,
                total_duration: std::time::Duration::ZERO,
                artifact_errors: vec![],
            };
            let _ = write_best_effort_report(
                &run_id,
                &session_id,
                test_path,
                effective,
                skip_cmds,
                &config_summary,
                &empty_outcome,
                &artifact_dir,
                &start_time,
                &end_time,
            );
            return TestRunResult {
                path: test_path.to_path_buf(),
                name: test_name.to_string(),
                exit_code: EXIT_STEP_ERROR,
                run_id: Some(run_id.0.clone()),
                report_path: Some(report::report_path(&artifact_dir).display().to_string()),
                error: Some(format!("execution error: {e}")),
            };
        }
    };

    // Phase 13: Close provider session
    if let Err(e) = session.close() {
        tracing::warn!(error = %e, "provider session close failed (non-fatal)");
    }

    // Phase 14: Teardown long-lived processes
    let teardown_results = command::teardown_processes(&mut tracked_processes);
    for td in &teardown_results {
        if !td.success {
            tracing::warn!(command = %td.name, message = %td.message, "teardown issue");
        }
    }

    // Phase 15: Write report
    let end_time = chrono::Utc::now();
    let exit_code = exit_code::exit_code_for_run(&outcome);

    let _ = write_best_effort_report(
        &run_id,
        &session_id,
        test_path,
        effective,
        skip_cmds,
        &config_summary,
        &outcome,
        &artifact_dir,
        &start_time,
        &end_time,
    );

    tracing::info!(
        run_id = %run_id,
        exit_code = exit_code,
        all_passed = outcome.all_passed,
        "test pipeline complete"
    );

    TestRunResult {
        path: test_path.to_path_buf(),
        name: test_name.to_string(),
        exit_code,
        run_id: Some(run_id.0.clone()),
        report_path: Some(report::report_path(&artifact_dir).display().to_string()),
        error: None,
    }
}

/// Write a report, logging but not propagating errors.
#[allow(clippy::too_many_arguments)]
fn write_best_effort_report(
    run_id: &bugatti::run::RunId,
    session_id: &bugatti::run::SessionId,
    test_path: &Path,
    effective: &bugatti::config::Config,
    skip_cmds: &[String],
    config_summary: &EffectiveConfigSummary,
    outcome: &executor::RunOutcome,
    artifact_dir: &ArtifactDir,
    start_time: &chrono::DateTime<chrono::Utc>,
    end_time: &chrono::DateTime<chrono::Utc>,
) -> Result<(), report::ReportError> {
    let input = ReportInput {
        run_id,
        session_id,
        root_test_file: &test_path.display().to_string(),
        provider_name: &effective.provider.name,
        start_time: &start_time.to_rfc3339(),
        end_time: &end_time.to_rfc3339(),
        skipped_commands: skip_cmds,
        config_summary,
        outcome,
        artifact_dir,
        artifact_errors: &outcome.artifact_errors,
    };
    match report::write_report(&input, artifact_dir) {
        Ok(()) => {
            tracing::info!("report written successfully");
            Ok(())
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to write report");
            eprintln!("WARNING: failed to write report: {e}");
            Err(e)
        }
    }
}

/// Discover and run all root test files, printing an aggregate summary.
/// Returns the aggregate exit code.
fn run_discovery(project_root: &Path, skip_cmds: &[String]) -> i32 {
    println!("Discovering root test files...");

    let discovery = match discover_root_tests(project_root) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("ERROR: {e}");
            return EXIT_CONFIG_ERROR;
        }
    };

    // Report per-file parse errors before starting any runs
    for err in &discovery.errors {
        eprintln!("PARSE ERROR: {err}");
    }

    if discovery.tests.is_empty() {
        if discovery.errors.is_empty() {
            println!("No root test files found.");
            return EXIT_OK;
        } else {
            eprintln!(
                "No runnable test files found ({} had parse errors).",
                discovery.errors.len()
            );
            return EXIT_CONFIG_ERROR;
        }
    }

    println!("Found {} root test file(s):\n", discovery.tests.len());
    for test in &discovery.tests {
        println!("  - {} ({})", test.name, test.path.display());
    }
    println!();

    // Run each discovered test, continuing past failures by default
    let mut results: Vec<TestRunResult> = Vec::new();
    for test in &discovery.tests {
        // Check for interruption between tests
        if is_interrupted() {
            results.push(TestRunResult {
                path: test.path.clone(),
                name: test.name.clone(),
                exit_code: EXIT_INTERRUPTED,
                run_id: None,
                report_path: None,
                error: Some("skipped due to interruption".to_string()),
            });
            continue;
        }
        let result = run_single_test(test, project_root, skip_cmds);
        results.push(result);
    }

    // Print aggregate summary
    print_aggregate_summary(&results, &discovery.errors);

    // Print run IDs and report paths for each run
    print_run_references(&results);

    // Compute aggregate exit code
    let run_codes: Vec<i32> = results.iter().map(|r| r.exit_code).collect();
    let has_parse_errors = !discovery.errors.is_empty();
    let code = exit_code::aggregate_exit_code(&run_codes, has_parse_errors);

    // Final exit status line
    println!(
        "\nExit code: {} ({})",
        code,
        exit_code::describe_exit_code(code)
    );

    code
}

/// Run a single discovered test file through the full pipeline.
fn run_single_test(
    test: &DiscoveredTest,
    project_root: &Path,
    skip_cmds: &[String],
) -> TestRunResult {
    println!("═══════════════════════════════════════════════════════");
    println!("Running: {} ({})", test.name, test.path.display());
    println!("═══════════════════════════════════════════════════════");

    let result = run_test_pipeline(project_root, &test.path, skip_cmds);

    if let Some(err) = &result.error {
        eprintln!("  ERROR: {err}");
    }
    println!();

    result
}

/// Print the aggregate summary for a multi-test run.
fn print_aggregate_summary(
    results: &[TestRunResult],
    parse_errors: &[bugatti::discovery::DiscoveryError],
) {
    println!("═══════════════════════════════════════════════════════");
    println!("                    AGGREGATE SUMMARY                 ");
    println!("═══════════════════════════════════════════════════════");
    println!();

    let total = results.len() + parse_errors.len();
    let passed = results.iter().filter(|r| r.exit_code == EXIT_OK).count();
    let failed = results
        .iter()
        .filter(|r| r.exit_code != EXIT_OK && r.exit_code != EXIT_INTERRUPTED)
        .count();
    let interrupted = results
        .iter()
        .filter(|r| r.exit_code == EXIT_INTERRUPTED)
        .count();
    let errored = parse_errors.len();

    for result in results {
        let status = match result.exit_code {
            EXIT_OK => "PASS",
            EXIT_INTERRUPTED => "SKIP",
            _ => "FAIL",
        };
        println!(
            "  {status} ........ {} ({})",
            result.name,
            result.path.display()
        );
        if let Some(err) = &result.error {
            println!("               {err}");
        }
    }
    for err in parse_errors {
        println!("  ERROR ....... {err}");
    }

    println!();
    let mut summary = format!("  {total} total, {passed} passed, {failed} failed");
    if interrupted > 0 {
        summary.push_str(&format!(", {interrupted} interrupted"));
    }
    if errored > 0 {
        summary.push_str(&format!(", {errored} parse errors"));
    }
    println!("{summary}");
    println!("═══════════════════════════════════════════════════════");
}

/// Print run ID and report path for each completed run.
fn print_run_references(results: &[TestRunResult]) {
    let has_refs = results
        .iter()
        .any(|r| r.run_id.is_some() || r.report_path.is_some());
    if !has_refs {
        return;
    }

    println!();
    for result in results {
        if let Some(run_id) = &result.run_id {
            print!("  Run ID: {run_id}");
            if let Some(report) = &result.report_path {
                print!("  Report: {report}");
            }
            println!("  ({})", result.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use bugatti::cli::Cli;

    #[test]
    fn test_subcommand_no_path() {
        let cli = Cli::parse_from(["bugatti", "test"]);
        match cli.command {
            bugatti::cli::Commands::Test { path, skip_cmds } => {
                assert!(path.is_none());
                assert!(skip_cmds.is_empty());
            }
        }
    }

    #[test]
    fn test_subcommand_with_path() {
        let cli = Cli::parse_from(["bugatti", "test", "some/path.test.toml"]);
        match cli.command {
            bugatti::cli::Commands::Test { path, skip_cmds } => {
                assert_eq!(path.unwrap(), "some/path.test.toml");
                assert!(skip_cmds.is_empty());
            }
        }
    }

    #[test]
    fn test_subcommand_with_skip_cmd() {
        let cli = Cli::parse_from(["bugatti", "test", "--skip-cmd", "migrate"]);
        match cli.command {
            bugatti::cli::Commands::Test { path, skip_cmds } => {
                assert!(path.is_none());
                assert_eq!(skip_cmds, vec!["migrate".to_string()]);
            }
        }
    }

    #[test]
    fn test_subcommand_with_multiple_skip_cmds() {
        let cli = Cli::parse_from([
            "bugatti",
            "test",
            "my.test.toml",
            "--skip-cmd",
            "migrate",
            "--skip-cmd",
            "server",
        ]);
        match cli.command {
            bugatti::cli::Commands::Test { path, skip_cmds } => {
                assert_eq!(path.unwrap(), "my.test.toml");
                assert_eq!(skip_cmds, vec!["migrate".to_string(), "server".to_string()]);
            }
        }
    }

    #[test]
    fn test_help_text_includes_exit_codes() {
        // Verify that exit codes are documented in the CLI help
        let result = Cli::try_parse_from(["bugatti", "--help"]);
        match result {
            Err(e) => {
                let help_text = e.to_string();
                assert!(
                    help_text.contains("Exit codes:"),
                    "help text should include exit codes section"
                );
                assert!(
                    help_text.contains("All steps passed"),
                    "help text should describe exit code 0"
                );
            }
            Ok(_) => panic!("--help should produce an error-like result from clap"),
        }
    }
}
