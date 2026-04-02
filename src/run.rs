use crate::config::Config;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Unique identifier for a single test run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunId(pub String);

/// Unique identifier for a provider session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionId(pub String);

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Metadata written to the run's artifact directory at the start of a run.
#[derive(Debug, Clone, Serialize)]
pub struct RunMetadata {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub root_test_file: String,
    pub project_root: String,
    pub provider_name: String,
    pub start_time: DateTime<Utc>,
    pub effective_config_summary: EffectiveConfigSummary,
}

/// Summary of the effective config for inclusion in run metadata.
#[derive(Debug, Clone, Serialize)]
pub struct EffectiveConfigSummary {
    pub provider_name: String,
    pub has_extra_system_prompt: bool,
    pub agent_args: Vec<String>,
    pub command_names: Vec<String>,
    pub step_timeout_secs: Option<u64>,
    pub strict_warnings: Option<bool>,
    pub base_url: Option<String>,
}

impl EffectiveConfigSummary {
    pub fn from_config(config: &Config) -> Self {
        Self {
            provider_name: config.provider.name.clone(),
            has_extra_system_prompt: config.provider.extra_system_prompt.is_some(),
            agent_args: config.provider.agent_args.clone(),
            command_names: config.commands.keys().cloned().collect(),
            step_timeout_secs: config.provider.step_timeout_secs,
            strict_warnings: config.provider.strict_warnings,
            base_url: config.provider.base_url.clone(),
        }
    }
}

/// The artifact directory layout for a single run.
#[derive(Debug, Clone)]
pub struct ArtifactDir {
    pub root: PathBuf,
    pub transcripts: PathBuf,
    pub screenshots: PathBuf,
    pub logs: PathBuf,
    pub diagnostics: PathBuf,
}

impl ArtifactDir {
    /// Compute artifact paths from a project root and run ID.
    /// Does not create the directories.
    pub fn from_run_id(project_root: &Path, run_id: &RunId) -> Self {
        let root = project_root.join(".bugatti").join("runs").join(&run_id.0);
        Self {
            transcripts: root.join("transcripts"),
            screenshots: root.join("screenshots"),
            logs: root.join("logs"),
            diagnostics: root.join("diagnostics"),
            root,
        }
    }

    /// Create all artifact directories on disk.
    pub fn create_all(&self) -> Result<(), ArtifactError> {
        for dir in [
            &self.root,
            &self.transcripts,
            &self.screenshots,
            &self.logs,
            &self.diagnostics,
        ] {
            std::fs::create_dir_all(dir).map_err(|e| ArtifactError::DirectoryCreation {
                path: dir.display().to_string(),
                source: e,
            })?;
        }
        Ok(())
    }

    /// Path to the run metadata JSON file.
    pub fn metadata_path(&self) -> PathBuf {
        self.root.join("run_metadata.json")
    }
}

/// Error type for artifact operations.
#[derive(Debug)]
pub enum ArtifactError {
    DirectoryCreation {
        path: String,
        source: std::io::Error,
    },
    MetadataWrite {
        path: String,
        source: std::io::Error,
    },
    MetadataSerialize(serde_json::Error),
}

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArtifactError::DirectoryCreation { path, source } => {
                write!(f, "failed to create artifact directory '{path}': {source}")
            }
            ArtifactError::MetadataWrite { path, source } => {
                write!(f, "failed to write run metadata to '{path}': {source}")
            }
            ArtifactError::MetadataSerialize(e) => {
                write!(f, "failed to serialize run metadata: {e}")
            }
        }
    }
}

impl std::error::Error for ArtifactError {}

/// Initialize a run: generate IDs, create artifact directories, and write metadata.
///
/// Returns the run ID, session ID, and artifact directory layout on success.
/// Fails with a clear error if directory creation or metadata writing fails.
pub fn initialize_run(
    project_root: &Path,
    root_test_file: &Path,
    effective_config: &Config,
) -> Result<(RunId, SessionId, ArtifactDir), ArtifactError> {
    let run_id = RunId::new();
    let session_id = SessionId::new();
    tracing::info!(
        run_id = %run_id,
        session_id = %session_id,
        test_file = %root_test_file.display(),
        "initializing run"
    );
    let artifact_dir = ArtifactDir::from_run_id(project_root, &run_id);

    artifact_dir.create_all()?;

    let metadata = RunMetadata {
        run_id: run_id.clone(),
        session_id: session_id.clone(),
        root_test_file: root_test_file.display().to_string(),
        project_root: project_root.display().to_string(),
        provider_name: effective_config.provider.name.clone(),
        start_time: Utc::now(),
        effective_config_summary: EffectiveConfigSummary::from_config(effective_config),
    };

    let json = serde_json::to_string_pretty(&metadata).map_err(ArtifactError::MetadataSerialize)?;

    let metadata_path = artifact_dir.metadata_path();
    std::fs::write(&metadata_path, json).map_err(|e| ArtifactError::MetadataWrite {
        path: metadata_path.display().to_string(),
        source: e,
    })?;

    Ok((run_id, session_id, artifact_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CommandDef, CommandKind, Config, ProviderConfig};
    use std::collections::BTreeMap;

    fn test_config() -> Config {
        let mut commands = BTreeMap::new();
        commands.insert(
            "migrate".to_string(),
            CommandDef {
                kind: CommandKind::ShortLived,
                cmd: "cargo sqlx migrate run".to_string(),
                readiness_url: None,
                readiness_urls: Vec::new(),
                readiness_timeout_secs: None,
            },
        );
        Config {
            provider: ProviderConfig {
                name: "claude-code".to_string(),
                extra_system_prompt: Some("Be concise".to_string()),
                agent_args: vec!["--verbose".to_string()],
                step_timeout_secs: None,
                strict_warnings: None,
                base_url: None,
            },
            commands,
            checkpoint: None,
        }
    }

    #[test]
    fn run_id_is_unique() {
        let a = RunId::new();
        let b = RunId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn session_id_is_unique() {
        let a = SessionId::new();
        let b = SessionId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn artifact_dir_paths_are_deterministic() {
        let run_id = RunId("test-run-123".to_string());
        let project_root = Path::new("/tmp/project");
        let dir = ArtifactDir::from_run_id(project_root, &run_id);

        assert_eq!(
            dir.root,
            PathBuf::from("/tmp/project/.bugatti/runs/test-run-123")
        );
        assert_eq!(dir.transcripts, dir.root.join("transcripts"));
        assert_eq!(dir.screenshots, dir.root.join("screenshots"));
        assert_eq!(dir.logs, dir.root.join("logs"));
        assert_eq!(dir.diagnostics, dir.root.join("diagnostics"));
    }

    #[test]
    fn create_artifact_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run".to_string());
        let dir = ArtifactDir::from_run_id(tmp.path(), &run_id);

        dir.create_all().unwrap();

        assert!(dir.root.is_dir());
        assert!(dir.transcripts.is_dir());
        assert!(dir.screenshots.is_dir());
        assert!(dir.logs.is_dir());
        assert!(dir.diagnostics.is_dir());
    }

    #[test]
    fn initialize_run_creates_dirs_and_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let config = test_config();
        let test_file_path = Path::new("tests/login.test.toml");

        let (run_id, session_id, artifact_dir) =
            initialize_run(tmp.path(), test_file_path, &config).unwrap();

        // Directories exist
        assert!(artifact_dir.root.is_dir());
        assert!(artifact_dir.transcripts.is_dir());
        assert!(artifact_dir.screenshots.is_dir());
        assert!(artifact_dir.logs.is_dir());
        assert!(artifact_dir.diagnostics.is_dir());

        // Metadata file exists and contains expected content
        let metadata_path = artifact_dir.metadata_path();
        assert!(metadata_path.is_file());

        let contents = std::fs::read_to_string(&metadata_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();

        assert_eq!(parsed["run_id"], run_id.0);
        assert_eq!(parsed["session_id"], session_id.0);
        assert_eq!(parsed["root_test_file"], "tests/login.test.toml");
        assert_eq!(parsed["project_root"], tmp.path().display().to_string());
        assert_eq!(parsed["provider_name"], "claude-code");
        assert!(parsed["start_time"].is_string());

        // Config summary
        let summary = &parsed["effective_config_summary"];
        assert_eq!(summary["provider_name"], "claude-code");
        assert_eq!(summary["has_extra_system_prompt"], true);
        assert_eq!(summary["agent_args"], serde_json::json!(["--verbose"]));
        assert_eq!(summary["command_names"], serde_json::json!(["migrate"]));
    }

    #[test]
    fn artifact_dir_creation_failure_reports_path() {
        let run_id = RunId("test".to_string());
        // Use an invalid path that cannot be created
        let dir = ArtifactDir::from_run_id(Path::new("/dev/null/impossible"), &run_id);
        let err = dir.create_all().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("failed to create artifact directory"),
            "got: {msg}"
        );
    }
}
