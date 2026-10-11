//! A path inside a Watched Root, in the one form every endpoint takes.

use std::fmt;

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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RootPath(String);

impl RootPath {
    /// The Root itself.
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Check that `text` is a path in the one accepted form.
    pub fn parse(text: &str) -> Result<Self, InvalidPath> {
        if text.is_empty() {
            return Ok(Self::root());
        }
        if text.contains('\0') {
            return Err(InvalidPath::Nul);
        }
        if is_absolute(text) {
            return Err(InvalidPath::Absolute);
        }

        for component in text.split('/') {
            match component {
                "" => return Err(InvalidPath::Empty),
                "." | ".." => return Err(InvalidPath::Relative),
                _ => {}
            }
        }

        Ok(Self(text.to_owned()))
    }

    /// Whether this is the Root itself.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The path as text, `/`-separated, empty for the Root itself.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Every directory above this path, from the top down, excluding the Root itself and the path.
    ///
    /// The order a walk enters them in, which is the order in which an excluded one prunes the rest.
    pub fn ancestors(&self) -> impl Iterator<Item = RootPath> + '_ {
        self.0
            .match_indices('/')
            .map(|(end, _)| Self(self.0[..end].to_owned()))
    }

    /// The directory holding this path — the Root itself for a top-level name, and for the Root.
    pub fn parent(&self) -> Self {
        match self.0.rfind('/') {
            Some(end) => Self(self.0[..end].to_owned()),
            None => Self::root(),
        }
    }

    /// The last component, empty for the Root itself.
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or_default()
    }

    /// The path one component deeper.
    pub fn child(&self, name: &str) -> Self {
        if self.0.is_empty() {
            Self(name.to_owned())
        } else {
            Self(format!("{}/{name}", self.0))
        }
    }
}

impl fmt::Display for RootPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Whether `text` is absolute on any platform a client may be running on.
///
/// The daemon answers clients on every platform, so a Windows drive or UNC prefix is refused on Unix
/// too: it can only have come from a client that forgot to make the path relative.
fn is_absolute(text: &str) -> bool {
    let bytes = text.as_bytes();
    // `C:` followed by a separator or nothing; `a:b` is an ordinary name on Unix.
    let drive = bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes.get(2), None | Some(b'/' | b'\\'));

    text.starts_with('/') || text.starts_with('\\') || drive
}
