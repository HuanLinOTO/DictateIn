#[derive(Debug, Clone)]
pub struct ForegroundTarget {
    pub process_id: u32,
}

#[cfg(windows)]
pub fn current() -> Option<ForegroundTarget> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return None;
    }

    let mut process_id = 0;
    unsafe {
        GetWindowThreadProcessId(window, Some(&mut process_id));
    }
    if process_id == 0 {
        return None;
    }

    Some(ForegroundTarget { process_id })
}

#[cfg(not(windows))]
pub fn current() -> Option<ForegroundTarget> {
    None
}
