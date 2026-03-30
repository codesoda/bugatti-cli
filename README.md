# bugatti-cli

An AI test harness that drives agents through structured test steps.

## Usage

Run a specific test file:

```sh
bugatti test path/to/test.test.toml
```

Run all discovered test files in the current directory (recursive):

```sh
bugatti test
```

Discovery finds all `*.test.toml` files recursively, skipping hidden directories and files prefixed with `_`.

## Test Files

Test files are TOML files with a `.test.toml` extension.

```toml
name = "Login flow test"

[[steps]]
instruction = "Navigate to /login and verify the page loads"

[[steps]]
instruction = "Enter valid credentials and submit"
```

### Shared Test Files

Files prefixed with `_` (e.g. `_setup.test.toml`) are skipped by discovery. Use them for shared steps that other tests include:

```toml
# _setup.test.toml
name = "Shared setup"

[[steps]]
instruction = "Verify the server is healthy"
```

```toml
# smoke.test.toml
name = "Smoke test"

[[steps]]
include_path = "_setup.test.toml"

[[steps]]
instruction = "Test the main feature"
```

### Step Options

| Field | Description |
|-------|-------------|
| `instruction` | The instruction text sent to the agent |
| `include_path` | Path to a test file to inline |
| `include_glob` | Glob pattern to inline multiple test files |
| `step_timeout_secs` | Per-step timeout override (seconds) |

Each step must have exactly one of `instruction`, `include_path`, or `include_glob`.

### Per-Test Overrides

```toml
name = "Custom test"

[overrides.provider]
name = "openai"
extra_system_prompt = "Be concise"
step_timeout_secs = 600
base_url = "http://localhost:5000"
```

## Configuration

Create a `bugatti.config.toml` in your project root:

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
| `extra_system_prompt` | — | Additional instructions for the agent |
| `agent_args` | `[]` | Extra CLI args passed to the provider |
| `step_timeout_secs` | `300` | Default timeout per step (seconds) |
| `strict_warnings` | `false` | Treat WARN results as failures |
| `base_url` | — | Base URL included in bootstrap metadata |

### Commands

Short-lived commands run to completion before tests start. Long-lived commands run in the background with optional readiness polling.

## CLI Flags

| Flag | Description |
|------|-------------|
| `--strict-warnings` | Treat WARN results as failures (overrides config) |
| `--skip-cmd <name>` | Skip a configured command |
| `--skip-readiness <name>` | Skip readiness check for a command |

## Examples

See the [`examples/`](examples/) directory for working examples.
