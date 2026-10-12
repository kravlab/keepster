//! The one stand-in the harness has: which volume a path is on (ticket 56).
//!
//! Unplugging a disk, or remounting one somewhere else, is not something a test can do to the machine it
//! runs on, so a test says what is mounted where instead. Everything else stays real: the paths are
//! real directories, and the daemon asks this exactly as it asks the platform.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use keepster_daemon::{Volume, Volumes};

/// Volumes as a test says they are.
#[derive(Debug, Default)]
pub struct FakeVolumes {
    /// What is mounted where, in the order it was mounted.
    mounts: Mutex<Vec<Mount>>,
}

/// One mounted volume.
#[derive(Debug, Clone)]
struct Mount {
    /// Where it is mounted.
    mount_point: PathBuf,
    /// Its identity, or `None` for a volume the platform cannot name.
    identity: Option<String>,
    /// Its filesystem's type.
    filesystem: String,
    /// Whether it is a network share.
    remote: bool,
}

impl FakeVolumes {
    /// Mount a local volume named `identity` — `None` for one the platform could not name — at
    /// `mount_point`, replacing whatever was mounted there.
    pub fn mount(&self, mount_point: &Path, identity: Option<&str>) {
        self.put(Mount {
            mount_point: mount_point.to_path_buf(),
            identity: identity.map(ToOwned::to_owned),
            filesystem: "testfs".to_owned(),
            remote: false,
        });
    }

    /// Mount a network share named `identity`, of the filesystem type `filesystem`, at `mount_point`.
    pub fn mount_network(&self, mount_point: &Path, filesystem: &str, identity: &str) {
        self.put(Mount {
            mount_point: mount_point.to_path_buf(),
            identity: Some(identity.to_owned()),
            filesystem: filesystem.to_owned(),
            remote: true,
        });
    }

    /// Unmount whatever is mounted at `mount_point`: what is beneath it is then on the volume beneath.
    pub fn unmount(&self, mount_point: &Path) {
        self.mounts()
            .retain(|mount| mount.mount_point != mount_point);
    }

    /// Mount `mount`, replacing whatever was mounted at its mount point.
    fn put(&self, mount: Mount) {
        let mut mounts = self.mounts();
        mounts.retain(|other| other.mount_point != mount.mount_point);
        mounts.push(mount);
    }

    /// What is mounted, however a test that panicked left it.
    fn mounts(&self) -> std::sync::MutexGuard<'_, Vec<Mount>> {
        self.mounts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Volumes for FakeVolumes {
    fn volume_of(&self, path: &Path) -> io::Result<Volume> {
        // As the platform does: a path that is not there has no volume.
        std::fs::metadata(path)?;
        let mounts = self.mounts();
        let mount = mounts
            .iter()
            .filter(|mount| path.starts_with(&mount.mount_point))
            .max_by_key(|mount| mount.mount_point.components().count())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "nothing is mounted there"))?;
        let within = path
            .strip_prefix(&mount.mount_point)
            .unwrap_or(Path::new(""))
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");

        Ok(Volume {
            identity: mount.identity.clone(),
            mount_point: mount.mount_point.clone(),
            within,
            filesystem: mount.filesystem.clone(),
            remote: mount.remote,
        })
    }

    fn mount_points(&self, identity: &str) -> io::Result<Vec<PathBuf>> {
        Ok(self
            .mounts()
            .iter()
            .filter(|mount| mount.identity.as_deref() == Some(identity))
            .map(|mount| mount.mount_point.clone())
            .collect())
    }
}
