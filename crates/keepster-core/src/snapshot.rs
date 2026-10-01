//! The identity of a Snapshot.

use crate::identity::ulid_identity;

ulid_identity! {
    /// A Snapshot's stable public identity, deliberately not the commit hash.
    ///
    /// Thinning a Segment recreates its commits (ADR 0001), so anything that referred to a Snapshot by
    /// hash would break the moment Retention ran. Being time-ordered, it also gives the timeline its
    /// ordering across Segments for free, and its embedded timestamp is a cheap Segment candidate when
    /// the derived index is rebuilt from git alone.
    SnapshotId
}
