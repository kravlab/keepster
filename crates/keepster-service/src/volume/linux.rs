//! Linux: the mount a path is on, from `/proc/self/mountinfo`, and its filesystem UUID, from the
//! `/dev/disk/by-uuid` links udev keeps.
//!
//! The mount is found by the device number the path's `stat` reports, which mountinfo carries too, and
//! the UUID by the device number its link resolves to — never by comparing names, since a mount source
//! such as `/dev/root` need not exist as a file at all.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::Volume;
use crate::mountinfo::{Mount, PATH as MOUNTINFO, mount_of, parse};

/// Where udev links each filesystem by its UUID.
const BY_UUID: &str = "/dev/disk/by-uuid";

/// The volume `path` is on: the deepest mount holding it, by the device its `stat` reports.
pub(super) fn volume_of(path: &Path) -> io::Result<Volume> {
    let real = fs::canonicalize(path)?;
    let device = fs::metadata(&real)?.dev();
    let mounts = parse(&fs::read(MOUNTINFO)?);
    let mount = mount_of(&mounts, &real, device)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no mount holds the path"))?;
    let within = super::within(&real, &mount.mount_point).unwrap_or_default();

    Ok(Volume {
        identity: identity_of(mount),
        mount_point: mount.mount_point.clone(),
        within,
        filesystem: mount.filesystem.clone(),
        remote: is_share(&mount.source),
    })
}

/// Every mount of the volume `identity`, by the identity each mount line works out to.
pub(super) fn mount_points(identity: &str) -> io::Result<Vec<PathBuf>> {
    let mounts = parse(&fs::read(MOUNTINFO)?);

    Ok(mounts
        .iter()
        .filter(|mount| identity_of(mount).as_deref() == Some(identity))
        .map(|mount| mount.mount_point.clone())
        .collect())
}

/// The persistent identity of what `mount` mounts: the filesystem's, and which part of it is mounted.
///
/// A bind mount of a subdirectory, or a btrfs subvolume, shows only part of a filesystem, and two such
/// parts of one filesystem must never be taken for each other: the part is in the identity.
fn identity_of(mount: &Mount) -> Option<String> {
    filesystem_identity(mount).map(|filesystem| part_of(filesystem, &mount.root))
}

/// `filesystem`'s identity narrowed to the part of it mounted from `root`.
fn part_of(filesystem: String, root: &str) -> String {
    if root == "/" {
        filesystem
    } else {
        format!("{filesystem}:{root}")
    }
}

/// The persistent identity of the filesystem `mount` is of: its UUID, a network share's source, or
/// nothing for a filesystem that has neither.
fn filesystem_identity(mount: &Mount) -> Option<String> {
    if is_share(&mount.source) {
        return Some(format!("remote:{}", mount.source));
    }
    if !mount.source.starts_with("/dev/") {
        return None;
    }
    // The device the mount line names, and the one its source resolves to: a btrfs mount carries an
    // anonymous device number, and its source is the block device.
    let source = fs::metadata(&mount.source).map(|found| found.rdev()).ok();
    let devices = [Some(mount.device), source];

    fs::read_dir(BY_UUID).ok()?.flatten().find_map(|link| {
        let target = fs::metadata(link.path()).ok()?.rdev();
        devices
            .contains(&Some(target))
            .then(|| format!("uuid:{}", link.file_name().to_string_lossy()))
    })
}

/// Whether a mount source names a network share: `host:/export` for NFS, `//host/share` for SMB.
fn is_share(source: &str) -> bool {
    source.starts_with("//") || (!source.starts_with('/') && source.contains(":/"))
}

#[cfg(test)]
mod tests {
    use super::{is_share, part_of};

    #[test]
    fn two_parts_of_one_filesystem_are_never_the_same_volume() {
        // btrfs subvolumes `@` and `@home`, both of one UUID.
        let root = part_of("uuid:1234".to_owned(), "/@");
        let home = part_of("uuid:1234".to_owned(), "/@home");

        assert_ne!(root, home);
        assert_eq!(
            part_of("uuid:1234".to_owned(), "/"),
            "uuid:1234",
            "the whole of it"
        );
    }

    #[test]
    fn a_share_is_told_from_a_device_and_a_pseudo_filesystem() {
        assert!(is_share("server:/export/home"));
        assert!(is_share("//server/share"));
        assert!(!is_share("/dev/sda1"));
        assert!(!is_share("tmpfs"));
        assert!(!is_share("overlay"));
    }
}
