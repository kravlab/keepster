//! The identity of a Watched Root.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::SystemTime;

use ulid::Ulid;

/// A Watched Root's identity: a surrogate id that survives renaming or moving its directory.
///
/// The path cannot serve as the identity — it is a mutable attribute, and a Root that moves keeps its
/// history — and an autoincrement could not survive a store copied to another machine. A ULID orders
/// lexicographically by time, which is what makes a registry listing and a timeline agree without a
/// sort key of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RootId(Ulid);

impl RootId {
    /// Mint an identity for a Root registered at `now`.
    ///
    /// The moment is a parameter rather than a clock read, because a ULID embeds its timestamp and
    /// ADR 0012 keeps every wall-clock read in `keepster-daemon`.
    pub fn generate(now: SystemTime) -> Self {
        Self(Ulid::from_datetime(now))
    }
}

impl fmt::Display for RootId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Why a string is not a Root identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a Root identity")]
pub struct NotARootId;

impl FromStr for RootId {
    type Err = NotARootId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(text).map(Self).map_err(|_| NotARootId)
    }
}

/// Which registered Root holds `path`, if any.
///
/// Comparison is by path component, never by string prefix: `…/site-old` is not inside `…/site`, and a
/// prefix test would put it there. Where several Roots would match — which registration forbids, but a
/// Registry read from disk is not something to trust blindly — the nearest one wins.
pub fn containing(path: &Path, roots: &[(RootId, PathBuf)]) -> Option<RootId> {
    roots
        .iter()
        .filter(|(_, root)| path.starts_with(root))
        .max_by_key(|(_, root)| root.components().count())
        .map(|(id, _)| *id)
}

/// Which registered Root would collide with a registration of `path`, if any.
///
/// Overlap is symmetric, unlike containment: a directory that would *contain* an existing Root is just
/// as forbidden as one that would sit inside it. Nesting would break the walk-up that every CLI command
/// rests on and would capture every change into two stores (ticket 07).
pub fn overlapping(path: &Path, roots: &[(RootId, PathBuf)]) -> Option<RootId> {
    roots
        .iter()
        .find(|(_, root)| path.starts_with(root) || root.starts_with(path))
        .map(|(id, _)| *id)
}
