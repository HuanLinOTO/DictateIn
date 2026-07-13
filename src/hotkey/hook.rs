use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossbeam_channel::{Sender, bounded};

use super::HotkeyBinding;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookEvent {
    pub virtual_key: u32,
    pub pressed: bool,
    pub injected: bool,
}

pub struct KeyboardHook {
    shutdown_sender: Sender<()>,
    binding: Arc<Mutex<HotkeyBinding>>,
    #[allow(dead_code)]
    suspended: Arc<AtomicBool>,
    capture_mode: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl KeyboardHook {
    pub fn install(event_sender: Sender<HookEvent>, binding: HotkeyBinding) -> Result<Self> {
        let binding = Arc::new(Mutex::new(binding));
        let suspended = Arc::new(AtomicBool::new(false));
        let capture_mode = Arc::new(AtomicBool::new(false));
        let (shutdown_sender, shutdown_receiver) = bounded(1);
        let binding_for_thread = Arc::clone(&binding);
        let suspended_for_thread = Arc::clone(&suspended);
        let capture_for_thread = Arc::clone(&capture_mode);
        let thread = std::thread::Builder::new()
            .name("keyboard-poller".into())
            .spawn(move || {
                run_poll_loop(
                    shutdown_receiver,
                    event_sender,
                    binding_for_thread,
                    suspended_for_thread,
                    capture_for_thread,
                )
            })?;
        Ok(Self {
            shutdown_sender,
            binding,
            suspended,
            capture_mode,
            thread: Some(thread),
        })
    }

    pub fn update_binding(&self, binding: HotkeyBinding) {
        if binding.suppress {
            tracing::warn!("快捷键轮询模式不支持屏蔽原按键");
        }
        if let Ok(mut current) = self.binding.lock() {
            *current = binding;
        }
    }

    #[allow(dead_code)]
    pub fn suspend(&self) {
        self.suspended.store(true, Ordering::Release);
    }

    #[allow(dead_code)]
    pub fn resume(&self) {
        self.suspended.store(false, Ordering::Release);
    }

    pub fn start_capture(&self) {
        self.capture_mode.store(true, Ordering::Release);
    }

    pub fn stop_capture(&self) {
        self.capture_mode.store(false, Ordering::Release);
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        let _ = self.shutdown_sender.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(windows)]
fn run_poll_loop(
    shutdown: crossbeam_channel::Receiver<()>,
    events: Sender<HookEvent>,
    binding: Arc<Mutex<HotkeyBinding>>,
    suspended: Arc<AtomicBool>,
    capture_mode: Arc<AtomicBool>,
) {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

    let mut previous = BTreeSet::new();
    loop {
        if shutdown.try_recv().is_ok() {
            break;
        }
        if suspended.load(Ordering::Acquire) {
            previous.clear();
            std::thread::sleep(std::time::Duration::from_millis(4));
            continue;
        }

        let current = if capture_mode.load(Ordering::Acquire) {
            (1u32..=254u32)
                .filter(|&key| unsafe { GetAsyncKeyState(key as i32) } < 0)
                .collect::<BTreeSet<_>>()
        } else {
            let keys = binding
                .lock()
                .map(|binding| binding.virtual_keys())
                .unwrap_or_default();
            keys.into_iter()
                .filter(|key| unsafe { GetAsyncKeyState(*key as i32) } < 0)
                .collect::<BTreeSet<_>>()
        };

        for key in current.difference(&previous) {
            let _ = events.try_send(HookEvent {
                virtual_key: *key,
                pressed: true,
                injected: false,
            });
        }
        for key in previous.difference(&current) {
            let _ = events.try_send(HookEvent {
                virtual_key: *key,
                pressed: false,
                injected: false,
            });
        }
        previous = current;
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
}

#[cfg(not(windows))]
fn run_poll_loop(
    shutdown: crossbeam_channel::Receiver<()>,
    _events: Sender<HookEvent>,
    _binding: Arc<Mutex<HotkeyBinding>>,
    _suspended: Arc<AtomicBool>,
    _capture_mode: Arc<AtomicBool>,
) {
    let _ = shutdown.recv();
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn poller_receives_key_down_and_up() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
        };

        let (sender, receiver) = bounded(8);
        let binding = HotkeyBinding::parse(&["A".into()], false).unwrap();
        let _hook = KeyboardHook::install(sender, binding).unwrap();
        let input = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(b'A' as u16),
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let down = [input(Default::default())];
        assert_eq!(
            unsafe { SendInput(&down, std::mem::size_of::<INPUT>() as i32) },
            1
        );
        let event = receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(event.pressed);

        let up = [input(KEYEVENTF_KEYUP)];
        assert_eq!(
            unsafe { SendInput(&up, std::mem::size_of::<INPUT>() as i32) },
            1
        );
        let event = receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(!event.pressed);
    }
}
