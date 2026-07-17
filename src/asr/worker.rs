use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};

use crate::asr::{
    AsrEngine, AsrSession, AudioChunk, FinalResult, FunAsrNanoEngine, ModelRegistry,
    Qwen3AsrEngine, RecognitionMetrics, SenseVoiceEngine, SessionOptions,
};
use crate::asr::{AsrError, ModelKind};
use crate::commands::AsrCommand;
use crate::events::AsrEvent;

pub struct AsrWorker {
    handle: Option<JoinHandle<()>>,
}

struct ActiveSession {
    session_id: u64,
    options: SessionOptions,
    current: Option<Box<dyn AsrSession>>,
    current_samples: usize,
    completed_text: String,
    metrics: RecognitionMetrics,
    revision: u64,
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
    let mut session: Option<ActiveSession> = None;

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
                let options = SessionOptions {
                    session_id,
                    hotwords,
                    enable_partials,
                };
                match engine.start_session(options.clone()) {
                    Ok(new_session) => {
                        session = Some(ActiveSession {
                            session_id,
                            options,
                            current: Some(new_session),
                            current_samples: 0,
                            completed_text: String::new(),
                            metrics: RecognitionMetrics::default(),
                            revision: 0,
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
            AsrCommand::FinishSession { session_id } => {
                while let Ok(chunk) = audio.try_recv() {
                    process_audio_chunk(&mut engine, &mut session, &events, chunk);
                }
                let Some(active) = session.take() else {
                    continue;
                };
                if active.session_id != session_id {
                    session = Some(active);
                    continue;
                }
                match finish_active_session(active) {
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
                if let Some(mut active) = session.take() {
                    if active.session_id == session_id {
                        if let Some(current) = active.current.take() {
                            current.cancel();
                        }
                    } else {
                        session = Some(active);
                    }
                }
                while audio.try_recv().is_ok() {}
                let _ = events.send(AsrEvent::SessionCancelled { session_id });
            }
            AsrCommand::Shutdown => {
                if let Some(mut active) = session.take()
                    && let Some(current) = active.current.take()
                {
                    current.cancel();
                }
                break;
            }
                }
            }
            recv(audio) -> chunk => {
                let Ok(chunk) = chunk else {
                    continue;
                };
                process_audio_chunk(&mut engine, &mut session, &events, chunk);
            }
        }
    }
}

fn process_audio_chunk(
    engine: &mut Option<Box<dyn AsrEngine>>,
    session: &mut Option<ActiveSession>,
    events: &Sender<AsrEvent>,
    chunk: AudioChunk,
) {
    let chunk_session_id = match &chunk {
        AudioChunk::Samples { session_id, .. } | AudioChunk::SegmentBoundary { session_id } => {
            *session_id
        }
    };
    let Some(mut active) = session.take() else {
        return;
    };
    if active.session_id != chunk_session_id {
        *session = Some(active);
        return;
    }

    let result = match chunk {
        AudioChunk::Samples { samples, .. } => {
            let result = active
                .current
                .as_mut()
                .ok_or_else(|| AsrError::Inference("ASR segment is not active".into()))
                .and_then(|current| {
                    current.accept_audio(&samples)?;
                    current.poll_partial()
                });
            if result.is_ok() {
                active.current_samples = active.current_samples.saturating_add(samples.len());
            }
            result.map(|partial| {
                partial.map(|partial| {
                    active.revision = active.revision.saturating_add(1);
                    let _ = partial.revision;
                    partial.text
                })
            })
        }
        AudioChunk::SegmentBoundary { .. } => engine
            .as_mut()
            .ok_or(AsrError::ModelNotReady)
            .and_then(|engine| finish_segment(engine.as_mut(), &mut active)),
    };

    match result {
        Ok(partial_text) => {
            if let Some(text) = partial_text {
                let _ = events.send(AsrEvent::Partial {
                    session_id: active.session_id,
                    text,
                    revision: active.revision,
                });
            }
            *session = Some(active);
        }
        Err(error) => {
            if let Some(current) = active.current.take() {
                current.cancel();
            }
            let _ = events.send(AsrEvent::Error {
                session_id: Some(chunk_session_id),
                error,
            });
        }
    }
}

fn finish_segment(
    engine: &mut dyn AsrEngine,
    active: &mut ActiveSession,
) -> Result<Option<String>, AsrError> {
    if active.current_samples == 0 {
        return Ok(None);
    }
    let current = active
        .current
        .take()
        .ok_or_else(|| AsrError::Inference("ASR segment is not active".into()))?;
    let result = current.finish()?;
    let previous_text_length = active.completed_text.len();
    append_segment(&mut active.completed_text, &result.text);
    active.metrics.audio_duration_ms = active
        .metrics
        .audio_duration_ms
        .saturating_add(result.metrics.audio_duration_ms);
    active.metrics.inference_duration_ms = active
        .metrics
        .inference_duration_ms
        .saturating_add(result.metrics.inference_duration_ms);
    active.current = Some(engine.start_session(active.options.clone())?);
    active.current_samples = 0;
    active.revision = active.revision.saturating_add(1);
    if active.completed_text.len() > previous_text_length {
        Ok(Some(active.completed_text.clone()))
    } else {
        Ok(None)
    }
}

fn finish_active_session(mut active: ActiveSession) -> Result<FinalResult, AsrError> {
    if active.current_samples > 0 {
        let current = active
            .current
            .take()
            .ok_or_else(|| AsrError::Inference("ASR segment is not active".into()))?;
        let result = current.finish()?;
        append_segment(&mut active.completed_text, &result.text);
        active.metrics.audio_duration_ms = active
            .metrics
            .audio_duration_ms
            .saturating_add(result.metrics.audio_duration_ms);
        active.metrics.inference_duration_ms = active
            .metrics
            .inference_duration_ms
            .saturating_add(result.metrics.inference_duration_ms);
    } else if let Some(current) = active.current.take() {
        current.cancel();
    }

    Ok(FinalResult {
        text: active.completed_text,
        metrics: active.metrics,
    })
}

fn append_segment(combined: &mut String, segment: &str) {
    let segment = segment.trim();
    if segment.is_empty() {
        return;
    }
    let needs_space = combined
        .chars()
        .next_back()
        .zip(segment.chars().next())
        .is_some_and(|(left, right)| {
            right.is_ascii_alphanumeric()
                && (left.is_ascii_alphanumeric()
                    || matches!(left, '.' | ',' | '!' | '?' | ';' | ':'))
        });
    if needs_space {
        combined.push(' ');
    }
    combined.push_str(segment);
}

fn load_engine(registry: &ModelRegistry, kind: ModelKind) -> anyhow::Result<Box<dyn AsrEngine>> {
    let directory = registry.validate(kind)?;
    match kind {
        ModelKind::SenseVoice => Ok(Box::new(SenseVoiceEngine::load(&directory)?)),
        ModelKind::FunAsrNano => Ok(Box::new(FunAsrNanoEngine::load(&directory)?)),
        ModelKind::Qwen3Asr => Ok(Box::new(Qwen3AsrEngine::load(&directory)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::append_segment;

    #[test]
    fn joins_chinese_segments_without_spaces() {
        let mut text = String::new();
        append_segment(&mut text, "今天下雨。");
        append_segment(&mut text, "记得带伞。");
        assert_eq!(text, "今天下雨。记得带伞。");
    }

    #[test]
    fn separates_ascii_words_across_segments() {
        let mut text = "hello".to_string();
        append_segment(&mut text, "world");
        assert_eq!(text, "hello world");
    }

    #[test]
    fn separates_ascii_sentences_after_punctuation() {
        let mut text = "Hello.".to_string();
        append_segment(&mut text, "World.");
        assert_eq!(text, "Hello. World.");
    }
}
