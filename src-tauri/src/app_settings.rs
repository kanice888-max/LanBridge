use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::platform::traits::Platform;

const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    #[serde(default = "default_discovery_enabled")]
    pub discovery_enabled: bool,
    #[serde(default)]
    pub last_update_check_unix_ms: Option<i64>,
    #[serde(default)]
    pub latest_release: Option<CachedUpdateRelease>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedUpdateRelease {
    pub version: String,
    pub tag_name: String,
    pub name: Option<String>,
    pub published_at: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            discovery_enabled: default_discovery_enabled(),
            last_update_check_unix_ms: None,
            latest_release: None,
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
            last_update_check_unix_ms: Some(1_725_000_000_000),
            latest_release: Some(CachedUpdateRelease {
                version: "0.2.0-beta.1".to_string(),
                tag_name: "v0.2.0-beta.1".to_string(),
                name: Some("Beta".to_string()),
                published_at: Some("2026-07-19T00:00:00Z".to_string()),
            }),
        };

        save_to_path(&path, &settings).unwrap();

        assert_eq!(load_from_path(&path).unwrap(), settings);
    }

    #[test]
    fn old_settings_file_uses_update_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{ "discovery_enabled": false }"#).unwrap();

        let settings = load_from_path(&path).unwrap();

        assert!(!settings.discovery_enabled);
        assert_eq!(settings.last_update_check_unix_ms, None);
        assert_eq!(settings.latest_release, None);
    }
}
