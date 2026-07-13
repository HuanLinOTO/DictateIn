use std::time::Duration;

use anyhow::{Context, Result};
use arboard::Clipboard;

pub fn copy_text(text: &str) -> Result<()> {
    let mut clipboard = open_with_retry()?;
    clipboard
        .set_text(text)
        .context("failed to write Unicode clipboard text")
}

pub fn paste_text(text: &str) -> Result<()> {
    let mut clipboard = open_with_retry()?;
    let previous_text = clipboard.get_text().ok();
    clipboard
        .set_text(text)
        .context("failed to write temporary clipboard text")?;
    drop(clipboard);

    super::send_input::send_paste_shortcut()?;
    std::thread::sleep(Duration::from_millis(300));

    if let Some(previous_text) = previous_text
        && let Ok(mut clipboard) = open_with_retry()
    {
        let _ = clipboard.set_text(previous_text);
    }
    Ok(())
}

fn open_with_retry() -> Result<Clipboard> {
    let mut last_error = None;
    for delay in [10, 25, 50, 100] {
        match Clipboard::new() {
            Ok(clipboard) => return Ok(clipboard),
            Err(error) => {
                last_error = Some(error);
                std::thread::sleep(Duration::from_millis(delay));
            }
        }
    }
    Err(last_error.unwrap()).context("failed to open clipboard")
}
