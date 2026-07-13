use anyhow::Result;

pub struct SingleInstance {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

impl SingleInstance {
    #[cfg(windows)]
    pub fn acquire() -> Result<Self> {
        use anyhow::bail;
        use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::CreateMutexW;
        use windows::core::w;

        let handle = unsafe { CreateMutexW(None, true, w!("Local\\DictateIn.SingleInstance"))? };

        let last_error = unsafe { windows::Win32::Foundation::GetLastError() };
        if last_error == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(handle)?;
            }
            bail!("DictateIn is already running");
        }

        Ok(Self { handle })
    }

    #[cfg(not(windows))]
    pub fn acquire() -> Result<Self> {
        Ok(Self {})
    }
}

#[cfg(windows)]
impl Drop for SingleInstance {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;

        let _ = unsafe { CloseHandle(self.handle) };
    }
}
