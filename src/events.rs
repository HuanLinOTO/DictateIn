use crate::asr::{AsrError, ModelInfo, ModelKind, RecognitionMetrics};

#[derive(Debug, Clone)]
pub enum AsrEvent {
    ModelLoading(ModelKind),
    ModelReady(ModelInfo),
    ModelLoadFailed {
        kind: ModelKind,
        error: AsrError,
        previous_model_available: bool,
    },
    Partial {
        session_id: u64,
        text: String,
        revision: u64,
    },
    Final {
        session_id: u64,
        text: String,
        metrics: RecognitionMetrics,
    },
    SessionCancelled {
        session_id: u64,
    },
    Error {
        session_id: Option<u64>,
        error: AsrError,
    },
}
