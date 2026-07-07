# Repository Instructions

## Quality Checks

- Formatting checks are not sufficient validation. Rust changes must pass a strict compile check.
- Treat Rust warnings as errors for Cargo validation commands. Use `RUSTFLAGS="-D warnings"` with `cargo check`, `cargo build`, and `cargo test`.
- Run Clippy with warnings denied: `cargo clippy -- -D warnings`.
