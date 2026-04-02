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

Add `skip = true` to any step to bypass it during execution. Skipped steps count as passed, take zero time, and do not send anything to the agent.

```toml
[[steps]]
instruction = "Create account and complete onboarding"
skip = true

[[steps]]
instruction = "Configure billing with test card"
skip = true

[[steps]]
instruction = "Invite team member and verify email"
```

**Console output** when steps are skipped:

```
SKIP 1/3 ... Create account and complete onboarding (from ftue.test.toml)
SKIP 2/3 ... Configure billing with test card (from ftue.test.toml)
STEP 3/3 ... Invite team member and verify email (from ftue.test.toml)
```

**When to use skip:**

- **Iterative development** — you've run the full suite and steps 1-4 pass. Now you're working on step 5. Mark 1-4 as `skip = true` so each run goes straight to step 5 without re-executing the passing steps.
- **Focusing on a failing step** — a step deep in the suite fails. Skip everything before it to iterate faster on the fix.
- **Pairing with checkpoints** — skip steps that set up state, and use checkpoints to restore that state instead (see below).

**Disabling a skip** — remove the line or comment it out with `#`:

```toml
[[steps]]
instruction = "Create account and complete onboarding"
#skip = true
```

**Important:** skipping a step does not undo its effects. If steps 1-3 set up database state and you skip them, that state won't exist unless you either run them first or restore it via a checkpoint.

### Checkpoints

Checkpoints save and restore external state (databases, files, services) at step boundaries. Combined with `skip = true`, they let you jump to any point in a test suite without re-executing earlier steps.

#### The problem checkpoints solve

A 10-step FTUE (first-time user experience) test takes 15 minutes. Step 8 fails. You fix the code and re-run, but steps 1-7 execute again — another 12 minutes wasted. With checkpoints, you run once, save state after step 7, then on subsequent runs skip steps 1-7 and restore the checkpoint. Step 8 runs immediately against the saved state.

#### Setup

**1. Add a `[checkpoint]` section to `bugatti.config.toml`:**

```toml
[checkpoint]
save = "./scripts/checkpoint.sh save"
restore = "./scripts/checkpoint.sh restore"
timeout_secs = 180   # optional, default 120s
```

The `save` and `restore` fields are shell commands. They receive environment variables telling them which checkpoint to operate on.

**2. Add `checkpoint = "name"` to steps in your test file:**

```toml
name = "FTUE: Full onboarding flow"

[[steps]]
instruction = "Create account via signup form"
checkpoint = "after-signup"

[[steps]]
instruction = "Complete onboarding wizard"
checkpoint = "after-onboarding"

[[steps]]
instruction = "Configure billing with test card"
checkpoint = "after-billing"

[[steps]]
instruction = "Invite team member"

[[steps]]
instruction = "Verify team member received invite email"
```

Checkpoint names must be unique within a test file. Not every step needs a checkpoint — place them at meaningful state boundaries.

#### How save works

When a **non-skipped** step with `checkpoint` passes, bugatti runs the save command immediately after:

```
STEP 1/5 ... Create account via signup form (from ftue.test.toml)
  OK 1/5 (23.4s)
SAVE ....... checkpoint "after-signup"
OK ......... checkpoint "after-signup" saved
STEP 2/5 ... Complete onboarding wizard (from ftue.test.toml)
  OK 2/5 (45.1s)
SAVE ....... checkpoint "after-onboarding"
OK ......... checkpoint "after-onboarding" saved
```

Checkpoints are **not saved** when a step fails — there's no point saving broken state.

#### How restore works

When you mark steps as `skip = true`, bugatti looks at the skipped steps to find the **last checkpoint** before the first non-skipped step, then runs the restore command:

```toml
[[steps]]
instruction = "Create account via signup form"
checkpoint = "after-signup"
skip = true

[[steps]]
instruction = "Complete onboarding wizard"
checkpoint = "after-onboarding"
skip = true

[[steps]]
instruction = "Configure billing with test card"
checkpoint = "after-billing"
skip = true

[[steps]]
instruction = "Invite team member"

[[steps]]
instruction = "Verify team member received invite email"
```

Console output:

```
SKIP 1/5 ... Create account via signup form (from ftue.test.toml)
SKIP 2/5 ... Complete onboarding wizard (from ftue.test.toml)
SKIP 3/5 ... Configure billing with test card (from ftue.test.toml)
RESTORE .... checkpoint "after-billing"
OK ......... checkpoint "after-billing" restored
STEP 4/5 ... Invite team member (from ftue.test.toml)
```

Only the **last** checkpoint is restored — restoring "after-billing" already includes the state from "after-signup" and "after-onboarding".

#### Gap warning

If you skip steps **after** the last checkpoint, bugatti warns you that the restored state may be incomplete:

```toml
[[steps]]
instruction = "Create account via signup form"
checkpoint = "after-signup"
skip = true

[[steps]]
instruction = "Complete onboarding wizard"
skip = true                                    # no checkpoint!

[[steps]]
instruction = "Configure billing with test card"
skip = true                                    # no checkpoint!

[[steps]]
instruction = "Invite team member"
```

```
WARN ....... restoring checkpoint "after-signup" from step 1, but 2 step(s) after it were also skipped without checkpoints
RESTORE .... checkpoint "after-signup"
OK ......... checkpoint "after-signup" restored
STEP 4/5 ... Invite team member (from ftue.test.toml)
```

This means steps 2-3 were skipped but their effects aren't captured in the restored checkpoint. The test may fail because of missing state. Either add checkpoints to those steps or accept the gap.

#### Environment variables

Save and restore commands receive these environment variables:

| Variable | Example | Description |
|----------|---------|-------------|
| `BUGATTI_CHECKPOINT_ID` | `after-onboarding` | The checkpoint name from the step |
| `BUGATTI_CHECKPOINT_PATH` | `.bugatti/checkpoints/after-onboarding/` | Directory for this checkpoint's data |

The checkpoint directory is created automatically before the command runs. Your script decides what to put in it.

#### Checkpoint config reference

| Field | Default | Description |
|-------|---------|-------------|
| `save` | *required* | Shell command to save a checkpoint |
| `restore` | *required* | Shell command to restore a checkpoint |
| `timeout_secs` | `120` | Timeout for save/restore commands (kills process on expiry) |

#### Example checkpoint script

A checkpoint script that saves and restores a PostgreSQL database and an uploads directory:

```bash
#!/bin/bash
set -eu
action="${1:?usage: checkpoint.sh save|restore}"

case "$action" in
  save)
    pg_dump myapp_dev > "$BUGATTI_CHECKPOINT_PATH/db.sql"
    cp -r ./uploads "$BUGATTI_CHECKPOINT_PATH/uploads"
    echo "Saved DB + uploads for checkpoint $BUGATTI_CHECKPOINT_ID"
    ;;
  restore)
    dropdb --if-exists myapp_dev
    createdb myapp_dev
    psql -d myapp_dev -f "$BUGATTI_CHECKPOINT_PATH/db.sql"
    rm -rf ./uploads
    cp -r "$BUGATTI_CHECKPOINT_PATH/uploads" ./uploads
    echo "Restored DB + uploads for checkpoint $BUGATTI_CHECKPOINT_ID"
    ;;
esac
```

#### Disabling checkpoints

Comment out the checkpoint line on individual steps:

```toml
[[steps]]
instruction = "Create account via signup form"
#checkpoint = "after-signup"
```

Or remove the `[checkpoint]` section from `bugatti.config.toml` — steps with `checkpoint` will be ignored if no save/restore commands are configured.

#### Typical workflow

```bash
# 1. First run — all steps execute, checkpoints saved at each boundary
bugatti test ftue.test.toml

# 2. Step 4 fails. Fix the code.

# 3. Edit ftue.test.toml — add skip = true to steps 1-3

# 4. Re-run — restores checkpoint from step 3, runs step 4+ against saved state
bugatti test ftue.test.toml

# 5. Step 4 passes now. Remove skip = true from steps 1-3 for the final run.
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
