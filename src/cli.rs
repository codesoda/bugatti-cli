use clap::{Parser, Subcommand};

/// Bugatti - Agent-assisted local application verification
#[derive(Parser, Debug)]
#[command(
    name = "bugatti",
    version,
    about,
    after_help = "\
Exit codes:
  0  All steps passed (OK or WARN)
  1  One or more steps had ERROR results
  2  Configuration, parse, or cycle error
  3  Provider startup or readiness failure
  4  Step execution timeout
  5  Run was interrupted (Ctrl+C)
"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run tests from a *.test.toml file or discover all root test files
    Test {
        /// Path to a specific .test.toml file to run
        path: Option<String>,

        /// Skip launching a harness command by name (can be repeated).
        /// The command will not be started, tracked, or torn down.
        /// Readiness checks for skipped commands still run by default.
        #[arg(long = "skip-cmd")]
        skip_cmds: Vec<String>,

        /// Skip readiness checks for the named commands (can be repeated).
        /// Only meaningful for commands also passed to --skip-cmd.
        #[arg(long = "skip-readiness")]
        skip_readiness: Vec<String>,

        /// Treat WARN verdicts as failures for exit code purposes.
        /// When set, runs with only warnings exit non-zero (exit code 1).
        #[arg(long)]
        strict_warnings: bool,

        /// Enable verbose output: show full prompts, provider command lines, and timing details.
        #[arg(long, short)]
        verbose: bool,
    },
}
