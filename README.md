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

### Pre-built binary (macOS arm64)

```sh
curl -sSf https://raw.githubusercontent.com/codesoda/bugatti-cli/main/install.sh | sh
```

Downloads the latest release binary from GitHub.

### From a clone (requires Rust)

```sh
git clone https://github.com/codesoda/bugatti-cli.git
cd bugatti-cli
sh install.sh
```

Builds from source with `cargo build --release`.

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

# Multiple readiness URLs and custom timeout
[commands.docker-stack]
kind = "long_lived"
cmd = "docker compose up"
readiness_urls = ["http://localhost:3000/health", "http://localhost:5432"]
readiness_timeout_secs = 120
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
| `long_lived` | Spawns in the background. Optional `readiness_url`/`readiness_urls` polled until ready. Torn down after tests complete. |

#### Long-lived command options

| Field | Default | Description |
|-------|---------|-------------|
| `readiness_url` | — | Single URL to poll before the command is considered ready |
| `readiness_urls` | `[]` | Multiple URLs to poll (all must respond) |
| `readiness_timeout_secs` | `30` | How long to wait for readiness before failing |

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
| `skip` | If `true`, step is skipped (counts as passed) |
| `checkpoint` | Checkpoint name — saves state after pass, restores if skipped |

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

### Skipping Steps

Mark steps as `skip = true` to bypass them during execution. Skipped steps count as passed and show as `SKIP` in the output.

```toml
[[steps]]
instruction = "Create account and complete onboarding"
skip = true

[[steps]]
instruction = "Verify the dashboard loads correctly"
```

This is useful for iterative development — run the full suite once, then skip early steps on subsequent runs to focus on later steps.

### Checkpoints

Checkpoints let you save and restore state at step boundaries, enabling fast iteration on long test suites.

**Config** — define save/restore commands in `bugatti.config.toml`:

```toml
[checkpoint]
save = "./scripts/checkpoint.sh save"
restore = "./scripts/checkpoint.sh restore"
timeout_secs = 180
```

**Steps** — add `checkpoint = "name"` to steps:

```toml
[[steps]]
instruction = "Create account and complete onboarding"
checkpoint = "after-onboarding"

[[steps]]
instruction = "Configure billing with test card"
checkpoint = "after-billing"

[[steps]]
instruction = "Invite team member"
```

**How it works:**

1. When a non-skipped step with a checkpoint passes, bugatti runs the **save** command
2. When skipped steps have checkpoints, bugatti runs the **restore** command for the last checkpoint before the first non-skipped step
3. If there are skipped steps after the restored checkpoint that lack their own checkpoints, a warning is printed

**Environment variables** passed to save/restore commands:

| Variable | Description |
|----------|-------------|
| `BUGATTI_CHECKPOINT_ID` | The checkpoint name (e.g. `after-onboarding`) |
| `BUGATTI_CHECKPOINT_PATH` | Directory for this checkpoint (`.bugatti/checkpoints/<id>/`) |

**Example checkpoint script:**

```bash
#!/bin/bash
set -eu
action="${1:?usage: checkpoint.sh save|restore}"

case "$action" in
  save)
    pg_dump ai_barometer_dev > "$BUGATTI_CHECKPOINT_PATH/db.sql"
    ;;
  restore)
    psql -d ai_barometer_dev < "$BUGATTI_CHECKPOINT_PATH/db.sql"
    ;;
esac
```

**Typical workflow:**

```bash
# First run — all steps execute, checkpoints saved
bugatti test ftue.test.toml

# Edit test file: mark steps 1-3 as skip = true
# Second run — restores checkpoint from step 3, runs step 4 onwards
bugatti test ftue.test.toml
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
| `4` | Step timed out |
| `5` | Run interrupted (Ctrl+C) |
| `6` | Setup command failed |

## License

MIT
