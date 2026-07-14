use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};

use super::{clipboard, foreground, send_input};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Unicode,
    ClipboardPaste,
    CopyOnly,
}

impl OutputMode {
    pub fn parse(value: &str) -> Self {
        match value {
            "clipboard" | "paste" => Self::ClipboardPaste,
            "copy" | "copy_only" => Self::CopyOnly,
            _ => Self::Unicode,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unicode => "unicode",
            Self::ClipboardPaste => "clipboard",
            Self::CopyOnly => "copy",
        }
    }
}

#[derive(Debug)]
pub enum OutputCommand {
    Write { session_id: u64, text: String },
    SetMode(OutputMode),
    SetStripPunctuation(bool),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum OutputEvent {
    Written {
        session_id: u64,
    },
    Retained {
        session_id: u64,
        text: String,
        reason: String,
    },
}

pub struct OutputWorker {
    handle: Option<JoinHandle<()>>,
}

impl OutputWorker {
    pub fn spawn(
        commands: Receiver<OutputCommand>,
        events: Sender<OutputEvent>,
        mode: OutputMode,
        strip_punctuation: bool,
    ) -> Self {
        let handle = thread::Builder::new()
            .name("text-output".into())
            .spawn(move || run(commands, events, mode, strip_punctuation))
            .expect("failed to spawn output worker");
        Self {
            handle: Some(handle),
        }
    }

    pub fn join(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run(
    commands: Receiver<OutputCommand>,
    events: Sender<OutputEvent>,
    mut mode: OutputMode,
    mut strip_punctuation: bool,
) {
    while let Ok(command) = commands.recv() {
        match command {
            OutputCommand::Write { session_id, text } => {
                let text = if strip_punctuation {
                    strip_trailing_punctuation(&text)
                } else {
                    text
                };
                if text.is_empty() {
                    continue;
                }

                if mode == OutputMode::CopyOnly {
                    match clipboard::copy_text(&text) {
                        Ok(()) => {
                            let _ = events.send(OutputEvent::Written { session_id });
                        }
                        Err(error) => retain(&events, session_id, text, &error.to_string()),
                    }
                    continue;
                }

                let Some(target) = foreground::current() else {
                    retain(&events, session_id, text, "没有有效的前台窗口");
                    continue;
                };
                if target.process_id == std::process::id() {
                    retain(&events, session_id, text, "前台窗口属于 DictateIn");
                    continue;
                }

                let result = match mode {
                    OutputMode::Unicode => send_input::send_unicode(&text),
                    OutputMode::ClipboardPaste => clipboard::paste_text(&text),
                    OutputMode::CopyOnly => unreachable!(),
                };
                match result {
                    Ok(()) => {
                        let _ = events.send(OutputEvent::Written { session_id });
                    }
                    Err(error) => {
                        retain(&events, session_id, text, &error.to_string());
                    }
                }
            }
            OutputCommand::SetMode(new_mode) => {
                mode = new_mode;
            }
            OutputCommand::SetStripPunctuation(value) => {
                strip_punctuation = value;
            }
            OutputCommand::Shutdown => break,
        }
    }
}

fn retain(events: &Sender<OutputEvent>, session_id: u64, text: String, reason: &str) {
    let reason = match clipboard::copy_text(&text) {
        Ok(()) => format!("{reason}，结果已复制到剪贴板"),
        Err(copy_error) => format!("{reason}，剪贴板保留失败：{copy_error}"),
    };
    let _ = events.send(OutputEvent::Retained {
        session_id,
        text,
        reason,
    });
}

fn strip_trailing_punctuation(text: &str) -> String {
    text.trim_end()
        .trim_end_matches(|c: char| {
            matches!(c,
                '。' | '！' | '？' | '，' | '；' | '：' | '、' | '…' | '—' | '·' | '～'
                | '.' | '!' | '?' | ',' | ';' | ':'
            )
        })
        .to_string()
}
