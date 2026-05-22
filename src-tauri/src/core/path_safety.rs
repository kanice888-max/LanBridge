use anyhow::Result;
use std::path::{Component, Path, PathBuf};

pub fn safe_join(root: &Path, relative_path: &str) -> Result<PathBuf> {
    let path = Path::new(relative_path);
    let mut dest = root.to_path_buf();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                validate_safe_component(part)?;
                dest.push(part)
            }
            Component::CurDir => {}
            _ => anyhow::bail!("invalid relative path"),
        }
    }
    if dest == root {
        anyhow::bail!("empty relative path");
    }
    Ok(dest)
}

fn validate_safe_component(part: &std::ffi::OsStr) -> Result<()> {
    let name = part
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid non-utf8 relative path"))?;
    if name.is_empty() {
        anyhow::bail!("empty path component");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        anyhow::bail!("invalid trailing dot or space in path component");
    }
    if name
        .chars()
        .any(|ch| ch == '\0' || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        anyhow::bail!("invalid character in relative path");
    }

    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if matches!(
        stem.as_str(),
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
    ) {
        anyhow::bail!("reserved device name in relative path: {}", name);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_rejects_path_traversal() {
        let root = Path::new("/tmp/lanbridge-root");
        assert!(safe_join(root, "../escape.txt").is_err());
        assert!(safe_join(root, "/absolute.txt").is_err());
    }

    #[test]
    fn safe_join_rejects_windows_reserved_names() {
        let root = Path::new("/tmp/lanbridge-root");
        assert!(safe_join(root, "nested/CON.txt").is_err());
        assert!(safe_join(root, "LPT1").is_err());
    }

    #[test]
    fn safe_join_accepts_normal_relative_path() {
        let root = Path::new("/tmp/lanbridge-root");
        let path = safe_join(root, "nested/file.txt").unwrap();
        assert_eq!(path, root.join("nested").join("file.txt"));
    }
}
