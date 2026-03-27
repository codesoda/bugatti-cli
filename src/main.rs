use clap::Parser;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bugatti::cli::{Cli, Commands};
use bugatti::discovery::{discover_root_tests, DiscoveredTest};
use bugatti::exit_code::{self, EXIT_CONFIG_ERROR, EXIT_INTERRUPTED, EXIT_OK};

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
            if !skip_cmds.is_empty() {
                println!("Skipping commands: {}", skip_cmds.join(", "));
            }
            match path {
                Some(p) => {
                    println!("Running test file: {p}");
                    // Single-file mode: placeholder until US-021 wires the full pipeline
                    EXIT_OK
                }
                None => {
                    let project_root = std::env::current_dir().unwrap_or_else(|e| {
                        eprintln!("ERROR: failed to determine current directory: {e}");
                        std::process::exit(EXIT_CONFIG_ERROR);
                    });
                    run_discovery(&project_root)
                }
            }
        }
    };

    std::process::exit(code);
}

/// Discover and run all root test files, printing an aggregate summary.
/// Returns the aggregate exit code.
fn run_discovery(project_root: &Path) -> i32 {
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
        let result = run_single_test(test);
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

/// Run a single discovered test file.
/// Currently a placeholder — returns a "not implemented" result.
fn run_single_test(test: &DiscoveredTest) -> TestRunResult {
    println!("═══════════════════════════════════════════════════════");
    println!("Running: {} ({})", test.name, test.path.display());
    println!("═══════════════════════════════════════════════════════");

    // Placeholder: actual execution will be wired in US-021
    println!("  (execution not yet wired — see US-021)");
    println!();

    TestRunResult {
        path: test.path.clone(),
        name: test.name.clone(),
        exit_code: EXIT_OK,
        run_id: None,
        report_path: None,
        error: None,
    }
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
