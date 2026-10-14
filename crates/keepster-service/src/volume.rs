//! Which volume a path is on, and where a volume is mounted (ticket 56, ADR 0011).
//!
//! A Root's history must never take an unmounted mount point — an ordinary empty directory — for the
//! tree it was registered as, so the daemon records the volume's persistent identity at registration and
//! compares it before every capture. This is the platform half: each platform names a volume its own way,
//! and what a match or a mismatch means is `keepster_core`'s to decide.

use std::io;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// The volume a path is on, as the platform reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// The volume's persistent identity — `uuid:` and a filesystem UUID, `volume:` and a Windows volume
    /// GUID path, `remote:` and a network share's source — or `None` where the platform can name none,
    /// such as a `tmpfs` or an overlay.
    pub identity: Option<String>,
    /// Where the volume is mounted.
    pub mount_point: PathBuf,
    /// The path relative to the mount point, `/`-separated, empty for the mount point itself.
    pub within: String,
    /// The filesystem's type as the platform names it: `ext4`, `nfs4`, `fuse.sshfs`, `apfs`, `smbfs`,
    /// `NTFS`. Empty where the platform does not say.
    pub filesystem: String,
    /// Whether the platform reports the volume as remote.
    pub remote: bool,
}

/// The volume `path` is on.
///
/// The path must exist: a vanished one has no volume to name, which the caller already reports as a
/// missing Root.
pub fn volume_of(path: &Path) -> io::Result<Volume> {
    #[cfg(target_os = "linux")]
    {
        linux::volume_of(path)
    }
    #[cfg(target_os = "macos")]
    {
        macos::volume_of(path)
    }
    #[cfg(windows)]
    {
        windows::volume_of(path)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "volumes cannot be identified on this platform",
        ))
    }
}

/// Every place the volume `identity` is mounted right now — none while it is not.
pub fn mount_points(identity: &str) -> io::Result<Vec<PathBuf>> {
    #[cfg(target_os = "linux")]
    {
        linux::mount_points(identity)
    }
    #[cfg(target_os = "macos")]
    {
        macos::mount_points(identity)
    }
    #[cfg(windows)]
    {
        windows::mount_points(identity)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = identity;
        Ok(Vec::new())
    }
}

/// `path` relative to `mount_point`, `/`-separated, or `None` when it is not beneath it.
#[cfg(unix)]
fn within(path: &Path, mount_point: &Path) -> Option<String> {
    let relative = path.strip_prefix(mount_point).ok()?;

    Some(
        relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}
