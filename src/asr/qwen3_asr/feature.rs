use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

pub struct QwenMelExtractor {
    filters: Vec<Vec<f32>>,
    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
}

impl QwenMelExtractor {
    const FFT_SIZE: usize = 400;
    const HOP: usize = 160;
    const MELS: usize = 128;

    pub fn new() -> Self {
        let mut planner = FftPlanner::new();
        Self {
            filters: slaney_filters(),
            window: (0..Self::FFT_SIZE)
                .map(|index| {
                    0.5 - 0.5
                        * (2.0 * std::f32::consts::PI * index as f32 / Self::FFT_SIZE as f32).cos()
                })
                .collect(),
            fft: planner.plan_fft_forward(Self::FFT_SIZE),
        }
    }

    pub fn extract(&self, audio: &[f32]) -> Vec<f32> {
        if audio.len() < 2 {
            return Vec::new();
        }
        let padding = Self::FFT_SIZE / 2;
        let mut padded = Vec::with_capacity(audio.len() + padding * 2);
        for index in (1..=padding).rev() {
            padded.push(audio[index.min(audio.len() - 1)]);
        }
        padded.extend_from_slice(audio);
        for index in 0..padding {
            padded.push(audio[audio.len().saturating_sub(2 + index.min(audio.len() - 2))]);
        }
        let frame_count = audio.len() / Self::HOP;
        let mut output = vec![0.0; Self::MELS * frame_count];
        let mut maximum = f32::NEG_INFINITY;

        for frame in 0..frame_count {
            let start = frame * Self::HOP;
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
            for mel in 0..Self::MELS {
                let energy = power
                    .iter()
                    .enumerate()
                    .map(|(frequency, value)| value * self.filters[frequency][mel])
                    .sum::<f32>();
                let value = energy.max(1e-10).log10();
                output[mel * frame_count + frame] = value;
                maximum = maximum.max(value);
            }
        }
        for value in &mut output {
            *value = (value.max(maximum - 8.0) + 4.0) / 4.0;
        }
        output
    }
}

fn slaney_filters() -> Vec<Vec<f32>> {
    let hz_to_mel = |frequency: f32| {
        let linear = frequency / (200.0 / 3.0);
        if frequency >= 1000.0 {
            15.0 + (frequency / 1000.0).ln() / (6.4_f32.ln() / 27.0)
        } else {
            linear
        }
    };
    let mel_to_hz = |mel: f32| {
        if mel >= 15.0 {
            1000.0 * ((6.4_f32.ln() / 27.0) * (mel - 15.0)).exp()
        } else {
            mel * (200.0 / 3.0)
        }
    };
    let mel_max = hz_to_mel(8000.0);
    let points = (0..QwenMelExtractor::MELS + 2)
        .map(|index| mel_to_hz(mel_max * index as f32 / (QwenMelExtractor::MELS + 1) as f32))
        .collect::<Vec<_>>();

    (0..=QwenMelExtractor::FFT_SIZE / 2)
        .map(|frequency_index| {
            let frequency = frequency_index as f32 * 16000.0 / QwenMelExtractor::FFT_SIZE as f32;
            (0..QwenMelExtractor::MELS)
                .map(|mel| {
                    let left = points[mel];
                    let center = points[mel + 1];
                    let right = points[mel + 2];
                    let triangle = if frequency < left || frequency > right {
                        0.0
                    } else if frequency <= center {
                        (frequency - left) / (center - left)
                    } else {
                        (right - frequency) / (right - center)
                    };
                    triangle * 2.0 / (right - left)
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qwen_mel_has_expected_shape() {
        let extractor = QwenMelExtractor::new();
        let output = extractor.extract(&vec![0.0; 16_000]);
        assert_eq!(output.len(), 128 * 100);
        assert!(output.iter().all(|value| value.is_finite()));
    }
}
