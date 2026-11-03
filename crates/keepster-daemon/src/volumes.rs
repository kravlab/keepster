//! Whether the tree in front of the daemon is the Root that was registered (ticket 56, ADR 0011).
//!
//! An unmounted mount point is an ordinary empty directory, so every capture first compares the Root's
//! place on its volume with the one recorded at registration. A volume that is not there makes the Root
//! `Unavailable` — no Snapshot, nothing for Retention to age the real history behind. A volume mounted
//! somewhere else, with the Root at the same place within it, is followed. A Root whose volume could not
//! be identified is captured, but an automatic capture that would delete every path is held.

use std::collections::HashSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, TryLockError};

use keepster_api::Event;
use keepster_core::{RootId, Trigger, VolumePlace, VolumeVerdict, verify_volume, watch_blind};
pub use keepster_service::Volume;
use keepster_store::RootRecord;

use crate::DaemonState;

/// What the daemon asks the platform about volumes: the platform itself in production, a stand-in that
/// a test can unmount and remount at will.
pub trait Volumes: fmt::Debug + Send + Sync {
    /// The volume `path` is on.
    fn volume_of(&self, path: &Path) -> io::Result<Volume>;
    /// Every place the volume `identity` is mounted right now.
    fn mount_points(&self, identity: &str) -> io::Result<Vec<PathBuf>>;
}

/// The platform's own answers.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlatformVolumes;

impl Volumes for PlatformVolumes {
    fn volume_of(&self, path: &Path) -> io::Result<Volume> {
        keepster_service::volume_of(path)
    }

    fn mount_points(&self, identity: &str) -> io::Result<Vec<PathBuf>> {
        keepster_service::mount_points(identity)
    }
}

/// Where `path` sits on its volume, or `None` where the volume has no identity or cannot be asked.
pub(crate) fn place_of(volumes: &dyn Volumes, path: &Path) -> Option<VolumePlace> {
    let volume = volumes.volume_of(path).ok()?;

    Some(VolumePlace {
        identity: volume.identity?,
        within: volume.within,
    })
}

/// What the Root `record`'s volume says of the tree at its path now.
pub(crate) fn verdict(volumes: &dyn Volumes, record: &RootRecord) -> VolumeVerdict {
    let found = match &record.volume {
        Some(_) => place_of(volumes, &record.path),
        None => None,
    };

    verify_volume(record.volume.as_ref(), found.as_ref())
}

/// Whether the Root at `path` is on a filesystem the platform's watcher is documented as blind on.
pub(crate) fn blind(volumes: &dyn Volumes, path: &Path) -> bool {
    volumes
        .volume_of(path)
        .is_ok_and(|volume| watch_blind(crate::git::platform(), &volume.filesystem, volume.remote))
}

/// What a capture of the Root `id` by `trigger` may do, as its volume — and, for a Root following its
/// link, where the link leads (ticket 77) — says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Capture, holding a pass that would delete every path when `hold` says so.
    Capture {
        /// Whether nothing proves the tree is the one registered, so a total deletion is held.
        hold: bool,
        /// The directory the capture walks: the one judged here — for a Root following its link, where
        /// the link led as it was judged, so a link pointed elsewhere meanwhile is not walked through
        /// unjudged (ticket 77). `None` where there was no record to judge, which the capture's own
        /// lookup refuses.
        path: Option<PathBuf>,
    },
    /// Capture nothing: the volume at the Root's path is not the one registered.
    Elsewhere,
    /// Capture nothing: the Root follows its link, which is pointed inside the Root `root` now, and
    /// capturing would capture that Root's files a second time.
    Overlaps {
        /// The Root the link leads inside.
        root: RootId,
    },
}

/// Whether the Root `id` may be captured by `trigger` — following its volume first, if it is mounted
/// somewhere else now.
///
/// Every capture passes here holding the store's lock, so this is also where a Root whose link is
/// pointed inside another Root is refused, whatever the reload tick has seen of it yet (ticket 77).
///
/// The caller holds the store's lock, so no other capture of this Root moves it meanwhile.
pub(crate) fn gate(state: &DaemonState, id: RootId, trigger: Trigger) -> Gate {
    let records = state.registry().readable().unwrap_or_default();
    let Some(record) = records.iter().find(|record| record.id == id).cloned() else {
        // No record to compare: the capture's own lookup says why.
        return Gate::Capture {
            hold: false,
            path: None,
        };
    };
    let record = follow(state, record);
    // Resolved once: the directory judged to overlap no other Root is the one walked.
    let path = match keepster_app::roots::walk_path(&record, &records) {
        Ok(path) => Some(path),
        Err(root) => return Gate::Overlaps { root },
    };

    match verdict(&*state.volumes, &record) {
        VolumeVerdict::Ours => Gate::Capture { hold: false, path },
        // A capture someone asked for is their confirmation that an empty tree is what they mean.
        VolumeVerdict::Unproven => Gate::Capture {
            hold: trigger != Trigger::Manual,
            path,
        },
        // A missing directory is the capture's own `root_unavailable`; a directory on another volume is
        // this refusal.
        VolumeVerdict::Elsewhere if !record.path.is_dir() => Gate::Capture { hold: false, path },
        VolumeVerdict::Elsewhere => Gate::Elsewhere,
    }
}

/// The Root `record`, at the path its volume puts it now: moved there when its volume is mounted
/// somewhere else with the Root at the same place within it — and unmoved otherwise.
///
/// A Root following its link has where the link leads now recorded first (ticket 77), since it goes
/// wherever its link is pointed, as the reload tick records it: an action right after the link is
/// pointed elsewhere judges the new target's volume, never the old one's, so it sees no false
/// `volume_mismatch`. Such a Root is never moved after its volume: where it is, is where its link
/// leads, and the link is the person's to point.
///
/// The caller holds the Root's store lock, so no capture of it walks the old path meanwhile.
pub(crate) fn follow(state: &DaemonState, record: RootRecord) -> RootRecord {
    let record = retargeted(state, record);
    if record.path_mode == Some(keepster_core::PathMode::Link) {
        return record;
    }
    let Some(place) = record.volume.clone() else {
        return record;
    };
    let here = record.path.is_dir() && verdict(&*state.volumes, &record) == VolumeVerdict::Ours;
    if here {
        return record;
    }
    let Some(path) = remounted(&*state.volumes, &place, &record.path) else {
        return record;
    };

    let registry = state.registry();
    // Looked at again under the Registry's lock: a move the person made meanwhile is theirs, and wins.
    let current = registry
        .readable()
        .ok()
        .and_then(|records| records.into_iter().find(|current| current.id == record.id));
    match current {
        Some(current) if current.path == record.path => {}
        Some(current) => return current,
        None => return record,
    }
    match keepster_app::roots::follow(&registry, record.id, &path) {
        Ok(followed) => {
            tracing::info!(root = %record.id, from = %record.path.display(), to = %path.display(), "the Root's volume is mounted somewhere else now; followed it");
            state.publish(Event::RootMoved {
                root: record.id.to_string(),
            });
            crate::configuration::reload_with(state, &registry);
            followed
        }
        Err(error) => {
            tracing::warn!(root = %record.id, to = %path.display(), %error, "the Root's volume is mounted somewhere else, but the Root cannot follow it there");
            record
        }
    }
}

/// The Root `record` with where its link leads now recorded, if it follows a link pointed elsewhere
/// since — and as it was otherwise, a failure to record it said in the log.
fn retargeted(state: &DaemonState, record: RootRecord) -> RootRecord {
    if record.path_mode != Some(keepster_core::PathMode::Link) {
        return record;
    }
    let registry = state.registry();
    retarget_one(state, &registry, &record).unwrap_or(record)
}

/// Record anew where the Root `record`'s link leads, through `registry`, when it leads elsewhere now:
/// the record as re-recorded, or `None` when nothing was, a failure said in the log.
fn retarget_one(
    state: &DaemonState,
    registry: &keepster_store::Registry,
    record: &RootRecord,
) -> Option<RootRecord> {
    match keepster_app::roots::retarget(registry, record.id, &|path| {
        place_of(&*state.volumes, path)
    }) {
        Ok(Some(retargeted)) => {
            tracing::info!(root = %record.id, path = %record.path.display(), to = ?retargeted.target, "the Root's link leads elsewhere now; followed it");
            Some(retargeted)
        }
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(root = %record.id, %error, "the Root's link leads elsewhere now, but where could not be recorded");
            None
        }
    }
}

/// Record anew where each Root following its link is led now, with its place on its volume, wherever
/// the link has been pointed somewhere other than recorded (ticket 77): such a Root goes wherever its
/// link is pointed.
///
/// Done by the reload tick, holding the Registry, before the runtimes are brought in line with it: so a
/// runtime is started afresh on the new target in the same pass, and the state the pass announces is
/// the one that follows from the new record. Silent, as the person agreed to it by choosing to follow
/// the link; a Root whose link leads inside another Root is left as recorded, `Unavailable` instead.
pub(crate) fn retarget_every_root(state: &DaemonState, registry: &keepster_store::Registry) {
    let records = registry.readable().unwrap_or_default();
    let linked = records
        .iter()
        .filter(|record| record.path_mode == Some(keepster_core::PathMode::Link));
    for record in linked {
        retarget_one(state, registry, record);
    }
}

/// Where the volume `place` names puts the Root now, if it is mounted at exactly one other place with a
/// directory where the Root was within it — proven by asking that directory's volume, never guessed.
fn remounted(volumes: &dyn Volumes, place: &VolumePlace, current: &Path) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = volumes
        .mount_points(&place.identity)
        .ok()?
        .into_iter()
        .map(|mount_point| place.at(&mount_point))
        .filter(|path| path != current && path.is_dir())
        .filter(|path| place_of(volumes, path).as_ref() == Some(place))
        .collect();

    match candidates.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// Follow every watched Root whose volume is mounted somewhere else now, as housekeeping looks.
///
/// Each under its store's lock, as a capture follows it, so that no capture is walking the old path when
/// the Root moves; a Root whose store is busy is followed by that capture, or at the next look.
pub(crate) fn follow_every_root(state: &DaemonState) {
    let records = state.registry().readable().unwrap_or_default();
    for record in records {
        let store = state.store_lock(record.id);
        let _held = match store.try_lock() {
            Ok(held) => held,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => continue,
        };
        follow(state, record);
    }
}

/// What the last capture of each Root found that its state keeps saying until a capture finds otherwise —
/// kept by the daemon rather than by a Root's runtime, which a reload or a follow starts afresh.
#[derive(Debug, Default)]
pub(crate) struct Outcomes {
    /// Roots whose last capture was held: every path gone, and nothing proving the tree is theirs.
    held: Mutex<HashSet<RootId>>,
    /// Roots whose History Store the last capture could not open.
    unreachable: Mutex<HashSet<RootId>>,
}

impl Outcomes {
    /// A pass over the Root `id` ran, held or not: its store was reachable.
    pub(crate) fn passed(&self, id: RootId, held: bool) {
        let mut holding = lock(&self.held);
        if held {
            holding.insert(id);
        } else {
            holding.remove(&id);
        }
        lock(&self.unreachable).remove(&id);
    }

    /// A capture of the Root `id` found nowhere to write.
    pub(crate) fn unreachable(&self, id: RootId) {
        lock(&self.unreachable).insert(id);
    }

    /// Whether the Root `id`'s last capture was held.
    pub(crate) fn is_held(&self, id: RootId) -> bool {
        lock(&self.held).contains(&id)
    }

    /// Whether the Root `id`'s last capture found its store unreachable.
    pub(crate) fn is_unreachable(&self, id: RootId) -> bool {
        lock(&self.unreachable).contains(&id)
    }
}

fn lock(set: &Mutex<HashSet<RootId>>) -> std::sync::MutexGuard<'_, HashSet<RootId>> {
    set.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The refusal a capture of the Root at `path`, whose link is pointed inside the Root `root`, earns
/// (ticket 77).
pub(crate) fn overlapping(path: &Path, root: RootId) -> crate::problem::ProblemResponse {
    crate::problem::ProblemResponse::new(
        axum::http::StatusCode::CONFLICT,
        keepster_api::ErrorCode::RootOverlaps,
        format!(
            "{} leads inside the Root {root} now, so nothing is captured from it until the link is \
             pointed back out of it",
            path.display()
        ),
    )
    .with_detail("path", path.to_string_lossy())
    .with_detail("root", root.to_string())
}

/// The refusal a capture or a Restore of a Root on a volume other than the one registered earns.
pub(crate) fn elsewhere(path: &Path) -> crate::problem::ProblemResponse {
    crate::problem::ProblemResponse::new(
        axum::http::StatusCode::CONFLICT,
        keepster_api::ErrorCode::VolumeMismatch,
        format!(
            "{} is not on the volume it was registered on — an empty mount point, most likely — so \
             nothing is captured from or restored into it; its history reads as ever",
            path.display()
        ),
    )
    .with_detail("path", path.to_string_lossy())
}
