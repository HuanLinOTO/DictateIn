use std::collections::BTreeSet;

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBinding {
    keys: BTreeSet<u32>,
    pub suppress: bool,
}

impl HotkeyBinding {
    pub fn parse(keys: &[String], suppress: bool) -> Result<Self> {
        let keys = keys
            .iter()
            .map(|key| virtual_key(key))
            .collect::<Result<BTreeSet<_>>>()?;

        if keys.is_empty() {
            bail!("hotkey must contain at least one key");
        }

        Ok(Self { keys, suppress })
    }

    pub fn contains(&self, virtual_key: u32) -> bool {
        self.keys.contains(&virtual_key)
    }

    pub fn is_satisfied_by(&self, pressed: &BTreeSet<u32>) -> bool {
        self.keys.iter().all(|key| pressed.contains(key))
    }

    pub fn virtual_keys(&self) -> Vec<u32> {
        self.keys.iter().copied().collect()
    }
}

fn virtual_key(name: &str) -> Result<u32> {
    let normalized = name.trim().to_ascii_lowercase();
    let key = match normalized.as_str() {
        "ctrl" | "control" => 0x11,
        "shift" => 0x10,
        "alt" => 0x12,
        "space" => 0x20,
        "capslock" | "caps lock" => 0x14,
        "xbutton1" | "x1" | "mouse4" | "side1" | "侧键1" | "鼠标侧键1" => 0x05,
        "xbutton2" | "x2" | "mouse5" | "side2" | "侧键2" | "鼠标侧键2" => 0x06,
        "middle" | "mbutton" | "mouse3" | "中键" | "鼠标中键" => 0x04,
        value if value.len() == 1 => {
            let character = value.chars().next().unwrap();
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase() as u32
            } else {
                bail!("unsupported hotkey key: {name}");
            }
        }
        _ => bail!("unsupported hotkey key: {name}"),
    };
    Ok(key)
}

pub fn vk_to_display_name(vk: u32) -> Option<String> {
    match vk {
        0x05 => Some("XButton1".into()),
        0x06 => Some("XButton2".into()),
        0x04 => Some("Middle".into()),
        0x01 => Some("Left".into()),
        0x02 => Some("Right".into()),
        0x11 => Some("Ctrl".into()),
        0x10 => Some("Shift".into()),
        0x12 => Some("Alt".into()),
        0x20 => Some("Space".into()),
        0x14 => Some("CapsLock".into()),
        0x08 => Some("Backspace".into()),
        0x09 => Some("Tab".into()),
        0x0D => Some("Enter".into()),
        0x1B => Some("Esc".into()),
        0x21 => Some("PageUp".into()),
        0x22 => Some("PageDown".into()),
        0x23 => Some("End".into()),
        0x24 => Some("Home".into()),
        0x25 => Some("Left".into()),
        0x26 => Some("Up".into()),
        0x27 => Some("Right".into()),
        0x28 => Some("Down".into()),
        0x2D => Some("Insert".into()),
        0x2E => Some("Delete".into()),
        v if (0x30..=0x5A).contains(&v) => {
            Some((v as u8 as char).to_string())
        }
        v if (0x70..=0x7B).contains(&v) => Some(format!("F{}", v - 0x6F)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_binding() {
        let binding = HotkeyBinding::parse(&["Ctrl".into(), "Space".into()], false).unwrap();

        assert!(binding.contains(0x11));
        assert!(binding.contains(0x20));
    }

    #[test]
    fn parses_mouse_side_buttons() {
        let binding = HotkeyBinding::parse(&["XButton1".into()], false).unwrap();
        assert!(binding.contains(0x05));

        let binding = HotkeyBinding::parse(&["XButton2".into()], false).unwrap();
        assert!(binding.contains(0x06));

        let binding = HotkeyBinding::parse(&["Ctrl".into(), "侧键1".into()], false).unwrap();
        assert!(binding.contains(0x11));
        assert!(binding.contains(0x05));
    }

    #[test]
    fn vk_to_display_name_roundtrips() {
        assert_eq!(vk_to_display_name(0x05).as_deref(), Some("XButton1"));
        assert_eq!(vk_to_display_name(0x06).as_deref(), Some("XButton2"));
        assert_eq!(vk_to_display_name(0x11).as_deref(), Some("Ctrl"));
        assert_eq!(vk_to_display_name(0x41).as_deref(), Some("A"));
    }
}
