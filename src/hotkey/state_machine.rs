use std::collections::BTreeSet;

use super::{HookEvent, HotkeyBinding};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    StartListening,
    StopListening,
}

#[derive(Debug)]
pub struct HotkeyStateMachine {
    binding: HotkeyBinding,
    pressed: BTreeSet<u32>,
    active: bool,
}

impl HotkeyStateMachine {
    pub fn new(binding: HotkeyBinding) -> Self {
        Self {
            binding,
            pressed: BTreeSet::new(),
            active: false,
        }
    }

    pub fn handle(&mut self, event: HookEvent) -> Option<HotkeyAction> {
        if event.injected {
            return None;
        }

        if event.pressed {
            self.pressed.insert(event.virtual_key);
            if !self.active && self.binding.is_satisfied_by(&self.pressed) {
                self.active = true;
                return Some(HotkeyAction::StartListening);
            }
            return None;
        }

        let was_binding_key = self.binding.contains(event.virtual_key);
        self.pressed.remove(&event.virtual_key);
        if self.active && was_binding_key {
            self.active = false;
            return Some(HotkeyAction::StopListening);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(virtual_key: u32, pressed: bool) -> HookEvent {
        HookEvent {
            virtual_key,
            pressed,
            injected: false,
        }
    }

    #[test]
    fn repeat_keydown_does_not_start_multiple_sessions() {
        let binding = HotkeyBinding::parse(&["Ctrl".into(), "Space".into()], false).unwrap();
        let mut state = HotkeyStateMachine::new(binding);

        assert_eq!(state.handle(event(0x11, true)), None);
        assert_eq!(
            state.handle(event(0x20, true)),
            Some(HotkeyAction::StartListening)
        );
        assert_eq!(state.handle(event(0x20, true)), None);
        assert_eq!(
            state.handle(event(0x20, false)),
            Some(HotkeyAction::StopListening)
        );
    }

    #[test]
    fn injected_events_are_ignored() {
        let binding = HotkeyBinding::parse(&["Space".into()], false).unwrap();
        let mut state = HotkeyStateMachine::new(binding);

        assert_eq!(
            state.handle(HookEvent {
                virtual_key: 0x20,
                pressed: true,
                injected: true,
            }),
            None
        );
    }
}
