mod capabilities;
mod engine;
mod fun_asr_nano;
mod llama_runtime;
mod model_registry;
mod ort_session;
mod partial;
mod qwen3_asr;
mod sensevoice;
mod worker;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use capabilities::{EngineCapabilities, HotwordCapability};
pub use engine::{AsrEngine, AsrSession, FinalResult, PartialResult, SessionOptions};
pub use fun_asr_nano::FunAsrNanoEngine;
pub use model_registry::ModelRegistry;
pub use ort_session::OrtSessionConfig;
pub use qwen3_asr::Qwen3AsrEngine;
pub use sensevoice::SenseVoiceEngine;
pub use worker::AsrWorker;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    SenseVoice,
    FunAsrNano,
    Qwen3Asr,
}

#[derive(Debug, Clone)]
pub struct ModelSelection {
    pub kind: ModelKind,
}

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub kind: ModelKind,
    pub display_name: String,
}

#[derive(Debug, Clone)]
pub struct Hotword {
    pub text: String,
    pub boost: f32,
}

#[derive(Debug)]
pub enum AudioChunk {
    Samples { session_id: u64, samples: Vec<f32> },
    SegmentBoundary { session_id: u64 },
}

#[derive(Debug, Clone, Default)]
pub struct RecognitionMetrics {
    pub audio_duration_ms: u64,
    pub inference_duration_ms: u64,
}

#[derive(Debug, Clone, Error)]
pub enum AsrError {
    #[error("model is not ready")]
    ModelNotReady,
    #[error("model inference failed: {0}")]
    Inference(String),
}
