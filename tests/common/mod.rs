use async_trait::async_trait;
use bugatti::provider::{
    AgentSession, BootstrapMessage, OutputChunk, OutputStream, ProviderError, StepMessage,
    VecOutputStream,
};
use bugatti::run::{ArtifactDir, RunId, SessionId};
use std::path::Path;
use tempfile::TempDir;

pub struct ArtifactCase {
    _tmp: TempDir,
    pub artifact_dir: ArtifactDir,
}

impl ArtifactCase {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run-001".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();
        Self {
            _tmp: tmp,
            artifact_dir,
        }
    }
}

pub struct RunCase {
    _tmp: TempDir,
    pub run_id: RunId,
    pub session_id: SessionId,
    pub artifact_dir: ArtifactDir,
}

impl RunCase {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = RunId("test-run-001".to_string());
        let session_id = SessionId("test-session-001".to_string());
        let artifact_dir = ArtifactDir::from_run_id(tmp.path(), &run_id);
        artifact_dir.create_all().unwrap();
        Self {
            _tmp: tmp,
            run_id,
            session_id,
            artifact_dir,
        }
    }
}

pub struct MockSession {
    responses: Vec<Vec<Result<OutputChunk, ProviderError>>>,
    call_count: usize,
}

impl MockSession {
    pub fn new(responses: Vec<Vec<Result<OutputChunk, ProviderError>>>) -> Self {
        Self {
            responses,
            call_count: 0,
        }
    }

    pub fn with_ok_responses(count: usize) -> Self {
        let mut responses = Vec::new();
        for _ in 0..count {
            responses.push(vec![
                Ok(OutputChunk::Text("RESULT OK\n".to_string())),
                Ok(OutputChunk::Done),
            ]);
        }
        Self::new(responses)
    }
}

#[async_trait]
impl AgentSession for MockSession {
    fn initialize(
        _config: &bugatti::config::Config,
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
    ) -> Result<Box<dyn OutputStream + '_>, ProviderError> {
        Ok(Box::new(VecOutputStream::empty()))
    }

    async fn send_step(
        &mut self,
        _message: StepMessage,
    ) -> Result<Box<dyn OutputStream + '_>, ProviderError> {
        if self.call_count < self.responses.len() {
            let idx = self.call_count;
            self.call_count += 1;
            Ok(Box::new(VecOutputStream::new(self.responses[idx].clone())))
        } else {
            Err(ProviderError::SendFailed("no more responses".to_string()))
        }
    }

    async fn close(&mut self) -> Result<(), ProviderError> {
        Ok(())
    }
}
