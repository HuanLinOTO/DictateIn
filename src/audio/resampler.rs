use anyhow::{Context, Result};
use rubato::{FftFixedInOut, Resampler};

pub struct AudioResampler {
    resampler: Option<FftFixedInOut<f32>>,
    pending: Vec<f32>,
    input_rate: u32,
    output_rate: u32,
}

impl AudioResampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Result<Self> {
        let resampler = if input_rate == output_rate {
            None
        } else {
            Some(
                FftFixedInOut::<f32>::new(input_rate as usize, output_rate as usize, 1024, 1)
                    .context("failed to create FFT resampler")?,
            )
        };

        Ok(Self {
            resampler,
            pending: Vec::new(),
            input_rate,
            output_rate,
        })
    }

    pub fn process(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(samples.to_vec());
        };

        self.pending.extend_from_slice(samples);
        let mut output = Vec::new();
        loop {
            let needed = resampler.input_frames_next();
            if self.pending.len() < needed {
                break;
            }

            let input = self.pending.drain(..needed).collect::<Vec<_>>();
            let resampled = resampler
                .process(&[input], None)
                .context("audio resampling failed")?;
            output.extend_from_slice(&resampled[0]);
        }
        Ok(output)
    }

    pub fn finish(&mut self) -> Result<Vec<f32>> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(std::mem::take(&mut self.pending));
        };
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }

        let input = vec![std::mem::take(&mut self.pending)];
        let resampled = resampler
            .process_partial(Some(&input), None)
            .context("failed to flush audio resampler")?;
        Ok(resampled.into_iter().next().unwrap_or_default())
    }

    pub fn rates(&self) -> (u32, u32) {
        (self.input_rate, self.output_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_to_expected_length_range() {
        let mut resampler = AudioResampler::new(48_000, 16_000).unwrap();
        let input = vec![0.25; 48_000];
        let mut output = resampler.process(&input).unwrap();
        output.extend(resampler.finish().unwrap());

        assert!((15_500..=16_500).contains(&output.len()));
        assert_eq!(resampler.rates(), (48_000, 16_000));
    }
}
