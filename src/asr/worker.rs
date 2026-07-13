use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};

use crate::asr::{
    AsrEngine, AsrSession, AudioChunk, FunAsrNanoEngine, ModelRegistry, Qwen3AsrEngine,
    SenseVoiceEngine, SessionOptions,
};
use crate::asr::{AsrError, ModelKind};
use crate::commands::AsrCommand;
use crate::events::AsrEvent;

pub struct AsrWorker {
    handle: Option<JoinHandle<()>>,
}

impl AsrWorker {
    pub fn spawn(
        commands: Receiver<AsrCommand>,
        audio: Receiver<AudioChunk>,
        events: Sender<AsrEvent>,
    ) -> Self {
        let handle = thread::Builder::new()
            .name("asr-worker".into())
            .spawn(move || run_real(commands, audio, events))
            .expect("failed to spawn ASR worker");
        Self {
            handle: Some(handle),
        }
    }

    pub fn join(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run_real(commands: Receiver<AsrCommand>, audio: Receiver<AudioChunk>, events: Sender<AsrEvent>) {
    let registry = match ModelRegistry::discover() {
        Ok(registry) => registry,
        Err(error) => {
            let _ = events.send(AsrEvent::Error {
                session_id: None,
                error: AsrError::Inference(error.to_string()),
            });
            return;
        }
    };
    let mut engine: Option<Box<dyn AsrEngine>> = None;
    let mut session: Option<(u64, Box<dyn AsrSession>)> = None;

    loop {
        crossbeam_channel::select_biased! {
            recv(commands) -> command => {
                let Ok(command) = command else {
                    break;
                };
                match command {
            AsrCommand::LoadModel(selection) => {
                let _ = events.send(AsrEvent::ModelLoading(selection.kind));
                match load_engine(&registry, selection.kind) {
                    Ok(new_engine) => {
                        let info = new_engine.info().clone();
                        engine = Some(new_engine);
                        session = None;
                        let _ = events.send(AsrEvent::ModelReady(info));
                    }
                    Err(error) => {
                        let _ = events.send(AsrEvent::ModelLoadFailed {
                            kind: selection.kind,
                            error: AsrError::Inference(error.to_string()),
                            previous_model_available: engine.is_some(),
                        });
                    }
                }
            }
            AsrCommand::StartSession {
                session_id,
                hotwords,
                enable_partials,
            } => {
                let Some(engine) = engine.as_mut() else {
                    let _ = events.send(AsrEvent::Error {
                        session_id: Some(session_id),
                        error: AsrError::ModelNotReady,
                    });
                    continue;
                };
                match engine.start_session(SessionOptions {
                    session_id,
                    hotwords,
                    enable_partials,
                }) {
                    Ok(new_session) => {
                        session = Some((session_id, new_session));
                    }
                    Err(error) => {
                        let _ = events.send(AsrEvent::Error {
                            session_id: Some(session_id),
                            error,
                        });
                    }
                }
            }
            AsrCommand::FinishSession { session_id } => {
                while let Ok(chunk) = audio.try_recv() {
                    if chunk.session_id == session_id
                        && let Some((active_id, active)) = session.as_mut()
                        && *active_id == session_id
                    {
                        let _ = active.accept_audio(&chunk.samples);
                    }
                }
                let Some((active_id, active)) = session.take() else {
                    continue;
                };
                if active_id != session_id {
                    session = Some((active_id, active));
                    continue;
                }
                match active.finish() {
                    Ok(final_result) => {
                        let _ = events.send(AsrEvent::Final {
                            session_id,
                            text: final_result.text,
                            metrics: final_result.metrics,
                        });
                    }
                    Err(error) => {
                        let _ = events.send(AsrEvent::Error {
                            session_id: Some(session_id),
                            error,
                        });
                    }
                }
            }
            AsrCommand::CancelSession { session_id } => {
                if let Some((active_id, active)) = session.take() {
                    if active_id == session_id {
                        active.cancel();
                    } else {
                        session = Some((active_id, active));
                    }
                }
            }
            AsrCommand::Shutdown => {
                if let Some((_, active)) = session.take() {
                    active.cancel();
                }
                break;
            }
                }
            }
            recv(audio) -> chunk => {
                let Ok(chunk) = chunk else {
                    continue;
                };
                process_audio_chunk(&mut session, &events, chunk);
            }
        }
    }
}

fn process_audio_chunk(
    session: &mut Option<(u64, Box<dyn AsrSession>)>,
    events: &Sender<AsrEvent>,
    chunk: AudioChunk,
) {
    let Some((session_id, active)) = session.as_mut() else {
        return;
    };
    if *session_id != chunk.session_id {
        return;
    }
    let result = active
        .accept_audio(&chunk.samples)
        .and_then(|_| active.poll_partial());
    match result {
        Ok(Some(partial)) => {
            let _ = events.send(AsrEvent::Partial {
                session_id: chunk.session_id,
                text: partial.text,
                revision: partial.revision,
            });
        }
        Ok(None) => {}
        Err(error) => {
            let _ = events.send(AsrEvent::Error {
                session_id: Some(chunk.session_id),
                error,
            });
        }
    }
}

fn load_engine(registry: &ModelRegistry, kind: ModelKind) -> anyhow::Result<Box<dyn AsrEngine>> {
    let directory = registry.validate(kind)?;
    match kind {
        ModelKind::SenseVoice => Ok(Box::new(SenseVoiceEngine::load(&directory)?)),
        ModelKind::FunAsrNano => Ok(Box::new(FunAsrNanoEngine::load(&directory)?)),
        ModelKind::Qwen3Asr => Ok(Box::new(Qwen3AsrEngine::load(&directory)?)),
    }
}
