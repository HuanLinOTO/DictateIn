use std::collections::VecDeque;

#[derive(Debug)]
pub struct StablePrefix {
    history: VecDeque<String>,
    capacity: usize,
}

impl StablePrefix {
    pub fn new(capacity: usize) -> Self {
        Self {
            history: VecDeque::with_capacity(capacity),
            capacity: capacity.max(2),
        }
    }

    pub fn push(&mut self, text: impl Into<String>) -> String {
        if self.history.len() == self.capacity {
            self.history.pop_front();
        }

        self.history.push_back(text.into());
        self.common_prefix()
    }

    fn common_prefix(&self) -> String {
        let mut values = self.history.iter();
        let Some(first) = values.next() else {
            return String::new();
        };

        let mut prefix = first.chars().collect::<Vec<_>>();
        for value in values {
            let common_length = prefix
                .iter()
                .zip(value.chars())
                .take_while(|(left, right)| left == &right)
                .count();
            prefix.truncate(common_length);
        }

        prefix.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_unicode_stable_prefix() {
        let mut prefix = StablePrefix::new(3);

        prefix.push("今天我们需要讨论");
        prefix.push("今天我们需要讨论一下");
        let stable = prefix.push("今天我们需要讨论一下模型");

        assert_eq!(stable, "今天我们需要讨论");
    }
}
