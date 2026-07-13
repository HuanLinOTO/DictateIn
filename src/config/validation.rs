use anyhow::{Result, bail};

use super::Settings;
use super::schema::CURRENT_SCHEMA_VERSION;

pub fn validate(settings: &Settings) -> Result<()> {
    if settings.schema_version != CURRENT_SCHEMA_VERSION {
        bail!("unsupported settings schema version");
    }

    if settings.hotkey.keys.is_empty() {
        bail!("hotkey must contain at least one key");
    }

    if !(200..=5_000).contains(&settings.asr.partial_interval_ms) {
        bail!("partial interval must be between 200 and 5000 ms");
    }

    if !(0.1..=10.0).contains(&settings.hotwords.boost) {
        bail!("hotword boost must be between 0.1 and 10.0");
    }

    Ok(())
}
