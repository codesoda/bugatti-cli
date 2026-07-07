pub mod claude_code;
pub mod cli;
pub mod codex;
pub mod command;
pub mod config;
pub mod diagnostics;
pub mod discovery;
pub mod executor;
pub mod exit_code;
pub mod expand;
pub mod output;
pub mod pi;
pub mod provider;
pub mod report;
pub mod run;
pub mod test_file;
// Shared test fixtures and mock providers. Available to unit tests via
// `crate::test_support` and to integration tests via the `test-support`
// feature (enabled by the self dev-dependency in Cargo.toml); compiled out
// of release builds.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod update;
