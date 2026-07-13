use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    pub config: PathBuf,
    pub models: PathBuf,
    pub logs: PathBuf,
    pub cache: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let executable = std::env::current_exe().context("failed to resolve executable path")?;
        let root = executable
            .parent()
            .context("executable path has no parent directory")?
            .to_path_buf();
        Ok(Self {
            config: root.join("config"),
            models: root.join("models"),
            logs: root.join("logs"),
            cache: root.join("cache"),
            root,
        })
    }

    pub fn ensure_directories(&self) -> Result<()> {
        fs::create_dir_all(&self.config)?;
        fs::create_dir_all(&self.models)?;
        fs::create_dir_all(&self.logs)?;
        fs::create_dir_all(&self.cache)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_relative_to_executable() {
        let paths = AppPaths::discover().unwrap();
        assert_eq!(paths.config.parent(), Some(paths.root.as_path()));
        assert_eq!(paths.models.parent(), Some(paths.root.as_path()));
        assert_eq!(paths.logs.parent(), Some(paths.root.as_path()));
    }
}
