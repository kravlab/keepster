//! Pins (ticket 53, ADR 0002): a pinned Snapshot re-created as a parentless commit under its own ref, so
//! reachability itself keeps it — Retention, thinning and a Maintenance Pass never need to know.

use keepster_core::SnapshotId;

use super::{HistoryStore, PIN_REFS, SnapshotSummary, StoreError};

impl HistoryStore {
    /// Pin the Snapshot `id`, saying whether it was not pinned already.
    ///
    /// The Pin is the Snapshot's own commit again — its tree, its message, its signatures — without a
    /// parent: one commit object, nothing else written, and the same commit however often it is made.
    /// Pinning a pinned Snapshot changes nothing, since the ref's name is the Snapshot's (ticket 07); a
    /// ref under that name that is not this Snapshot's Pin — written by hand, or broken — is replaced,
    /// since the index would not count it and Retention would take the Snapshot it seems to hold.
    ///
    /// The caller holds the store's lock, as for a capture: a Maintenance Pass pruning between the read
    /// of the Snapshot and the write of its Pin would leave the Pin naming a tree that is gone.
    pub fn pin(&self, id: SnapshotId) -> Result<bool, StoreError> {
        let reference = pin_ref(id);
        if self.pinned_at(&reference)? == Some(id) {
            return Ok(false);
        }

        let original = self.snapshot_commit(id)?;
        let tree = original.tree().map_err(self.failed("read"))?;
        let message = original.message_raw().map_err(|_| {
            self.failed("read")(git2::Error::from_str(
                "the Snapshot's message is not UTF-8, so no Pin can repeat it",
            ))
        })?;
        let pin = self
            .repository
            .commit(
                None,
                &original.author(),
                &original.committer(),
                message,
                &tree,
                &[],
            )
            .map_err(self.failed("written"))?;
        self.repository
            .reference(&reference, pin, true, "pin")
            .map_err(self.failed("written"))?;
        self.catch_up()?;

        Ok(true)
    }

    /// Unpin the Snapshot `id`, saying whether it was pinned. The Snapshot itself stays for as long as
    /// its Segment holds it; unpinned, it is Retention's again.
    pub fn unpin(&self, id: SnapshotId) -> Result<bool, StoreError> {
        let reference = pin_ref(id);
        let mut found = match self.repository.find_reference(&reference) {
            Ok(found) => found,
            Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(false),
            Err(source) => return Err(self.failed("read")(source)),
        };
        found.delete().map_err(self.failed("written"))?;
        self.catch_up()?;

        Ok(true)
    }

    /// Every pinned Snapshot, newest first.
    pub fn pins(&self) -> Result<Vec<SnapshotSummary>, StoreError> {
        self.catch_up()?;

        Ok(self
            .index()?
            .pinned()?
            .into_iter()
            .map(SnapshotSummary::from)
            .collect())
    }

    /// Which Snapshot the Pin at `reference` holds, if it holds one as a Pin does: one parentless commit
    /// carrying the Snapshot's own trailers.
    fn pinned_at(&self, reference: &str) -> Result<Option<SnapshotId>, StoreError> {
        let found = match self.repository.find_reference(reference) {
            Ok(found) => found,
            Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(source) => return Err(self.failed("read")(source)),
        };

        Ok(found
            .peel_to_commit()
            .ok()
            .filter(|commit| commit.parent_count() == 0)
            .and_then(|commit| commit.message().ok().and_then(crate::index::trailers))
            .map(|trailers| trailers.id))
    }
}

/// The ref that holds the Pin of `id`.
fn pin_ref(id: SnapshotId) -> String {
    format!("{PIN_REFS}/{id}")
}
