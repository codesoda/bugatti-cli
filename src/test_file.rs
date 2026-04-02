use serde::Deserialize;
use std::path::Path;

/// A parsed test file loaded from a *.test.toml file.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TestFile {
    /// Name of the test.
    pub name: String,
    /// Optional per-test overrides.
    #[serde(default)]
    pub overrides: Option<TestOverrides>,
    /// Ordered list of steps to execute.
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// Per-test overrides that merge over the global config.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TestOverrides {
    #[serde(default)]
    pub provider: Option<ProviderOverrides>,
}

/// Provider-level overrides for a single test.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderOverrides {
    pub name: Option<String>,
    pub extra_system_prompt: Option<String>,
    pub agent_args: Option<Vec<String>>,
    pub step_timeout_secs: Option<u64>,
    pub base_url: Option<String>,
}

/// A single step in a test file — either an instruction or an include.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Human-readable instruction text for the agent.
    pub instruction: Option<String>,
    /// Path to a single test file to include inline.
    pub include_path: Option<String>,
    /// Glob pattern to include multiple test files inline.
    pub include_glob: Option<String>,
    /// Optional per-step timeout override in seconds.
    pub step_timeout_secs: Option<u64>,
    /// If true, this step is skipped during execution (counts as passed).
    #[serde(default)]
    pub skip: bool,
}

/// Error type for test file parsing.
#[derive(Debug)]
pub enum TestFileError {
    /// Failed to read the test file.
    ReadError {
        path: String,
        source: std::io::Error,
    },
    /// Failed to parse the TOML content.
    ParseError {
        path: String,
        source: toml::de::Error,
    },
    /// Step has invalid field combination.
    InvalidStep {
        path: String,
        step_index: usize,
        message: String,
    },
}

impl std::fmt::Display for TestFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestFileError::ReadError { path, source } => {
                write!(f, "failed to read test file '{path}': {source}")
            }
            TestFileError::ParseError { path, source } => {
                write!(f, "failed to parse test file '{path}': {source}")
            }
            TestFileError::InvalidStep {
                path,
                step_index,
                message,
            } => {
                write!(f, "invalid step {step_index} in '{path}': {message}")
            }
        }
    }
}

impl std::error::Error for TestFileError {}

/// Parse a test file from the given path.
pub fn parse_test_file(path: &Path) -> Result<TestFile, TestFileError> {
    let path_str = path.display().to_string();
    let contents = std::fs::read_to_string(path).map_err(|e| TestFileError::ReadError {
        path: path_str.clone(),
        source: e,
    })?;
    let test_file: TestFile = toml::from_str(&contents).map_err(|e| TestFileError::ParseError {
        path: path_str.clone(),
        source: e,
    })?;

    // Validate each step has exactly one of: instruction, include_path, include_glob
    for (i, step) in test_file.steps.iter().enumerate() {
        let has_instruction = step.instruction.is_some();
        let has_include_path = step.include_path.is_some();
        let has_include_glob = step.include_glob.is_some();

        let set_count = has_instruction as u8 + has_include_path as u8 + has_include_glob as u8;

        if set_count == 0 {
            return Err(TestFileError::InvalidStep {
                path: path_str,
                step_index: i,
                message: "step must have one of: instruction, include_path, include_glob"
                    .to_string(),
            });
        }
        if set_count > 1 {
            return Err(TestFileError::InvalidStep {
                path: path_str,
                step_index: i,
                message: "step must have only one of: instruction, include_path, include_glob"
                    .to_string(),
            });
        }
    }

    Ok(test_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parse_instruction_steps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.test.toml");
        fs::write(
            &path,
            r#"
name = "Login flow test"

[[steps]]
instruction = "Navigate to /login and verify the page loads"

[[steps]]
instruction = "Enter valid credentials and submit"
"#,
        )
        .unwrap();

        let test_file = parse_test_file(&path).unwrap();
        assert_eq!(test_file.name, "Login flow test");
        assert!(test_file.overrides.is_none());
        assert_eq!(test_file.steps.len(), 2);
        assert_eq!(
            test_file.steps[0].instruction.as_deref(),
            Some("Navigate to /login and verify the page loads")
        );
        assert_eq!(
            test_file.steps[1].instruction.as_deref(),
            Some("Enter valid credentials and submit")
        );
    }

    #[test]
    fn parse_include_steps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("suite.test.toml");
        fs::write(
            &path,
            r#"
name = "Full suite"

[[steps]]
include_path = "setup.test.toml"

[[steps]]
include_glob = "tests/*.test.toml"

[[steps]]
instruction = "Verify final state"
"#,
        )
        .unwrap();

        let test_file = parse_test_file(&path).unwrap();
        assert_eq!(test_file.steps.len(), 3);
        assert_eq!(
            test_file.steps[0].include_path.as_deref(),
            Some("setup.test.toml")
        );
        assert_eq!(
            test_file.steps[1].include_glob.as_deref(),
            Some("tests/*.test.toml")
        );
        assert!(test_file.steps[2].instruction.is_some());
    }

    #[test]
    fn parse_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.test.toml");
        fs::write(
            &path,
            r#"
name = "Custom provider test"

[overrides.provider]
name = "openai"
extra_system_prompt = "Be brief"

[[steps]]
instruction = "Do something"
"#,
        )
        .unwrap();

        let test_file = parse_test_file(&path).unwrap();
        let overrides = test_file.overrides.unwrap();
        let provider = overrides.provider.unwrap();
        assert_eq!(provider.name.as_deref(), Some("openai"));
        assert_eq!(provider.extra_system_prompt.as_deref(), Some("Be brief"));
        assert!(provider.agent_args.is_none());
    }

    #[test]
    fn parse_error_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.test.toml");
        fs::write(&path, "this is not valid [[[").unwrap();

        let err = parse_test_file(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("failed to parse test file"));
        assert!(msg.contains("bad.test.toml"));
    }

    #[test]
    fn parse_error_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unknown.test.toml");
        fs::write(
            &path,
            r#"
name = "Test"
bogus_field = true

[[steps]]
instruction = "Do something"
"#,
        )
        .unwrap();

        let err = parse_test_file(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("failed to parse test file"));
    }

    #[test]
    fn parse_error_empty_step() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty_step.test.toml");
        fs::write(
            &path,
            r#"
name = "Test"

[[steps]]
"#,
        )
        .unwrap();

        let err = parse_test_file(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid step 0"));
        assert!(msg.contains("must have one of"));
    }

    #[test]
    fn parse_error_ambiguous_step() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ambiguous.test.toml");
        fs::write(
            &path,
            r#"
name = "Test"

[[steps]]
instruction = "Do something"
include_path = "other.test.toml"
"#,
        )
        .unwrap();

        let err = parse_test_file(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid step 0"));
        assert!(msg.contains("must have only one of"));
    }

    #[test]
    fn parse_step_timeout_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("timeout.test.toml");
        fs::write(
            &path,
            r#"
name = "Timeout test"

[[steps]]
instruction = "Quick step"

[[steps]]
instruction = "Slow migration"
step_timeout_secs = 900
"#,
        )
        .unwrap();

        let test_file = parse_test_file(&path).unwrap();
        assert_eq!(test_file.steps.len(), 2);
        assert!(test_file.steps[0].step_timeout_secs.is_none());
        assert_eq!(test_file.steps[1].step_timeout_secs, Some(900));
    }

    #[test]
    fn parse_error_file_not_found() {
        let path = Path::new("/nonexistent/path/test.test.toml");
        let err = parse_test_file(path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("failed to read test file"));
    }
}
