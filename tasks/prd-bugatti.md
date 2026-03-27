# PRD: Bugatti

## 1. Introduction / Overview

Bugatti is a Rust CLI for plain-English, agent-assisted local application verification.

It is designed to replace a fragile manual loop that many developers follow today when validating end-to-end flows locally: reset state, start services, wait for readiness, drive the app in a real browser, inspect logs and database state, and then reason about the outcome with an agent.

The core product goal is to make that workflow dramatically easier to author and repeat. Test authors should be able to describe flows in plain English inside `*.test.toml` files, while Bugatti owns the deterministic parts of execution: configuration, command lifecycle, readiness checks, run identity, artifact layout, transcript capture, and final reporting.

Bugatti v1 is intentionally narrow:

- one root test file run creates one fresh test session
- one test session creates one fresh agent session
- all steps in that root test run share state
- the harness owns setup, orchestration, and reporting
- the provider layer hosts the agent session behind a trait
- Claude Code is the first provider implementation
- the default deliverable is a human-readable Markdown report under the project root

Bugatti is not trying to replace low-level deterministic browser automation frameworks. It is trying to become the fastest, clearest way to verify complex local product flows with a real browser and real local evidence sources, while leaning on an existing coding-agent subscription rather than a per-test API-key model.

## 2. Goals

- Make end-to-end style local verification significantly easier to author in plain-English TOML.
- Move setup, orchestration, and runtime lifecycle concerns out of test files and into the harness.
- Preserve agent, browser, and session state across all steps in a single root test run.
- Support a provider abstraction via a narrow agent-session trait, with Claude Code first.
- Stream useful progress to stdout while tests run.
- Always write a default human-readable test report to disk.
- Preserve enough artifacts, transcript data, logs, and evidence references for a human or coding agent to investigate failures.
- Support reusable global configuration with per-test overrides.
- Support composing larger test flows from smaller included sub-test files.
- Enable teams to replace a meaningful portion of manual local QA for stateful product flows.

## 3. Definition of Done

**Definition of Done (applies to all stories):**
- All acceptance criteria met.
- Linting, formatting, and type checking pass with no warnings.
- Automated tests are written where appropriate and pass.
- New CLI behavior is covered by focused unit and/or integration tests where appropriate.
- Error states produce actionable messages rather than silent failure.
- The implementation preserves deterministic behavior where this PRD requires determinism.
- The implementation writes run artifacts only inside the expected run directory for a given invocation.
- Documentation and example configuration/test files are updated when behavior changes user-facing contracts.

## 4. User Stories

### US-001: Load project config and apply per-test overrides
**Description:** As a test author, I want Bugatti to load project-level configuration and merge per-test overrides so that I can keep reusable harness and provider settings in one place without repeating them in every test.

**Acceptance Criteria:**
- [ ] Bugatti looks for a project-level `bugatti.config.toml` before execution.
- [ ] Each `*.test.toml` file may override compatible global settings for that run only.
- [ ] The merged config is resolved before setup, provider startup, or step expansion begins.
- [ ] The resolved config object is what gets passed into harness initialization and the agent-session trait implementation.
- [ ] Global config supports reusable command definitions such as `reset_db` and `start_app`, including whether a command is short-lived or long-lived.
- [ ] Global config supports provider/session defaults such as provider name, system prompt additions, harness prompt additions, and agent arguments.
- [ ] Invalid config values fail fast with a clear config error before launching the app or agent.
- [ ] The run report includes a human-readable effective config summary that indicates which values came from global config versus test overrides.

### US-002: Parse test files and expand referenced sub-tests into one execution plan
**Description:** As a test author, I want to compose one test from smaller test files so that I can reuse common flows without duplicating steps.

**Acceptance Criteria:**
- [ ] Bugatti parses a root `*.test.toml` file into a normalized in-memory model before execution begins.
- [ ] A step may be either a direct executable step with plain-English instruction text or an include step that references one or more sub-test files by path or glob.
- [ ] Include expansion happens before provider startup or step execution, producing one flattened ordered execution plan for the run.
- [ ] Glob expansion is deterministic and repeatable.
- [ ] Nested includes are supported.
- [ ] Direct and indirect cyclic includes are detected before execution begins.
- [ ] On cycle detection, Bugatti fails fast and shows the include chain that caused the error.
- [ ] Expanded steps retain source provenance including original file path, local step name, and parent include chain.
- [ ] Each expanded step gets a stable step ID within the run.
- [ ] The root file remains the owner of the test session; included files never create nested sessions.
- [ ] In v1, run-level concerns such as setup, provider selection, and artifact root come from the root file plus merged config; included files do not redefine session-level setup mid-run.

### US-003: Manage harness commands and long-lived subprocess lifecycle
**Description:** As a test author, I want Bugatti to own reusable setup and runtime commands so that test files stay simple and the harness can reliably start, monitor, and stop the local environment.

**Acceptance Criteria:**
- [ ] Bugatti loads reusable command definitions from `bugatti.config.toml`.
- [ ] A command definition can be marked as short-lived or long-lived.
- [ ] Test files can reference these commands in setup/teardown flow without redefining them.
- [ ] Per-test overrides may adjust command configuration for that run.
- [ ] Bugatti captures stdout/stderr for harness-managed commands and stores them in run artifacts.
- [ ] Bugatti can enforce readiness checks after starting long-lived commands.
- [ ] If a required short-lived command fails, the run fails before step execution begins.
- [ ] If a long-lived process exits unexpectedly during the run, Bugatti marks the run as failed unless explicitly configured otherwise.
- [ ] On completion, cancellation, or failure, Bugatti attempts orderly teardown of all tracked long-lived processes.
- [ ] If teardown is incomplete, Bugatti reports that clearly in the final run report.
- [ ] Bugatti supports one or more CLI skip flags for harness-defined commands, for example `--skip-cmd start_app`.
- [ ] A skipped command is treated as intentionally not executed, not as a failure.
- [ ] Skipped command names are validated against known command names in the effective config before execution begins.
- [ ] Skipped commands are recorded in live output and in the final report.
- [ ] Skipped long-lived commands are not launched, tracked, or torn down by Bugatti.
- [ ] If a skipped command normally has readiness checks associated with it, Bugatti can still run those readiness checks unless explicitly disabled.

### US-004: Create run identity and artifact layout under the project root
**Description:** As an operator, I want each Bugatti run to have a stable identity and predictable artifact layout so that I can inspect results, compare runs, and hand failures to another agent or teammate.

**Acceptance Criteria:**
- [ ] Before setup commands, provider startup, or step execution begin, Bugatti creates a new run record with a unique run ID.
- [ ] Each run is written under the test project root at `.bugatti/runs/<run-id>/`.
- [ ] The project root is the directory that owns `bugatti.config.toml`; if no project config is present, the current working directory becomes the project root for that invocation.
- [ ] Each run has one fresh test session ID, and each expanded step has a stable step ID within that run.
- [ ] Step IDs are available to the harness and included in the messages/context sent to the provider for each step.
- [ ] The run artifact directory contains a default human-readable report file named `report.md`.
- [ ] The run artifact directory also contains dedicated locations for transcript content, screenshots, command/process logs, and provider/harness diagnostics, even if some directories are empty on a successful run.
- [ ] Bugatti writes a run metadata file that records, at minimum, the root test file path, resolved project root, run ID, session ID, provider name, start time, and effective config source summary.
- [ ] Artifact paths are deterministic and discoverable from the run ID alone.
- [ ] If artifact directory creation fails, Bugatti exits before launching commands, browser, or provider sessions.

### US-005: Define the agent session trait and implement the Claude Code adapter
**Description:** As a harness developer, I want a provider-agnostic agent session trait with a Claude Code implementation so that Bugatti can run one stateful agent session per test file without coupling test definitions to a specific provider.

**Acceptance Criteria:**
- [ ] Bugatti defines a provider/session trait that represents one long-lived agent session for a single root test-file run.
- [ ] The trait supports initializing a session from resolved config, starting a fresh conversation, sending an initial bootstrap/context message, sending subsequent step messages into the same ongoing session, receiving streamed output and a final completed response, and closing the session cleanly.
- [ ] The session trait receives the resolved config object, not raw TOML text.
- [ ] The session initialization path supports provider-specific options from config including provider name, extra system prompt content, harness prompt additions, and agent CLI arguments.
- [ ] Bugatti ships with a Claude Code adapter in v1.
- [ ] The Claude Code adapter preserves one ongoing conversation for the full expanded root test run; included sub-tests do not create nested provider sessions.
- [ ] Step execution messages include run ID, session ID, and step ID context.
- [ ] The provider adapter can surface streamed assistant output back to Bugatti so the CLI can show live progress and the harness can capture transcript content.
- [ ] Reserved Bugatti log lines in streamed output are recognized and converted into step-scoped run events.
- [ ] The provider layer does not hardcode browser, database, or desktop tooling itself; it consumes effective config and the user’s underlying agent/tool setup, while Bugatti adds only its own harness-specific capabilities.
- [ ] If provider startup fails, Bugatti fails the run before step execution begins and records the failure in run artifacts.
- [ ] If the provider session crashes or becomes unavailable mid-run, Bugatti marks the run as failed and records the failure cause in the report.

### US-006: Execute steps in one stateful session with an explicit final result contract
**Description:** As a test author, I want Bugatti to execute all expanded steps in one stateful agent session and require a parseable final result for each step so that long flows remain coherent and the harness can determine pass/fail reliably.

**Acceptance Criteria:**
- [ ] Bugatti executes all expanded steps sequentially within one fresh agent session for the root test-file run.
- [ ] Browser state, agent conversational context, and any session-scoped environment remain available across steps unless the test explicitly resets them.
- [ ] Before each step, Bugatti sends a step message that includes at minimum the run ID, session ID, step ID, source provenance, and the plain-English instruction text.
- [ ] During step execution, streamed provider output is surfaced live to the console and captured in transcript artifacts.
- [ ] Reserved streamed log lines are recognized and recorded as step-scoped Bugatti log events.
- [ ] A step is not considered complete until the provider emits an explicit final result marker.
- [ ] The v1 final result contract is one of:
  - `RESULT` followed by `OK`
  - `RESULT` followed by `WARN: ...`
  - `RESULT` followed by `ERROR: ...`
- [ ] Free-form reasoning, narration, and observations are allowed before the final result marker.
- [ ] After the final result marker is received, Bugatti records the step outcome and advances to the next step or stops the run based on configured failure behavior.
- [ ] If provider output ends without a valid final result marker, Bugatti marks the step as failed with a protocol error.
- [ ] If the step times out before a valid final result marker is produced, Bugatti marks the step as failed and records the timeout in run artifacts.
- [ ] Step outcomes and any accompanying warning/error text appear in the final report.

### US-007: Stream live run progress and compile a default Markdown report
**Description:** As an operator, I want to see test progress live in the terminal and always get a readable run report so that I can monitor execution in real time and investigate failures afterward.

**Acceptance Criteria:**
- [ ] `bugatti test` prints run progress to stdout as execution happens, including setup phase progress, command status, step start, step completion, and final run status.
- [ ] When a harness command is skipped via CLI, stdout shows it explicitly as skipped.
- [ ] When the provider emits a recognized Bugatti log line during streamed output, the CLI renders it in a human-friendly single-line format such as `LOG ........ <message>`.
- [ ] Each step’s terminal output clearly shows when the step begins and when Bugatti has recorded its final result.
- [ ] Every run writes a default `report.md` file under `.bugatti/runs/<run-id>/report.md`.
- [ ] `report.md` includes, at minimum, the run ID, root test file path, provider name, start/end time or duration, effective command skip list, ordered step results, any warning/error text returned as final step outcomes, relevant step-scoped Bugatti log entries, and artifact paths or references for deeper investigation.
- [ ] The report compiler is isolated behind a reporting module boundary so that future output formats can be added without changing step execution semantics.
- [ ] If report compilation partially fails after execution completes, Bugatti still writes the best available report content and clearly notes the compilation problem in stdout and the report itself.
- [ ] A successful run and a failed run both produce `report.md`.

### US-008: Capture agent logs, harness tracing, and evidence references in run artifacts
**Description:** As an operator, I want Bugatti to capture both harness-level diagnostics and step-level evidence so that I can understand what happened during a run without re-running the test blindly.

**Acceptance Criteria:**
- [ ] Bugatti uses structured `tracing` internally for harness/runtime behavior including config load, command execution, readiness checks, provider startup, step lifecycle, timeouts, teardown, and report compilation.
- [ ] Harness tracing output is persisted as a run artifact separate from the human-facing `report.md`.
- [ ] Bugatti captures the full streamed provider transcript for the run as an artifact, even when the report only includes excerpts or summaries.
- [ ] Reserved agent log lines are recorded as step-scoped Bugatti log events and associated with the active run ID and step ID.
- [ ] Bugatti log events are distinguishable from harness tracing events in storage and in the final report.
- [ ] The run artifact model supports references to evidence generated during execution, including screenshots, harness-managed command stdout/stderr logs, browser console output when available, network failure output when available, and SQL or CLI evidence explicitly used to justify a step outcome when available.
- [ ] Evidence references in the final report point to durable paths under the run directory rather than embedding large raw payloads inline.
- [ ] When a step ends in `WARN` or `ERROR`, the report includes the most relevant step-scoped Bugatti log entries and evidence references for that step.
- [ ] Artifact capture failures do not silently disappear; Bugatti records missing or failed artifact collection in both tracing output and the final report.
- [ ] Bugatti can complete a run even if some optional evidence sources are unavailable, but the report makes that clear.
- [ ] The internal run model distinguishes between harness diagnostics, agent progress logs, raw transcript content, evidence artifacts, and compiled report output.

### US-009: Discover and execute root tests from the CLI
**Description:** As a developer, I want `bugatti test` to run either a specific test file or the project’s discovered root tests so that I can use Bugatti both for targeted debugging and broader local regression checks.

**Acceptance Criteria:**
- [ ] `bugatti test <path>` runs the specified root `*.test.toml` file.
- [ ] `bugatti test` with no path discovers root test files under the project root and executes them.
- [ ] Discovery order is deterministic across repeated runs on the same filesystem contents.
- [ ] Each discovered root test file creates its own fresh run ID, session ID, artifact directory, and provider session.
- [ ] Included sub-test files are expanded into their parent root execution plan and do not create nested sessions.
- [ ] Bugatti supports a way to prevent include-only files from being treated as discovered root tests in no-arg discovery mode.
- [ ] If a discovered file is marked include-only, it may still be referenced by path or glob from another root test file.
- [ ] Multi-test invocation prints an aggregate console summary showing each root test and its final outcome.
- [ ] If one discovered root test fails, Bugatti records that failure and continues by default so the full aggregate picture can be seen.
- [ ] CLI discovery errors, parse errors, and include-cycle errors are reported with the source file path before execution reaches provider startup for that root test.
- [ ] Console output and the report make it clear which files were root tests and which files were expanded includes.

### US-010: Return stable exit codes and cleanly finalize runs
**Description:** As a developer or automation user, I want Bugatti to return stable exit codes and finalize runs predictably so that I can rely on it from the terminal, scripts, and CI.

**Acceptance Criteria:**
- [ ] `bugatti test` returns a stable process exit code for the overall invocation.
- [ ] A single root test run that finishes with only `OK` step outcomes exits successfully.
- [ ] A root test run with one or more `ERROR` step outcomes exits non-zero.
- [ ] Warning-only runs exit zero by default.
- [ ] Bugatti may support `--fail-on-warn` in v1 if easy; if implemented, warning-only runs exit non-zero when that flag is present.
- [ ] Config, parse, include-cycle, provider-startup, readiness, timeout, and teardown-failure conditions map to documented non-zero exit behavior.
- [ ] In multi-test discovery mode, Bugatti computes the overall exit code from aggregate outcomes rather than only the last executed test.
- [ ] Even when a run fails, Bugatti still attempts finalization in this order: record final step/run status, flush transcript/log/report artifacts as best as possible, stop tracked long-lived subprocesses, print final run summary, then exit with the appropriate code.
- [ ] Interrupted runs such as Ctrl+C are marked clearly in run output and still attempt best-effort cleanup and report writing.
- [ ] Final console output includes the run ID and report path for each completed or partially completed run.
- [ ] Exit behavior is documented in the CLI help or docs so users can script around it without reverse-engineering semantics.

## 5. Functional Requirements

1. **FR-1:** The system must load an optional project-level `bugatti.config.toml` before running tests.
2. **FR-2:** The system must allow each test file to override compatible global config fields for that run only.
3. **FR-3:** The system must support reusable command definitions in global config, including lifecycle type (`short_lived` or `long_lived`).
4. **FR-4:** The system must pass the resolved effective config into harness setup and provider session initialization.
5. **FR-5:** The system must fail before execution begins when config parsing, validation, or merge rules fail.
6. **FR-6:** The system must parse a root `*.test.toml` file into a normalized execution model before execution starts.
7. **FR-7:** The system must support step-level inclusion of sub-test files by path or glob.
8. **FR-8:** The system must flatten included sub-tests into a deterministic ordered execution plan for a single session.
9. **FR-9:** The system must detect and reject recursive or cyclic includes, whether introduced by direct file references or glob expansion, before any execution begins, and must emit a clear error showing the include chain.
10. **FR-10:** The system must preserve source provenance for every expanded step in the run report and runtime metadata.
11. **FR-11:** The system must treat the root test file as the authority for session-scoped configuration in v1.
12. **FR-12:** The system must support reusable harness command definitions in project config.
13. **FR-13:** The system must distinguish between short-lived and long-lived commands.
14. **FR-14:** The system must track, log, and tear down long-lived subprocesses it launches.
15. **FR-15:** The system must capture stdout/stderr for harness-managed commands as run artifacts.
16. **FR-16:** The system must support readiness checks separate from command launch.
17. **FR-17:** The system must fail fast when required setup commands fail.
18. **FR-18:** The system must support skipping configured harness commands via CLI flags.
19. **FR-19:** The system must validate skipped command names against the resolved effective config before execution begins.
20. **FR-20:** The system must record skipped commands in live output and in the final run report.
21. **FR-21:** The system must not manage lifecycle or teardown for commands skipped via CLI.
22. **FR-22:** The system should allow readiness checks to remain active even when the associated startup command is skipped.
23. **FR-23:** The system must create a unique run ID before any execution begins.
24. **FR-24:** The system must store each run under `.bugatti/runs/<run-id>/` within the resolved project root.
25. **FR-25:** The system must create one fresh session ID per test-file run and stable step IDs for all expanded steps.
26. **FR-26:** The system must expose run ID and step ID context to the provider layer for step execution and logging.
27. **FR-27:** The system must write a default human-readable `report.md` file for every run.
28. **FR-28:** The system must persist run metadata sufficient to identify the source test, provider, config sources, and timing information.
29. **FR-29:** The system must fail fast if it cannot create the artifact root for a run.
30. **FR-30:** The system must define a provider-agnostic trait for one long-lived agent session per root test-file run.
31. **FR-31:** The system must initialize provider sessions from the resolved effective config object.
32. **FR-32:** The system must support sending one bootstrap message and multiple sequential step messages into the same ongoing provider conversation.
33. **FR-33:** The system must pass run ID, session ID, and step ID context into provider-mediated step execution.
34. **FR-34:** The system must support streamed provider output for live console display and transcript capture.
35. **FR-35:** The system must ship with a Claude Code provider adapter in v1.
36. **FR-36:** The system must allow provider-specific prompt additions and agent arguments through config.
37. **FR-37:** The system must expose Bugatti harness capabilities, including agent-visible logging, through the provider session interface.
38. **FR-38:** The system must fail the run cleanly when provider startup or mid-run session continuity fails.
39. **FR-39:** The system must execute all expanded steps for a root test file in one stateful agent session.
40. **FR-40:** The system must include run, session, and step identity in each step execution message.
41. **FR-41:** The system must support live streamed provider output during step execution.
42. **FR-42:** The system must recognize reserved streamed Bugatti log lines and record them as step-scoped run events.
43. **FR-43:** The system must require an explicit final result marker for every step.
44. **FR-44:** The system must support `OK`, `WARN: ...`, and `ERROR: ...` as valid final step outcomes.
45. **FR-45:** The system must fail a step when provider output ends or times out without a valid final result marker.
46. **FR-46:** The system must stream live run progress to stdout during execution.
47. **FR-47:** The system must render recognized agent log events in a human-friendly console format during the run.
48. **FR-48:** The system must write a default `report.md` file for every run.
49. **FR-49:** The system must include run metadata, ordered step outcomes, and investigation references in the default report.
50. **FR-50:** The system must isolate report compilation behind a modular reporting boundary so additional output formats can be added later.
51. **FR-51:** The system must attempt report generation for both successful and failed runs.
52. **FR-52:** The system must use structured tracing for internal harness/runtime execution and persist that output as a run artifact.
53. **FR-53:** The system must capture the full provider transcript for each run as a durable artifact.
54. **FR-54:** The system must persist agent-originated Bugatti log events separately from harness tracing events.
55. **FR-55:** The system must support durable evidence references for screenshots, command logs, browser/runtime diagnostics, and SQL or CLI evidence used during verification.
56. **FR-56:** The system must include relevant step-scoped logs and evidence references in the final report for warning and error outcomes.
57. **FR-57:** The system must record artifact capture failures explicitly rather than silently omitting them.
58. **FR-58:** The system must tolerate unavailable optional evidence sources while marking them clearly in run outputs.
59. **FR-59:** The system must support running a specific root test file via `bugatti test <path>`.
60. **FR-60:** The system must support discovering root `*.test.toml` files when `bugatti test` is invoked without a path.
61. **FR-61:** The system must execute discovered root tests in deterministic order.
62. **FR-62:** The system must create an independent run/session/artifact context for each discovered root test.
63. **FR-63:** The system must distinguish root tests from include-only test files during discovery.
64. **FR-64:** The system must preserve source provenance in outputs so operators can tell whether a step came from a root test or an included file.
65. **FR-65:** The system must provide an aggregate outcome summary for multi-test invocations.
66. **FR-66:** The system must return stable documented exit codes for successful, failed, and infrastructure-error runs.
67. **FR-67:** The system must compute overall exit status correctly for multi-test invocations.
68. **FR-68:** The system must attempt best-effort artifact flush and teardown before process exit, including on interrupted runs.
69. **FR-69:** The system must print final run-identifying information, including run ID and report path, before exit.
70. **FR-70:** The system must document warning and failure exit semantics for CLI users.

## 6. Non-Goals (Out of Scope)

- Replacing Playwright or other deterministic browser automation frameworks for low-level scripted assertions.
- Building a cloud browser lab or hosted execution platform.
- Cross-browser matrix execution in v1.
- Visual regression testing in v1.
- A full multi-provider ecosystem in v1 beyond the Claude Code implementation behind the provider trait.
- Automatic test generation from recordings, prompts, or UI exploration in v1.
- A generalized autonomous QA platform for arbitrary remote environments.
- Fully standardizing every third-party artifact format in v1.
- Rich report export formats beyond the default Markdown report in v1.
- Fine-grained policy controls for every possible tool the agent might use; Bugatti should respect the user’s agent environment and config rather than reimplementing all of it.

## 7. Design Considerations

### Authoring model
- Favor low-ceremony test files written in plain-English TOML.
- Keep session-scoped configuration near the root test file and shared reusable settings in `bugatti.config.toml`.
- Make composition explicit and readable via include steps rather than hidden dynamic execution.

### Human-first reporting
- The default report should be easy to skim by a human and also structured enough to be consumed by a follow-up coding agent.
- Reports should emphasize outcomes, key logs, and artifact references rather than dumping every raw payload inline.

### Console UX
- The CLI should feel operational, not verbose.
- Default terminal output should show setup progress, skipped commands, step boundaries, step results, and agent-originated progress lines.
- A friendly console rendering such as `LOG ........ <message>` is preferred for agent progress feedback.

### Protocol markers
- Use reserved machine-readable markers in provider output to separate stream-friendly chatter from harness-parseable events.
- Recommended v1 markers:
  - `BUGATTI_LOG <message>` for agent progress entries
  - `RESULT` followed by a final status line for step completion

### Suggested example config shapes

```toml
# bugatti.config.toml
[provider]
name = "claude_code"
extra_system_prompt = "Follow the Bugatti result contract exactly."
agent_args = ["--some-provider-flag"]

[commands.reset_db]
kind = "short_lived"
cmd = "./scripts/reset-db.sh"

[commands.start_app]
kind = "long_lived"
cmd = "pnpm dev"
readiness_url = "http://localhost:3000/health"
```

```toml
# ftue.test.toml
name = "ftue"

[overrides.provider]
extra_system_prompt = "Prefer browser evidence over assumptions."

[[steps]]
include_glob = "onboarding/*.test.toml"

[[steps]]
instruction = "Verify the new user lands on the dashboard and the correct org exists in Postgres."
```

## 8. Technical Considerations

- Implement in Rust with a strong typed internal model for config, normalized execution plans, run metadata, step metadata, and report compilation inputs.
- Keep the provider trait narrow. It should model session hosting, message passing, streaming, and shutdown, not every external tool capability.
- Use structured `tracing` for harness observability.
- Treat report generation as a compiler from a normalized run model rather than scraping console output.
- Use deterministic ordering for file discovery and glob expansion.
- Preserve source provenance for expanded steps and include chains.
- Use a run-scoped artifact directory created before execution starts.
- Keep artifact capture reference-based wherever possible.
- Separate harness errors from test failures in the internal model even if both are non-zero exits.
- Make interrupted runs first-class: record interruption state, flush best-effort artifacts, and perform best-effort cleanup.

## 9. Success Metrics

### Primary metric
- At least one team can replace a meaningful portion of manual local QA for complex stateful flows with Bugatti.

### Supporting metrics
- Teams can express at least three real-world multi-step flows as root Bugatti tests with reusable sub-tests.
- Operators can diagnose a failed run from the saved run directory without needing an immediate blind rerun in most cases.
- New tests can be authored with materially less ceremony than equivalent harness-heavy setups.
- Local developers can selectively skip harness-managed startup commands and still use Bugatti effectively during iterative debugging.
- The default report is good enough that a follow-up coding agent can use it as the starting point for investigation.

## 10. Open Questions

- None required for v1 scope. The main intentional future extension points are additional provider implementations and additional report output formats.
