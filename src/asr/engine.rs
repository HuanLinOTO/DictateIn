use crate::asr::{AsrError, EngineCapabilities, Hotword, ModelInfo, RecognitionMetrics};

#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub session_id: u64,
    pub hotwords: Vec<Hotword>,
    pub enable_partials: bool,
}

#[derive(Debug, Clone)]
pub struct PartialResult {
    pub text: String,
    pub revision: u64,
}

#[derive(Debug, Clone)]
pub struct FinalResult {
    pub text: String,
    pub metrics: RecognitionMetrics,
}

pub trait AsrEngine: Send {
    fn info(&self) -> &ModelInfo;
    fn capabilities(&self) -> EngineCapabilities;
    fn start_session(&mut self, options: SessionOptions) -> Result<Box<dyn AsrSession>, AsrError>;
}

pub trait AsrSession: Send {
    fn accept_audio(&mut self, samples: &[f32]) -> Result<(), AsrError>;
    fn poll_partial(&mut self) -> Result<Option<PartialResult>, AsrError>;
    fn finish(self: Box<Self>) -> Result<FinalResult, AsrError>;
    fn cancel(self: Box<Self>);
}
