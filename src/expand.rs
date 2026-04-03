use crate::test_file::{parse_test_file, TestFile, TestFileError};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A single expanded step ready for execution.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedStep {
    /// Sequential step ID (0-based).
    pub step_id: usize,
    /// The instruction text for this step.
    pub instruction: String,
    /// Source provenance: which file this step came from.
    pub source_file: PathBuf,
    /// Index of the step within its source file.
    pub source_step_index: usize,
    /// Chain of parent includes that led to this step (outermost first).
    pub parent_chain: Vec<PathBuf>,
    /// Optional per-step timeout override in seconds.
    pub step_timeout_secs: Option<u64>,
    /// If true, this step is skipped during execution.
    pub skip: bool,
    /// Optional checkpoint name for save/restore.
    pub checkpoint: Option<String>,
}

/// Error type for step expansion.
#[derive(Debug)]
pub enum ExpandError {
    /// A cycle was detected in include references.
    Cycle { chain: Vec<PathBuf> },
    /// Failed to parse an included test file.
    TestFileError(TestFileError),
    /// A glob pattern matched no files or failed.
    GlobError { pattern: String, message: String },
}

impl std::fmt::Display for ExpandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpandError::Cycle { chain } => {
                let chain_str: Vec<String> =
                    chain.iter().map(|p| p.display().to_string()).collect();
                write!(f, "include cycle detected: {}", chain_str.join(" -> "))
            }
            ExpandError::TestFileError(e) => write!(f, "{e}"),
            ExpandError::GlobError { pattern, message } => {
                write!(f, "glob pattern '{pattern}' failed: {message}")
            }
        }
    }
}

impl std::error::Error for ExpandError {}

impl From<TestFileError> for ExpandError {
    fn from(e: TestFileError) -> Self {
        ExpandError::TestFileError(e)
    }
}

/// Expand a parsed test file's steps into a flat list of executable steps.
///
/// Include steps are recursively resolved. Cycle detection prevents infinite loops.
/// The `root_path` is the canonical path of the file being expanded.
pub fn expand_steps(
    root_path: &Path,
    test_file: &TestFile,
) -> Result<Vec<ExpandedStep>, ExpandError> {
    let canonical_root = root_path.canonicalize().map_err(|e| {
        ExpandError::TestFileError(TestFileError::ReadError {
            path: root_path.display().to_string(),
            source: e,
        })
    })?;

    let mut visited = HashSet::new();
    visited.insert(canonical_root.clone());

    let mut steps = Vec::new();
    let mut step_id: usize = 0;
    let parent_chain = Vec::new();

    expand_steps_inner(
        &canonical_root,
        test_file,
        &parent_chain,
        &mut visited,
        &mut steps,
        &mut step_id,
    )?;

    Ok(steps)
}

fn expand_steps_inner(
    file_path: &Path,
    test_file: &TestFile,
    parent_chain: &[PathBuf],
    visited: &mut HashSet<PathBuf>,
    steps: &mut Vec<ExpandedStep>,
    step_id: &mut usize,
) -> Result<(), ExpandError> {
    let base_dir = file_path.parent().unwrap_or(Path::new("."));

    for (i, step) in test_file.steps.iter().enumerate() {
        if let Some(ref instruction) = step.instruction {
            steps.push(ExpandedStep {
                step_id: *step_id,
                instruction: instruction.clone(),
                source_file: file_path.to_path_buf(),
                source_step_index: i,
                parent_chain: parent_chain.to_vec(),
                step_timeout_secs: step.step_timeout_secs,
                skip: step.skip,
                checkpoint: step.checkpoint.clone(),
            });
            *step_id += 1;
        } else if let Some(ref include_path) = step.include_path {
            let resolved = base_dir.join(include_path);
            expand_included_file(&resolved, file_path, parent_chain, visited, steps, step_id)?;
        } else if let Some(ref include_glob) = step.include_glob {
            let pattern = base_dir.join(include_glob);
            let pattern_str = pattern.display().to_string();
            let mut matches: Vec<PathBuf> = glob::glob(&pattern_str)
                .map_err(|e| ExpandError::GlobError {
                    pattern: include_glob.clone(),
                    message: e.to_string(),
                })?
                .filter_map(|entry| entry.ok())
                .collect();
            // Deterministic sorted order
            matches.sort();

            for matched_path in matches {
                expand_included_file(
                    &matched_path,
                    file_path,
                    parent_chain,
                    visited,
                    steps,
                    step_id,
                )?;
            }
        }
    }

    Ok(())
}

fn expand_included_file(
    resolved_path: &Path,
    includer_path: &Path,
    parent_chain: &[PathBuf],
    visited: &mut HashSet<PathBuf>,
    steps: &mut Vec<ExpandedStep>,
    step_id: &mut usize,
) -> Result<(), ExpandError> {
    let canonical = resolved_path.canonicalize().map_err(|e| {
        ExpandError::TestFileError(TestFileError::ReadError {
            path: resolved_path.display().to_string(),
            source: e,
        })
    })?;

    if !visited.insert(canonical.clone()) {
        // Cycle detected — build the chain for the error message
        let mut chain = parent_chain.to_vec();
        chain.push(includer_path.to_path_buf());
        chain.push(canonical);
        return Err(ExpandError::Cycle { chain });
    }

    let included_file = parse_test_file(&canonical)?;

    let mut child_chain = parent_chain.to_vec();
    child_chain.push(includer_path.to_path_buf());

    expand_steps_inner(
        &canonical,
        &included_file,
        &child_chain,
        visited,
        steps,
        step_id,
    )?;

    visited.remove(&canonical);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn expand_instruction_steps_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("root.test.toml");
        fs::write(
            &path,
            r#"
name = "Simple test"

[[steps]]
instruction = "Step one"

[[steps]]
instruction = "Step two"
"#,
        )
        .unwrap();

        let test_file = parse_test_file(&path).unwrap();
        let expanded = expand_steps(&path, &test_file).unwrap();

        assert_eq!(expanded.len(), 2);
        assert_eq!(expanded[0].step_id, 0);
        assert_eq!(expanded[0].instruction, "Step one");
        assert_eq!(expanded[0].source_step_index, 0);
        assert!(expanded[0].parent_chain.is_empty());
        assert_eq!(expanded[1].step_id, 1);
        assert_eq!(expanded[1].instruction, "Step two");
    }

    #[test]
    fn expand_single_include() {
        let dir = tempfile::tempdir().unwrap();

        let included_path = dir.path().join("setup.test.toml");
        fs::write(
            &included_path,
            r#"
name = "Setup"


[[steps]]
instruction = "Run migrations"
"#,
        )
        .unwrap();

        let root_path = dir.path().join("root.test.toml");
        fs::write(
            &root_path,
            r#"
name = "Root test"

[[steps]]
include_path = "setup.test.toml"

[[steps]]
instruction = "Verify state"
"#,
        )
        .unwrap();

        let test_file = parse_test_file(&root_path).unwrap();
        let expanded = expand_steps(&root_path, &test_file).unwrap();

        assert_eq!(expanded.len(), 2);
        assert_eq!(expanded[0].step_id, 0);
        assert_eq!(expanded[0].instruction, "Run migrations");
        assert_eq!(
            expanded[0].source_file,
            included_path.canonicalize().unwrap()
        );
        assert_eq!(expanded[0].parent_chain.len(), 1);
        assert_eq!(expanded[1].step_id, 1);
        assert_eq!(expanded[1].instruction, "Verify state");
        assert!(expanded[1].parent_chain.is_empty());
    }

    #[test]
    fn expand_glob_include() {
        let dir = tempfile::tempdir().unwrap();
        let tests_dir = dir.path().join("tests");
        fs::create_dir(&tests_dir).unwrap();

        fs::write(
            tests_dir.join("a.test.toml"),
            r#"
name = "Test A"


[[steps]]
instruction = "Step A"
"#,
        )
        .unwrap();

        fs::write(
            tests_dir.join("b.test.toml"),
            r#"
name = "Test B"


[[steps]]
instruction = "Step B"
"#,
        )
        .unwrap();

        let root_path = dir.path().join("root.test.toml");
        fs::write(
            &root_path,
            r#"
name = "Glob test"

[[steps]]
include_glob = "tests/*.test.toml"
"#,
        )
        .unwrap();

        let test_file = parse_test_file(&root_path).unwrap();
        let expanded = expand_steps(&root_path, &test_file).unwrap();

        assert_eq!(expanded.len(), 2);
        // Sorted order: a before b
        assert_eq!(expanded[0].instruction, "Step A");
        assert_eq!(expanded[1].instruction, "Step B");
        // Sequential step IDs
        assert_eq!(expanded[0].step_id, 0);
        assert_eq!(expanded[1].step_id, 1);
    }

    #[test]
    fn expand_nested_includes() {
        let dir = tempfile::tempdir().unwrap();

        let leaf_path = dir.path().join("leaf.test.toml");
        fs::write(
            &leaf_path,
            r#"
name = "Leaf"


[[steps]]
instruction = "Leaf step"
"#,
        )
        .unwrap();

        let mid_path = dir.path().join("mid.test.toml");
        fs::write(
            &mid_path,
            r#"
name = "Mid"


[[steps]]
instruction = "Mid before"

[[steps]]
include_path = "leaf.test.toml"

[[steps]]
instruction = "Mid after"
"#,
        )
        .unwrap();

        let root_path = dir.path().join("root.test.toml");
        fs::write(
            &root_path,
            r#"
name = "Root"

[[steps]]
include_path = "mid.test.toml"

[[steps]]
instruction = "Root final"
"#,
        )
        .unwrap();

        let test_file = parse_test_file(&root_path).unwrap();
        let expanded = expand_steps(&root_path, &test_file).unwrap();

        assert_eq!(expanded.len(), 4);
        assert_eq!(expanded[0].instruction, "Mid before");
        assert_eq!(expanded[1].instruction, "Leaf step");
        assert_eq!(expanded[2].instruction, "Mid after");
        assert_eq!(expanded[3].instruction, "Root final");

        // Leaf step has 2 parents in chain (root -> mid)
        assert_eq!(expanded[1].parent_chain.len(), 2);

        // Sequential IDs
        for (i, step) in expanded.iter().enumerate() {
            assert_eq!(step.step_id, i);
        }
    }

    #[test]
    fn detect_direct_cycle() {
        let dir = tempfile::tempdir().unwrap();

        let path = dir.path().join("self_ref.test.toml");
        fs::write(
            &path,
            r#"
name = "Self referencing"

[[steps]]
include_path = "self_ref.test.toml"
"#,
        )
        .unwrap();

        let test_file = parse_test_file(&path).unwrap();
        let err = expand_steps(&path, &test_file).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("include cycle detected"), "got: {msg}");
        assert!(msg.contains("self_ref.test.toml"), "got: {msg}");
    }

    #[test]
    fn detect_indirect_cycle() {
        let dir = tempfile::tempdir().unwrap();

        let a_path = dir.path().join("a.test.toml");
        let b_path = dir.path().join("b.test.toml");

        fs::write(
            &a_path,
            r#"
name = "A"

[[steps]]
include_path = "b.test.toml"
"#,
        )
        .unwrap();

        fs::write(
            &b_path,
            r#"
name = "B"


[[steps]]
include_path = "a.test.toml"
"#,
        )
        .unwrap();

        let test_file = parse_test_file(&a_path).unwrap();
        let err = expand_steps(&a_path, &test_file).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("include cycle detected"), "got: {msg}");
    }

    #[test]
    fn provenance_tracks_source_file() {
        let dir = tempfile::tempdir().unwrap();

        let included_path = dir.path().join("inc.test.toml");
        fs::write(
            &included_path,
            r#"
name = "Included"


[[steps]]
instruction = "Included step 0"

[[steps]]
instruction = "Included step 1"
"#,
        )
        .unwrap();

        let root_path = dir.path().join("root.test.toml");
        fs::write(
            &root_path,
            r#"
name = "Root"

[[steps]]
instruction = "Root step"

[[steps]]
include_path = "inc.test.toml"
"#,
        )
        .unwrap();

        let test_file = parse_test_file(&root_path).unwrap();
        let expanded = expand_steps(&root_path, &test_file).unwrap();

        assert_eq!(expanded.len(), 3);

        // Root step
        assert_eq!(expanded[0].source_file, root_path.canonicalize().unwrap());
        assert_eq!(expanded[0].source_step_index, 0);

        // Included steps
        let inc_canonical = included_path.canonicalize().unwrap();
        assert_eq!(expanded[1].source_file, inc_canonical);
        assert_eq!(expanded[1].source_step_index, 0);
        assert_eq!(expanded[2].source_file, inc_canonical);
        assert_eq!(expanded[2].source_step_index, 1);
    }
}
