use crate::exit_code::{EXIT_CONFIG_ERROR, EXIT_OK};
use crate::output;
use std::path::Path;

const CONFIG_TEMPLATE: &str = r#"# bugatti project configuration
# See https://bugatti.dev/llms/cli-reference.txt for all options.

[provider]
name = "claude-code"

# Example harness command. Uncomment and adjust for your project.
# [commands.dev]
# kind = "long_lived"
# cmd = "npm run dev"
# readiness_url = "http://localhost:3000"
"#;

const EXAMPLE_TEST_TEMPLATE: &str = r#"name = "example"

[[steps]]
instruction = "Open the application and verify the main page loads successfully."
"#;

/// Scaffold a default config file and an example test file in `dir`.
pub fn run_init(dir: &Path, yes: bool) -> i32 {
    let c = output::stdout_colors();
    println!("{}Scaffolding bugatti files{}", c.bold, c.reset);
    if yes {
        println!("{}Using defaults (--yes).{}", c.dim, c.reset);
    }

    for (name, contents) in [
        ("bugatti.config.toml", CONFIG_TEMPLATE),
        ("example.test.toml", EXAMPLE_TEST_TEMPLATE),
    ] {
        let path = dir.join(name);
        if path.exists() {
            println!("{}!{} {name} already exists — skipped", c.prompt, c.reset);
            continue;
        }

        if let Err(e) = std::fs::write(&path, contents) {
            eprintln!("failed to write {}: {e}", path.display());
            return EXIT_CONFIG_ERROR;
        }
        println!("{}✓{} created {name}", c.result, c.reset);
    }

    print_project_hint(dir);

    println!();
    println!("Next steps:");
    println!("  bugatti test example.test.toml");

    EXIT_OK
}

fn print_project_hint(dir: &Path) {
    let c = output::stdout_colors();
    if dir.join("package.json").exists() {
        println!();
        println!(
            "{}Detected package.json.{} If your app needs a dev server, add:",
            c.dim, c.reset
        );
        println!(
            r#"
[commands.dev]
kind = "long_lived"
cmd = "npm run dev"
readiness_url = "http://localhost:3000"
"#
        );
    } else if dir.join("Cargo.toml").exists() {
        println!();
        println!(
            "{}Detected Cargo.toml.{} If your app needs a dev server, add:",
            c.dim, c.reset
        );
        println!(
            r#"
[commands.dev]
kind = "long_lived"
cmd = "cargo run"
readiness_url = "http://localhost:3000"
"#
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config, test_file};
    use std::fs;

    #[test]
    fn creates_files_with_parseable_templates() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(run_init(dir.path(), true), EXIT_OK);

        let config_path = dir.path().join("bugatti.config.toml");
        let test_path = dir.path().join("example.test.toml");
        assert!(config_path.exists());
        assert!(test_path.exists());

        let config = config::load_config(dir.path()).unwrap();
        assert_eq!(config.provider.name, "claude-code");

        let test = test_file::parse_test_file(&test_path).unwrap();
        assert_eq!(test.name, "example");
        assert_eq!(test.steps.len(), 1);
        assert!(test.steps[0].instruction.is_some());
    }

    #[test]
    fn second_run_skips_existing_files_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(run_init(dir.path(), true), EXIT_OK);

        let config_path = dir.path().join("bugatti.config.toml");
        let test_path = dir.path().join("example.test.toml");
        fs::write(&config_path, "custom config").unwrap();
        fs::write(&test_path, "custom test").unwrap();

        assert_eq!(run_init(dir.path(), true), EXIT_OK);

        assert_eq!(fs::read_to_string(config_path).unwrap(), "custom config");
        assert_eq!(fs::read_to_string(test_path).unwrap(), "custom test");
    }

    #[test]
    fn partial_scaffold_creates_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("bugatti.config.toml");
        fs::write(&config_path, "custom config").unwrap();

        assert_eq!(run_init(dir.path(), true), EXIT_OK);

        assert_eq!(fs::read_to_string(config_path).unwrap(), "custom config");
        assert!(dir.path().join("example.test.toml").exists());
    }
}
