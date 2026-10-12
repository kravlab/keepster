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

/// Where udev links each filesystem by its UUID.
const BY_UUID: &str = "/dev/disk/by-uuid";

/// One line of `/proc/self/mountinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Mount {
    /// The device the mount is of, as `stat` reports it.
    pub(super) device: u64,
    /// Which directory of the filesystem is mounted: `/` for the whole of it, a subdirectory for a bind
    /// mount.
    pub(super) root: String,
    /// Where it is mounted.
    pub(super) mount_point: PathBuf,
    /// The filesystem type.
    pub(super) filesystem: String,
    /// What is mounted: a device, a share, or a name a pseudo-filesystem gives itself.
    pub(super) source: String,
}

/// The volume `path` is on: the deepest mount holding it, by the device its `stat` reports.
pub(super) fn volume_of(path: &Path) -> io::Result<Volume> {
    let real = fs::canonicalize(path)?;
    let device = fs::metadata(&real)?.dev();
    let mounts = parse(&fs::read_to_string("/proc/self/mountinfo")?);

    // The deepest mount the path is beneath, preferring one of its own device: a btrfs subvolume
    // reports a device of its own that no mount line carries, so the deepest mount is the fallback.
    let beneath = |mount: &&Mount| real.starts_with(&mount.mount_point);
    let deepest = |mount: &&Mount| mount.mount_point.as_os_str().len();
    let mount = mounts
        .iter()
        .filter(beneath)
        .filter(|mount| mount.device == device)
        .max_by_key(deepest)
        .or_else(|| mounts.iter().filter(beneath).max_by_key(deepest))
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
    let mounts = parse(&fs::read_to_string("/proc/self/mountinfo")?);

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

/// Every mount `/proc/self/mountinfo` lists, in its order; a line it cannot read is passed over.
pub(super) fn parse(text: &str) -> Vec<Mount> {
    text.lines().filter_map(parse_line).collect()
}

/// One line: `id parent major:minor root mount-point options [optional…] - type source super-options`.
fn parse_line(line: &str) -> Option<Mount> {
    let (before, after) = line.split_once(" - ")?;
    let mut fields = before.split(' ');
    let _id = fields.next()?;
    let _parent = fields.next()?;
    let (major, minor) = fields.next()?.split_once(':')?;
    let root = unescape(fields.next()?);
    let mount_point = PathBuf::from(unescape(fields.next()?));
    let mut tail = after.split(' ');
    let filesystem = unescape(tail.next()?);
    let source = unescape(tail.next()?);

    Some(Mount {
        device: makedev(major.parse().ok()?, minor.parse().ok()?),
        root,
        mount_point,
        filesystem,
        source,
    })
}

/// A mountinfo field with its octal escapes — `\040` for a space and the like — put back.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let escape = bytes.get(at + 1..at + 4).filter(|digits| {
            bytes[at] == b'\\' && digits.iter().all(|digit| (b'0'..=b'7').contains(digit))
        });
        match escape
            .and_then(|digits| u8::from_str_radix(std::str::from_utf8(digits).ok()?, 8).ok())
        {
            Some(byte) => {
                out.push(byte);
                at += 4;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// The device number `stat` reports for `major:minor`, as glibc's `makedev` composes it.
fn makedev(major: u64, minor: u64) -> u64 {
    ((major & 0xffff_f000) << 32)
        | ((major & 0x0000_0fff) << 8)
        | ((minor & 0xffff_ff00) << 12)
        | (minor & 0x0000_00ff)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Mount, is_share, makedev, parse, part_of};

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
    fn a_mountinfo_line_reads_as_its_device_root_place_type_and_source() {
        let text = "\
36 35 98:0 /mnt1 /mnt/with\\040space rw,noatime master:1 - ext3 /dev/root rw,errors=continue
40 30 0:45 / /home/someone/share rw,relatime shared:9 - nfs4 server:/export/home rw,vers=4.2
41 30 8:17 / /run/media/someone/DISK1 rw - vfat /dev/sdb1 rw
not a mount line
";

        assert_eq!(
            parse(text),
            vec![
                Mount {
                    device: makedev(98, 0),
                    root: "/mnt1".to_owned(),
                    mount_point: PathBuf::from("/mnt/with space"),
                    filesystem: "ext3".to_owned(),
                    source: "/dev/root".to_owned(),
                },
                Mount {
                    device: makedev(0, 45),
                    root: "/".to_owned(),
                    mount_point: PathBuf::from("/home/someone/share"),
                    filesystem: "nfs4".to_owned(),
                    source: "server:/export/home".to_owned(),
                },
                Mount {
                    device: makedev(8, 17),
                    root: "/".to_owned(),
                    mount_point: PathBuf::from("/run/media/someone/DISK1"),
                    filesystem: "vfat".to_owned(),
                    source: "/dev/sdb1".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn a_device_number_is_composed_as_stat_reports_it() {
        // `/dev/sdb1` is 8:17; a large minor spills into the high bits.
        assert_eq!(makedev(8, 17), 0x811);
        assert_eq!(makedev(259, 65_536), 0x1001_0300);

        // And as this kernel reports it: `/proc` is a mount of its own wherever this runs.
        let mounts = parse(&std::fs::read_to_string("/proc/self/mountinfo").expect("mountinfo"));
        let proc = mounts
            .iter()
            .find(|mount| mount.mount_point == std::path::Path::new("/proc"))
            .expect("/proc is mounted");
        assert_eq!(
            proc.device,
            std::os::unix::fs::MetadataExt::dev(&std::fs::metadata("/proc").expect("/proc"))
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
