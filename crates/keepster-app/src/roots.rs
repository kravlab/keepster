//! Registering, listing and deregistering Watched Roots.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use keepster_core::{RootId, SnapshotId};
use keepster_store::{Registry, RegistryEntry, RegistryError, RootRecord, StoreError};

use crate::snapshots;

/// Why a directory could not become a Watched Root.
#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    /// The path does not exist, or is not a directory.
    #[error("{path} is not a directory that can be watched")]
    NotADirectory {
        /// The path as the caller gave it.
        path: PathBuf,
        /// What the filesystem said, where it said anything.
        #[source]
        source: Option<io::Error>,
    },
    /// The path could not be made absolute.
    #[error("{path} could not be resolved to an absolute path")]
    Unresolvable {
        /// The path as the caller gave it.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: io::Error,
    },
    /// Another Root already covers this path, or would be covered by it.
    #[error("{path} overlaps the Root {existing}")]
    Overlaps {
        /// The path as the caller gave it, made absolute.
        path: PathBuf,
        /// The Root standing in the way.
        existing: RootId,
    },
    /// The Registry itself failed.
    #[error(transparent)]
    Registry(#[from] RegistryError),
}

/// Register a directory as a Watched Root and return what the Registry now holds.
///
/// A directory unwatched before is watched again under its old identity, its history decayed down the
/// ladder meanwhile rather than frozen, so there is nothing to warn about (ticket 53). The match is on
/// the path exactly as it was registered: the same rule that keeps moves explicit.
///
/// The path is made absolute but **not** canonicalised: resolving symlinks here would silently decide
/// something ticket 49 owns — whether a Root registered through a link follows the link or its target.
/// `now` is a parameter because the identity embeds it (ADR 0012).
pub fn register(
    registry: &Registry,
    now: SystemTime,
    path: &Path,
) -> Result<RootRecord, RegisterError> {
    let path = std::path::absolute(path).map_err(|source| RegisterError::Unresolvable {
        path: path.to_path_buf(),
        source,
    })?;

    match path.metadata() {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(RegisterError::NotADirectory { path, source: None });
        }
        Err(source) => {
            return Err(RegisterError::NotADirectory {
                path,
                source: Some(source),
            });
        }
    }

    let existing = registry.readable()?;
    if let Some(existing) = keepster_core::overlapping(&path, &paths_of(&existing)) {
        return Err(RegisterError::Overlaps { path, existing });
    }

    if let Some(unwatched) = registry
        .unwatched()?
        .into_iter()
        .rev()
        .find(|record| record.path == path)
    {
        registry.rewatch(unwatched.id)?;
        return Ok(unwatched);
    }

    let record = RootRecord {
        id: RootId::generate(now),
        path,
    };
    registry.insert(&record)?;

    Ok(record)
}

/// Every registered Root, in registration order, including the ones whose record cannot be read.
///
/// Reporting is the one caller that must see a broken Root rather than skip it: ticket 08 makes such a
/// Root a value, and `CONTEXT.md` calls that value `Unavailable`.
pub fn list(registry: &Registry) -> Result<Vec<RegistryEntry>, RegistryError> {
    registry.list()
}

/// The Root holding `path`, if any — the answer behind "which Root am I in".
///
/// The path is made absolute first, so a caller may pass one relative to its own working directory.
pub fn resolve(registry: &Registry, path: &Path) -> Result<Option<RootRecord>, RegistryError> {
    let Ok(path) = std::path::absolute(path) else {
        return Ok(None);
    };

    let records = registry.readable()?;
    let found = keepster_core::containing(&path, &paths_of(&records));

    Ok(found.and_then(|id| records.into_iter().find(|record| record.id == id)))
}

/// Why a Root could not be deregistered or moved.
#[derive(Debug, thiserror::Error)]
pub enum RootError {
    /// No Root with that identity is registered.
    #[error("no Root with the identity {id} is registered")]
    NotFound {
        /// The identity that was asked for.
        id: RootId,
    },
    /// The new path would overlap another Root.
    #[error("{path} overlaps the Root {existing}")]
    Overlaps {
        /// The path as the caller gave it, made absolute.
        path: PathBuf,
        /// The Root standing in the way.
        existing: RootId,
    },
    /// The new path is not a directory that can be watched.
    #[error("{path} is not a directory that can be watched")]
    NotADirectory {
        /// The path as the caller gave it, made absolute.
        path: PathBuf,
        /// What the filesystem said, where it said anything — a permission denial must not be reported
        /// as "not a directory".
        #[source]
        source: Option<io::Error>,
    },
    /// The Registry itself failed.
    #[error(transparent)]
    Registry(#[from] RegistryError),
    /// The history could not be pinned before the Root stopped being watched.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Deregister a Root, keeping its History Store — or, with `purge`, erase that store too — and say
/// which Snapshot was pinned for it.
///
/// Unwatching is often temporary and the history is the insurance the tool exists to provide, so
/// erasing takes an explicit request; the watched directory itself is never touched either way. The
/// history kept goes on decaying down the ladder, to its newest Snapshot, pinned here first so that it
/// survives (ticket 07) — before the Root stops being watched, so a Root never decays unpinned, and a
/// history that cannot be pinned is not unwatched. A Root no longer watched can still be purged: `purge`
/// is the one way to erase a history, whatever became of its directory. The caller holds the store's
/// lock.
pub fn deregister(
    registry: &Registry,
    id: RootId,
    purge: bool,
) -> Result<Option<SnapshotId>, RootError> {
    let watched = registry.contains(id);
    if !watched && !(purge && registry.holds(id)) {
        return Err(RootError::NotFound { id });
    }

    if purge {
        registry.purge(id)?;
        return Ok(None);
    }
    let pinned = snapshots::pin_newest(&snapshots::location(&registry.root_dir(id)))?;
    registry.remove(id)?;

    Ok(pinned)
}

/// Record that a Root's directory is now somewhere else.
///
/// A move is explicit because to the daemon it is indistinguishable from a deletion — no platform
/// reliably announces a directory move — so any auto-detection would be a heuristic that one day grafts
/// a stranger's history onto yours (ticket 07). The identity does not change, so the history does not
/// move with it.
pub fn move_root(registry: &Registry, id: RootId, path: &Path) -> Result<RootRecord, RootError> {
    let records = registry.readable()?;
    if !records.iter().any(|record| record.id == id) {
        return Err(RootError::NotFound { id });
    }

    let path = std::path::absolute(path).map_err(|source| RootError::NotADirectory {
        path: path.to_path_buf(),
        source: Some(source),
    })?;
    match path.metadata() {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(RootError::NotADirectory { path, source: None }),
        Err(source) => {
            return Err(RootError::NotADirectory {
                path,
                source: Some(source),
            });
        }
    }

    // The no-overlap rule holds for a move as much as for a registration, or it is bypassed in two
    // steps. The Root being moved is excluded, since it cannot overlap itself.
    let others: Vec<_> = records
        .iter()
        .filter(|record| record.id != id)
        .cloned()
        .collect();
    if let Some(existing) = keepster_core::overlapping(&path, &paths_of(&others)) {
        return Err(RootError::Overlaps { path, existing });
    }

    let record = RootRecord { id, path };
    registry.insert(&record)?;

    Ok(record)
}

fn paths_of(records: &[RootRecord]) -> Vec<(RootId, PathBuf)> {
    records
        .iter()
        .map(|record| (record.id, record.path.clone()))
        .collect()
}
