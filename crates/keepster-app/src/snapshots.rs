//! Taking a Snapshot of a Watched Root.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use keepster_core::RootId;
use keepster_store::{CapturedSnapshot, HistoryStore, RegistryError, RootRecord, StoreError};

/// The directory inside a Root's own directory that holds its History Store.
///
/// A name of its own rather than the Root directory itself, so that the Registry's record, the derived
/// index of ticket 39 and the store each have a place and none has to be told apart from the others.
const STORE_DIRECTORY: &str = "history.git";

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

/// Where one Root's history lives, given the directory the Registry keeps it in.
pub fn store_path(root_dir: &Path) -> PathBuf {
    root_dir.join(STORE_DIRECTORY)
}

/// Capture the Root's current contents.
///
/// `now` is a parameter all the way down (ADR 0012): it dates the Snapshot, names its Segment and mints
/// its identity, so a month of history can be produced by this path in a test rather than waited for.
///
/// The Root's record and store location are taken as arguments rather than looked up here, so that the
/// caller can release its hold on the Registry before a walk that may take seconds — an unrelated Root
/// has no reason to wait for this one. The exclusive lock the capture does need is the store's own,
/// which the caller holds: git concedes a corruption risk under concurrent access, and two captures
/// racing would leave one of them unreferenced in any case (ticket 06).
pub fn capture(
    root: &RootRecord,
    store_path: &Path,
    now: SystemTime,
) -> Result<CapturedSnapshot, CaptureError> {
    if !root.path.is_dir() {
        return Err(CaptureError::RootUnavailable {
            path: root.path.clone(),
        });
    }

    let store = HistoryStore::open(store_path)?;

    Ok(store.capture(root.id, &root.path, now)?)
}
