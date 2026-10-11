//! Taking a Snapshot of a Watched Root, and reading its history back.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use keepster_core::{CapturePolicy, Ladder, RootId, RootPath, SnapshotId};
use keepster_store::{
    CaptureRequest, Comparison, Depth, DiffTarget, HistoryStore, Listing, Pass, RegistryError,
    Retained, RootRecord, StoreError, TimelinePage, TimelineQuery,
};

/// The directory inside a Root's own directory that holds its History Store.
///
/// A name of its own rather than the Root directory itself, so that the Registry's record, the derived
/// index and the store each have a place and none has to be told apart from the others.
const STORE_DIRECTORY: &str = "history.git";

/// The file inside a Root's own directory that holds its derived index.
///
/// Beside the store rather than inside it: nothing but SQLite may ever open this file (ticket 03), and
/// a file inside a git directory is one a git command might be pointed at.
const INDEX_FILE: &str = "index.sqlite";

/// Why a Snapshot could not be taken.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// No Root with that identity is registered.
    #[error("no Root with the identity {id} is registered")]
    NotFound {
        /// The identity that was asked for.
        id: RootId,
    },
    /// The Root's record cannot be read, so its path is unknown.
    #[error("the record of the Root {id} cannot be read")]
    Unreadable {
        /// The identity whose record is unreadable.
        id: RootId,
        /// Why it cannot be read.
        #[source]
        source: RegistryError,
    },
    /// The Root's directory is not there to capture.
    #[error("{path} is not a directory that can be captured")]
    RootUnavailable {
        /// Where the Root was last known to be.
        path: PathBuf,
    },
    /// The Registry itself failed.
    #[error(transparent)]
    Registry(#[from] RegistryError),
    /// The History Store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Where one Root's history lives: its History Store and the derived index beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreLocation {
    /// The History Store, a bare git repository.
    pub store: PathBuf,
    /// The derived index, a SQLite database that may be deleted at any time.
    pub index: PathBuf,
}

/// Where one Root's history lives, given the directory the Registry keeps it in.
pub fn location(root_dir: &Path) -> StoreLocation {
    StoreLocation {
        store: root_dir.join(STORE_DIRECTORY),
        index: root_dir.join(INDEX_FILE),
    }
}

/// One pass over the Root: walk it, read what the last pass's evidence cannot vouch for, and write a
/// Snapshot unless the request says an unchanged tree needs none.
///
/// `request.now` is a parameter all the way down (ADR 0012): it dates the Snapshot, names its Segment,
/// mints its identity and starts the pass, so a month of history can be produced by this path in a test
/// rather than waited for.
///
/// The Root's record and store location are taken as arguments rather than looked up here, so that the
/// caller can release its hold on the Registry before a walk that may take seconds — an unrelated Root
/// has no reason to wait for this one. The exclusive lock the capture does need is the store's own,
/// which the caller holds: git concedes a corruption risk under concurrent access, and two captures
/// racing would leave one of them unreferenced in any case (ticket 06).
pub fn pass(
    root: &RootRecord,
    location: &StoreLocation,
    policy: &CapturePolicy,
    request: &CaptureRequest,
) -> Result<Pass, CaptureError> {
    let store = open_for_capture(root, location)?;

    Ok(store.pass(root.id, &root.path, policy, request)?)
}

fn open_for_capture(
    root: &RootRecord,
    location: &StoreLocation,
) -> Result<HistoryStore, CaptureError> {
    if !root.path.is_dir() {
        return Err(CaptureError::RootUnavailable {
            path: root.path.clone(),
        });
    }

    Ok(HistoryStore::open(&location.store, &location.index)?)
}

/// Bring a Root's derived index up to date, rebuilding it if it is damaged or missing.
///
/// What the daemon does for every Root at start: a daemon killed between a ref update and an index
/// write left the index lagging, and the watermark repairs it here (ticket 03). A Root with no history
/// yet has nothing to catch up.
pub fn recover(location: &StoreLocation) -> Result<(), StoreError> {
    HistoryStore::recover(&location.store, &location.index).map(|_| ())
}

/// Open a Root's history for reading, or `None` if nothing was ever captured into it.
pub fn open_for_reading(location: &StoreLocation) -> Result<Option<HistoryStore>, StoreError> {
    HistoryStore::open_existing(&location.store, &location.index)
}

/// One page of a Root's chronology, newest first.
///
/// A Root never captured has an empty chronology rather than a missing one: the question has an
/// answer, and the answer is "nothing yet".
pub fn timeline(
    location: &StoreLocation,
    query: &TimelineQuery,
) -> Result<TimelinePage, StoreError> {
    match open_for_reading(location)? {
        Some(store) => store.timeline(query),
        None => Ok(TimelinePage {
            snapshots: Vec::new(),
            more: false,
        }),
    }
}

/// The newest Snapshot a Root has, or `None` when it has none.
pub fn newest(location: &StoreLocation) -> Result<Option<SnapshotId>, StoreError> {
    let page = timeline(
        location,
        &TimelineQuery {
            path: None,
            before: None,
            not_before: None,
            limit: 1,
        },
    )?;

    Ok(page.snapshots.first().map(|snapshot| snapshot.id))
}

/// One Retention pass over a Root's history at `now` under `ladder` (ticket 51): what survives is
/// planned from the Segments as they are, and carried out. A Root never captured has nothing to keep.
///
/// The caller holds the store's lock, as for a capture: re-chaining a Segment while a capture appends
/// to the same store would leave one of them unreferenced (ticket 06).
pub fn retain(
    location: &StoreLocation,
    ladder: &Ladder,
    now: SystemTime,
) -> Result<Retained, StoreError> {
    match open_for_reading(location)? {
        Some(store) => store.retain(ladder, now),
        None => Ok(Retained::default()),
    }
}

/// The paths one Snapshot holds under a directory.
pub fn entries(
    location: &StoreLocation,
    snapshot: SnapshotId,
    directory: &RootPath,
    depth: Depth,
) -> Result<Listing, StoreError> {
    match open_for_reading(location)? {
        Some(store) => store.entries(snapshot, directory, depth),
        None => Err(StoreError::SnapshotNotFound(snapshot)),
    }
}

/// What changed from the Snapshot `from` to `to`: every changed file without hunks, or the one `path`
/// names with them.
pub fn diff(
    location: &StoreLocation,
    from: SnapshotId,
    to: &DiffTarget<'_>,
    path: Option<&RootPath>,
) -> Result<Comparison, StoreError> {
    match open_for_reading(location)? {
        Some(store) => store.diff(from, to, path),
        None => Err(StoreError::SnapshotNotFound(from)),
    }
}

/// The same comparison as [`diff`], as the unified patch git prints.
pub fn unified(
    location: &StoreLocation,
    from: SnapshotId,
    to: &DiffTarget<'_>,
    path: Option<&RootPath>,
) -> Result<Vec<u8>, StoreError> {
    match open_for_reading(location)? {
        Some(store) => store.unified(from, to, path),
        None => Err(StoreError::SnapshotNotFound(from)),
    }
}
