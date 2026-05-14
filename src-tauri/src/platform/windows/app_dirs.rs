use anyhow::{bail, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::platform::traits::{IgnoreDecision, Platform};

use super::fs_rules;

/// Windows platform implementation.
pub struct WinPlatform {
    app_data: PathBuf,
}

impl WinPlatform {
    pub fn new() -> Result<Self> {
        let app_data = std::env::var("APPDATA")
            .map(PathBuf::from)
            .or_else(|_| {
                let home = std::env::var("USERPROFILE")
                    .or_else(|_| std::env::var("HOME"))
                    .map_err(|_| anyhow::anyhow!("APPDATA and USERPROFILE not set"))?;
                Ok::<PathBuf, anyhow::Error>(PathBuf::from(home).join("AppData").join("Roaming"))
            })?
            .join("LanBridge");
        Ok(Self { app_data })
    }

    pub fn with_data_dir(dir: PathBuf) -> Self {
        Self { app_data: dir }
    }
}

impl Platform for WinPlatform {
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
        let canonical = path.canonicalize().map_err(|e| {
            anyhow::anyhow!("cannot canonicalize sync root '{}': {}", path.display(), e)
        })?;

        if !canonical.is_dir() {
            bail!("sync root '{}' is not a directory", canonical.display());
        }

        // Reject drive roots (e.g., C:\, D:\)
        // On Windows, a canonical drive root has no parent directory.
        if canonical.parent().is_none() {
            bail!(
                "drive roots are not allowed as sync roots: {}",
                canonical.display()
            );
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

        // Windows invalid characters
        const INVALID_CHARS: &[char] = &['<', '>', ':', '"', '|', '?', '*'];

        for component in relative_path.split(|c| c == '/' || c == '\\') {
            if component == ".." {
                bail!("path traversal not allowed: {}", relative_path);
            }
            if component.is_empty() || component == "." {
                continue;
            }

            // Check for null byte
            if component.contains('\0') {
                bail!("null byte in path component: {}", component);
            }

            // Check for invalid Windows characters
            for ch in component.chars() {
                if INVALID_CHARS.contains(&ch) || (ch as u32) < 32 {
                    bail!("invalid character '{}' in path: {}", ch, relative_path);
                }
            }

            // Check trailing spaces and dots
            if component.ends_with(' ') || component.ends_with('.') {
                bail!("path component cannot end with space or dot: {}", component);
            }

            // Check reserved device names (case-insensitive)
            let stem = component.split('.').next().unwrap_or(component);
            if is_reserved_name(stem) {
                bail!(
                    "reserved device name '{}' in path: {}",
                    component,
                    relative_path
                );
            }

            // Check path length
            if component.len() > 255 {
                bail!(
                    "path component too long ({} chars): {}",
                    component.len(),
                    component
                );
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
}

/// Check if a name is a Windows reserved device name.
fn is_reserved_name(name: &str) -> bool {
    let upper = name.to_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reserved_names() {
        assert!(is_reserved_name("CON"));
        assert!(is_reserved_name("con"));
        assert!(is_reserved_name("LPT1"));
        assert!(is_reserved_name("lpt1"));
        assert!(!is_reserved_name("COM10"));
        assert!(!is_reserved_name("COM"));
        assert!(!is_reserved_name("readme"));
    }

    #[test]
    fn test_validate_path_invalid_chars() {
        let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp"));
        assert!(platform.validate_target_relative_path("file<name").is_err());
        assert!(platform.validate_target_relative_path("file|name").is_err());
        assert!(platform.validate_target_relative_path("file*name").is_err());
        assert!(platform.validate_target_relative_path("file?name").is_err());
    }

    #[test]
    fn test_validate_path_reserved_names() {
        let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp"));
        assert!(platform.validate_target_relative_path("CON").is_err());
        assert!(platform.validate_target_relative_path("con.txt").is_err());
        assert!(platform.validate_target_relative_path("LPT1").is_err());
        assert!(platform.validate_target_relative_path("COM9.log").is_err());
    }

    #[test]
    fn test_validate_path_traversal() {
        let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp"));
        assert!(platform
            .validate_target_relative_path("../etc/passwd")
            .is_err());
        assert!(platform.validate_target_relative_path("a/../b").is_err());
    }

    #[test]
    fn test_validate_path_trailing_dot_space() {
        let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp"));
        assert!(platform.validate_target_relative_path("file.").is_err());
        assert!(platform.validate_target_relative_path("file ").is_err());
    }

    #[test]
    fn test_validate_path_valid() {
        let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp"));
        assert!(platform.validate_target_relative_path("readme.txt").is_ok());
        assert!(platform
            .validate_target_relative_path("docs/report.pdf")
            .is_ok());
        assert!(platform.validate_target_relative_path("a/b/c").is_ok());
    }

    #[test]
    fn test_new_missing_env_vars() {
        // When APPDATA, USERPROFILE, HOME are all unset, new() should fail
        let orig_appdata = std::env::var("APPDATA").ok();
        let orig_userprofile = std::env::var("USERPROFILE").ok();
        let orig_home = std::env::var("HOME").ok();

        std::env::remove_var("APPDATA");
        std::env::remove_var("USERPROFILE");
        std::env::remove_var("HOME");

        let result = WinPlatform::new();
        assert!(result.is_err(), "should fail when no env vars are set");

        // Restore env vars
        if let Some(v) = orig_appdata {
            std::env::set_var("APPDATA", v);
        }
        if let Some(v) = orig_userprofile {
            std::env::set_var("USERPROFILE", v);
        }
        if let Some(v) = orig_home {
            std::env::set_var("HOME", v);
        }
    }

    #[test]
    fn test_validate_path_component_too_long() {
        let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp"));
        let long_name = "a".repeat(256);
        assert!(platform.validate_target_relative_path(&long_name).is_err());

        let ok_name = "a".repeat(255);
        assert!(platform.validate_target_relative_path(&ok_name).is_ok());
    }

    #[test]
    fn test_validate_path_null_byte_in_component() {
        let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp"));
        assert!(platform.validate_target_relative_path("a\0b").is_err());
    }
}
