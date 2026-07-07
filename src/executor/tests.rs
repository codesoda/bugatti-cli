use super::*;
use crate::config::Config;
use crate::provider::{AgentSession, BootstrapMessage, OutputChunk, ProviderError, StepMessage};
use std::path::Path;

// --- Result marker parsing tests ---

#[test]
fn parse_result_ok() {
    assert_eq!(parse_result_marker("RESULT OK"), Some(StepVerdict::Ok));
}

#[test]
fn parse_result_warn() {
    assert_eq!(
        parse_result_marker("RESULT WARN: slow response"),
        Some(StepVerdict::Warn("slow response".to_string()))
    );
}

#[test]
fn parse_result_error() {
    assert_eq!(
        parse_result_marker("RESULT ERROR: page not found"),
        Some(StepVerdict::Error("page not found".to_string()))
    );
}

#[test]
fn parse_result_with_freeform_text_before() {
    let text = "I checked the page and everything looks fine.\nThe login form works.\nRESULT OK";
    assert_eq!(parse_result_marker(text), Some(StepVerdict::Ok));
}

#[test]
fn parse_result_with_freeform_text_after_marker_ignored() {
    // Last RESULT line wins
    let text = "RESULT OK\nsome trailing text\nRESULT ERROR: actually failed";
    assert_eq!(
        parse_result_marker(text),
        Some(StepVerdict::Error("actually failed".to_string()))
    );
}

#[test]
fn parse_result_missing_returns_none() {
    assert_eq!(parse_result_marker("No result here"), None);
}

#[test]
fn parse_result_partial_returns_none() {
    assert_eq!(parse_result_marker("RESULT"), None);
    assert_eq!(parse_result_marker("RESULT UNKNOWN"), None);
}

#[test]
fn parse_result_whitespace_trimmed() {
    assert_eq!(parse_result_marker("  RESULT OK  "), Some(StepVerdict::Ok));
}

#[test]
fn parse_result_embedded_in_line() {
    // Agent omits newline before RESULT marker
    let text = "Success message confirmed visible.RESULT OK";
    assert_eq!(parse_result_marker(text), Some(StepVerdict::Ok));
}

#[test]
fn parse_result_embedded_with_duplicate() {
    // Agent emits RESULT OK twice without newlines
    let text = "Log line.RESULT OKRESULT OK";
    assert_eq!(parse_result_marker(text), Some(StepVerdict::Ok));
}

#[test]
fn parse_result_warn_with_extra_whitespace() {
    assert_eq!(
        parse_result_marker("RESULT WARN:   extra spaces  "),
        Some(StepVerdict::Warn("extra spaces".to_string()))
    );
}

// --- Mock provider for execution tests ---

struct MockSession {
    responses: Vec<Vec<Result<OutputChunk, ProviderError>>>,
    call_count: usize,
}

impl MockSession {
    fn new(responses: Vec<Vec<Result<OutputChunk, ProviderError>>>) -> Self {
        Self {
            responses,
            call_count: 0,
        }
    }
}

#[async_trait::async_trait]
impl AgentSession for MockSession {
    fn initialize(
        _config: &Config,
        _artifact_dir: &Path,
        _verbose: bool,
    ) -> Result<Self, ProviderError>
    where
        Self: Sized,
    {
        Ok(Self::new(vec![]))
    }

    async fn start(&mut self) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn send_bootstrap(
        &mut self,
        _message: BootstrapMessage,
    ) -> Result<Box<dyn crate::provider::OutputStream + '_>, ProviderError> {
        Ok(Box::new(crate::provider::VecOutputStream::empty()))
    }

    async fn send_step(
        &mut self,
        _message: StepMessage,
    ) -> Result<Box<dyn crate::provider::OutputStream + '_>, ProviderError> {
        if self.call_count < self.responses.len() {
            let idx = self.call_count;
            self.call_count += 1;
            Ok(Box::new(crate::provider::VecOutputStream::new(
                self.responses[idx].clone(),
            )))
        } else {
            Err(ProviderError::SendFailed("no more responses".to_string()))
        }
    }

    async fn close(&mut self) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn test_steps() -> Vec<ExpandedStep> {
    vec![
        ExpandedStep {
            step_id: 0,
            instruction: "Check the homepage loads".to_string(),
            source_file: PathBuf::from("/test/root.test.toml"),
            source_step_index: 0,
            parent_chain: vec![],
            step_timeout_secs: None,
            skip: false,
            setup: false,
            checkpoint: None,
        },
        ExpandedStep {
            step_id: 1,
            instruction: "Verify login form exists".to_string(),
            source_file: PathBuf::from("/test/root.test.toml"),
            source_step_index: 1,
            parent_chain: vec![],
            step_timeout_secs: None,
            skip: false,
            setup: false,
            checkpoint: None,
        },
    ]
}

fn test_run_ids() -> (RunId, SessionId) {
    (
        RunId("test-run-001".to_string()),
        SessionId("test-session-001".to_string()),
    )
}

fn test_artifact_dir() -> (tempfile::TempDir, ArtifactDir) {
    let tmp = tempfile::tempdir().unwrap();
    let run_id = RunId("test-run-001".to_string());
    let dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
    dir.create_all().unwrap();
    (tmp, dir)
}

#[tokio::test]
async fn execute_steps_all_ok() {
    let steps = test_steps();
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![
        vec![
            Ok(OutputChunk::Text(
                "Page loaded successfully.\nRESULT OK".to_string(),
            )),
            Ok(OutputChunk::Done),
        ],
        vec![
            Ok(OutputChunk::Text(
                "Login form found.\nRESULT OK".to_string(),
            )),
            Ok(OutputChunk::Done),
        ],
    ]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(outcome.all_passed);
    assert_eq!(outcome.steps.len(), 2);
    assert_eq!(
        outcome.steps[0].result,
        StepResult::Verdict(StepVerdict::Ok)
    );
    assert_eq!(
        outcome.steps[1].result,
        StepResult::Verdict(StepVerdict::Ok)
    );
}

#[tokio::test]
async fn execute_steps_stops_on_error() {
    let steps = test_steps();
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![
        vec![
            Ok(OutputChunk::Text(
                "RESULT ERROR: page returned 500".to_string(),
            )),
            Ok(OutputChunk::Done),
        ],
        // Second step should never execute
        vec![
            Ok(OutputChunk::Text("RESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ],
    ]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(!outcome.all_passed);
    assert_eq!(outcome.steps.len(), 1); // Only first step executed
    assert_eq!(
        outcome.steps[0].result,
        StepResult::Verdict(StepVerdict::Error("page returned 500".to_string()))
    );
}

#[tokio::test]
async fn execute_steps_warn_continues() {
    let steps = test_steps();
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![
        vec![
            Ok(OutputChunk::Text("RESULT WARN: slow response".to_string())),
            Ok(OutputChunk::Done),
        ],
        vec![
            Ok(OutputChunk::Text("RESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ],
    ]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(outcome.all_passed);
    assert_eq!(outcome.steps.len(), 2);
    assert_eq!(
        outcome.steps[0].result,
        StepResult::Verdict(StepVerdict::Warn("slow response".to_string()))
    );
}

#[tokio::test]
async fn execute_steps_missing_result_marker() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![vec![
        Ok(OutputChunk::Text(
            "I checked the page and it looks fine.".to_string(),
        )),
        Ok(OutputChunk::Done),
    ]]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(!outcome.all_passed);
    assert!(matches!(
        &outcome.steps[0].result,
        StepResult::ProtocolError(_)
    ));
}

#[tokio::test]
async fn execute_steps_provider_send_failure() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    // Empty responses means send_step returns error
    let mut session = MockSession::new(vec![]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(!outcome.all_passed);
    assert!(matches!(
        &outcome.steps[0].result,
        StepResult::ProviderFailed(_)
    ));
}

#[tokio::test]
async fn execute_steps_provider_stream_error() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![vec![
        Ok(OutputChunk::Text("partial output".to_string())),
        Err(ProviderError::SessionCrashed("process died".to_string())),
    ]]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(!outcome.all_passed);
    assert!(matches!(
        &outcome.steps[0].result,
        StepResult::ProviderFailed(_)
    ));
    // Partial transcript is preserved
    assert!(outcome.steps[0].transcript.contains("partial output"));
}

#[tokio::test]
async fn execute_steps_writes_transcript_artifacts() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![vec![
        Ok(OutputChunk::Text("Check complete.\nRESULT OK".to_string())),
        Ok(OutputChunk::Done),
    ]]);

    let _outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    // Per-step transcript
    let step_transcript = artifact_dir.transcripts.join("step_0000.txt");
    assert!(step_transcript.is_file());
    let contents = std::fs::read_to_string(&step_transcript).unwrap();
    assert!(contents.contains("Check complete."));
    assert!(contents.contains("RESULT OK"));

    // Combined transcript
    let full_transcript = artifact_dir.transcripts.join("full_transcript.txt");
    assert!(full_transcript.is_file());
}

#[tokio::test]
async fn execute_steps_full_transcript_written_incrementally() {
    let steps = test_steps();
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![
        vec![
            Ok(OutputChunk::Text(
                "First step output.\nRESULT OK".to_string(),
            )),
            Ok(OutputChunk::Done),
        ],
        vec![
            Ok(OutputChunk::Text(
                "Second step output.\nRESULT OK".to_string(),
            )),
            Ok(OutputChunk::Done),
        ],
    ]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(outcome.artifact_errors.is_empty());

    // Full transcript should exist and contain both steps
    let full_transcript_path = artifact_dir.transcripts.join("full_transcript.txt");
    assert!(full_transcript_path.is_file());
    let contents = std::fs::read_to_string(&full_transcript_path).unwrap();
    assert!(contents.contains("=== Step 0 ==="));
    assert!(contents.contains("First step output."));
    assert!(contents.contains("=== Step 1 ==="));
    assert!(contents.contains("Second step output."));
}

#[tokio::test]
async fn execute_steps_full_transcript_captures_partial_on_failure() {
    let steps = test_steps();
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![
        vec![
            Ok(OutputChunk::Text(
                "RESULT ERROR: something broke".to_string(),
            )),
            Ok(OutputChunk::Done),
        ],
        // Second step will not execute
        vec![
            Ok(OutputChunk::Text("RESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ],
    ]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    // Full transcript should contain the first (failed) step but not the second
    let full_transcript_path = artifact_dir.transcripts.join("full_transcript.txt");
    let contents = std::fs::read_to_string(&full_transcript_path).unwrap();
    assert!(contents.contains("=== Step 0 ==="));
    assert!(contents.contains("something broke"));
    assert!(!contents.contains("=== Step 1 ==="));
    assert!(outcome.artifact_errors.is_empty());
}

#[tokio::test]
async fn execute_steps_records_duration() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![vec![
        Ok(OutputChunk::Text("RESULT OK".to_string())),
        Ok(OutputChunk::Done),
    ]]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    // Duration should be very small but non-zero
    assert!(outcome.steps[0].duration.as_nanos() > 0);
    assert!(outcome.total_duration.as_nanos() > 0);
}

#[test]
fn step_result_is_pass() {
    assert!(StepResult::Verdict(StepVerdict::Ok).is_pass());
    assert!(StepResult::Verdict(StepVerdict::Warn("x".to_string())).is_pass());
    assert!(!StepResult::Verdict(StepVerdict::Error("x".to_string())).is_pass());
    assert!(!StepResult::ProtocolError("x".to_string()).is_pass());
    assert!(!StepResult::Timeout.is_pass());
    assert!(!StepResult::ProviderFailed("x".to_string()).is_pass());
}

#[tokio::test]
async fn execute_steps_multiple_text_chunks_concatenated() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![vec![
        Ok(OutputChunk::Text("First chunk. ".to_string())),
        Ok(OutputChunk::Text("Second chunk.\n".to_string())),
        Ok(OutputChunk::Text("RESULT OK".to_string())),
        Ok(OutputChunk::Done),
    ]]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(outcome.all_passed);
    assert!(outcome.steps[0].transcript.contains("First chunk."));
    assert!(outcome.steps[0].transcript.contains("Second chunk."));
}

// --- BUGATTI_LOG parsing tests ---

#[test]
fn parse_log_events_single_line() {
    let events = parse_log_events("BUGATTI_LOG Server started on port 3000", "run-1", 0);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].run_id, "run-1");
    assert_eq!(events[0].step_id, 0);
    assert_eq!(events[0].message, "Server started on port 3000");
}

#[test]
fn parse_log_events_multiple_lines() {
    let text =
        "Some output\nBUGATTI_LOG first event\nMore output\nBUGATTI_LOG second event\nRESULT OK";
    let events = parse_log_events(text, "run-2", 3);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].message, "first event");
    assert_eq!(events[0].step_id, 3);
    assert_eq!(events[1].message, "second event");
}

#[test]
fn parse_log_events_none_found() {
    let events = parse_log_events("No log events here\nRESULT OK", "run-1", 0);
    assert!(events.is_empty());
}

#[test]
fn parse_log_events_whitespace_trimmed() {
    let events = parse_log_events("  BUGATTI_LOG trimmed message  ", "run-1", 0);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].message, "trimmed message  ");
}

#[test]
fn parse_log_events_prefix_only_no_message() {
    // "BUGATTI_LOG " with trailing space but no content - still valid, empty message
    let events = parse_log_events("BUGATTI_LOG ", "run-1", 0);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].message, "");
}

#[test]
fn parse_log_events_no_space_after_prefix_not_matched() {
    // "BUGATTI_LOG" without trailing space is not matched (prefix is "BUGATTI_LOG ")
    let events = parse_log_events("BUGATTI_LOG", "run-1", 0);
    assert!(events.is_empty());
}

// --- BUGATTI_LOG in execution tests ---

#[tokio::test]
async fn execute_steps_captures_log_events() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![vec![
            Ok(OutputChunk::Text(
                "Starting check\nBUGATTI_LOG Database connected\nBUGATTI_LOG Schema validated\nRESULT OK".to_string(),
            )),
            Ok(OutputChunk::Done),
        ]]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(outcome.all_passed);
    assert_eq!(outcome.steps[0].log_events.len(), 2);
    assert_eq!(outcome.steps[0].log_events[0].message, "Database connected");
    assert_eq!(outcome.steps[0].log_events[0].run_id, "test-run-001");
    assert_eq!(outcome.steps[0].log_events[0].step_id, 0);
    assert_eq!(outcome.steps[0].log_events[1].message, "Schema validated");
}

#[tokio::test]
async fn execute_steps_no_log_events() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![vec![
        Ok(OutputChunk::Text("No logs here.\nRESULT OK".to_string())),
        Ok(OutputChunk::Done),
    ]]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(outcome.steps[0].log_events.is_empty());
    // No log events file should be created when there are no events
    let log_events_path = artifact_dir.logs.join("bugatti_log_events.txt");
    assert!(!log_events_path.exists());
}

#[tokio::test]
async fn execute_steps_writes_log_events_file() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![vec![
        Ok(OutputChunk::Text(
            "BUGATTI_LOG Migration complete\nRESULT OK".to_string(),
        )),
        Ok(OutputChunk::Done),
    ]]);

    let _outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    // Log events file should exist and be separate from transcript
    let log_events_path = artifact_dir.logs.join("bugatti_log_events.txt");
    assert!(log_events_path.is_file());
    let contents = std::fs::read_to_string(&log_events_path).unwrap();
    assert!(contents.contains("[step 0] Migration complete"));

    // Transcript should still contain the BUGATTI_LOG line (raw transcript is unfiltered)
    let transcript_path = artifact_dir.transcripts.join("step_0000.txt");
    let transcript = std::fs::read_to_string(&transcript_path).unwrap();
    assert!(transcript.contains("BUGATTI_LOG Migration complete"));
}

// --- Evidence ref tests ---

#[tokio::test]
async fn execute_steps_evidence_refs_for_error() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![vec![
        Ok(OutputChunk::Text(
            "RESULT ERROR: page not found".to_string(),
        )),
        Ok(OutputChunk::Done),
    ]]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert_eq!(outcome.steps[0].evidence_refs.len(), 1);
    assert_eq!(
        outcome.steps[0].evidence_refs[0].kind,
        crate::diagnostics::EvidenceKind::CommandLog
    );
    assert!(outcome.steps[0].evidence_refs[0].collection_error.is_none());
}

#[tokio::test]
async fn execute_steps_evidence_refs_for_warn() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![vec![
        Ok(OutputChunk::Text("RESULT WARN: slow response".to_string())),
        Ok(OutputChunk::Done),
    ]]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert_eq!(outcome.steps[0].evidence_refs.len(), 1);
}

#[tokio::test]
async fn execute_steps_evidence_refs_empty_for_ok() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![vec![
        Ok(OutputChunk::Text("RESULT OK".to_string())),
        Ok(OutputChunk::Done),
    ]]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(outcome.steps[0].evidence_refs.is_empty());
}

// --- Bootstrap tests ---

#[test]
fn build_bootstrap_content_includes_result_contract() {
    let (_tmp, artifact_dir) = test_artifact_dir();
    let config = BootstrapConfig {
        test_name: "Login test",
        test_file: "tests/login.test.toml",
        extra_system_prompt: None,
        base_url: None,
        artifact_dir: &artifact_dir,
    };
    let run_id = RunId("run-1".to_string());
    let session_id = SessionId("sess-1".to_string());
    let content = build_bootstrap_content(&config, 3, &run_id, &session_id);

    assert!(content.contains("RESULT OK"));
    assert!(content.contains("RESULT WARN:"));
    assert!(content.contains("RESULT ERROR:"));
    assert!(content.contains("BUGATTI_LOG"));
}

#[test]
fn build_bootstrap_content_includes_test_metadata() {
    let (_tmp, artifact_dir) = test_artifact_dir();
    let config = BootstrapConfig {
        test_name: "Login test",
        test_file: "tests/login.test.toml",
        extra_system_prompt: None,
        base_url: None,
        artifact_dir: &artifact_dir,
    };
    let run_id = RunId("run-abc".to_string());
    let session_id = SessionId("sess-xyz".to_string());
    let content = build_bootstrap_content(&config, 5, &run_id, &session_id);

    assert!(content.contains("Login test"));
    assert!(content.contains("tests/login.test.toml"));
    assert!(content.contains("Steps: 5"));
    assert!(content.contains("run-abc"));
    assert!(content.contains("sess-xyz"));
}

#[test]
fn build_bootstrap_content_includes_extra_system_prompt() {
    let (_tmp, artifact_dir) = test_artifact_dir();
    let config = BootstrapConfig {
        test_name: "Test",
        test_file: "test.test.toml",
        extra_system_prompt: Some("Be concise and thorough"),
        base_url: None,
        artifact_dir: &artifact_dir,
    };
    let run_id = RunId("run-1".to_string());
    let session_id = SessionId("sess-1".to_string());
    let content = build_bootstrap_content(&config, 1, &run_id, &session_id);

    assert!(content.contains("Be concise and thorough"));
    // Extra system prompt should appear before the harness instructions
    let prompt_pos = content.find("Be concise").unwrap();
    let contract_pos = content.find("Result Contract").unwrap();
    assert!(prompt_pos < contract_pos);
}

#[test]
fn build_bootstrap_content_omits_prompt_when_none() {
    let (_tmp, artifact_dir) = test_artifact_dir();
    let config = BootstrapConfig {
        test_name: "Test",
        test_file: "test.test.toml",
        extra_system_prompt: None,
        base_url: None,
        artifact_dir: &artifact_dir,
    };
    let run_id = RunId("run-1".to_string());
    let session_id = SessionId("sess-1".to_string());
    let content = build_bootstrap_content(&config, 1, &run_id, &session_id);

    // Should start with harness instructions, not a blank line
    assert!(content.starts_with("You are being driven"));
}

#[test]
fn build_bootstrap_content_includes_base_url() {
    let (_tmp, artifact_dir) = test_artifact_dir();
    let config = BootstrapConfig {
        test_name: "Test",
        test_file: "test.test.toml",
        extra_system_prompt: None,
        base_url: Some("http://localhost:3000"),
        artifact_dir: &artifact_dir,
    };
    let run_id = RunId("run-1".to_string());
    let session_id = SessionId("sess-1".to_string());
    let content = build_bootstrap_content(&config, 1, &run_id, &session_id);
    assert!(content.contains("- Base URL: http://localhost:3000"));
}

#[test]
fn build_bootstrap_content_omits_base_url_when_none() {
    let (_tmp, artifact_dir) = test_artifact_dir();
    let config = BootstrapConfig {
        test_name: "Test",
        test_file: "test.test.toml",
        extra_system_prompt: None,
        base_url: None,
        artifact_dir: &artifact_dir,
    };
    let run_id = RunId("run-1".to_string());
    let session_id = SessionId("sess-1".to_string());
    let content = build_bootstrap_content(&config, 1, &run_id, &session_id);
    assert!(!content.contains("Base URL"));
}

#[tokio::test]
async fn execute_steps_with_bootstrap_writes_transcript() {
    let steps = vec![test_steps().remove(0)];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![vec![
        Ok(OutputChunk::Text("RESULT OK".to_string())),
        Ok(OutputChunk::Done),
    ]]);

    let bootstrap = BootstrapConfig {
        test_name: "Test",
        test_file: "test.test.toml",
        extra_system_prompt: None,
        base_url: None,
        artifact_dir: &artifact_dir,
    };

    let _outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        Some(&bootstrap),
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    // Bootstrap transcript file should exist
    let bootstrap_path = artifact_dir.transcripts.join("bootstrap.txt");
    assert!(bootstrap_path.is_file());

    // Full transcript should contain bootstrap section
    let full =
        std::fs::read_to_string(artifact_dir.transcripts.join("full_transcript.txt")).unwrap();
    assert!(full.contains("=== Bootstrap ==="));
}

// --- Interrupt tests ---

#[tokio::test]
async fn execute_steps_interrupted_between_steps() {
    let steps = test_steps(); // 2 steps
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![
        vec![
            Ok(OutputChunk::Text("RESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ],
        vec![
            Ok(OutputChunk::Text("RESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ],
    ]);

    // Set interrupted flag — will be checked before step 2
    let flag = AtomicBool::new(false);

    // We can't set the flag between steps in this test easily,
    // so set it before execution starts — step 1 won't even run
    flag.store(true, Ordering::Relaxed);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &flag,
    )
    .await
    .unwrap();

    // No steps should have executed
    assert_eq!(outcome.steps.len(), 0);
    // Interrupted runs must not report as passed
    assert!(!outcome.all_passed);
}

// --- Setup step tests ---

#[tokio::test]
async fn setup_step_tolerates_missing_result_marker() {
    let steps = vec![ExpandedStep {
        step_id: 0,
        instruction: "Start the browser in headed mode".to_string(),
        source_file: PathBuf::from("/test/root.test.toml"),
        source_step_index: 0,
        parent_chain: vec![],
        step_timeout_secs: None,
        skip: false,
        setup: true,
        checkpoint: None,
    }];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    // Agent responds without a RESULT marker — should be OK for setup steps
    let mut session = MockSession::new(vec![vec![
        Ok(OutputChunk::Text(
            "Browser started in headed mode.".to_string(),
        )),
        Ok(OutputChunk::Done),
    ]]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(outcome.all_passed);
    assert_eq!(outcome.steps.len(), 1);
    assert!(outcome.steps[0].setup);
    assert_eq!(
        outcome.steps[0].result,
        StepResult::Verdict(StepVerdict::Ok)
    );
}

#[tokio::test]
async fn setup_step_bypasses_skip() {
    let steps = vec![
        ExpandedStep {
            step_id: 0,
            instruction: "Start the browser".to_string(),
            source_file: PathBuf::from("/test/root.test.toml"),
            source_step_index: 0,
            parent_chain: vec![],
            step_timeout_secs: None,
            skip: true,
            setup: true,
            checkpoint: None,
        },
        ExpandedStep {
            step_id: 1,
            instruction: "Seed the database".to_string(),
            source_file: PathBuf::from("/test/root.test.toml"),
            source_step_index: 1,
            parent_chain: vec![],
            step_timeout_secs: None,
            skip: true,
            setup: false,
            checkpoint: None,
        },
        ExpandedStep {
            step_id: 2,
            instruction: "Verify the homepage".to_string(),
            source_file: PathBuf::from("/test/root.test.toml"),
            source_step_index: 2,
            parent_chain: vec![],
            step_timeout_secs: None,
            skip: false,
            setup: false,
            checkpoint: None,
        },
    ];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    // Only 2 steps should execute: the setup step (bypasses skip) and the non-skipped step
    let mut session = MockSession::new(vec![
        vec![
            Ok(OutputChunk::Text("Browser started.\nRESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ],
        vec![
            Ok(OutputChunk::Text("Homepage loaded.\nRESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ],
    ]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(outcome.all_passed);
    // 3 outcomes: setup (executed), skipped (auto-ok), test (executed)
    assert_eq!(outcome.steps.len(), 3);
    // Step 0: setup ran
    assert!(outcome.steps[0].setup);
    assert_eq!(
        outcome.steps[0].result,
        StepResult::Verdict(StepVerdict::Ok)
    );
    // Step 1: skipped
    assert!(!outcome.steps[1].setup);
    assert_eq!(outcome.steps[1].duration, Duration::ZERO);
    // Step 2: test ran
    assert!(!outcome.steps[2].setup);
    assert_eq!(
        outcome.steps[2].result,
        StepResult::Verdict(StepVerdict::Ok)
    );
}

#[tokio::test]
async fn setup_step_not_counted_in_all_passed() {
    // A setup step + a failing test step: all_passed should be false
    // because the test step failed, not because of the setup step.
    let steps = vec![
        ExpandedStep {
            step_id: 0,
            instruction: "Start the browser".to_string(),
            source_file: PathBuf::from("/test/root.test.toml"),
            source_step_index: 0,
            parent_chain: vec![],
            step_timeout_secs: None,
            skip: false,
            setup: true,
            checkpoint: None,
        },
        ExpandedStep {
            step_id: 1,
            instruction: "Check the homepage".to_string(),
            source_file: PathBuf::from("/test/root.test.toml"),
            source_step_index: 1,
            parent_chain: vec![],
            step_timeout_secs: None,
            skip: false,
            setup: false,
            checkpoint: None,
        },
    ];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    let mut session = MockSession::new(vec![
        vec![
            Ok(OutputChunk::Text("Browser started.".to_string())),
            Ok(OutputChunk::Done),
        ],
        vec![
            Ok(OutputChunk::Text(
                "RESULT ERROR: page not found".to_string(),
            )),
            Ok(OutputChunk::Done),
        ],
    ]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert!(!outcome.all_passed);
    assert_eq!(outcome.steps.len(), 2);
    // Setup step succeeded (no RESULT marker, tolerated)
    assert!(outcome.steps[0].setup);
    assert_eq!(
        outcome.steps[0].result,
        StepResult::Verdict(StepVerdict::Ok)
    );
    // Test step failed
    assert!(!outcome.steps[1].setup);
    assert!(outcome.steps[1].result.is_failure());
}

#[tokio::test]
async fn setup_step_failure_aborts_run() {
    let steps = vec![
        ExpandedStep {
            step_id: 0,
            instruction: "Start the browser".to_string(),
            source_file: PathBuf::from("/test/root.test.toml"),
            source_step_index: 0,
            parent_chain: vec![],
            step_timeout_secs: None,
            skip: false,
            setup: true,
            checkpoint: None,
        },
        ExpandedStep {
            step_id: 1,
            instruction: "Check the homepage".to_string(),
            source_file: PathBuf::from("/test/root.test.toml"),
            source_step_index: 1,
            parent_chain: vec![],
            step_timeout_secs: None,
            skip: false,
            setup: false,
            checkpoint: None,
        },
    ];
    let (run_id, session_id) = test_run_ids();
    let (_tmp, artifact_dir) = test_artifact_dir();

    // Setup step gets a provider error — should abort, second step never runs
    let mut session = MockSession::new(vec![
        vec![Err(ProviderError::SessionCrashed(
            "process died".to_string(),
        ))],
        vec![
            Ok(OutputChunk::Text("RESULT OK".to_string())),
            Ok(OutputChunk::Done),
        ],
    ]);

    let outcome = execute_steps(
        &mut session,
        &steps,
        &run_id,
        &session_id,
        &artifact_dir,
        None,
        None,
        None,
        std::path::Path::new("."),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();

    // Only the setup step executed, and it failed
    assert_eq!(outcome.steps.len(), 1);
    assert!(outcome.steps[0].setup);
    assert!(outcome.steps[0].result.is_failure());
}
