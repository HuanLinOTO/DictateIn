const CALIBRATION_MS: u64 = 200;
const SPEECH_CONFIRM_MS: u64 = 80;
const SEGMENT_SILENCE_MS: u64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilenceState {
    Waiting,
    SpeechStarted,
    Speaking,
    SegmentEnded,
}

#[derive(Debug)]
pub struct SilenceDetector {
    sample_rate: u32,
    calibration_samples: u64,
    noise_floor: f32,
    candidate_samples: u64,
    voiced_samples: u64,
    silent_samples: u64,
    speaking: bool,
}

impl SilenceDetector {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            calibration_samples: 0,
            noise_floor: 0.003,
            candidate_samples: 0,
            voiced_samples: 0,
            silent_samples: 0,
            speaking: false,
        }
    }

    pub fn observe(&mut self, rms: f32, sample_count: usize) -> SilenceState {
        let sample_count = sample_count as u64;
        self.calibration_samples = self.calibration_samples.saturating_add(sample_count);
        let speech_threshold = (self.noise_floor * 1.8).clamp(0.005, 0.025);
        let release_threshold = speech_threshold * 0.7;

        if !self.speaking {
            if rms >= speech_threshold {
                self.candidate_samples = self.candidate_samples.saturating_add(sample_count);
                if self.candidate_samples >= self.samples_for_ms(SPEECH_CONFIRM_MS) {
                    self.speaking = true;
                    self.voiced_samples = self.candidate_samples;
                    self.candidate_samples = 0;
                    return SilenceState::SpeechStarted;
                }
            } else {
                self.candidate_samples = 0;
                self.update_noise_floor(rms, 0.05);
            }
            return SilenceState::Waiting;
        }

        if rms >= release_threshold {
            self.voiced_samples = self.voiced_samples.saturating_add(sample_count);
            self.silent_samples = 0;
            return SilenceState::Speaking;
        }

        self.silent_samples = self.silent_samples.saturating_add(sample_count);
        if self.silent_samples >= self.samples_for_ms(SEGMENT_SILENCE_MS) {
            self.speaking = false;
            self.candidate_samples = 0;
            self.voiced_samples = 0;
            self.silent_samples = 0;
            return SilenceState::SegmentEnded;
        }

        SilenceState::Speaking
    }

    #[cfg(test)]
    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    pub fn should_flush_tail(&self) -> bool {
        self.speaking
            || self.candidate_samples > 0
            || self.calibration_samples < self.samples_for_ms(CALIBRATION_MS)
    }

    fn samples_for_ms(&self, milliseconds: u64) -> u64 {
        self.sample_rate as u64 * milliseconds / 1000
    }

    fn update_noise_floor(&mut self, rms: f32, alpha: f32) {
        self.noise_floor += (rms.clamp(0.0001, 0.012) - self.noise_floor) * alpha;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK_SAMPLES: usize = 160;

    fn feed(detector: &mut SilenceDetector, rms: f32, blocks: usize) -> Vec<SilenceState> {
        (0..blocks)
            .map(|_| detector.observe(rms, BLOCK_SAMPLES))
            .collect()
    }

    #[test]
    fn ends_segment_after_six_hundred_ms_of_silence() {
        let mut detector = SilenceDetector::new(16_000);
        feed(&mut detector, 0.002, 20);
        feed(&mut detector, 0.08, 40);

        let states = feed(&mut detector, 0.001, 60);

        assert_eq!(states.last(), Some(&SilenceState::SegmentEnded));
    }

    #[test]
    fn pure_silence_never_ends_a_segment() {
        let mut detector = SilenceDetector::new(16_000);

        let states = feed(&mut detector, 0.002, 200);

        assert!(!states.contains(&SilenceState::SegmentEnded));
    }

    #[test]
    fn short_noise_burst_does_not_start_speech() {
        let mut detector = SilenceDetector::new(16_000);
        feed(&mut detector, 0.002, 20);

        let states = feed(&mut detector, 0.08, 4);

        assert!(!states.contains(&SilenceState::SpeechStarted));
        assert!(!detector.is_speaking());
    }

    #[test]
    fn immediate_speech_is_not_consumed_by_calibration() {
        let mut detector = SilenceDetector::new(16_000);

        let states = feed(&mut detector, 0.01, 20);

        assert!(states.contains(&SilenceState::SpeechStarted));
        assert!(detector.is_speaking());
    }

    #[test]
    fn short_confirmed_utterance_still_reaches_a_boundary() {
        let mut detector = SilenceDetector::new(16_000);
        feed(&mut detector, 0.002, 20);
        feed(&mut detector, 0.08, 10);

        let states = feed(&mut detector, 0.001, 60);

        assert_eq!(states.last(), Some(&SilenceState::SegmentEnded));
    }
}
