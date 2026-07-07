/// Progress output abstraction used by the command and executor pipelines.
pub trait ProgressReporter: Send + Sync {
    /// Emit one user-facing progress line.
    fn line(&self, line: &str);
}

/// Default reporter that writes progress lines to stdout.
#[derive(Debug, Default, Clone, Copy)]
pub struct StdoutProgressReporter;

impl ProgressReporter for StdoutProgressReporter {
    fn line(&self, line: &str) {
        println!("{line}");
    }
}

/// Shared stdout reporter instance.
pub const STDOUT_PROGRESS_REPORTER: StdoutProgressReporter = StdoutProgressReporter;
