//! The identity of a Watched Root.

use std::path::{Path, PathBuf};

use crate::identity::ulid_identity;

ulid_identity! {
    /// A Watched Root's identity: a surrogate id that survives renaming or moving its directory.
    ///
    /// The path cannot serve as the identity — it is a mutable attribute, and a Root that moves keeps
    /// its history — and an autoincrement could not survive a store copied to another machine. A ULID
    /// orders lexicographically by time, which is what makes a registry listing and a timeline agree
    /// without a sort key of their own.
    RootId
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
