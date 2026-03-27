use clap::Parser;

use bugatti::cli::{Cli, Commands};

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Test { path } => match path {
            Some(p) => println!("Running test file: {p}"),
            None => println!("Discovering and running all root test files..."),
        },
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
            bugatti::cli::Commands::Test { path } => {
                assert!(path.is_none());
            }
        }
    }

    #[test]
    fn test_subcommand_with_path() {
        let cli = Cli::parse_from(["bugatti", "test", "some/path.test.toml"]);
        match cli.command {
            bugatti::cli::Commands::Test { path } => {
                assert_eq!(path.unwrap(), "some/path.test.toml");
            }
        }
    }
}
