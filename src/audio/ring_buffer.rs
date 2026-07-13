use std::collections::VecDeque;

#[derive(Debug)]
pub struct AudioRingBuffer {
    samples: VecDeque<f32>,
    capacity: usize,
    dropped_samples: u64,
}

impl AudioRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
            dropped_samples: 0,
        }
    }

    pub fn push(&mut self, incoming: &[f32]) {
        let overflow = self
            .samples
            .len()
            .saturating_add(incoming.len())
            .saturating_sub(self.capacity);
        for _ in 0..overflow {
            self.samples.pop_front();
        }
        self.dropped_samples = self.dropped_samples.saturating_add(overflow as u64);

        let start = incoming.len().saturating_sub(self.capacity);
        self.samples.extend(&incoming[start..]);
    }

    pub fn drain(&mut self, count: usize) -> Vec<f32> {
        let count = count.min(self.samples.len());
        self.samples.drain(..count).collect()
    }

    pub fn drain_all(&mut self) -> Vec<f32> {
        self.samples.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    #[cfg(test)]
    pub fn dropped_samples(&self) -> u64 {
        self.dropped_samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_oldest_samples_on_overflow() {
        let mut buffer = AudioRingBuffer::new(4);
        buffer.push(&[1.0, 2.0, 3.0]);
        buffer.push(&[4.0, 5.0, 6.0]);

        assert_eq!(buffer.drain_all(), vec![3.0, 4.0, 5.0, 6.0]);
        assert_eq!(buffer.dropped_samples(), 2);
    }
}
