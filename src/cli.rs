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
  6  Setup command failed

Docs:    https://bugatti.dev
LLM ref: https://bugatti.dev/llms.txt
"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run tests from a *.test.toml file or discover all root test files
    #[command(after_help = "Docs: https://bugatti.dev/llms/cli-reference.txt")]
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

        /// Resume from a named checkpoint: automatically skip all steps up to and
        /// including the step with this checkpoint, restore it, then execute the rest.
        #[arg(long = "from-checkpoint")]
        from_checkpoint: Option<String>,

        /// Enable verbose output: show full prompts, provider command lines, and timing details.
        #[arg(long, short)]
        verbose: bool,
    },
}
