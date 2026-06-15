use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::platform::traits::Platform;

const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    #[serde(default = "default_discovery_enabled")]
    pub discovery_enabled: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            discovery_enabled: default_discovery_enabled(),
        }
    }
}

fn default_discovery_enabled() -> bool {
    true
}

pub fn settings_path(platform: &dyn Platform) -> Result<PathBuf> {
    Ok(platform.app_data_dir()?.join(SETTINGS_FILE))
}

pub fn load(platform: &dyn Platform) -> Result<AppSettings> {
    load_from_path(&settings_path(platform)?)
}

pub fn save(platform: &dyn Platform, settings: &AppSettings) -> Result<()> {
    save_to_path(&settings_path(platform)?, settings)
}

fn load_from_path(path: &Path) -> Result<AppSettings> {
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let bytes = std::fs::read(path)?;
    let settings = serde_json::from_slice(&bytes)?;
    Ok(settings)
}

fn save_to_path(path: &Path, settings: &AppSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(settings)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_file_uses_safe_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let settings = load_from_path(&dir.path().join("settings.json")).unwrap();
        assert!(settings.discovery_enabled);
    }

    #[test]
    fn settings_roundtrip_preserves_discovery_preference() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let settings = AppSettings {
            discovery_enabled: false,
        };

        save_to_path(&path, &settings).unwrap();

        assert_eq!(load_from_path(&path).unwrap(), settings);
    }
}
