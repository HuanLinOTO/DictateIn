use crate::asr::{Hotword, ModelSelection};

#[derive(Debug)]
pub enum AsrCommand {
    LoadModel(ModelSelection),
    StartSession {
        session_id: u64,
        hotwords: Vec<Hotword>,
        enable_partials: bool,
    },
    FinishSession {
        session_id: u64,
    },
    CancelSession {
        session_id: u64,
    },
    Shutdown,
}
