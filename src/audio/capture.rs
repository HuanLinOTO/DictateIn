use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use crossbeam_channel::Sender;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::format::{i16_to_f32, mix_interleaved_to_mono, u16_to_f32};

#[derive(Debug)]
pub struct CapturedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub peak: f32,
}

pub struct AudioCapture {
    stream: Stream,
    pub sample_rate: u32,
    pub device_name: String,
    dropped_blocks: Arc<AtomicU64>,
}

impl AudioCapture {
    pub fn open(selected_name: Option<&str>, sender: Sender<CapturedAudio>) -> Result<Self> {
        let host = cpal::default_host();
        let device = if let Some(selected_name) = selected_name {
            host.input_devices()?
                .find(|device| device.name().ok().as_deref() == Some(selected_name))
                .or_else(|| host.default_input_device())
        } else {
            host.default_input_device()
        }
        .context("no input audio device is available")?;
        let device_name = device
            .name()
            .unwrap_or_else(|_| "Default microphone".into());
        let supported = device.default_input_config()?;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let channels = config.channels;
        let sample_rate = config.sample_rate.0;
        let dropped_blocks = Arc::new(AtomicU64::new(0));
        let error_callback = |error| {
            tracing::error!(%error, "audio input stream error");
        };

        let stream = match sample_format {
            SampleFormat::F32 => {
                let sender = sender.clone();
                let dropped_blocks = Arc::clone(&dropped_blocks);
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _| {
                        send_block(
                            data.to_vec(),
                            channels,
                            sample_rate,
                            &sender,
                            &dropped_blocks,
                        );
                    },
                    error_callback,
                    None,
                )?
            }
            SampleFormat::I16 => {
                let sender = sender.clone();
                let dropped_blocks = Arc::clone(&dropped_blocks);
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _| {
                        send_block(
                            i16_to_f32(data),
                            channels,
                            sample_rate,
                            &sender,
                            &dropped_blocks,
                        );
                    },
                    error_callback,
                    None,
                )?
            }
            SampleFormat::U16 => {
                let dropped_blocks = Arc::clone(&dropped_blocks);
                device.build_input_stream(
                    &config,
                    move |data: &[u16], _| {
                        send_block(
                            u16_to_f32(data),
                            channels,
                            sample_rate,
                            &sender,
                            &dropped_blocks,
                        );
                    },
                    error_callback,
                    None,
                )?
            }
            other => bail!("unsupported microphone sample format: {other}"),
        };

        Ok(Self {
            stream,
            sample_rate,
            device_name,
            dropped_blocks,
        })
    }

    pub fn start(&self) -> Result<()> {
        self.stream.play().context("failed to start microphone")
    }

    pub fn pause(&self) -> Result<()> {
        self.stream.pause().context("failed to pause microphone")
    }

    pub fn dropped_blocks(&self) -> u64 {
        self.dropped_blocks.load(Ordering::Relaxed)
    }
}

fn send_block(
    interleaved: Vec<f32>,
    channels: u16,
    sample_rate: u32,
    sender: &Sender<CapturedAudio>,
    dropped_blocks: &AtomicU64,
) {
    let samples = mix_interleaved_to_mono(&interleaved, channels as usize);
    let peak = samples
        .iter()
        .fold(0.0_f32, |current, sample| current.max(sample.abs()));
    if sender
        .try_send(CapturedAudio {
            samples,
            sample_rate,
            peak,
        })
        .is_err()
    {
        dropped_blocks.fetch_add(1, Ordering::Relaxed);
    }
}
