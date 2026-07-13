use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::Settings;
use super::schema::CURRENT_SCHEMA_VERSION;
use super::validation::validate;
use crate::paths::AppPaths;

#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn discover() -> Result<Self> {
        let paths = AppPaths::discover()?;
        paths.ensure_directories()?;
        let path = paths.config.join("settings.toml");
        Ok(Self { path })
    }

    pub fn load_or_default(&self) -> Result<Settings> {
        if !self.path.exists() {
            return Ok(Settings::default());
        }

        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        let mut settings = toml::from_str::<Settings>(&content)
            .with_context(|| format!("failed to parse {}", self.path.display()))?;

        if settings.schema_version < CURRENT_SCHEMA_VERSION {
            let migrated = migrate(&mut settings);
            if migrated {
                let _ = self.save(&settings);
            }
        }

        validate(&settings)?;
        Ok(settings)
    }

    pub fn save(&self, settings: &Settings) -> Result<()> {
        validate(settings)?;
        let parent = self.path.parent().context("settings path has no parent")?;
        fs::create_dir_all(parent)?;

        let temporary_path = self.path.with_extension("toml.tmp");
        let content = toml::to_string_pretty(settings)?;
        let mut file = File::create(&temporary_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        replace_file(&temporary_path, &self.path)?;
        Ok(())
    }
}

#[cfg(windows)]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> Result<()> {
    fs::rename(source, destination)?;
    Ok(())
}

fn migrate(settings: &mut Settings) -> bool {
    let mut changed = false;

    if settings.schema_version < 2 {
        settings.hotwords = super::schema::HotwordSettings::default();
        changed = true;
    }

    settings.schema_version = CURRENT_SCHEMA_VERSION;
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_settings_file() {
        let directory =
            std::env::temp_dir().join(format!("dictate-in-settings-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let store = SettingsStore {
            path: directory.join("settings.toml"),
        };
        let mut settings = Settings::default();
        store.save(&settings).unwrap();
        settings.hotwords.items = vec!["second write".into()];
        store.save(&settings).unwrap();

        let loaded = store.load_or_default().unwrap();
        assert_eq!(loaded.hotwords.items, vec!["second write"]);
        fs::remove_dir_all(directory).unwrap();
    }
}
