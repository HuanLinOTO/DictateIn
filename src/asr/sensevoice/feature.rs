use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

pub struct SenseVoiceFeatureExtractor {
    filters: Vec<Vec<f32>>,
    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
}

impl SenseVoiceFeatureExtractor {
    const SAMPLE_RATE: usize = 16_000;
    const FFT_SIZE: usize = 400;
    const HOP_LENGTH: usize = 160;
    const MEL_BINS: usize = 80;

    pub fn new() -> Self {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(Self::FFT_SIZE);
        Self {
            filters: build_mel_filters(),
            window: (0..Self::FFT_SIZE)
                .map(|index| {
                    0.54 - 0.46
                        * (2.0 * std::f32::consts::PI * index as f32 / Self::FFT_SIZE as f32).cos()
                })
                .collect(),
            fft,
        }
    }

    pub fn extract(&self, audio: &[f32]) -> Vec<Vec<f32>> {
        if audio.is_empty() {
            return Vec::new();
        }

        let mean = audio.iter().sum::<f32>() / audio.len() as f32;
        let centered = audio.iter().map(|sample| sample - mean).collect::<Vec<_>>();
        let mut emphasized = vec![0.0; centered.len()];
        emphasized[0] = centered[0];
        for index in 1..centered.len() {
            emphasized[index] = centered[index] - 0.97 * centered[index - 1];
        }

        let padding = Self::FFT_SIZE / 2;
        let mut padded = vec![0.0; emphasized.len() + padding * 2];
        padded[padding..padding + emphasized.len()].copy_from_slice(&emphasized);
        let frame_count = 1 + (padded.len() - Self::FFT_SIZE) / Self::HOP_LENGTH;
        let mut mel_frames = Vec::with_capacity(frame_count);

        for frame_index in 0..frame_count {
            let start = frame_index * Self::HOP_LENGTH;
            let mut spectrum = padded[start..start + Self::FFT_SIZE]
                .iter()
                .zip(&self.window)
                .map(|(sample, window)| Complex32::new(sample * window, 0.0))
                .collect::<Vec<_>>();
            self.fft.process(&mut spectrum);
            let power = spectrum[..=Self::FFT_SIZE / 2]
                .iter()
                .map(|value| value.norm_sqr())
                .collect::<Vec<_>>();
            let mel = (0..Self::MEL_BINS)
                .map(|mel_index| {
                    let energy = power
                        .iter()
                        .enumerate()
                        .map(|(frequency, value)| value * self.filters[frequency][mel_index])
                        .sum::<f32>();
                    (energy + 1e-7).ln()
                })
                .collect::<Vec<_>>();
            mel_frames.push(mel);
        }

        lfr_stack(&mel_frames)
    }
}

fn build_mel_filters() -> Vec<Vec<f32>> {
    let hz_to_mel = |frequency: f32| 2595.0 * (1.0 + frequency / 700.0).log10();
    let mel_to_hz = |mel: f32| 700.0 * (10.0_f32.powf(mel / 2595.0) - 1.0);
    let mel_min = hz_to_mel(20.0);
    let mel_max = hz_to_mel(8000.0);
    let points = (0..SenseVoiceFeatureExtractor::MEL_BINS + 2)
        .map(|index| {
            let ratio = index as f32 / (SenseVoiceFeatureExtractor::MEL_BINS + 1) as f32;
            mel_to_hz(mel_min + ratio * (mel_max - mel_min))
        })
        .collect::<Vec<_>>();

    (0..=SenseVoiceFeatureExtractor::FFT_SIZE / 2)
        .map(|frequency_index| {
            let frequency = frequency_index as f32 * SenseVoiceFeatureExtractor::SAMPLE_RATE as f32
                / SenseVoiceFeatureExtractor::FFT_SIZE as f32;
            (0..SenseVoiceFeatureExtractor::MEL_BINS)
                .map(|mel_index| {
                    let left = points[mel_index];
                    let center = points[mel_index + 1];
                    let right = points[mel_index + 2];
                    if frequency < left || frequency > right {
                        0.0
                    } else if frequency <= center {
                        (frequency - left) / (center - left)
                    } else {
                        (right - frequency) / (right - center)
                    }
                })
                .collect()
        })
        .collect()
}

fn lfr_stack(mel_frames: &[Vec<f32>]) -> Vec<Vec<f32>> {
    if mel_frames.is_empty() {
        return Vec::new();
    }

    let output_frames = mel_frames.len().div_ceil(6);
    let mut padded = Vec::with_capacity(3 + mel_frames.len() + 16);
    padded.extend(std::iter::repeat_n(mel_frames[0].clone(), 3));
    padded.extend_from_slice(mel_frames);
    let right_padding = output_frames * 6 + 7 - mel_frames.len();
    padded.extend(std::iter::repeat_n(
        mel_frames.last().unwrap().clone(),
        right_padding,
    ));

    (0..output_frames)
        .map(|output_index| {
            let mut frame = Vec::with_capacity(560);
            for stack_index in 0..7 {
                frame.extend_from_slice(&padded[output_index * 6 + stack_index]);
            }
            frame
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_lfr_560_features() {
        let extractor = SenseVoiceFeatureExtractor::new();
        let audio = vec![0.0; 16_000];
        let features = extractor.extract(&audio);

        assert!((16..=18).contains(&features.len()));
        assert!(features.iter().all(|frame| frame.len() == 560));
        assert!(features.iter().flatten().all(|value| value.is_finite()));
    }
}
