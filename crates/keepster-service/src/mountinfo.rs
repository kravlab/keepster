//! `/proc/self/mountinfo`: which mounts there are, and which of them a path is on.
//!
//! Pure functions over the file's bytes, compiled on every platform so their tests run on every
//! platform. The Linux volume lookup (ticket 56) and the performance tripwire's description of the
//! machine (ticket 69) both read mounts through here, so the two never disagree about which filesystem
//! a path is on. A mount is chosen by the device a path's `stat` reports, which each line carries too,
//! rather than by its name: a mount stacked over another hides it, and only the device says which one a
//! path resolves through.

use std::path::{Path, PathBuf};

/// Where Linux lists the mounts the reading process sees.
pub const PATH: &str = "/proc/self/mountinfo";

/// One line of `/proc/self/mountinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// The device the mount is of, as `stat` reports it.
    pub device: u64,
    /// Which directory of the filesystem is mounted: `/` for the whole of it, a subdirectory for a bind
    /// mount.
    pub root: String,
    /// Where it is mounted, byte for byte as the kernel names it.
    pub mount_point: PathBuf,
    /// The filesystem type.
    pub filesystem: String,
    /// What is mounted: a device, a share, or a name a pseudo-filesystem gives itself.
    pub source: String,
}

/// Every mount the bytes of `/proc/self/mountinfo` list, in their order; a line that cannot be read is
/// passed over.
pub fn parse(text: &[u8]) -> Vec<Mount> {
    text.split(|byte| *byte == b'\n')
        .filter_map(parse_line)
        .collect()
}

/// The mount `path` — absolute, with no symlink in it — is on, given `device`, the device its `stat`
/// reports: the deepest mount holding it of that device; failing that, as for a btrfs subvolume whose
/// device no line carries, the deepest mount holding it at all. Of mounts stacked on one point, the
/// later in the list wins. `None` when no mount holds it.
pub fn mount_of<'a>(mounts: &'a [Mount], path: &Path, device: u64) -> Option<&'a Mount> {
    let beneath = |mount: &&Mount| path.starts_with(&mount.mount_point);
    let deepest = |mount: &&Mount| mount.mount_point.as_os_str().len();
    mounts
        .iter()
        .filter(beneath)
        .filter(|mount| mount.device == device)
        .max_by_key(deepest)
        .or_else(|| mounts.iter().filter(beneath).max_by_key(deepest))
}

/// The device number `stat` reports for `major:minor`, as glibc's `makedev` composes it.
pub fn makedev(major: u64, minor: u64) -> u64 {
    ((major & 0xffff_f000) << 32)
        | ((major & 0x0000_0fff) << 8)
        | ((minor & 0xffff_ff00) << 12)
        | (minor & 0x0000_00ff)
}

/// One line: `id parent major:minor root mount-point options [optional…] - type source super-options`.
///
/// The ` - ` separator cannot occur inside a field: the kernel escapes every space in a name.
fn parse_line(line: &[u8]) -> Option<Mount> {
    let at = line.windows(3).position(|window| window == b" - ")?;
    let (before, after) = (&line[..at], &line[at + 3..]);
    let mut fields = before.split(|byte| *byte == b' ');
    let _id = fields.next()?;
    let _parent = fields.next()?;
    let (major, minor) = std::str::from_utf8(fields.next()?).ok()?.split_once(':')?;
    let root = text(unescape(fields.next()?));
    let mount_point = path(unescape(fields.next()?));
    let mut tail = after.split(|byte| *byte == b' ');
    let filesystem = text(unescape(tail.next()?));
    let source = text(unescape(tail.next()?));

    Some(Mount {
        device: makedev(major.parse().ok()?, minor.parse().ok()?),
        root,
        mount_point,
        filesystem,
        source,
    })
}

/// A mountinfo field with its octal escapes — `\040` for a space and the like — put back.
fn unescape(field: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(field.len());
    let mut at = 0;
    while at < field.len() {
        let escape = field.get(at + 1..at + 4).filter(|digits| {
            field[at] == b'\\' && digits.iter().all(|digit| (b'0'..=b'7').contains(digit))
        });
        match escape
            .and_then(|digits| u8::from_str_radix(std::str::from_utf8(digits).ok()?, 8).ok())
        {
            Some(byte) => {
                out.push(byte);
                at += 4;
            }
            None => {
                out.push(field[at]);
                at += 1;
            }
        }
    }
    out
}

/// `bytes` as text, any that are not UTF-8 replaced: a type, a source or a root is only ever shown.
fn text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `bytes` as a path. On Unix a path is bytes, so it keeps every one and compares exactly with the path
/// `stat` was asked about; elsewhere there is no mountinfo to read, and lossy text serves the tests.
fn path(bytes: Vec<u8>) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(text(bytes))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Mount, makedev, mount_of, parse};

    #[test]
    fn a_mountinfo_line_reads_as_its_device_root_place_type_and_source() {
        let text = "\
36 35 98:0 /mnt1 /mnt/with\\040space rw,noatime master:1 - ext3 /dev/root rw,errors=continue
40 30 0:45 / /home/someone/share rw,relatime shared:9 - nfs4 server:/export/home rw,vers=4.2
41 30 8:17 / /run/media/someone/DISK1 rw - vfat /dev/sdb1 rw
not a mount line
";

        assert_eq!(
            parse(text.as_bytes()),
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
    fn every_escape_the_kernel_writes_is_put_back() {
        // Space, tab, line feed and backslash: the four bytes `proc(5)` says a name has escaped.
        let text = "1 0 8:1 / /a\\040b\\011c\\012d\\134e rw - ext4 /dev/sda1 rw\n";

        assert_eq!(
            parse(text.as_bytes())[0].mount_point,
            PathBuf::from("/a b\tc\nd\\e")
        );
    }

    #[test]
    fn a_backslash_that_begins_no_escape_stays_as_written() {
        // Not three octal digits, cut short at the end, or past a byte's reach.
        let text = "1 0 8:1 / /a\\9b\\08c\\400d\\04 rw - ext4 /dev/sda1 rw\n";

        assert_eq!(
            parse(text.as_bytes())[0].mount_point,
            PathBuf::from("/a\\9b\\08c\\400d\\04")
        );
    }

    #[test]
    fn a_line_short_of_a_field_is_passed_over() {
        let text = "\
1 0 8:1 / / rw shared:1 ext4 /dev/sda1 rw
2 0 8:1 / /no-type rw -
3 0 8:1 / /no-source rw - ext4
4 0 eight:one / /bad-device rw - ext4 /dev/sda1 rw
5 0 8:1 / /fine rw - ext4 /dev/sda1 rw
";

        let read: Vec<PathBuf> = parse(text.as_bytes())
            .into_iter()
            .map(|mount| mount.mount_point)
            .collect();

        assert_eq!(read, [PathBuf::from("/fine")]);
    }

    #[cfg(unix)]
    #[test]
    fn a_mount_point_that_is_not_utf8_keeps_its_bytes() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let mounts = parse(
            b"1 0 8:1 / / rw - ext4 /dev/sda1 rw\n2 1 8:2 / /mnt/\xff rw - xfs /dev/sdb1 rw\n",
        );
        let other = Path::new(OsStr::from_bytes(b"/mnt/\xfe/bench"));
        let same = Path::new(OsStr::from_bytes(b"/mnt/\xff/bench"));

        assert_eq!(
            mount_of(&mounts, same, 0).map(|mount| mount.filesystem.as_str()),
            Some("xfs")
        );
        // Two names that differ only in bytes no text can hold are still two names.
        assert_eq!(
            mount_of(&mounts, other, 0).map(|mount| mount.filesystem.as_str()),
            Some("ext4")
        );
    }

    #[test]
    fn a_path_is_on_the_mount_of_its_own_device() {
        // A tmpfs mounted over `/data` after `/data/scratch` hides it: the path's own device, not the
        // deepest mount point, says which mount it resolves through.
        let mounts = parse(
            b"\
22 1 8:1 / / rw - ext4 /dev/sda1 rw
24 22 253:0 / /data rw - btrfs /dev/mapper/example rw
26 24 8:17 / /data/scratch rw - xfs /dev/sdb1 rw
40 24 0:40 / /data rw - tmpfs tmpfs rw
",
        );
        let path = Path::new("/data/scratch/x");

        assert_eq!(
            mount_of(&mounts, path, makedev(0, 40)).map(|mount| mount.filesystem.as_str()),
            Some("tmpfs")
        );
        assert_eq!(
            mount_of(&mounts, path, makedev(8, 17)).map(|mount| mount.filesystem.as_str()),
            Some("xfs")
        );
    }

    #[test]
    fn a_device_no_mount_carries_falls_back_to_the_deepest_mount_and_the_latest_of_a_stack() {
        // A btrfs subvolume reports a device of its own that no mount line carries.
        let mounts = parse(
            b"\
22 1 8:1 / / rw - ext4 /dev/sda1 rw
24 22 0:50 / /data rw - btrfs /dev/mapper/example rw
30 22 8:2 / /mnt rw - ext4 /dev/sdc1 rw
31 30 0:40 / /mnt rw - tmpfs tmpfs rw
",
        );
        let unknown = makedev(0, 99);

        assert_eq!(
            mount_of(&mounts, Path::new("/data/sub/x"), unknown)
                .map(|mount| mount.filesystem.as_str()),
            Some("btrfs")
        );
        assert_eq!(
            mount_of(&mounts, Path::new("/mnt/bench"), unknown)
                .map(|mount| mount.filesystem.as_str()),
            Some("tmpfs")
        );
    }

    #[test]
    fn a_mount_point_holds_only_whole_components() {
        let mounts = parse(
            b"\
22 1 8:1 / / rw - ext4 /dev/sda1 rw
24 22 253:0 / /data rw - btrfs /dev/mapper/example rw
",
        );

        // `/database` begins with `/data` but is not beneath it.
        assert_eq!(
            mount_of(&mounts, Path::new("/database/x"), makedev(0, 99))
                .map(|mount| mount.filesystem.as_str()),
            Some("ext4")
        );
        assert_eq!(
            mount_of(&mounts, Path::new("/data"), makedev(253, 0))
                .map(|mount| mount.filesystem.as_str()),
            Some("btrfs")
        );
    }

    #[test]
    fn a_path_no_mount_holds_is_on_none() {
        let mounts = parse(b"23 22 0:21 / /proc rw - proc proc rw\n");

        assert_eq!(mount_of(&mounts, Path::new("/data/bench"), 0), None);
        assert_eq!(mount_of(&[], Path::new("/data/bench"), 0), None);
    }

    #[test]
    fn a_device_number_is_composed_as_stat_reports_it() {
        // `/dev/sdb1` is 8:17; a large minor spills into the high bits.
        assert_eq!(makedev(8, 17), 0x811);
        assert_eq!(makedev(259, 65_536), 0x1001_0300);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_device_number_is_composed_as_this_kernel_reports_it() {
        // `/proc` is a mount of its own wherever this runs.
        let mounts = parse(&std::fs::read(super::PATH).expect("mountinfo"));
        let proc = mounts
            .iter()
            .find(|mount| mount.mount_point == Path::new("/proc"))
            .expect("/proc is mounted");
        assert_eq!(
            proc.device,
            std::os::unix::fs::MetadataExt::dev(&std::fs::metadata("/proc").expect("/proc"))
        );
    }
}
