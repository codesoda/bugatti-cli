use crate::run::{ArtifactDir, RunId, SessionId};

/// Configuration for the bootstrap message sent at session start.
pub struct BootstrapConfig<'a> {
    pub test_name: &'a str,
    pub test_file: &'a str,
    pub extra_system_prompt: Option<&'a str>,
    pub base_url: Option<&'a str>,
    pub artifact_dir: &'a ArtifactDir,
}

/// Build the bootstrap message content sent to the provider at session start.
///
/// Includes the result contract, BUGATTI_LOG format, test metadata, and
/// extra system prompt (if configured).
pub fn build_bootstrap_content(
    config: &BootstrapConfig,
    total_steps: usize,
    run_id: &RunId,
    session_id: &SessionId,
) -> String {
    let mut content = String::new();

    // Extra system prompt first (if provided)
    if let Some(prompt) = config.extra_system_prompt {
        content.push_str(prompt);
        content.push_str("\n\n");
    }

    // Harness instructions
    content.push_str("You are being driven by the Bugatti test harness. ");
    content.push_str("Follow these rules for every step:\n\n");

    // Result contract
    content.push_str("## Result Contract\n\n");
    content.push_str("After completing each step, you MUST emit exactly one result line as the final line of your response:\n");
    content.push_str("- `RESULT OK` — the step passed\n");
    content.push_str("- `RESULT WARN: <message>` — the step passed with a warning\n");
    content.push_str("- `RESULT ERROR: <message>` — the step failed\n\n");
    content.push_str("Free-form text before the result line is allowed and encouraged.\n\n");

    // BUGATTI_LOG format
    content.push_str("## Logging\n\n");
    content.push_str(
        "To emit structured log events visible in the harness, output a line matching:\n",
    );
    content.push_str("`BUGATTI_LOG <message>`\n\n");

    // Test metadata
    content.push_str("## Test Metadata\n\n");
    content.push_str(&format!("- Test: {}\n", config.test_name));
    content.push_str(&format!("- File: {}\n", config.test_file));
    content.push_str(&format!("- Steps: {}\n", total_steps));
    content.push_str(&format!("- Run ID: {}\n", run_id));
    content.push_str(&format!("- Session ID: {}\n", session_id));
    if let Some(base_url) = config.base_url {
        content.push_str(&format!("- Base URL: {}\n", base_url));
        content.push_str("\nAll URLs in step instructions are relative to the Base URL unless a full URL (with host) is provided.\n");
    }

    // Artifact directories
    content.push_str("\n## Artifacts\n\n");
    content.push_str("Save any files produced during the test run to these directories:\n\n");
    content.push_str(&format!(
        "- **Root**: `{}`\n",
        config.artifact_dir.root.display()
    ));
    content.push_str(&format!(
        "- **Screenshots**: `{}`\n",
        config.artifact_dir.screenshots.display()
    ));
    content.push_str(&format!(
        "- **Logs**: `{}`\n",
        config.artifact_dir.logs.display()
    ));
    content.push_str("\nScreenshots, videos, downloaded files, and any other evidence should be saved to the appropriate directory above.\n");

    content
}
