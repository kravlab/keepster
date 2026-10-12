//! Windows: the volume a path is on, from `GetVolumePathNameW`, and its persistent identity, the volume
//! GUID path `GetVolumeNameForVolumeMountPointW` gives.
//!
//! One hazard is guarded explicitly (ADR 0011): `GetVolumePathNameW` silently answers the boot volume
//! for a relative or NT-namespace path, so a Root on a vanished volume could resolve to `C:\` with no
//! error. A path that is not absolute is refused, and a volume the path is not beneath is an error.
#![allow(
    unsafe_code,
    reason = "the Win32 volume calls have no safe wrapper in std; they are confined to this module"
)]

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, GetLastError};
use windows_sys::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    GetVolumePathNamesForVolumeNameW,
};

use super::Volume;

/// What `GetDriveTypeW` answers for a network drive (`DRIVE_REMOTE`).
const DRIVE_REMOTE: u32 = 4;

/// The longest path the volume calls answer without a `\\?\` prefix.
const MAX_PATH: usize = 260;

/// How long a volume GUID path is, `\\?\Volume{…}\` and its NUL, with room to spare.
const VOLUME_NAME: usize = 64;

/// The volume `path` is on: refused for a path that is not absolute, and an error for a volume the
/// path is not beneath, rather than the boot volume either way.
pub(super) fn volume_of(path: &Path) -> io::Result<Volume> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path is not absolute, and Windows would name the boot volume for it",
        ));
    }
    // Resolved first, as on every platform: a junction leads to another volume, which is the one the
    // files are on. It also fails for a path that does not exist, which `GetVolumePathNameW` would answer.
    let plain = plain(&std::fs::canonicalize(path)?);
    let asked = wide(plain.as_os_str());
    // As long as the path and a trailing backslash, which a mounted folder's name gains.
    let mut found = vec![0u16; asked.len().max(MAX_PATH) + 1];
    // SAFETY: `asked` is NUL-terminated and `found` holds the length passed.
    let ok = unsafe { GetVolumePathNameW(asked.as_ptr(), found.as_mut_ptr(), length(&found)) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let mount_point = PathBuf::from(text(&found));
    let within = within(&plain, &mount_point).ok_or_else(|| {
        io::Error::other(format!(
            "Windows named the volume at {} for {}, which is not on it",
            mount_point.display(),
            plain.display()
        ))
    })?;
    let mount = wide(mount_point.as_os_str());
    // SAFETY: `mount` is NUL-terminated.
    let remote = unsafe { GetDriveTypeW(mount.as_ptr()) } == DRIVE_REMOTE;

    Ok(Volume {
        identity: identity(&mount_point, remote),
        mount_point,
        within,
        filesystem: String::new(),
        remote,
    })
}

/// Every place the volume `identity` is mounted: a volume's mount points as Windows lists them, or a
/// share while it can be reached.
pub(super) fn mount_points(identity: &str) -> io::Result<Vec<PathBuf>> {
    if let Some(share) = identity.strip_prefix("remote:") {
        let share = PathBuf::from(share);
        return Ok(if share.is_dir() {
            vec![share]
        } else {
            Vec::new()
        });
    }
    let Some(volume) = identity.strip_prefix("volume:") else {
        return Ok(Vec::new());
    };
    let name = wide(OsStr::new(volume));
    let mut names = vec![0u16; 1024];
    loop {
        let mut needed = 0u32;
        // SAFETY: `name` is NUL-terminated, `names` holds the length passed, and `needed` may be
        // written.
        let ok = unsafe {
            GetVolumePathNamesForVolumeNameW(
                name.as_ptr(),
                names.as_mut_ptr(),
                length(&names),
                &mut needed,
            )
        };
        if ok != 0 {
            break;
        }
        // SAFETY: reads the calling thread's last error, set by the call just above.
        if unsafe { GetLastError() } != ERROR_MORE_DATA {
            return Err(io::Error::last_os_error());
        }
        names = vec![0u16; usize::try_from(needed).unwrap_or(0).max(names.len() * 2)];
    }

    // A list of NUL-terminated names, ended by an empty one.
    Ok(names
        .split(|unit| *unit == 0)
        .take_while(|name| !name.is_empty())
        .map(|name| PathBuf::from(OsString::from_wide(name)))
        .collect())
}

/// The persistent identity of the volume mounted at `mount_point`: its GUID path, or a share's own
/// name; a mapped network drive's letter is no identity, so it has none.
fn identity(mount_point: &Path, remote: bool) -> Option<String> {
    let text = mount_point.to_string_lossy();
    if let Some(named) = named_by_its_path(&text) {
        return Some(named);
    }
    if remote {
        return None;
    }
    let mount = wide(mount_point.as_os_str());
    let mut name = vec![0u16; VOLUME_NAME];
    // SAFETY: `mount` is NUL-terminated and ends in a backslash, as `GetVolumePathNameW` gave it, and
    // `name` holds the length passed.
    let ok = unsafe {
        GetVolumeNameForVolumeMountPointW(mount.as_ptr(), name.as_mut_ptr(), length(&name))
    };

    (ok != 0).then(|| format!("volume:{}", text_of(&name)))
}

/// The identity a mount point's own text gives: a volume GUID path is one, and so is a share's name —
/// `\\host\share\`, never a `\\?\` or `\\.\` path, which only look like one.
fn named_by_its_path(text: &str) -> Option<String> {
    if text.to_ascii_lowercase().starts_with(r"\\?\volume{") {
        return Some(format!("volume:{text}"));
    }
    let share = text.starts_with(r"\\") && !text.starts_with(r"\\?\") && !text.starts_with(r"\\.\");

    share.then(|| format!("remote:{}", text.to_lowercase()))
}

/// `path` without the `\\?\` prefix `canonicalize` puts on it, which `GetVolumePathNameW` does not
/// read as a drive: `\\?\C:\x` is `C:\x`, and `\\?\UNC\host\share\x` is `\\host\share\x`.
fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// `path` relative to `mount_point`, `/`-separated, comparing as Windows does — without regard to case —
/// or `None` when it is not beneath it.
fn within(path: &Path, mount_point: &Path) -> Option<String> {
    let same = |a: Component<'_>, b: Component<'_>| {
        a.as_os_str().to_string_lossy().to_lowercase()
            == b.as_os_str().to_string_lossy().to_lowercase()
    };
    let mut parts = path.components();
    for part in mount_point.components() {
        if !same(parts.next()?, part) {
            return None;
        }
    }

    Some(
        parts
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// `text` as the Windows API takes it: UTF-16, NUL-terminated.
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

/// A buffer's length as the Windows API takes it.
fn length(buffer: &[u16]) -> u32 {
    u32::try_from(buffer.len()).unwrap_or(u32::MAX)
}

/// A NUL-terminated buffer as a path's text.
fn text(buffer: &[u16]) -> OsString {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());

    OsString::from_wide(&buffer[..end])
}

/// A NUL-terminated buffer as text.
fn text_of(buffer: &[u16]) -> String {
    text(buffer).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{named_by_its_path, plain, within};

    #[test]
    fn a_volume_guid_path_and_a_share_name_themselves_and_nothing_else_does() {
        assert_eq!(
            named_by_its_path(r"\\?\Volume{1b2c}\").as_deref(),
            Some(r"volume:\\?\Volume{1b2c}\")
        );
        assert_eq!(
            named_by_its_path(r"\\Host\Share\").as_deref(),
            Some(r"remote:\\host\share\")
        );
        assert_eq!(named_by_its_path(r"C:\"), None);
        assert_eq!(
            named_by_its_path(r"\\?\C:\"),
            None,
            "a drive, only spelled as a device path"
        );
        assert_eq!(named_by_its_path(r"\\.\pipe\"), None);
    }

    #[test]
    fn a_verbatim_path_is_read_as_the_drive_or_share_it_names() {
        assert_eq!(
            plain(Path::new(r"\\?\C:\Users\x")),
            PathBuf::from(r"C:\Users\x")
        );
        assert_eq!(
            plain(Path::new(r"\\?\UNC\host\share\x")),
            PathBuf::from(r"\\host\share\x")
        );
        assert_eq!(plain(Path::new(r"D:\a")), PathBuf::from(r"D:\a"));
    }

    #[test]
    fn a_path_is_within_its_volume_whatever_the_case_and_never_within_another() {
        assert_eq!(
            within(Path::new(r"c:\Users\Someone\app"), Path::new(r"C:\")).as_deref(),
            Some("Users/Someone/app")
        );
        assert_eq!(
            within(Path::new(r"C:\"), Path::new(r"C:\")).as_deref(),
            Some("")
        );
        assert_eq!(
            within(Path::new(r"D:\mnt\disk\app"), Path::new(r"D:\mnt\disk\")).as_deref(),
            Some("app")
        );
        assert_eq!(
            within(Path::new(r"E:\app"), Path::new(r"C:\")),
            None,
            "the boot volume named for a path on a vanished one"
        );
    }
}
