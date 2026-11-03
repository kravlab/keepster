//! Registering, listing and deregistering Watched Roots.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use keepster_core::{PathMode, RootId, SnapshotId, VolumePlace};
use keepster_path::real_path;
use keepster_store::{
    HeldEntry, Registry, RegistryEntry, RegistryError, RootRecord, Standing, StoreError,
};

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
        /// The path the Root would be registered at.
        path: PathBuf,
        /// The Root standing in the way.
        existing: RootId,
    },
    /// The path passes through a link, and the caller has not said whether the Root follows the link
    /// or its target (ticket 77).
    #[error(transparent)]
    PathModeRequired(#[from] PathModeRequired),
    /// The Registry itself failed.
    #[error(transparent)]
    Registry(#[from] RegistryError),
}

/// A path through a link given without saying whether the Root follows the link or its target (ticket
/// 77), as registering and moving both refuse it.
#[derive(Debug, thiserror::Error)]
#[error(
    "{path} passes through a link to {real}: say whether the Root follows the link or its target"
)]
pub struct PathModeRequired {
    /// The path as the caller gave it, made absolute.
    pub path: PathBuf,
    /// Where it really is.
    pub real: PathBuf,
}

/// Register a directory as a Watched Root and return what the Registry now holds.
///
/// A directory unwatched before is watched again under its old identity, its history decayed down the
/// ladder meanwhile rather than frozen, so there is nothing to warn about (ticket 53). The match is on
/// the path exactly as it was registered: the same rule that keeps moves explicit. Failing that, it is on
/// where that path really is, or where its link led when last recorded, so that registering through a
/// link in the other path mode finds the same Root.
///
/// The path is made absolute. One that passes through a link is not registered until `path_mode` says
/// whether the Root follows the link or its target (ticket 77): resolving it silently would decide for
/// the person. A Root following its target is registered at the real path; one following the link at
/// the path as given, with where the link leads. A path through no link records no path mode, and one
/// given is ignored. `now` is a parameter because the identity embeds it (ADR 0012).
pub fn register(
    registry: &Registry,
    now: SystemTime,
    path: &Path,
    path_mode: Option<PathMode>,
    place: &dyn Fn(&Path) -> Option<VolumePlace>,
) -> Result<RootRecord, RegisterError> {
    let given = path;
    let path = std::path::absolute(given).map_err(|source| RegisterError::Unresolvable {
        path: given.to_path_buf(),
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

    let unresolvable = |source| RegisterError::Unresolvable {
        path: given.to_path_buf(),
        source,
    };
    let real = real_path(&path).map_err(unresolvable)?;
    let linked = keepster_path::through_link(&path).map_err(unresolvable)?;
    let Placement {
        path,
        path_mode,
        target,
    } = placement(path, real.clone(), linked, path_mode)?;

    let existing = registry.readable()?;
    if let Some(existing) = overlapped(&path, &real, &existing) {
        return Err(RegisterError::Overlaps { path, existing });
    }

    let unwatched = registry.unwatched()?;
    // The path as recorded first; then where the record led or really is, so that the same link
    // registered in the other mode is the same Root (ticket 77). Only Roots no longer watched are
    // looked at, and registration has just kept this one apart from every watched Root.
    let matched = unwatched
        .iter()
        .rev()
        .find(|record| record.path == path)
        .or_else(|| {
            unwatched.iter().rev().find(|record| {
                record.target.as_ref() == Some(&real)
                    || real_path(&record.path).is_ok_and(|recorded| recorded == real)
            })
        })
        .cloned();
    if let Some(unwatched) = matched {
        registry.rewatch(unwatched.id)?;
        // Watched again is registered again: its path, where it sits on its volume, and how it
        // resolves a link, are recorded as they are now.
        let record = RootRecord {
            id: unwatched.id,
            volume: place(&path),
            path,
            path_mode,
            target,
        };
        registry.insert(&record)?;
        return Ok(record);
    }

    // Where it sits on its volume, recorded now so that every capture can tell this tree from an empty
    // mount point (ADR 0011).
    let volume = place(&path);
    let record = RootRecord {
        id: RootId::generate(now),
        path,
        volume,
        path_mode,
        target,
    };
    registry.insert(&record)?;

    Ok(record)
}

/// Every registered Root, in registration order, including the ones whose record cannot be read.
///
/// Reporting is the one caller that must see a broken Root rather than skip it: ticket 08 makes such a
/// Root a value, and `GLOSSARY.md` calls that value `Unavailable`.
pub fn list(registry: &Registry) -> Result<Vec<RegistryEntry>, RegistryError> {
    registry.list()
}

/// Every Root no longer watched whose history is kept, in the Registry's order, including the ones
/// whose record cannot be read: its identity alone reaches its history.
pub fn unwatched(registry: &Registry) -> Result<Vec<HeldEntry>, RegistryError> {
    Ok(registry
        .held()?
        .into_iter()
        .filter(|held| held.standing == Standing::Unwatched)
        .collect())
}

/// The Root holding `path`, if any — the answer behind "which Root am I in".
///
/// The path is made absolute first, so a caller may pass one relative to its own working directory.
/// It is compared by where it and each Root really are, and only then with each Root as recorded
/// (ticket 77), so the answer is the one the CLI finds standing there, its working directory resolved:
/// a path spelt through a link finds the Root registered at its target, even where the spelling runs
/// through another Root, and a path inside where a link-mode Root leads finds that Root. A path not
/// made yet is resolved through its nearest ancestor that is, and a recorded path that does not resolve
/// stands for itself.
pub fn resolve(registry: &Registry, path: &Path) -> Result<Option<RootRecord>, RegistryError> {
    let Ok(path) = std::path::absolute(path) else {
        return Ok(None);
    };

    let records = registry.readable()?;
    let found = real_or_nearest(&path)
        .and_then(|real| keepster_core::containing(&real, &real_paths_of(&records)))
        .or_else(|| keepster_core::containing(&path, &paths_of(&records)));

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
    /// The new path passes through a link, and the caller has not said whether the Root follows the
    /// link or its target (ticket 77).
    #[error(transparent)]
    PathModeRequired(#[from] PathModeRequired),
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

/// Erase one Root's history (ticket 68): a watched Root stays registered and watched, and its next
/// capture starts the history over; one no longer watched, whose history was all it still had, is
/// erased whole, as `deregister` with `purge` erases it. The caller holds the store's lock, or knows no
/// daemon runs.
pub fn erase_history(registry: &Registry, id: RootId) -> Result<(), RootError> {
    if registry.contains(id) {
        snapshots::erase_history(&registry.root_dir(id))
    } else if registry.holds(id) {
        Ok(registry.purge(id)?)
    } else {
        Err(RootError::NotFound { id })
    }
}

/// What `keepster uninstall` erases besides the autostart entries (ticket 68), each only when asked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Erasure {
    /// Every Root's history: a watched Root stays registered, to start its history over; one no longer
    /// watched, which held nothing else, is erased whole.
    pub history: bool,
    /// Every Root's registration: each watched Root is unwatched as `unwatch` does it, its last state
    /// pinned and its history left to decay. With `history` too, every Root is erased whole.
    pub roots: bool,
}

/// What [`erase`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Erased {
    /// How many Roots' histories were erased.
    pub histories: usize,
    /// How many watched Roots were unwatched.
    pub unwatched: usize,
}

/// Erase what `erasure` selects from every Root the Registry holds, watched or not.
///
/// Nothing reaches here without the user's separate consent, and no daemon may be running: the caller
/// has stopped it, so no store lock is taken. A Root whose record cannot be read is still erased, since
/// its identity — the directory's name — is all the erasing needs.
pub fn erase(registry: &Registry, erasure: Erasure) -> Result<Erased, RootError> {
    let mut erased = Erased::default();
    for held in registry.held()? {
        let watched = held.standing == Standing::Watched;
        match (erasure.history, erasure.roots, watched) {
            (true, true, _) => {
                registry.purge(held.id)?;
                erased.histories += 1;
            }
            (true, false, _) => {
                erase_history(registry, held.id)?;
                erased.histories += 1;
            }
            (false, true, true) => {
                deregister(registry, held.id, false)?;
                erased.unwatched += 1;
            }
            (false, _, _) => {}
        }
    }

    Ok(erased)
}

/// Record that a Root's directory is now somewhere else.
///
/// A move is explicit because to the daemon it is indistinguishable from a deletion — no platform
/// reliably announces a directory move — so any auto-detection would be a heuristic that one day grafts
/// a stranger's history onto yours (ticket 07). The identity does not change, so the history does not
/// move with it.
///
/// The Root is somewhere the person chose, so where it sits on its volume is recorded again, as at
/// registration, and a new path through a link takes `path_mode` as registering does (ticket 77).
pub fn move_root(
    registry: &Registry,
    id: RootId,
    path: &Path,
    path_mode: Option<PathMode>,
    place: &dyn Fn(&Path) -> Option<VolumePlace>,
) -> Result<RootRecord, RootError> {
    relocate(
        registry,
        id,
        path,
        |path| {
            let unresolvable = |source| RootError::NotADirectory {
                path: path.clone(),
                source: Some(source),
            };
            let real = real_path(&path).map_err(unresolvable)?;
            let linked = keepster_path::through_link(&path).map_err(unresolvable)?;
            Ok(placement(path, real, linked, path_mode)?)
        },
        |path| place(path),
    )
}

/// Record that a Root's volume is mounted somewhere else now, with the Root at the same place within
/// it (ADR 0011).
///
/// Not a move: the operating system chose the mount point and the person did nothing, and the volume's
/// identity is the proof — not a heuristic — that this is the same tree. Ticket 07's no-overlap rule is
/// run all the same, since the new mount point may lie inside another Root. How the Root resolves a
/// link is kept as recorded.
pub fn follow(registry: &Registry, id: RootId, path: &Path) -> Result<RootRecord, RootError> {
    let recorded = registry
        .readable()?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or(RootError::NotFound { id })?;

    relocate(
        registry,
        id,
        path,
        |path| {
            Ok(Placement {
                path,
                path_mode: recorded.path_mode,
                target: recorded.target.clone(),
            })
        },
        |_| recorded.volume.clone(),
    )
}

/// Record where the link-mode Root `id`'s link leads now, and its place on its volume as `place` says,
/// when the link has been pointed somewhere other than recorded (ticket 77): a Root following its link
/// goes where the link leads now — the record as re-recorded, or `None` when there was nothing to
/// record.
///
/// A Root following its link has agreed to go wherever the link is pointed, so a new target is not the
/// tree ADR 0011 guards against — an empty mount point in place of the registered volume — and its
/// volume is recorded anew rather than reported as `volume_mismatch`. A link leading where it was
/// recorded keeps the recorded place, so a volume gone from under it is still a mismatch. Nothing is
/// recorded for a Root that does not follow a link, one whose link leads nowhere (`root_missing`), or
/// one whose link leads inside another Root ([`overlapped_now`]).
pub fn retarget(
    registry: &Registry,
    id: RootId,
    place: &dyn Fn(&Path) -> Option<VolumePlace>,
) -> Result<Option<RootRecord>, RootError> {
    let records = registry.readable()?;
    let record = records
        .iter()
        .find(|record| record.id == id)
        .cloned()
        .ok_or(RootError::NotFound { id })?;
    if record.path_mode != Some(PathMode::Link) {
        return Ok(None);
    }
    let Ok(real) = real_path(&record.path) else {
        return Ok(None);
    };
    // Led inside another Root, the Root is `Unavailable` rather than followed there, since two Roots
    // must never capture the same files: what was recorded stands, for the link pointed back.
    if record.target.as_ref() == Some(&real) || overlapped_now(&record, &records).is_some() {
        return Ok(None);
    }

    let record = RootRecord {
        volume: place(&record.path),
        target: Some(real),
        ..record
    };
    registry.insert(&record)?;

    Ok(Some(record))
}

/// Record the Root `id` where `placement` puts `path`, which must be a directory overlapping no other
/// Root by recorded or real paths, with its place on its volume as `place` says.
fn relocate(
    registry: &Registry,
    id: RootId,
    path: &Path,
    placement: impl FnOnce(PathBuf) -> Result<Placement, RootError>,
    place: impl Fn(&Path) -> Option<VolumePlace>,
) -> Result<RootRecord, RootError> {
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

    let Placement {
        path,
        path_mode,
        target,
    } = placement(path)?;
    // The directory was just found, so it resolves; were it gone already, its path stands for itself.
    let real = real_path(&path).unwrap_or_else(|_| path.clone());

    // The no-overlap rule holds for a move as much as for a registration, or it is bypassed in two
    // steps. The Root being moved is excluded, since it cannot overlap itself.
    let others: Vec<_> = records
        .iter()
        .filter(|record| record.id != id)
        .cloned()
        .collect();
    if let Some(existing) = overlapped(&path, &real, &others) {
        return Err(RootError::Overlaps { path, existing });
    }

    let volume = place(&path);
    let record = RootRecord {
        id,
        path,
        volume,
        path_mode,
        target,
    };
    registry.insert(&record)?;

    Ok(record)
}

/// Which of `records` a Root at `path`, which really is at `real`, would overlap, if any.
///
/// Ticket 07's no-overlap rule is stated over real paths, so the paths are compared as recorded and
/// then as they resolve (ticket 77): a Root through a link must not nest inside another's real tree,
/// nor another inside its own, whatever the typed paths say. A recorded path that does not resolve,
/// such as a directory gone missing, stands for itself. Both sides of the second comparison come from
/// the filesystem, which spells them as they are on disk, so no case folding is needed on Windows.
fn overlapped(path: &Path, real: &Path, records: &[RootRecord]) -> Option<RootId> {
    keepster_core::overlapping(path, &paths_of(records))
        .or_else(|| keepster_core::overlapping(real, &real_paths_of(records)))
}

/// Which of `records` holds, in its real tree, where the link-mode Root `record` leads now — the same
/// directory included (ticket 77).
///
/// A Root following its link goes wherever the link is pointed, which may be inside another Root: the
/// two would then capture the same files, so the Root is `Unavailable` until the link is pointed back
/// out. Only a link-mode Root can come to overlap this way, since every other Root is where it was
/// registered and registration keeps Roots apart; a link pointed at a directory that *contains* another
/// Root is no overlap, as the capture Withholds that Root. Of two Roots following their links to the
/// same directory, only the one registered later overlaps. `records` may include `record` itself, which
/// never counts. `None` as well where the link leads nowhere, which is the Root's `root_missing`.
pub fn overlapped_now(record: &RootRecord, records: &[RootRecord]) -> Option<RootId> {
    walk_path(record, records).err()
}

/// Where a capture of the Root `record` walks: for a Root following its link, where the link leads as
/// this looks, resolved once, so that the walk goes over the very directory found to overlap no other
/// of `records` rather than resolving the link again (ticket 77); for any other Root, its path. Where
/// the link leads inside another Root, that Root instead, as [`overlapped_now`] says. A link that leads
/// nowhere is walked at its path, which the capture finds missing.
pub fn walk_path(record: &RootRecord, records: &[RootRecord]) -> Result<PathBuf, RootId> {
    if record.path_mode != Some(PathMode::Link) {
        return Ok(record.path.clone());
    }
    let Ok(real) = real_path(&record.path) else {
        return Ok(record.path.clone());
    };
    // Two Roots following their links to the one directory would each hold the other: the one
    // registered first keeps it. A Root's identity embeds the moment it was registered, so identities
    // order by that moment, and by identity within it.
    let others: Vec<(RootId, PathBuf)> = real_paths_of(records)
        .into_iter()
        .zip(records)
        .filter(|((id, other_real), other)| {
            let later_to_the_same =
                other.path_mode == Some(PathMode::Link) && *other_real == real && *id > record.id;
            *id != record.id && !later_to_the_same
        })
        .map(|(other, _)| other)
        .collect();

    match keepster_core::containing(&real, &others) {
        Some(root) => Err(root),
        None => Ok(real),
    }
}

/// Each of `records` with the path it is recorded at.
fn paths_of(records: &[RootRecord]) -> Vec<(RootId, PathBuf)> {
    records
        .iter()
        .map(|record| (record.id, record.path.clone()))
        .collect()
}

/// Each of `records` with where its recorded path really is (ticket 77); one that does not resolve,
/// such as a directory gone missing, stands for itself.
fn real_paths_of(records: &[RootRecord]) -> Vec<(RootId, PathBuf)> {
    records
        .iter()
        .map(|record| {
            let real = real_path(&record.path).unwrap_or_else(|_| record.path.clone());
            (record.id, real)
        })
        .collect()
}

/// Where a Root is recorded, and how it resolves the link its path passes through (ticket 77).
struct Placement {
    /// The path the Root is recorded at: as given, or its real path for a `canonical` Root.
    path: PathBuf,
    /// How the Root resolves the link its path passes through; `None` for a path through no link.
    path_mode: Option<PathMode>,
    /// Where the link leads, recorded for a `link` Root only.
    target: Option<PathBuf>,
}

/// Where the Root given at `absolute`, which really is at `real` and passes through a link when `linked`
/// says so, is recorded under `path_mode` — or, for a path through a link with no path mode, both paths
/// back in the refusal, for the caller to ask with.
///
/// A path passes through a link when any of its components is one, as
/// [`keepster_path::through_link`] asks the filesystem.
fn placement(
    absolute: PathBuf,
    real: PathBuf,
    linked: bool,
    path_mode: Option<PathMode>,
) -> Result<Placement, PathModeRequired> {
    if !linked {
        return Ok(Placement {
            path: absolute,
            path_mode: None,
            target: None,
        });
    }

    match path_mode {
        None => Err(PathModeRequired {
            path: absolute,
            real,
        }),
        Some(PathMode::Canonical) => Ok(Placement {
            path: real,
            path_mode: Some(PathMode::Canonical),
            target: None,
        }),
        Some(PathMode::Link) => Ok(Placement {
            path: absolute,
            path_mode: Some(PathMode::Link),
            target: Some(real),
        }),
    }
}

/// Where `path` really is: its nearest ancestor that exists, resolved as [`real_path`] resolves it, and
/// the rest as written — so that a path not made yet beneath a link still says where it would be. `None`
/// when no ancestor resolves.
pub fn real_or_nearest(path: &Path) -> Option<PathBuf> {
    let mut missing = Vec::new();
    let mut at = path;
    loop {
        if let Ok(real) = real_path(at) {
            return Some(
                missing
                    .iter()
                    .rev()
                    .fold(real, |real, name| real.join(name)),
            );
        }
        missing.push(at.file_name()?);
        at = at.parent()?;
    }
}
