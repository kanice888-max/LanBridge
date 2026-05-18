use anyhow::{bail, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::platform::traits::{IgnoreDecision, Platform};

use super::fs_rules;

/// macOS platform implementation.
pub struct MacPlatform {
    app_data: PathBuf,
}

impl MacPlatform {
    pub fn new() -> Result<Self> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        let app_data = PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("LanBridge");
        Ok(Self { app_data })
    }

    pub fn with_data_dir(dir: PathBuf) -> Self {
        Self { app_data: dir }
    }
}

impl Platform for MacPlatform {
    fn app_data_dir(&self) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.app_data)?;
        Ok(self.app_data.clone())
    }

    fn database_path(&self) -> Result<PathBuf> {
        let dir = self.app_data_dir()?;
        Ok(dir.join("lanbridge.db"))
    }

    fn identity_key_path(&self) -> Result<PathBuf> {
        let dir = self.app_data_dir()?;
        Ok(dir.join("identity.key"))
    }

    fn peer_pins_path(&self) -> Result<PathBuf> {
        let dir = self.app_data_dir()?;
        Ok(dir.join("peer-pins.json"))
    }

    fn log_path(&self) -> Result<PathBuf> {
        let dir = self.app_data_dir()?;
        Ok(dir.join("lanbridge.log"))
    }

    fn normalize_relative_path(&self, path: &str) -> String {
        path.replace('\\', "/")
    }

    fn validate_sync_root(&self, path: &Path) -> Result<PathBuf> {
        // Canonicalize to resolve symlinks and prevent escape
        let canonical = path.canonicalize().map_err(|e| {
            anyhow::anyhow!("cannot canonicalize sync root '{}': {}", path.display(), e)
        })?;

        if !canonical.is_dir() {
            bail!("sync root '{}' is not a directory", canonical.display());
        }

        Ok(canonical)
    }

    fn validate_target_relative_path(&self, relative_path: &str) -> Result<()> {
        if relative_path.is_empty() {
            bail!("relative path cannot be empty");
        }

        if relative_path.starts_with('/') || relative_path.starts_with('\\') {
            bail!(
                "relative path cannot start with separator: {}",
                relative_path
            );
        }

        // Check for path traversal
        for component in relative_path.split(|c| c == '/' || c == '\\') {
            if component == ".." {
                bail!("path traversal not allowed: {}", relative_path);
            }
            if component.is_empty() {
                continue;
            }
            // macOS allows most characters, but check for null byte
            if component.contains('\0') {
                bail!("null byte in path component: {}", component);
            }
        }

        Ok(())
    }

    fn classify_ignored_entry(&self, entry_name: &str, is_dir: bool) -> IgnoreDecision {
        fs_rules::classify_entry(entry_name, is_dir)
    }

    fn detect_case_collisions(&self, paths: &[String]) -> Vec<Vec<String>> {
        let mut lower_map: HashMap<String, Vec<String>> = HashMap::new();

        for path in paths {
            let lower = path.to_lowercase();
            lower_map.entry(lower).or_default().push(path.clone());
        }

        lower_map
            .into_values()
            .filter(|group| group.len() > 1)
            .collect()
    }

    fn start_watcher(
        &self,
        sync_root: &Path,
    ) -> Result<(
        notify::RecommendedWatcher,
        std::sync::mpsc::Receiver<crate::platform::traits::PlatformWatcherEvent>,
    )> {
        super::watcher::start_watcher(sync_root)
    }
}
