use anyhow::Result;

#[cfg(windows)]
pub fn send_unicode(text: &str) -> Result<()> {
    use anyhow::bail;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput,
        VIRTUAL_KEY,
    };

    let mut inputs = Vec::with_capacity(text.encode_utf16().count() * 2);
    for code_unit in text.encode_utf16() {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: code_unit,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: code_unit,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }

    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        bail!("SendInput sent {sent} of {} events", inputs.len());
    }
    Ok(())
}

#[cfg(windows)]
pub fn send_paste_shortcut() -> Result<()> {
    use anyhow::bail;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
        VK_CONTROL,
    };

    let key = |virtual_key: VIRTUAL_KEY, key_up| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: 0,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let v = VIRTUAL_KEY(b'V' as u16);
    let inputs = [
        key(VK_CONTROL, false),
        key(v, false),
        key(v, true),
        key(VK_CONTROL, true),
    ];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        bail!("failed to send Ctrl+V shortcut");
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn send_unicode(_text: &str) -> Result<()> {
    Ok(())
}

#[cfg(not(windows))]
pub fn send_paste_shortcut() -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn non_bmp_text_encodes_as_surrogate_pair() {
        let units = "A😀中".encode_utf16().collect::<Vec<_>>();
        assert_eq!(units.len(), 4);
        assert_eq!(units[1], 0xD83D);
        assert_eq!(units[2], 0xDE00);
    }
}
