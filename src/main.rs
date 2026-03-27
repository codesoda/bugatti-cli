use clap::Parser;

use bugatti::cli::{Cli, Commands};

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Test { path, skip_cmds } => {
            if !skip_cmds.is_empty() {
                println!("Skipping commands: {}", skip_cmds.join(", "));
            }
            match path {
                Some(p) => println!("Running test file: {p}"),
                None => println!("Discovering and running all root test files..."),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use bugatti::cli::Cli;

    #[test]
    fn test_subcommand_no_path() {
        let cli = Cli::parse_from(["bugatti", "test"]);
        match cli.command {
            bugatti::cli::Commands::Test { path, skip_cmds } => {
                assert!(path.is_none());
                assert!(skip_cmds.is_empty());
            }
        }
    }

    #[test]
    fn test_subcommand_with_path() {
        let cli = Cli::parse_from(["bugatti", "test", "some/path.test.toml"]);
        match cli.command {
            bugatti::cli::Commands::Test { path, skip_cmds } => {
                assert_eq!(path.unwrap(), "some/path.test.toml");
                assert!(skip_cmds.is_empty());
            }
        }
    }

    #[test]
    fn test_subcommand_with_skip_cmd() {
        let cli = Cli::parse_from(["bugatti", "test", "--skip-cmd", "migrate"]);
        match cli.command {
            bugatti::cli::Commands::Test { path, skip_cmds } => {
                assert!(path.is_none());
                assert_eq!(skip_cmds, vec!["migrate".to_string()]);
            }
        }
    }

    #[test]
    fn test_subcommand_with_multiple_skip_cmds() {
        let cli = Cli::parse_from([
            "bugatti",
            "test",
            "my.test.toml",
            "--skip-cmd",
            "migrate",
            "--skip-cmd",
            "server",
        ]);
        match cli.command {
            bugatti::cli::Commands::Test { path, skip_cmds } => {
                assert_eq!(path.unwrap(), "my.test.toml");
                assert_eq!(skip_cmds, vec!["migrate".to_string(), "server".to_string()]);
            }
        }
    }
}
