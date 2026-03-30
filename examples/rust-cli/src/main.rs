use std::env;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    match args.len() {
        1 => {
            eprintln!("Usage: greeter <name> [--shout]");
            process::exit(1);
        }
        2 => {
            println!("Hello, {}!", args[1]);
        }
        3 if args[2] == "--shout" => {
            println!("HELLO, {}!", args[1].to_uppercase());
        }
        _ => {
            eprintln!("Error: unexpected arguments");
            process::exit(1);
        }
    }
}
