use anyhow::Result;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct TaskRootHandle {
    root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct MutationGuard {
    directories: Vec<DirectoryIdentity>,
}

#[derive(Debug, Clone)]
struct DirectoryIdentity {
    path: PathBuf,
    identity: PlatformFileIdentity,
}

#[cfg(unix)]
type PlatformFileIdentity = (u64, u64);

#[cfg(windows)]
type PlatformFileIdentity = (u32, u64);

#[cfg(not(any(unix, windows)))]
type PlatformFileIdentity = PathBuf;

impl TaskRootHandle {
    pub fn new(root: &Path) -> Result<Self> {
        ensure_not_link_or_reparse(root)?;
        if !std::fs::metadata(root)?.is_dir() {
            anyhow::bail!("UnsafePath: task root is not a directory");
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    pub fn resolve(&self, relative_path: &str) -> Result<PathBuf> {
        safe_join(&self.root, relative_path)
    }

    pub fn prepare_mutation(&self, destination: &Path) -> Result<MutationGuard> {
        create_safe_parent_dirs(&self.root, destination)?;
        MutationGuard::capture(&self.root, destination)
    }
}

impl MutationGuard {
    fn capture(root: &Path, destination: &Path) -> Result<Self> {
        let parent = destination
            .parent()
            .ok_or_else(|| anyhow::anyhow!("destination has no parent"))?;
        let relative_parent = parent
            .strip_prefix(root)
            .map_err(|_| anyhow::anyhow!("PathEscapesTaskRoot"))?;
        let mut current = root.to_path_buf();
        let mut directories = vec![directory_identity(&current)?];
        for component in relative_parent.components() {
            let Component::Normal(part) = component else {
                anyhow::bail!("UnsafePath");
            };
            current.push(part);
            directories.push(directory_identity(&current)?);
        }
        Ok(Self { directories })
    }

    pub fn validate(&self) -> Result<()> {
        for expected in &self.directories {
            let current = directory_identity(&expected.path)?;
            if current.identity != expected.identity {
                anyhow::bail!("UnsafePath: parent directory identity changed");
            }
        }
        Ok(())
    }
}

fn directory_identity(path: &Path) -> Result<DirectoryIdentity> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure_metadata_not_link_or_reparse(&metadata)?;
    if !metadata.is_dir() {
        anyhow::bail!("UnsafePath: parent component is not a directory");
    }
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(windows)]
    let identity = windows_directory_identity(path)?;
    #[cfg(not(any(unix, windows)))]
    let identity = path.canonicalize()?;
    Ok(DirectoryIdentity {
        path: path.to_path_buf(),
        identity,
    })
}

#[cfg(windows)]
fn windows_directory_identity(path: &Path) -> Result<PlatformFileIdentity> {
    use std::mem::MaybeUninit;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    let directory = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let succeeded = unsafe {
        GetFileInformationByHandle(
            directory.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
            information.as_mut_ptr(),
        )
    };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let information = unsafe { information.assume_init() };
    let file_index = ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64;
    Ok((information.dwVolumeSerialNumber, file_index))
}

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

    validate_filesystem_boundary(root, &dest)?;
    Ok(dest)
}

/// Revalidate a previously resolved path immediately before a filesystem
/// mutation. Callers must not rely on a `PathBuf` that was validated only at
/// transfer start because an intermediate directory may have changed.
pub fn ensure_safe_for_mutation(root: &Path, destination: &Path) -> Result<()> {
    validate_filesystem_boundary(root, destination)
}

/// Create missing parent directories one component at a time and re-open the
/// trust boundary after each creation. Existing symlinks and Windows reparse
/// points are rejected rather than followed.
pub fn create_safe_parent_dirs(root: &Path, destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow::anyhow!("destination has no parent"))?;
    let relative_parent = parent
        .strip_prefix(root)
        .map_err(|_| anyhow::anyhow!("PathEscapesTaskRoot"))?;
    let mut current = root.to_path_buf();
    ensure_not_link_or_reparse(&current)?;
    for component in relative_parent.components() {
        let Component::Normal(part) = component else {
            anyhow::bail!("UnsafePath");
        };
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                ensure_metadata_not_link_or_reparse(&metadata)?;
                if !metadata.is_dir() {
                    anyhow::bail!("UnsafePath: parent component is not a directory");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)?;
                ensure_not_link_or_reparse(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
        validate_filesystem_boundary(root, &current)?;
    }
    validate_filesystem_boundary(root, destination)
}

fn validate_filesystem_boundary(root: &Path, destination: &Path) -> Result<()> {
    let Some(root_ancestor) = existing_ancestor(root)? else {
        // The caller may be preparing a new task root. Lexical validation above
        // still applies, and there is no filesystem boundary to inspect yet.
        return Ok(());
    };
    let canonical_root = root_ancestor
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("failed to canonicalize task root: {e}"))?;
    if root.exists() {
        ensure_not_link_or_reparse(root)?;
    }

    let existing_path = existing_ancestor(destination)?
        .ok_or_else(|| anyhow::anyhow!("unable to inspect destination path"))?;
    let canonical_existing = existing_path
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("failed to canonicalize destination: {e}"))?;
    if !canonical_existing.starts_with(&canonical_root) {
        anyhow::bail!("PathEscapesTaskRoot");
    }

    let mut current = root.to_path_buf();
    for component in destination
        .strip_prefix(root)
        .map_err(|_| anyhow::anyhow!("destination is outside task root"))?
        .components()
    {
        let Component::Normal(part) = component else {
            continue;
        };
        current.push(part);
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        };
        ensure_metadata_not_link_or_reparse(&metadata)?;
    }
    Ok(())
}

fn ensure_not_link_or_reparse(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure_metadata_not_link_or_reparse(&metadata)
}

fn ensure_metadata_not_link_or_reparse(metadata: &std::fs::Metadata) -> Result<()> {
    if metadata.file_type().is_symlink() {
        anyhow::bail!("SymlinkNotAllowed");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            anyhow::bail!("SymlinkNotAllowed: Windows reparse point");
        }
    }
    Ok(())
}

fn existing_ancestor(path: &Path) -> Result<Option<PathBuf>> {
    let mut current = path.to_path_buf();
    loop {
        if std::fs::symlink_metadata(&current).is_ok() {
            return Ok(Some(current));
        }
        if !current.pop() {
            return Ok(None);
        }
    }
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

    #[cfg(unix)]
    #[test]
    fn safe_join_rejects_symlinked_parent() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();

        assert!(safe_join(root.path(), "link/secret.txt").is_err());
    }

    #[test]
    fn safe_join_rejects_existing_destination_outside_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("secret.txt");
        std::fs::write(&outside_file, "secret").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_file, root.path().join("secret.txt")).unwrap();

        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&outside_file, root.path().join("secret.txt")).unwrap();

        assert!(safe_join(root.path(), "secret.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn mutation_revalidation_rejects_parent_replaced_by_symlink() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let parent = root.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        let destination = safe_join(root.path(), "parent/file.txt").unwrap();

        std::fs::remove_dir(&parent).unwrap();
        std::os::unix::fs::symlink(outside.path(), &parent).unwrap();

        assert!(ensure_safe_for_mutation(root.path(), &destination).is_err());
    }

    #[test]
    fn create_safe_parent_dirs_builds_only_inside_root() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("one/two/file.txt");
        create_safe_parent_dirs(root.path(), &destination).unwrap();
        assert!(root.path().join("one/two").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn mutation_guard_rejects_parent_replaced_by_another_directory() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        let handle = TaskRootHandle::new(root.path()).unwrap();
        let destination = handle.resolve("parent/file.txt").unwrap();
        let guard = handle.prepare_mutation(&destination).unwrap();

        std::fs::rename(&parent, root.path().join("old-parent")).unwrap();
        std::fs::create_dir(&parent).unwrap();

        assert!(guard.validate().is_err());
    }
}
