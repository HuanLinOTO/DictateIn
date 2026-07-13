use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};

#[derive(Debug, Clone)]
pub struct InputDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub fn list_input_devices() -> Result<Vec<InputDeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let mut devices = Vec::new();

    for (index, device) in host.input_devices()?.enumerate() {
        let name = device
            .name()
            .unwrap_or_else(|_| format!("Input device {}", index + 1));
        devices.push(InputDeviceInfo {
            id: format!("{}::{index}", name),
            is_default: default_name.as_deref() == Some(name.as_str()),
            name,
        });
    }

    Ok(devices)
}
