use crate::run::ArtifactDir;
use serde::Serialize;
use std::path::PathBuf;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The filename for the harness diagnostics log within the diagnostics/ directory.
const DIAGNOSTICS_FILENAME: &str = "harness_trace.jsonl";

/// A guard that keeps the tracing file writer alive.
/// When dropped, the file is flushed and closed.
pub struct TracingGuard {
    _guard: tracing::subscriber::DefaultGuard,
}

/// Initialize structured tracing that writes JSON lines to the diagnostics directory.
///
/// Returns a guard that must be held alive for the duration of the run.
/// When the guard is dropped, the tracing subscriber is removed.
pub fn init_tracing(artifact_dir: &ArtifactDir) -> Result<TracingGuard, TracingError> {
    let diagnostics_path = diagnostics_log_path(artifact_dir);

    let file = std::fs::File::create(&diagnostics_path).map_err(|e| TracingError::FileCreate {
        path: diagnostics_path.display().to_string(),
        source: e,
    })?;

    let file_writer = FileWriter(std::sync::Mutex::new(file));

    let json_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(file_writer)
        .with_target(true)
        .with_timer(tracing_subscriber::fmt::time::uptime())
        .with_level(true);

    let subscriber = tracing_subscriber::registry().with(json_layer);

    let guard = subscriber.set_default();

    Ok(TracingGuard { _guard: guard })
}

/// The path to the diagnostics log file within a run's artifact directory.
pub fn diagnostics_log_path(artifact_dir: &ArtifactDir) -> PathBuf {
    artifact_dir.diagnostics.join(DIAGNOSTICS_FILENAME)
}

/// Thread-safe file writer for tracing-subscriber.
struct FileWriter(std::sync::Mutex<std::fs::File>);

impl<'a> MakeWriter<'a> for FileWriter {
    type Writer = FileWriterGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        FileWriterGuard(self.0.lock().expect("tracing file writer lock poisoned"))
    }
}

/// Guard returned by FileWriter that implements io::Write.
struct FileWriterGuard<'a>(std::sync::MutexGuard<'a, std::fs::File>);

impl std::io::Write for FileWriterGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        std::io::Write::write(&mut *self.0, buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut *self.0)
    }
}

/// Error type for tracing initialization.
#[derive(Debug)]
pub enum TracingError {
    /// Failed to create the diagnostics log file.
    FileCreate {
        path: String,
        source: std::io::Error,
    },
}

impl std::fmt::Display for TracingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TracingError::FileCreate { path, source } => {
                write!(
                    f,
                    "failed to create diagnostics log file '{path}': {source}"
                )
            }
        }
    }
}

impl std::error::Error for TracingError {}

/// The type of evidence that can be referenced in a run artifact.
///
/// Evidence references point to durable paths under the run directory
/// rather than carrying inline payloads.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// A screenshot capture (e.g., browser screenshot during a step).
    Screenshot,
    /// Output from a harness command (stdout/stderr logs).
    CommandLog,
    /// Browser console output captured during execution.
    BrowserConsole,
    /// Network failure details (e.g., failed HTTP requests).
    NetworkFailure,
    /// SQL query output or CLI command evidence.
    SqlCliEvidence,
}

impl std::fmt::Display for EvidenceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvidenceKind::Screenshot => write!(f, "Screenshot"),
            EvidenceKind::CommandLog => write!(f, "Command Log"),
            EvidenceKind::BrowserConsole => write!(f, "Browser Console"),
            EvidenceKind::NetworkFailure => write!(f, "Network Failure"),
            EvidenceKind::SqlCliEvidence => write!(f, "SQL/CLI Evidence"),
        }
    }
}

/// A reference to a piece of evidence collected during a step.
///
/// Evidence references point to durable paths under the run directory.
/// If evidence collection fails, the reference can carry an error note
/// instead of a valid path so the report can indicate the failure.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvidenceRef {
    /// The type of evidence.
    pub kind: EvidenceKind,
    /// Path to the evidence file relative to the run directory, or absolute.
    pub path: PathBuf,
    /// Human-readable description of this evidence.
    pub description: String,
    /// If evidence collection failed, this carries the error message.
    /// The run still completes; the report notes the failure.
    pub collection_error: Option<String>,
}

impl EvidenceRef {
    /// Whether this evidence was successfully collected (path exists, no error).
    pub fn is_available(&self) -> bool {
        self.collection_error.is_none() && self.path.exists()
    }
}

/// Categorization of run artifacts for the internal run model.
///
/// The run model distinguishes between different types of artifacts:
/// - **HarnessDiagnostics**: Structured tracing from the harness itself (config load,
///   command execution, provider lifecycle, step execution, report compilation).
///   Stored in diagnostics/ as JSONL.
/// - **AgentLogs**: BUGATTI_LOG events parsed from provider output, scoped to steps.
///   Stored in logs/bugatti_log_events.txt.
/// - **Transcript**: Raw provider output captured during step execution.
///   Stored in transcripts/ (per-step and combined).
/// - **Evidence**: References to external evidence (screenshots, command logs, etc.).
///   Stored under the run directory at referenced paths.
/// - **Report**: The compiled human-readable report.md.
///   Stored at the run root.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// Structured harness tracing output (JSONL).
    HarnessDiagnostics,
    /// BUGATTI_LOG events from provider output.
    AgentLogs,
    /// Raw provider transcript (per-step and combined).
    Transcript,
    /// Evidence references (screenshots, command logs, browser output, etc.).
    Evidence,
    /// The compiled report.md.
    Report,
}

/// A reference to a run artifact with its kind and path.
#[derive(Debug, Clone, Serialize)]
pub struct ArtifactRef {
    /// The kind of artifact.
    pub kind: ArtifactKind,
    /// The path to the artifact relative to the run directory, or absolute.
    pub path: PathBuf,
    /// Optional description of the artifact.
    pub description: Option<String>,
}

/// Collect all artifact references for a completed run.
pub fn collect_artifact_refs(artifact_dir: &ArtifactDir) -> Vec<ArtifactRef> {
    let mut refs = Vec::new();

    // Harness diagnostics
    let diag_path = diagnostics_log_path(artifact_dir);
    if diag_path.exists() {
        refs.push(ArtifactRef {
            kind: ArtifactKind::HarnessDiagnostics,
            path: diag_path,
            description: Some("Structured harness tracing events (JSONL)".to_string()),
        });
    }

    // Agent logs
    let agent_log_path = artifact_dir.logs.join("bugatti_log_events.txt");
    if agent_log_path.exists() {
        refs.push(ArtifactRef {
            kind: ArtifactKind::AgentLogs,
            path: agent_log_path,
            description: Some("BUGATTI_LOG events from provider output".to_string()),
        });
    }

    // Transcripts
    let full_transcript = artifact_dir.transcripts.join("full_transcript.txt");
    if full_transcript.exists() {
        refs.push(ArtifactRef {
            kind: ArtifactKind::Transcript,
            path: full_transcript,
            description: Some("Combined provider transcript".to_string()),
        });
    }

    // Evidence files in screenshots directory
    if artifact_dir.screenshots.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&artifact_dir.screenshots) {
            for entry in entries.flatten() {
                if entry.path().is_file() {
                    refs.push(ArtifactRef {
                        kind: ArtifactKind::Evidence,
                        path: entry.path(),
                        description: Some(format!(
                            "Evidence: {}",
                            entry.file_name().to_string_lossy()
                        )),
                    });
                }
            }
        }
    }

    // Report
    let report_path = artifact_dir.root.join("report.md");
    if report_path.exists() {
        refs.push(ArtifactRef {
            kind: ArtifactKind::Report,
            path: report_path,
            description: Some("Human-readable run report".to_string()),
        });
    }

    refs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{ArtifactDir, RunId};

    #[test]
    fn diagnostics_log_path_is_in_diagnostics_dir() {
        let run_id = RunId("test-run".to_string());
        let dir = ArtifactDir::from_run_id(std::path::Path::new("/tmp/project"), &run_id);
        let path = diagnostics_log_path(&dir);
        assert!(path.starts_with(&dir.diagnostics));
        assert!(path.to_str().unwrap().ends_with("harness_trace.jsonl"));
    }

    #[test]
    fn init_tracing_creates_diagnostics_file() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        dir.create_all().unwrap();

        let guard = init_tracing(&dir).unwrap();
        let log_path = diagnostics_log_path(&dir);
        assert!(log_path.is_file());

        // Emit a tracing event and verify it's written
        tracing::info!(phase = "test", "test event");

        // Drop the guard to flush
        drop(guard);

        let contents = std::fs::read_to_string(&log_path).unwrap();
        assert!(
            contents.contains("test event"),
            "diagnostics file should contain tracing output: {contents}"
        );
    }

    #[test]
    fn init_tracing_writes_json_format() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        dir.create_all().unwrap();

        let guard = init_tracing(&dir).unwrap();
        tracing::info!(component = "config", "config loaded");
        drop(guard);

        let contents = std::fs::read_to_string(diagnostics_log_path(&dir)).unwrap();
        // Each line should be valid JSON
        for line in contents.lines() {
            if !line.trim().is_empty() {
                let parsed: serde_json::Value = serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("line is not valid JSON: {e}\nline: {line}"));
                assert!(parsed.is_object());
                assert!(parsed["fields"]["message"].is_string());
            }
        }
    }

    #[test]
    fn harness_tracing_distinct_from_agent_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        dir.create_all().unwrap();

        // Write a mock agent log file
        let agent_log_path = dir.logs.join("bugatti_log_events.txt");
        std::fs::write(&agent_log_path, "[step 0] Agent log event\n").unwrap();

        // Write harness tracing
        let guard = init_tracing(&dir).unwrap();
        tracing::info!(phase = "setup", "harness event");
        drop(guard);

        // Verify they are in separate files
        let diag_contents = std::fs::read_to_string(diagnostics_log_path(&dir)).unwrap();
        let agent_contents = std::fs::read_to_string(&agent_log_path).unwrap();

        assert!(diag_contents.contains("harness event"));
        assert!(!diag_contents.contains("Agent log event"));
        assert!(agent_contents.contains("Agent log event"));
        assert!(!agent_contents.contains("harness event"));
    }

    #[test]
    fn artifact_kind_serialization() {
        let json = serde_json::to_string(&ArtifactKind::HarnessDiagnostics).unwrap();
        assert_eq!(json, "\"harness_diagnostics\"");

        let json = serde_json::to_string(&ArtifactKind::AgentLogs).unwrap();
        assert_eq!(json, "\"agent_logs\"");

        let json = serde_json::to_string(&ArtifactKind::Transcript).unwrap();
        assert_eq!(json, "\"transcript\"");

        let json = serde_json::to_string(&ArtifactKind::Evidence).unwrap();
        assert_eq!(json, "\"evidence\"");

        let json = serde_json::to_string(&ArtifactKind::Report).unwrap();
        assert_eq!(json, "\"report\"");
    }

    #[test]
    fn collect_artifact_refs_empty_run() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        dir.create_all().unwrap();

        let refs = collect_artifact_refs(&dir);
        assert!(refs.is_empty());
    }

    #[test]
    fn collect_artifact_refs_with_files() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        dir.create_all().unwrap();

        // Create various artifact files
        std::fs::write(diagnostics_log_path(&dir), "{}").unwrap();
        std::fs::write(dir.logs.join("bugatti_log_events.txt"), "log").unwrap();
        std::fs::write(dir.transcripts.join("full_transcript.txt"), "transcript").unwrap();
        std::fs::write(dir.root.join("report.md"), "# Report").unwrap();

        let refs = collect_artifact_refs(&dir);
        assert_eq!(refs.len(), 4);

        let kinds: Vec<&ArtifactKind> = refs.iter().map(|r| &r.kind).collect();
        assert!(kinds.contains(&&ArtifactKind::HarnessDiagnostics));
        assert!(kinds.contains(&&ArtifactKind::AgentLogs));
        assert!(kinds.contains(&&ArtifactKind::Transcript));
        assert!(kinds.contains(&&ArtifactKind::Report));
    }

    #[test]
    fn collect_artifact_refs_includes_evidence_files() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        dir.create_all().unwrap();

        // Create screenshot evidence files
        std::fs::write(dir.screenshots.join("step_0_login.png"), "png-data").unwrap();
        std::fs::write(dir.screenshots.join("step_1_dashboard.png"), "png-data").unwrap();

        let refs = collect_artifact_refs(&dir);
        let evidence_refs: Vec<_> = refs
            .iter()
            .filter(|r| r.kind == ArtifactKind::Evidence)
            .collect();
        assert_eq!(evidence_refs.len(), 2);
    }

    #[test]
    fn evidence_kind_serialization() {
        assert_eq!(
            serde_json::to_string(&EvidenceKind::Screenshot).unwrap(),
            "\"screenshot\""
        );
        assert_eq!(
            serde_json::to_string(&EvidenceKind::CommandLog).unwrap(),
            "\"command_log\""
        );
        assert_eq!(
            serde_json::to_string(&EvidenceKind::BrowserConsole).unwrap(),
            "\"browser_console\""
        );
        assert_eq!(
            serde_json::to_string(&EvidenceKind::NetworkFailure).unwrap(),
            "\"network_failure\""
        );
        assert_eq!(
            serde_json::to_string(&EvidenceKind::SqlCliEvidence).unwrap(),
            "\"sql_cli_evidence\""
        );
    }

    #[test]
    fn evidence_kind_display() {
        assert_eq!(EvidenceKind::Screenshot.to_string(), "Screenshot");
        assert_eq!(EvidenceKind::CommandLog.to_string(), "Command Log");
        assert_eq!(EvidenceKind::BrowserConsole.to_string(), "Browser Console");
        assert_eq!(EvidenceKind::NetworkFailure.to_string(), "Network Failure");
        assert_eq!(EvidenceKind::SqlCliEvidence.to_string(), "SQL/CLI Evidence");
    }

    #[test]
    fn evidence_ref_is_available_when_file_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("screenshot.png");
        std::fs::write(&file_path, "data").unwrap();

        let evidence = EvidenceRef {
            kind: EvidenceKind::Screenshot,
            path: file_path,
            description: "Login page screenshot".to_string(),
            collection_error: None,
        };
        assert!(evidence.is_available());
    }

    #[test]
    fn evidence_ref_not_available_when_file_missing() {
        let evidence = EvidenceRef {
            kind: EvidenceKind::Screenshot,
            path: PathBuf::from("/nonexistent/screenshot.png"),
            description: "Login page screenshot".to_string(),
            collection_error: None,
        };
        assert!(!evidence.is_available());
    }

    #[test]
    fn evidence_ref_not_available_when_collection_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("screenshot.png");
        std::fs::write(&file_path, "data").unwrap();

        let evidence = EvidenceRef {
            kind: EvidenceKind::Screenshot,
            path: file_path,
            description: "Login page screenshot".to_string(),
            collection_error: Some("browser not available".to_string()),
        };
        assert!(!evidence.is_available());
    }

    #[test]
    fn evidence_ref_serialization() {
        let evidence = EvidenceRef {
            kind: EvidenceKind::Screenshot,
            path: PathBuf::from("screenshots/step_0.png"),
            description: "Login page".to_string(),
            collection_error: None,
        };
        let json: serde_json::Value = serde_json::to_value(&evidence).unwrap();
        assert_eq!(json["kind"], "screenshot");
        assert_eq!(json["path"], "screenshots/step_0.png");
        assert_eq!(json["description"], "Login page");
        assert!(json["collection_error"].is_null());
    }

    #[test]
    fn evidence_ref_serialization_with_error() {
        let evidence = EvidenceRef {
            kind: EvidenceKind::BrowserConsole,
            path: PathBuf::from("logs/console.txt"),
            description: "Browser console".to_string(),
            collection_error: Some("browser crashed".to_string()),
        };
        let json: serde_json::Value = serde_json::to_value(&evidence).unwrap();
        assert_eq!(json["collection_error"], "browser crashed");
    }
}
