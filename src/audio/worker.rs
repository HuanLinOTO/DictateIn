use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender, bounded};

use crate::asr::AudioChunk;

use super::{AudioCapture, AudioResampler, AudioRingBuffer, CapturedAudio};

#[derive(Debug)]
pub enum AudioCommand {
    Start { session_id: u64 },
    Stop { session_id: u64 },
    SelectDevice { name: String },
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
    let mut resampler = None;
    let mut ring_buffer = AudioRingBuffer::new(160_000);
    let mut dropped_blocks_at_start = capture.dropped_blocks();
    loop {
        crossbeam_channel::select! {
            recv(commands) -> command => {
                let Ok(command) = command else {
                    break;
                };
                match command {
                    AudioCommand::Start { session_id } => {
                        tracing::info!(session_id, sample_rate = capture.sample_rate, "audio start command received");
                        match AudioResampler::new(capture.sample_rate, 16_000) {
                            Ok(new_resampler) => {
                                tracing::info!(session_id, "audio resampler created");
                                resampler = Some(new_resampler);
                                ring_buffer = AudioRingBuffer::new(160_000);
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
                        while let Ok(block) = capture_receiver.try_recv() {
                            process_block(
                                session_id,
                                block,
                                &mut resampler,
                                &mut ring_buffer,
                                &events,
                                &asr_audio,
                            );
                        }
                        if let Some(resampler) = resampler.as_mut()
                            && let Ok(samples) = resampler.finish()
                            && !samples.is_empty()
                        {
                            ring_buffer.push(&samples);
                        }
                        let tail = ring_buffer.drain_all();
                        if !tail.is_empty() {
                            let _ = asr_audio.send(AudioChunk {
                                session_id,
                                samples: tail,
                            });
                        }
                        active_session = None;
                        resampler = None;
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
                    process_block(
                        session_id,
                        block,
                        &mut resampler,
                        &mut ring_buffer,
                        &events,
                        &asr_audio,
                    );
                    if capture.dropped_blocks() > dropped_blocks_at_start {
                        let _ = capture.pause();
                        active_session = None;
                        resampler = None;
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
            Ok(AudioCommand::Start { session_id }) => {
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
    resampler: &mut Option<AudioResampler>,
    ring_buffer: &mut AudioRingBuffer,
    events: &Sender<AudioEvent>,
    asr_audio: &Sender<AudioChunk>,
) {
    let _ = events.try_send(AudioEvent::Level {
        session_id,
        peak: block.peak,
    });
    let Some(resampler) = resampler.as_mut() else {
        return;
    };
    debug_assert_eq!(block.sample_rate, resampler.rates().0);
    match resampler.process(&block.samples) {
        Ok(samples) if !samples.is_empty() => {
            ring_buffer.push(&samples);
            while ring_buffer.len() >= 12_800 {
                let samples = ring_buffer.drain(12_800);
                let _ = asr_audio.send(AudioChunk {
                    session_id,
                    samples,
                });
            }
        }
        Ok(_) => {}
        Err(error) => {
            let _ = events.send(AudioEvent::Error {
                session_id: Some(session_id),
                message: error.to_string(),
            });
        }
    }
}
