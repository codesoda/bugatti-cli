# bugatti

Write test steps in plain English. An AI agent executes them. You get structured pass/fail results.

Bugatti is a test harness that drives AI coding agents through structured test plans defined in TOML files. Point it at a Flask app, an Express API, a static site, or a CLI tool — the agent figures out how to verify each step and reports back with `OK`, `WARN`, or `ERROR`.

## Why?

Manual QA doesn't scale. Traditional E2E test frameworks are brittle and expensive to maintain. Bugatti sits in between — you describe *what* to test in natural language, and an AI agent handles the *how*.

- **Plain-English test steps** — no selectors, no page objects, no test framework DSL
- **Structured results** — `RESULT OK`, `RESULT WARN`, `RESULT ERROR` per step
- **Composable test files** — include shared setup, glob multiple suites together
- **Built-in infrastructure** — short-lived setup commands, long-lived servers with readiness polling
- **Full audit trail** — transcripts, logs, and reports saved per run

## How It Works

1. **Config** — loads `bugatti.config.toml` (optional)
2. **Parse** — reads the test file and expands includes into a flat step list
3. **Setup** — runs short-lived commands, spawns long-lived commands, polls readiness
4. **Bootstrap** — sends harness instructions + result contract to the agent
5. **Execute** — sends each step instruction, streams the response, parses the `RESULT` verdict
6. **Report** — writes run metadata, transcripts, and a markdown report to `.bugatti/runs/<run_id>/`
7. **Teardown** — stops long-lived processes

## Install

### From source (requires Rust)

```sh
curl -sSf https://raw.githubusercontent.com/codesoda/bugatti-cli/main/install.sh | sh
```

### From a clone

```sh
git clone https://github.com/codesoda/bugatti-cli.git
cd bugatti-cli
sh install.sh
```

Local installs use symlinks so edits to the repo are immediately reflected.

## Quick Start

Create a test file:

```toml
# login.test.toml
name = "Login flow"

[[steps]]
instruction = "Navigate to /login and verify the page loads"

[[steps]]
instruction = "Enter valid credentials and submit the form"

[[steps]]
instruction = "Verify you are redirected to the dashboard"
```

Run it:

```sh
bugatti test login.test.toml
```

Or discover and run all test files in the project:

```sh
bugatti test
```

Discovery finds all `*.test.toml` files recursively, skipping hidden directories and `_`-prefixed files.

## Configuration

Create `bugatti.config.toml` in your project root:

```toml
[provider]
name = "claude-code"
extra_system_prompt = "Use the browser for UI tests."
agent_args = ["--dangerously-skip-permissions"]
step_timeout_secs = 300
strict_warnings = true
base_url = "http://localhost:3000"

[commands.migrate]
kind = "short_lived"
cmd = "npm run db:migrate"

[commands.server]
kind = "long_lived"
cmd = "npm start"
readiness_url = "http://localhost:3000/health"
```

### Provider Settings

| Field | Default | Description |
|-------|---------|-------------|
| `name` | `"claude-code"` | Provider to use |
| `extra_system_prompt` | — | Additional system prompt for the agent |
| `agent_args` | `[]` | Extra CLI args passed to the provider |
| `step_timeout_secs` | `300` | Default timeout per step (seconds) |
| `strict_warnings` | `false` | Treat WARN results as failures |
| `base_url` | — | Base URL for the app under test (relative URLs in steps resolve against this) |

### Commands

| Kind | Behavior |
|------|----------|
| `short_lived` | Runs to completion before tests start. Fails the run on non-zero exit. |
| `long_lived` | Spawns in the background. Optional `readiness_url` is polled until ready. Torn down after tests complete. |

### CLI Flags

| Flag | Description |
|------|-------------|
| `--strict-warnings` | Treat WARN results as failures (overrides config) |
| `--skip-cmd <name>` | Skip a configured command |
| `--skip-readiness <name>` | Skip readiness check for a command |

## Test Files

Test files are TOML with a `.test.toml` extension. Each step must have exactly one of `instruction`, `include_path`, or `include_glob`.

### Steps

| Field | Description |
|-------|-------------|
| `instruction` | Plain-English instruction sent to the agent |
| `include_path` | Path to another test file to inline |
| `include_glob` | Glob pattern to inline multiple test files |
| `step_timeout_secs` | Per-step timeout override (seconds) |

### Shared Test Files

Prefix with `_` to exclude from discovery. Other tests pull them in via `include_path`:

```toml
# _setup.test.toml
name = "Shared setup"

[[steps]]
instruction = "Verify the health endpoint returns 200"
```

```toml
# smoke.test.toml
name = "Smoke test"

[[steps]]
include_path = "_setup.test.toml"

[[steps]]
instruction = "Verify the homepage renders"
```

### Per-Test Overrides

Override provider settings for a specific test:

```toml
name = "Custom provider test"

[overrides.provider]
extra_system_prompt = "Be concise"
step_timeout_secs = 600
base_url = "http://localhost:5000"
```

## Examples

Working examples in [`examples/`](examples/):

| Example | What it tests | Key features |
|---------|--------------|--------------|
| [`static-html`](examples/static-html/) | Local HTML page via browser | No server, browser testing |
| [`python-flask`](examples/python-flask/) | Flask API + UI | Long-lived server, readiness URL, strict warnings |
| [`node-express`](examples/node-express/) | Express TypeScript API + UI | pnpm install, shared setup via `_` prefix, multi-port, test discovery |
| [`rust-cli`](examples/rust-cli/) | Rust CLI tool | Short-lived build command, per-step timeout |

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | All steps passed |
| `1` | One or more steps failed |
| `2` | Configuration or parse error |
| `3` | Provider or readiness failure |
| `4` | Run interrupted (Ctrl+C) |
| `5` | Step timed out |

## License

MIT
