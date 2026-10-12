//! macOS: the mount a path is on, from `statfs`, and the volume's UUID, from `getattrlist`.
#![allow(
    unsafe_code,
    reason = "statfs, getfsstat and getattrlist have no safe wrapper in std; they are confined to this module"
)]

use std::ffi::{CString, c_char, c_void};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::Volume;

/// The volume `path` is on, as `statfs` names its mount.
pub(super) fn volume_of(path: &Path) -> io::Result<Volume> {
    let real = fs::canonicalize(path)?;
    let name = CString::new(real.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    // SAFETY: an all-zero `statfs` is a valid value of a plain C struct, and is overwritten below.
    let mut found: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `name` is NUL-terminated and `found` is a `statfs` the call may write.
    if unsafe { libc::statfs(name.as_ptr(), &mut found) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mount = Mounted::from(&found);
    let within = super::within(&real, &mount.mount_point)
        .or_else(|| firmlinked(&real, &mount.mount_point))
        .unwrap_or_default();

    Ok(Volume {
        identity: mount.identity(),
        within,
        filesystem: mount.filesystem.clone(),
        remote: mount.remote,
        mount_point: mount.mount_point,
    })
}

/// Every mount of the volume `identity`, among all `getfsstat` lists.
pub(super) fn mount_points(identity: &str) -> io::Result<Vec<PathBuf>> {
    // SAFETY: a null buffer asks only how many mounts there are.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut mounts: Vec<libc::statfs> = (0..count)
        // SAFETY: an all-zero `statfs` is a valid value of a plain C struct.
        .map(|_| unsafe { std::mem::zeroed() })
        .collect();
    let size = std::mem::size_of::<libc::statfs>() * mounts.len();
    let size = libc::c_int::try_from(size)
        .map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "too many mounts"))?;
    // SAFETY: `mounts` holds `size` bytes of `statfs` the call may write.
    let filled = unsafe { libc::getfsstat(mounts.as_mut_ptr(), size, libc::MNT_NOWAIT) };
    if filled < 0 {
        return Err(io::Error::last_os_error());
    }
    let filled = usize::try_from(filled).unwrap_or(0).min(mounts.len());

    Ok(mounts[..filled]
        .iter()
        .map(Mounted::from)
        .filter(|mount| mount.identity().as_deref() == Some(identity))
        .map(|mount| mount.mount_point)
        .collect())
}

/// `real` relative to `mount_point` when a firmlink puts it there: `/Users/x` is
/// `/System/Volumes/Data/Users/x`, the same directory, though neither path is beneath the other.
fn firmlinked(real: &Path, mount_point: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;

    let relative = real.strip_prefix("/").ok()?;
    let here = fs::metadata(real).ok()?;
    let there = fs::metadata(mount_point.join(relative)).ok()?;

    (here.dev() == there.dev() && here.ino() == there.ino())
        .then(|| super::within(&Path::new("/").join(relative), Path::new("/")))
        .flatten()
}

/// What `statfs` says of one mount.
struct Mounted {
    /// Where it is mounted.
    mount_point: PathBuf,
    /// What is mounted: a device, or a share.
    source: String,
    /// The filesystem's type.
    filesystem: String,
    /// Whether it is not local.
    remote: bool,
}

impl From<&libc::statfs> for Mounted {
    fn from(found: &libc::statfs) -> Self {
        Self {
            mount_point: PathBuf::from(std::ffi::OsStr::from_bytes(
                text(&found.f_mntonname).as_bytes(),
            )),
            source: text(&found.f_mntfromname),
            filesystem: text(&found.f_fstypename),
            remote: found.f_flags & (libc::MNT_LOCAL as u32) == 0,
        }
    }
}

impl Mounted {
    /// The volume's persistent identity: a network share's source, or the volume's UUID.
    fn identity(&self) -> Option<String> {
        if self.remote {
            return Some(format!("remote:{}", self.source));
        }
        // Neither has a volume of its own to name, and asking an automount trigger fires it.
        if matches!(self.filesystem.as_str(), "autofs" | "devfs") {
            return None;
        }

        volume_uuid(&self.mount_point).map(|uuid| format!("uuid:{uuid}"))
    }
}

/// The UUID of the volume mounted at `mount_point`, if its filesystem has one.
fn volume_uuid(mount_point: &Path) -> Option<String> {
    /// What `getattrlist` writes for `ATTR_VOL_UUID`: the length of what it wrote, then the UUID.
    #[repr(C)]
    struct Answer {
        length: u32,
        uuid: [u8; 16],
    }

    let name = CString::new(mount_point.as_os_str().as_bytes()).ok()?;
    let mut asked = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut answer = Answer {
        length: 0,
        uuid: [0; 16],
    };
    // SAFETY: `name` is NUL-terminated, `asked` is a valid attribute list, and `answer` holds the size
    // passed.
    let failed = unsafe {
        libc::getattrlist(
            name.as_ptr(),
            (&mut asked as *mut libc::attrlist).cast::<c_void>(),
            (&mut answer as *mut Answer).cast::<c_void>(),
            std::mem::size_of::<Answer>(),
            // Never through the path: on an automount trigger, following it fires the mount.
            libc::FSOPT_NOFOLLOW,
        )
    } != 0;
    if failed || answer.uuid == [0; 16] {
        return None;
    }
    let hex: String = answer
        .uuid
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect();

    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// A fixed-size, NUL-terminated C string field as text.
fn text(field: &[c_char]) -> String {
    let bytes: Vec<u8> = field
        .iter()
        .take_while(|unit| **unit != 0)
        .map(|unit| unit.to_ne_bytes()[0])
        .collect();

    String::from_utf8_lossy(&bytes).into_owned()
}
