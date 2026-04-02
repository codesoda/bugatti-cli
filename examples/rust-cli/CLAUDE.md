This is a simple Rust CLI called `greeter`.

After `cargo build`, the binary is at `./target/debug/greeter`.

Usage:
- `./target/debug/greeter` — prints usage to stderr, exits 1
- `./target/debug/greeter <name>` — prints "Hello, <name>!"
- `./target/debug/greeter <name> --shout` — prints "HELLO, <NAME>!"
