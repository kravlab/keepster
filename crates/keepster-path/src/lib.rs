//! Paths as this machine spells them (ticket 77): resolved, written the way a person types them, and
//! compared as written.
//!
//! A crate of its own because both sides of the wire need it: the daemon records and compares real
//! paths, and the CLI sends the working directory it stands in, resolved. Everything here but
//! [`real_path`] and [`through_link`] is pure, so the domain core may use it and stay free of I/O (ADR 0003).

use std::io;
use std::path::{Component, Path, PathBuf};

/// Which a Root registered through a link is (ticket 77): the link as typed, or where it led.
///
/// A request naming any other value is refused as `body_invalid`: a guess at what the person meant would
/// register the wrong directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "lowercase")
)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum PathMode {
    /// The Root is the path as typed: when the link is pointed elsewhere, the Root follows it.
    Link,
    /// The Root is the link's target, its real path, recorded at registration; the link no longer
    /// matters.
    Canonical,
}

/// Where `path` really is, every link resolved, written as a person would type it (ticket 77): the
/// filesystem's canonical path, [`without_verbatim`].
pub fn real_path(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path).map(|real| without_verbatim(&real))
}

/// `path` without the Windows verbatim prefix (`\\?\`, `\\?\UNC\`) where what follows it is an ordinary
/// drive or share path, as a person would type it; any other path unchanged (ticket 77).
///
/// Windows canonicalises to the verbatim form, which compares unequal to every path typed without it,
/// so a real path is written the ordinary way before it is compared or recorded. A verbatim path naming
/// a volume by its GUID, or a device, has no ordinary spelling and is kept. Read as text, so a path from
/// any platform passes through: no Unix path starts with the prefix.
pub fn without_verbatim(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{share}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if is_drive_path(rest) => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Whether `text` starts with a drive, `C:`, followed by nothing or a separator.
fn is_drive_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes.get(2).is_none_or(|&byte| byte == b'\\')
}

/// Whether `path` passes through a link: whether any of its components, read from the top down, is a
/// symbolic link (ticket 77). On Windows that takes in a junction too: `FileType::is_symlink` is true
/// there for every reparse point whose tag is a name surrogate, which symbolic links and junctions are.
///
/// Asked of the filesystem one component at a time rather than read off a comparison of where `path`
/// really is with how it is written: the two differ in case, or by a Windows 8.3 short name, with no
/// link on the way, and agree where a `..` after a link climbs back out of it.
pub fn through_link(path: &Path) -> io::Result<bool> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        if matches!(component, Component::Normal(_))
            && std::fs::symlink_metadata(&prefix)?.file_type().is_symlink()
        {
            return Ok(true);
        }
    }

    Ok(false)
}

/// `path` with `.` and `..` resolved by name, as the shell that typed it reads them, without touching the
/// filesystem: `..` at the top stays there.
pub fn lexical(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    resolved.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    resolved.pop();
                } else if !resolved.has_root() {
                    resolved.push(component);
                }
            }
            other => resolved.push(other),
        }
    }

    resolved
}

/// Points `link` at `target` instead, as a user re-pointing a symlink does.
#[cfg(all(feature = "test-util", unix))]
pub fn retarget_link(link: &Path, target: &Path) {
    std::fs::remove_file(link).expect("the old link removed");
    std::os::unix::fs::symlink(target, link).expect("the link pointed elsewhere");
}
