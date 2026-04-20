# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.2] - 2026-04-20

### Added

- `--config <PATH>` flag on `bugatti test` to point at an explicit `bugatti.config.toml`; a missing file is now a hard error instead of a silent fallback (#45)

### Fixed

- Print a stderr `WARNING:` when no `bugatti.config.toml` is found in the current directory instead of only logging an `INFO` line to `diagnostics/harness_trace.jsonl`, so the silent fallback to defaults is visible in the terminal and run report (#45)

## [0.4.1] - 2026-04-13

### Fixed

- Config commands now execute in declaration order instead of alphabetical key order (#39)
- Interrupted runs (Ctrl+C) now correctly report as "INTERRUPTED" instead of falsely reporting "PASSED"
- Exit code is now `5` (interrupted) instead of `0` (passed) when a run is interrupted

### Changed

- Removed hardcoded `--no-session-persistence` from the Claude Code adapter; this flag can still be passed via `agent_args` in config

## [0.4.0] - 2026-04-08

### Added

- Self-update command with passive version checking
- Setup steps (`setup = true`) that always run even when checkpoint-skipped and are not counted in pass/fail
- Test file `name` field now defaults to the file stem when omitted (e.g. `login-flow.test.toml` defaults to "login-flow")
- Changelog based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)

### Changed

- Release workflow now extracts notes from CHANGELOG.md instead of auto-generating from commits

## [0.3.1] - 2026-04-04

### Added

- Agent setup hint in CLI help output
- Coding agent quick-start prompt in README and getting-started docs
- Links to bugatti.dev in README and CLI help

## [0.3.0] - 2026-04-04

### Added

- Documentation site, llms.txt, and docs CI workflow
- Checkpoint save/restore and step skip with checkpoint support
- `--from-checkpoint` CLI flag and timestamp-based run IDs
- Checkpoint timeout configuration
- Comprehensive docs for includes, shared test files, skip, and checkpoints
- Node.js Express and Python Flask example projects
- Release workflow and installer script
- Readiness URL checks for long-lived services
- CLI skip flags for harness commands (`--skip-setup`, `--skip-teardown`)
- Long-lived subprocess management with readiness checks
- Result marker parser, report generation, and run artifacts
- Claude Code provider adapter
- Test discovery, step expansion with cycle detection, and end-to-end pipeline
- Config types and `bugatti.config.toml` parsing
- Test file types and `*.test.toml` parsing
- CLI scaffold with `bugatti test` subcommand

### Fixed

- Clippy warnings for release build
- Docs deploy workflow triggers and Node version
- Result marker parser handling of embedded markers

[Unreleased]: https://github.com/codesoda/bugatti-cli/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/codesoda/bugatti-cli/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/codesoda/bugatti-cli/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/codesoda/bugatti-cli/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/codesoda/bugatti-cli/releases/tag/v0.3.0
