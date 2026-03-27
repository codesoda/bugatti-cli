use clap::{Parser, Subcommand};

/// Bugatti - Agent-assisted local application verification
#[derive(Parser, Debug)]
#[command(name = "bugatti", version, about)]
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
    },
}
