use clap::Parser;
use std::path::{Path, PathBuf};

use bugatti::cli::{Cli, Commands};
use bugatti::discovery::{discover_root_tests, DiscoveredTest};

/// Outcome of running a single root test file.
#[derive(Debug)]
struct TestRunResult {
    /// Path to the test file.
    path: PathBuf,
    /// Name from the test file.
    name: String,
    /// Whether the test passed (true) or had errors (false).
    passed: bool,
    /// Error message if the test failed before execution.
    error: Option<String>,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Test { path, skip_cmds } => {
            if !skip_cmds.is_empty() {
                println!("Skipping commands: {}", skip_cmds.join(", "));
            }
            match path {
                Some(p) => {
                    println!("Running test file: {p}");
                }
                None => {
                    let project_root = std::env::current_dir().unwrap_or_else(|e| {
                        eprintln!("ERROR: failed to determine current directory: {e}");
                        std::process::exit(1);
                    });
                    run_discovery(&project_root);
                }
            }
        }
    }
}

/// Discover and run all root test files, printing an aggregate summary.
fn run_discovery(project_root: &Path) {
    println!("Discovering root test files...");

    let discovery = match discover_root_tests(project_root) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    };

    // Report per-file parse errors before starting any runs
    for err in &discovery.errors {
        eprintln!("PARSE ERROR: {err}");
    }

    if discovery.tests.is_empty() {
        if discovery.errors.is_empty() {
            println!("No root test files found.");
        } else {
            eprintln!(
                "No runnable test files found ({} had parse errors).",
                discovery.errors.len()
            );
            std::process::exit(1);
        }
        return;
    }

    println!("Found {} root test file(s):\n", discovery.tests.len());
    for test in &discovery.tests {
        println!("  - {} ({})", test.name, test.path.display());
    }
    println!();

    // Run each discovered test
    let mut results: Vec<TestRunResult> = Vec::new();
    for test in &discovery.tests {
        let result = run_single_test(test);
        results.push(result);
    }

    // Print aggregate summary
    print_aggregate_summary(&results, &discovery.errors);

    // Exit based on aggregate outcome
    let has_failures = results.iter().any(|r| !r.passed) || !discovery.errors.is_empty();
    if has_failures {
        std::process::exit(1);
    }
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
        passed: true,
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
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.iter().filter(|r| !r.passed).count();
    let errored = parse_errors.len();

    for result in results {
        let status = if result.passed { "PASS" } else { "FAIL" };
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
    println!("  {total} total, {passed} passed, {failed} failed, {errored} parse errors");
    println!("═══════════════════════════════════════════════════════");
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
}
