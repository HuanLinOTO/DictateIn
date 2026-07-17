use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};

use crate::asr::AudioChunk;

use super::{
    AudioCapture, AudioResampler, AudioRingBuffer, CapturedAudio, SilenceDetector, SilenceState,
};

#[derive(Debug)]
pub enum AudioCommand {
    Start {
        session_id: u64,
        segment_on_silence: bool,
    },
    Stop {
        session_id: u64,
    },
    SelectDevice {
        name: String,
    },
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum AudioEvent {
    Ready {
        device_name: String,
        sample_rate: u32,
    },
    Level {
        session_id: u64,
        peak: f32,
    },
    Stopped {
        session_id: u64,
    },
    Error {
        session_id: Option<u64>,
        message: String,
    },
}

pub struct AudioWorker {
    handle: Option<JoinHandle<()>>,
}

struct AudioPipeline {
    resampler: AudioResampler,
    ring_buffer: AudioRingBuffer,
    pre_roll: AudioRingBuffer,
    silence_detector: Option<SilenceDetector>,
}

impl AudioPipeline {
    fn new(input_sample_rate: u32, segment_on_silence: bool) -> anyhow::Result<Self> {
        Ok(Self {
            resampler: AudioResampler::new(input_sample_rate, 16_000)?,
            ring_buffer: AudioRingBuffer::new(160_000),
            pre_roll: AudioRingBuffer::new(4_800),
            silence_detector: segment_on_silence.then(|| SilenceDetector::new(16_000)),
        })
    }
}

impl AudioWorker {
    pub fn spawn(
        commands: Receiver<AudioCommand>,
        events: Sender<AudioEvent>,
        asr_audio: Sender<AudioChunk>,
    ) -> Self {
        let handle = thread::Builder::new()
            .name("audio-worker".into())
            .spawn(move || run(commands, events, asr_audio))
            .expect("failed to spawn audio worker");
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

fn run(
    commands: Receiver<AudioCommand>,
    events: Sender<AudioEvent>,
    asr_audio: Sender<AudioChunk>,
) {
    let (capture_sender, capture_receiver) = bounded(16);
    let Some(mut capture) = wait_for_capture(&commands, &events, &capture_sender) else {
        return;
    };
    let _ = events.send(AudioEvent::Ready {
        device_name: capture.device_name.clone(),
        sample_rate: capture.sample_rate,
    });

    let mut active_session = None;
    let mut pipeline = None;
    let mut dropped_blocks_at_start = capture.dropped_blocks();
    loop {
        crossbeam_channel::select! {
            recv(commands) -> command => {
                let Ok(command) = command else {
                    break;
                };
                match command {
                    AudioCommand::Start {
                        session_id,
                        segment_on_silence,
                    } => {
                        tracing::info!(session_id, sample_rate = capture.sample_rate, "audio start command received");
                        match AudioPipeline::new(capture.sample_rate, segment_on_silence) {
                            Ok(new_pipeline) => {
                                tracing::info!(session_id, "audio resampler created");
                                pipeline = Some(new_pipeline);
                                dropped_blocks_at_start = capture.dropped_blocks();
                                active_session = Some(session_id);
                                tracing::info!(session_id, "starting CPAL input stream");
                                if let Err(error) = capture.start() {
                                    active_session = None;
                                    let _ = events.send(AudioEvent::Error {
                                        session_id: Some(session_id),
                                        message: error.to_string(),
                                    });
                                } else {
                                    tracing::info!(session_id, "CPAL input stream started");
                                }
                            }
                            Err(error) => {
                                let _ = events.send(AudioEvent::Error {
                                    session_id: Some(session_id),
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    AudioCommand::Stop { session_id } => {
                        if active_session != Some(session_id) {
                            continue;
                        }
                        let _ = capture.pause();
                        let mut processing_error = None;
                        while let Ok(block) = capture_receiver.try_recv() {
                            if let Err(error) = process_block(
                                session_id,
                                block,
                                &mut pipeline,
                                &events,
                                &asr_audio,
                            ) {
                                processing_error = Some(error);
                                break;
                            }
                        }
                        if let Some(message) = processing_error {
                            active_session = None;
                            pipeline = None;
                            let _ = events.send(AudioEvent::Error {
                                session_id: Some(session_id),
                                message,
                            });
                            continue;
                        }
                        if let Some(active_pipeline) = pipeline.as_mut() {
                            let samples = match active_pipeline.resampler.finish() {
                                Ok(samples) => samples,
                                Err(error) => {
                                    active_session = None;
                                    pipeline = None;
                                    let _ = events.send(AudioEvent::Error {
                                        session_id: Some(session_id),
                                        message: error.to_string(),
                                    });
                                    continue;
                                }
                            };
                            if !samples.is_empty() {
                                let silence_state = active_pipeline
                                    .silence_detector
                                    .as_mut()
                                    .map(|detector| {
                                        detector.observe(root_mean_square(&samples), samples.len())
                                    });
                                match silence_state {
                                    None
                                    | Some(SilenceState::Speaking)
                                    | Some(SilenceState::SegmentEnded) => {
                                        active_pipeline.ring_buffer.push(&samples);
                                    }
                                    Some(SilenceState::SpeechStarted) => {
                                        active_pipeline.pre_roll.push(&samples);
                                        let buffered = active_pipeline.pre_roll.drain_all();
                                        active_pipeline.ring_buffer.push(&buffered);
                                    }
                                    Some(SilenceState::Waiting) => {
                                        active_pipeline.pre_roll.push(&samples);
                                    }
                                }
                            }
                            if active_pipeline
                                .silence_detector
                                .as_ref()
                                .is_some_and(SilenceDetector::should_flush_tail)
                                && active_pipeline.ring_buffer.len() == 0
                            {
                                let tail = active_pipeline.pre_roll.drain_all();
                                active_pipeline.ring_buffer.push(&tail);
                            }
                            if let Err(message) = flush_all(
                                session_id,
                                &mut active_pipeline.ring_buffer,
                                &asr_audio,
                            ) {
                                active_session = None;
                                pipeline = None;
                                let _ = events.send(AudioEvent::Error {
                                    session_id: Some(session_id),
                                    message,
                                });
                                continue;
                            }
                        }
                        active_session = None;
                        pipeline = None;
                        let _ = events.send(AudioEvent::Stopped { session_id });
                    }
                    AudioCommand::SelectDevice { name } => {
                        if active_session.is_some() {
                            continue;
                        }
                        match AudioCapture::open(Some(&name), capture_sender.clone()) {
                            Ok(new_capture) => {
                                capture = new_capture;
                                let _ = events.send(AudioEvent::Ready {
                                    device_name: capture.device_name.clone(),
                                    sample_rate: capture.sample_rate,
                                });
                            }
                            Err(error) => {
                                let _ = events.send(AudioEvent::Error {
                                    session_id: None,
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    AudioCommand::Shutdown => {
                        let _ = capture.pause();
                        break;
                    }
                }
            }
            recv(capture_receiver) -> block => {
                let Ok(block) = block else {
                    break;
                };
                if let Some(session_id) = active_session {
                    if let Err(message) = process_block(
                        session_id,
                        block,
                        &mut pipeline,
                        &events,
                        &asr_audio,
                    ) {
                        let _ = capture.pause();
                        active_session = None;
                        pipeline = None;
                        let _ = events.send(AudioEvent::Error {
                            session_id: Some(session_id),
                            message,
                        });
                    } else if capture.dropped_blocks() > dropped_blocks_at_start {
                        let _ = capture.pause();
                        active_session = None;
                        pipeline = None;
                        let _ = events.send(AudioEvent::Error {
                            session_id: Some(session_id),
                            message: "音频回调过载，当前会话已终止".into(),
                        });
                    }
                }
            }
        }
    }
}

fn wait_for_capture(
    commands: &Receiver<AudioCommand>,
    events: &Sender<AudioEvent>,
    capture_sender: &Sender<CapturedAudio>,
) -> Option<AudioCapture> {
    let mut selected_name = None;
    loop {
        match AudioCapture::open(selected_name.as_deref(), capture_sender.clone()) {
            Ok(capture) => return Some(capture),
            Err(error) => {
                let _ = events.send(AudioEvent::Error {
                    session_id: None,
                    message: format!("{}；等待麦克风连接后自动重试", error),
                });
            }
        }
        match commands.recv_timeout(std::time::Duration::from_secs(3)) {
            Ok(AudioCommand::SelectDevice { name }) => selected_name = Some(name),
            Ok(AudioCommand::Start { session_id, .. }) => {
                let _ = events.send(AudioEvent::Error {
                    session_id: Some(session_id),
                    message: "当前没有可用麦克风".into(),
                });
            }
            Ok(AudioCommand::Stop { .. }) => {}
            Ok(AudioCommand::Shutdown) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return None;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn process_block(
    session_id: u64,
    block: CapturedAudio,
    pipeline: &mut Option<AudioPipeline>,
    events: &Sender<AudioEvent>,
    asr_audio: &Sender<AudioChunk>,
) -> Result<(), String> {
    let _ = events.try_send(AudioEvent::Level {
        session_id,
        peak: block.peak,
    });
    let Some(pipeline) = pipeline.as_mut() else {
        return Ok(());
    };
    debug_assert_eq!(block.sample_rate, pipeline.resampler.rates().0);
    match pipeline.resampler.process(&block.samples) {
        Ok(samples) if !samples.is_empty() => {
            let rms = root_mean_square(&samples);
            let silence_state = pipeline
                .silence_detector
                .as_mut()
                .map(|detector| detector.observe(rms, samples.len()));
            match silence_state {
                None => pipeline.ring_buffer.push(&samples),
                Some(SilenceState::Waiting) => pipeline.pre_roll.push(&samples),
                Some(SilenceState::SpeechStarted) => {
                    pipeline.pre_roll.push(&samples);
                    let buffered = pipeline.pre_roll.drain_all();
                    pipeline.ring_buffer.push(&buffered);
                }
                Some(SilenceState::Speaking) | Some(SilenceState::SegmentEnded) => {
                    pipeline.ring_buffer.push(&samples);
                }
            }
            flush_ready(session_id, &mut pipeline.ring_buffer, asr_audio)?;
            if silence_state == Some(SilenceState::SegmentEnded) {
                flush_all(session_id, &mut pipeline.ring_buffer, asr_audio)?;
                tracing::debug!(session_id, "silence segment boundary detected");
                send_asr(asr_audio, AudioChunk::SegmentBoundary { session_id })?;
            }
        }
        Ok(_) => {}
        Err(error) => {
            return Err(error.to_string());
        }
    }
    Ok(())
}

fn flush_ready(
    session_id: u64,
    ring_buffer: &mut AudioRingBuffer,
    asr_audio: &Sender<AudioChunk>,
) -> Result<(), String> {
    while ring_buffer.len() >= 12_800 {
        let samples = ring_buffer.drain(12_800);
        send_asr(
            asr_audio,
            AudioChunk::Samples {
                session_id,
                samples,
            },
        )?;
    }
    Ok(())
}

fn flush_all(
    session_id: u64,
    ring_buffer: &mut AudioRingBuffer,
    asr_audio: &Sender<AudioChunk>,
) -> Result<(), String> {
    let samples = ring_buffer.drain_all();
    if !samples.is_empty() {
        send_asr(
            asr_audio,
            AudioChunk::Samples {
                session_id,
                samples,
            },
        )?;
    }
    Ok(())
}

fn send_asr(asr_audio: &Sender<AudioChunk>, chunk: AudioChunk) -> Result<(), String> {
    asr_audio.try_send(chunk).map_err(|error| match error {
        TrySendError::Full(_) => "ASR 处理速度不足，当前录音已终止".into(),
        TrySendError::Disconnected(_) => "ASR 音频通道已断开".into(),
    })
}

fn root_mean_square(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean_square =
        samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32;
    mean_square.sqrt()
}
