pub fn mix_interleaved_to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }

    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

pub fn i16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples
        .iter()
        .map(|sample| *sample as f32 / i16::MAX as f32)
        .collect()
}

pub fn u16_to_f32(samples: &[u16]) -> Vec<f32> {
    samples
        .iter()
        .map(|sample| (*sample as f32 - 32768.0) / 32768.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_stereo_frames() {
        let mono = mix_interleaved_to_mono(&[1.0, -1.0, 0.5, 0.5], 2);
        assert_eq!(mono, vec![0.0, 0.5]);
    }

    #[test]
    fn converts_integer_extremes() {
        let signed = i16_to_f32(&[i16::MIN, 0, i16::MAX]);
        assert!(signed[0] <= -1.0);
        assert_eq!(signed[1], 0.0);
        assert_eq!(signed[2], 1.0);

        let unsigned = u16_to_f32(&[0, 32768, u16::MAX]);
        assert_eq!(unsigned[0], -1.0);
        assert_eq!(unsigned[1], 0.0);
        assert!(unsigned[2] < 1.0);
    }
}
