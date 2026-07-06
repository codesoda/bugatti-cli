# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0] - 2026-07-07

### Added

- Shorthand test file names: `bugatti test ftue` now resolves to `ftue.test.toml` when the exact path doesn't exist. Exact paths still take precedence, and the not-found error reports both candidates tried (#37, #43)
- Actionable error guidance for common failures: config read/parse errors, missing test files, empty test discovery, and provider initialization failures now include a concrete next step or docs link (#13, #44)

### Changed

- Colored output now respects the `NO_COLOR` environment variable and automatically disables ANSI escape codes when output is piped (not a TTY). Color decisions are made per stream (stdout/stderr), and the ANSI palette is centralized in a shared `output` module (#34)

## [0.5.2] - 2026-07-07

### Fixed

- `pi` provider: step completion is now detected as soon as the driving agent finishes its turn. pi 0.80.x keeps the `--print` subprocess alive after emitting the terminal `agent_end` event, and the adapter blocked waiting for that process to exit — so every step hung until `step_timeout_secs` and was recorded as `TIMEOUT`, making the provider unusable. The adapter now treats `agent_end` as authoritative: it gives the process a short grace period to exit, then kills and reaps it, and also cleans up a lingering subprocess if a step is abandoned mid-turn (timeout or Ctrl+C) (#48).

## [0.5.1] - 2026-06-25

### Fixed

- `pi` provider: the driving agent now reliably emits the required `RESULT` marker. Previously the harness bootstrap (carrying the RESULT-marker protocol) was appended onto `pi`'s large default system prompt via `--append-system-prompt`, where the standing protocol rule got buried and silently dropped — every step failed with `PROTOCOL ERROR: output ended without a valid RESULT marker` even though the underlying task succeeded. The adapter now sets a compact base `--system-prompt` so the appended bootstrap stays prominent, and passes the bootstrap as inline content rather than a file path (#47).

## [0.5.0] - 2026-06-25

### Added

- New `codex` provider: set `name = "codex"` under `[provider]` to drive test runs with the OpenAI `codex` CLI. The adapter runs `codex exec --json` for the first turn and `codex exec resume <thread_id> --json` for subsequent steps, preserving conversation continuity across steps.
- New `pi` provider: set `name = "pi"` under `[provider]` to drive test runs with the [`pi`](https://pi.dev) CLI. The adapter runs `pi --print --mode json` one turn per step, preserving conversation continuity via a per-run session id and session directory.

### Fixed

- Provider selection now honors the configured `[provider] name` for `codex` and `pi`. Previously `bugatti test` always initialized the `claude-code` adapter regardless of the configured provider.

### Removed

- Internal `ralph` automation scripts (`scripts/ralph/`), which were development tooling not intended to ship with the CLI.

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

[Unreleased]: https://github.com/codesoda/bugatti-cli/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/codesoda/bugatti-cli/compare/v0.5.2...v0.6.0
[0.5.2]: https://github.com/codesoda/bugatti-cli/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/codesoda/bugatti-cli/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/codesoda/bugatti-cli/compare/v0.4.2...v0.5.0
[0.4.2]: https://github.com/codesoda/bugatti-cli/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/codesoda/bugatti-cli/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/codesoda/bugatti-cli/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/codesoda/bugatti-cli/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/codesoda/bugatti-cli/releases/tag/v0.3.0
