//! Whether the tree in front of the daemon is the one that was registered (ticket 56, ADR 0011).
//!
//! An unmounted mount point is an ordinary empty directory: nothing about reading it fails, so a capture
//! of it records that every file was deleted, and over a month of such captures Retention ages the last
//! Snapshot with real content off the ladder — silently. So a Root's place on its volume is recorded when
//! it is registered and compared before every capture. The platform answers the questions; what the
//! answers mean is decided here.

use std::path::{Path, PathBuf};

use crate::git::Platform;

/// Where a Root sits: the persistent identity of the volume holding it, and its path within that
/// volume.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VolumePlace {
    /// The volume's identity as the platform reports it — a filesystem UUID, a volume GUID, a network
    /// share's source — which survives a reboot and a change of mount point.
    pub identity: String,
    /// The Root's path relative to the volume's mount point, `/`-separated whatever the platform, and
    /// empty for a Root that is the mount point itself.
    pub within: String,
}

impl VolumePlace {
    /// Where the Root is when its volume is mounted at `mount_point`.
    pub fn at(&self, mount_point: &Path) -> PathBuf {
        self.within
            .split('/')
            .filter(|part| !part.is_empty())
            .fold(mount_point.to_path_buf(), |path, part| path.join(part))
    }
}

/// What comparing a Root's recorded place with the one found says about the tree in front of the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeVerdict {
    /// The registered volume, at the registered place: an empty tree here is a deletion.
    Ours,
    /// Another volume, another place on it, or no volume that can be named: whatever is here is not the
    /// tree that was registered, and nothing may be captured from it.
    Elsewhere,
    /// The volume could not be identified when the Root was registered, so there is nothing to compare:
    /// capture proceeds, but one that would delete every path is held.
    Unproven,
}

/// What the place `found` now says, against the place `recorded` at registration.
pub fn verify_volume(recorded: Option<&VolumePlace>, found: Option<&VolumePlace>) -> VolumeVerdict {
    match recorded {
        None => VolumeVerdict::Unproven,
        Some(recorded) if found == Some(recorded) => VolumeVerdict::Ours,
        Some(_) => VolumeVerdict::Elsewhere,
    }
}

/// Whether the platform's watcher is documented as blind to changes on a filesystem of type `filesystem`,
/// which the platform reports as `remote` or not.
///
/// Exactly where it is, not everywhere a network is involved (ticket 29): on Linux inotify and fanotify
/// see no remote event, and FUSE emits none even for a change it knows about; on macOS FSEvents sees only
/// local writes; on Windows `ReadDirectoryChangesW` documents SMB as supported.
pub fn watch_blind(platform: Platform, filesystem: &str, remote: bool) -> bool {
    match platform {
        Platform::Linux => {
            remote
                || NETWORK_FILESYSTEMS.contains(&filesystem)
                || filesystem == "fuse"
                || filesystem.starts_with("fuse.")
        }
        Platform::MacOs => remote,
        Platform::Windows => false,
    }
}

/// The Linux filesystem types that are network filesystems: whatever changes on the server is never an
/// event here. `fuseblk` is not among them — a FUSE filesystem over a local block device is written
/// through this kernel.
const NETWORK_FILESYSTEMS: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "ncpfs",
    "9p",
    "ceph",
    "glusterfs",
    "afs",
    "lustre",
    "gpfs",
    "davfs",
    "virtiofs",
];
