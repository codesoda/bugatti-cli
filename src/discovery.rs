use crate::test_file::{parse_test_file, TestFileError};
use std::path::{Path, PathBuf};

/// A discovered root test file ready for execution.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredTest {
    /// Path to the *.test.toml file.
    pub path: PathBuf,
    /// The parsed test name.
    pub name: String,
}

/// Error type for test discovery.
#[derive(Debug)]
pub enum DiscoveryError {
    /// Failed to read the project root directory.
    ReadDir {
        path: String,
        source: std::io::Error,
    },
    /// A test file was found but failed to parse.
    ParseError {
        path: PathBuf,
        source: TestFileError,
    },
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryError::ReadDir { path, source } => {
                write!(f, "failed to read directory '{path}': {source}")
            }
            DiscoveryError::ParseError { path, source } => {
                write!(
                    f,
                    "failed to parse test file '{}': {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// Result of discovering test files — includes both successfully parsed roots and per-file errors.
#[derive(Debug)]
pub struct DiscoveryResult {
    /// Root test files that parsed successfully (excludes `_`-prefixed files).
    pub tests: Vec<DiscoveredTest>,
    /// Per-file parse/cycle errors encountered during discovery.
    pub errors: Vec<DiscoveryError>,
}

/// Discover all root *.test.toml files under the given directory (recursively).
///
/// Files with a `_` prefix (e.g. `_setup.test.toml`) are skipped — these are
/// meant to be included by other test files, not run directly.
/// Discovery order is deterministic (sorted by path).
/// Parse errors are collected per-file rather than aborting discovery.
pub fn discover_root_tests(root: &Path) -> Result<DiscoveryResult, DiscoveryError> {
    let mut test_files = Vec::new();
    collect_test_files(root, &mut test_files).map_err(|e| DiscoveryError::ReadDir {
        path: root.display().to_string(),
        source: e,
    })?;

    // Sort for deterministic order
    test_files.sort();

    let mut tests = Vec::new();
    let mut errors = Vec::new();

    for path in test_files {
        match parse_test_file(&path) {
            Ok(test_file) => {
                tests.push(DiscoveredTest {
                    path,
                    name: test_file.name,
                });
            }
            Err(e) => {
                errors.push(DiscoveryError::ParseError { path, source: e });
            }
        }
    }

    Ok(DiscoveryResult { tests, errors })
}

/// Recursively collect all *.test.toml files under a directory.
fn collect_test_files(dir: &Path, results: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    let entries = std::fs::read_dir(dir)?;
    let mut subdirs = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Skip hidden directories and .bugatti
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if !name.starts_with('.') {
                    subdirs.push(path);
                }
            }
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".test.toml") && !name.starts_with('_') {
                results.push(path);
            }
        }
    }

    // Sort subdirectories for deterministic traversal
    subdirs.sort();
    for subdir in subdirs {
        collect_test_files(&subdir, results)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discover_root_tests_basic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("login.test.toml"),
            r#"
name = "Login test"

[[steps]]
instruction = "Navigate to login"
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("signup.test.toml"),
            r#"
name = "Signup test"

[[steps]]
instruction = "Navigate to signup"
"#,
        )
        .unwrap();

        let result = discover_root_tests(dir.path()).unwrap();
        assert_eq!(result.tests.len(), 2);
        assert!(result.errors.is_empty());
        // Sorted by path
        assert_eq!(result.tests[0].name, "Login test");
        assert_eq!(result.tests[1].name, "Signup test");
    }

    #[test]
    fn discover_excludes_underscore_prefixed_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("root.test.toml"),
            r#"
name = "Root test"

[[steps]]
instruction = "Do something"
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("_setup.test.toml"),
            r#"
name = "Shared steps"

[[steps]]
instruction = "Setup step"
"#,
        )
        .unwrap();

        let result = discover_root_tests(dir.path()).unwrap();
        assert_eq!(result.tests.len(), 1);
        assert_eq!(result.tests[0].name, "Root test");
    }

    #[test]
    fn discover_collects_parse_errors() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("good.test.toml"),
            r#"
name = "Good test"

[[steps]]
instruction = "Works fine"
"#,
        )
        .unwrap();
        fs::write(dir.path().join("bad.test.toml"), "invalid [[[toml").unwrap();

        let result = discover_root_tests(dir.path()).unwrap();
        assert_eq!(result.tests.len(), 1);
        assert_eq!(result.tests[0].name, "Good test");
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].to_string().contains("bad.test.toml"));
    }

    #[test]
    fn discover_recursive_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("tests");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            dir.path().join("root.test.toml"),
            r#"
name = "Root"

[[steps]]
instruction = "Step"
"#,
        )
        .unwrap();
        fs::write(
            sub.join("nested.test.toml"),
            r#"
name = "Nested"

[[steps]]
instruction = "Step"
"#,
        )
        .unwrap();

        let result = discover_root_tests(dir.path()).unwrap();
        assert_eq!(result.tests.len(), 2);
    }

    #[test]
    fn discover_skips_hidden_directories() {
        let dir = tempfile::tempdir().unwrap();
        let hidden = dir.path().join(".bugatti");
        fs::create_dir_all(&hidden).unwrap();
        fs::write(
            hidden.join("internal.test.toml"),
            r#"
name = "Hidden"

[[steps]]
instruction = "Step"
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("visible.test.toml"),
            r#"
name = "Visible"

[[steps]]
instruction = "Step"
"#,
        )
        .unwrap();

        let result = discover_root_tests(dir.path()).unwrap();
        assert_eq!(result.tests.len(), 1);
        assert_eq!(result.tests[0].name, "Visible");
    }

    #[test]
    fn discover_deterministic_order() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["c", "a", "b"] {
            fs::write(
                dir.path().join(format!("{name}.test.toml")),
                format!(
                    r#"
name = "{name}"

[[steps]]
instruction = "Step"
"#
                ),
            )
            .unwrap();
        }

        let result = discover_root_tests(dir.path()).unwrap();
        let names: Vec<&str> = result.tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn discover_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let result = discover_root_tests(dir.path()).unwrap();
        assert!(result.tests.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn discover_nonexistent_directory() {
        let err = discover_root_tests(Path::new("/nonexistent/path")).unwrap_err();
        assert!(err.to_string().contains("failed to read directory"));
    }
}
