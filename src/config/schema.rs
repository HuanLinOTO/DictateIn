use serde::{Deserialize, Serialize};

use crate::asr::ModelKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub general: GeneralSettings,
    pub hotkey: HotkeySettings,
    pub audio: AudioSettings,
    pub asr: AsrSettings,
    pub hotwords: HotwordSettings,
    pub output: OutputSettings,
    pub overlay: OverlaySettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            general: GeneralSettings::default(),
            hotkey: HotkeySettings::default(),
            audio: AudioSettings::default(),
            asr: AsrSettings::default(),
            hotwords: HotwordSettings::default(),
            output: OutputSettings::default(),
            overlay: OverlaySettings::default(),
        }
    }
}

pub const CURRENT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    pub launch_at_login: bool,
    pub minimize_to_tray: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            minimize_to_tray: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeySettings {
    pub keys: Vec<String>,
    pub suppress: bool,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            keys: vec!["Ctrl".into(), "Space".into()],
            suppress: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub device_id: String,
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrSettings {
    pub model: ModelKind,
    pub provider: String,
    pub partial_interval_ms: u64,
}

impl Default for AsrSettings {
    fn default() -> Self {
        Self {
            model: ModelKind::SenseVoice,
            provider: "cpu".into(),
            partial_interval_ms: 800,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HotwordSettings {
    pub items: Vec<String>,
    pub boost: f32,
}

impl Default for HotwordSettings {
    fn default() -> Self {
        Self {
            items: vec![
                "CapsWriter".into(),
                "Paraformer".into(),
                "SenseVoice-small".into(),
                "Fun-ASR-Nano".into(),
                "Qwen3-ASR".into(),
                "Qwen3-TTS".into(),
                "Claude".into(),
                "Claude Code".into(),
                "Llama.cpp".into(),
                "CUDA".into(),
                "PyTorch".into(),
                "TensorRT".into(),
                "DirectML".into(),
                "TensorFlow".into(),
                "Transformer".into(),
            ],
            boost: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSettings {
    pub mode: String,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            mode: "unicode".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlaySettings {
    pub enabled: bool,
    pub monitor: String,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            monitor: "foreground".into(),
        }
    }
}
