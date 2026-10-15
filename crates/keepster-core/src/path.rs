//! A path inside a Watched Root, in the one form every endpoint takes.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

/// Why a string is not a path inside a Root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidPath {
    /// The path is absolute, so it names a place outside any Root's frame of reference.
    #[error("the path is absolute; it must be relative to the Root")]
    Absolute,
    /// A component is `.` or `..`, so the path does not name one place without resolving it first.
    #[error("the path contains a `.` or `..` component")]
    Relative,
    /// A component is empty: a doubled, leading or trailing separator.
    #[error("the path contains an empty component")]
    Empty,
    /// The path contains a NUL byte, which no platform allows in a name.
    #[error("the path contains a NUL byte")]
    Nul,
}

/// A path relative to a Watched Root, `/`-separated, naming exactly one place.
///
/// Ticket 07 fixes one form of path parameter across every endpoint: relative to the Root, always `/`,
/// with absolute paths, `..` and a leading `./` rejected rather than resolved. Resolving them here would
/// make two spellings name one resource, and a `..` that climbs out of the Root is the classic way a
/// read endpoint becomes a read-anything endpoint. The empty path is the Root itself.
///
/// Names are bytes (tickets 10 and 74): a Unix name need not be UTF-8, and the path holds it exactly,
/// so it is captured, read and restored under the name it has. On Windows every path built from a name
/// on disk is UTF-8, because a name that is not — one holding an unpaired surrogate — is Withheld rather
/// than held. Paths order by their bytes, the order git's index keeps.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RootPath(Vec<u8>);

impl RootPath {
    /// The Root itself.
    pub fn root() -> Self {
        Self(Vec::new())
    }

    /// Check that `text` is a path in the one accepted form.
    pub fn parse(text: &str) -> Result<Self, InvalidPath> {
        Self::parse_bytes(text.as_bytes())
    }

    /// Check that `bytes` is a path in the one accepted form, whether or not it is UTF-8.
    pub fn parse_bytes(bytes: &[u8]) -> Result<Self, InvalidPath> {
        if bytes.is_empty() {
            return Ok(Self::root());
        }
        if bytes.contains(&0) {
            return Err(InvalidPath::Nul);
        }
        if is_absolute(bytes) {
            return Err(InvalidPath::Absolute);
        }

        for component in bytes.split(|byte| *byte == b'/') {
            match component {
                b"" => return Err(InvalidPath::Empty),
                b"." | b".." => return Err(InvalidPath::Relative),
                _ => {}
            }
        }

        Ok(Self(bytes.to_vec()))
    }

    /// Whether this is the Root itself.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The path's exact bytes, `/`-separated, empty for the Root itself.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The path as text, or `None` where a name is not UTF-8.
    pub fn to_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// The path as text, a name that is not UTF-8 shown with U+FFFD: for people, never for finding it.
    pub fn to_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }

    /// Every directory above this path, from the top down, excluding the Root itself and the path.
    ///
    /// The order a walk enters them in, which is the order in which an excluded one prunes the rest.
    pub fn ancestors(&self) -> impl Iterator<Item = RootPath> + '_ {
        self.0
            .iter()
            .enumerate()
            .filter(|(_, byte)| **byte == b'/')
            .map(|(end, _)| Self(self.0[..end].to_vec()))
    }

    /// The directory holding this path — the Root itself for a top-level name, and for the Root.
    pub fn parent(&self) -> Self {
        match self.0.iter().rposition(|byte| *byte == b'/') {
            Some(end) => Self(self.0[..end].to_vec()),
            None => Self::root(),
        }
    }

    /// The last component, empty for the Root itself.
    pub fn name(&self) -> &[u8] {
        self.0
            .rsplit(|byte| *byte == b'/')
            .next()
            .unwrap_or_default()
    }

    /// The path one component deeper.
    ///
    /// `name` is one component as the filesystem or a tree handed it over, whatever bytes it holds; it is
    /// never parsed.
    pub fn child(&self, name: impl AsRef<[u8]>) -> Self {
        let name = name.as_ref();
        if self.0.is_empty() {
            Self(name.to_vec())
        } else {
            let mut bytes = Vec::with_capacity(self.0.len() + 1 + name.len());
            bytes.extend_from_slice(&self.0);
            bytes.push(b'/');
            bytes.extend_from_slice(name);
            Self(bytes)
        }
    }

    /// Where this path is beneath `base`, spelled the way this platform spells paths.
    ///
    /// `None` only on Windows, for bytes that are not UTF-8: no Windows name is spelled so, since one
    /// that cannot be is Withheld, so such a path can only have come from a client and names nothing.
    pub fn native(&self, base: &Path) -> Option<PathBuf> {
        let mut path = base.to_path_buf();
        if self.is_root() {
            return Some(path);
        }
        for component in self.0.split(|byte| *byte == b'/') {
            path.push(native_name(component)?);
        }
        Some(path)
    }

    /// The path in the Root that `relative`, a platform path beneath it, names.
    ///
    /// `None` when a component is not a plain name — `..`, `.`, a root or a drive prefix — or is a name
    /// no path holds: a Windows name with an unpaired surrogate (see [`name_bytes`]).
    pub fn from_native(relative: &Path) -> Option<Self> {
        let mut path = Self::root();
        for component in relative.components() {
            let std::path::Component::Normal(name) = component else {
                return None;
            };
            path = path.child(name_bytes(name)?);
        }
        Some(path)
    }
}

impl fmt::Display for RootPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_lossy())
    }
}

/// A name's exact bytes, as a path holds it: its bytes on Unix, where a name is bytes; its UTF-8
/// elsewhere, and `None` for a Windows name holding an unpaired surrogate, which no UTF-8 spells and
/// which is therefore Withheld rather than held (ticket 74).
#[cfg(unix)]
pub fn name_bytes(name: &OsStr) -> Option<&[u8]> {
    use std::os::unix::ffi::OsStrExt;

    Some(name.as_bytes())
}

/// A name's exact bytes, as a path holds it: its bytes on Unix, where a name is bytes; its UTF-8
/// elsewhere, and `None` for a Windows name holding an unpaired surrogate, which no UTF-8 spells and
/// which is therefore Withheld rather than held (ticket 74).
#[cfg(not(unix))]
pub fn name_bytes(name: &OsStr) -> Option<&[u8]> {
    name.to_str().map(str::as_bytes)
}

/// One component as this platform holds a name: its bytes on Unix, its text elsewhere.
#[cfg(unix)]
fn native_name(component: &[u8]) -> Option<&OsStr> {
    use std::os::unix::ffi::OsStrExt;

    Some(OsStr::from_bytes(component))
}

/// One component as this platform holds a name: its bytes on Unix, its text elsewhere.
#[cfg(not(unix))]
fn native_name(component: &[u8]) -> Option<&OsStr> {
    std::str::from_utf8(component).ok().map(OsStr::new)
}

/// Whether `bytes` is absolute on any platform a client may be running on.
///
/// The daemon answers clients on every platform, so a Windows drive or UNC prefix is refused on Unix
/// too: it can only have come from a client that forgot to make the path relative.
fn is_absolute(bytes: &[u8]) -> bool {
    // `C:` followed by a separator or nothing; `a:b` is an ordinary name on Unix.
    let drive = bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes.get(2), None | Some(b'/' | b'\\'));

    bytes.starts_with(b"/") || bytes.starts_with(b"\\") || drive
}
