//! All git plumbing and the derived index; the only crate that names a git concept.
//!
//! It also owns the Registry, which is not a git concept but is persistence: ticket 03 splits the
//! daemon's state by whether git can reproduce it, and this crate is where both halves live so that no
//! use-case above it can write them in the wrong order.

mod format;
mod git;
mod history;
mod index;
mod registry;

pub use git::{Git, GitUnavailable};
pub use history::{
    CaptureRequest, CapturedSnapshot, Comparison, Content, DIFF_MAX_SIZE, Depth, DiffLine,
    DiffTarget, Entry, EntryKind, FileChange, FileDiff, Forgotten, HistoryStore, Hunk,
    IncompletePath, LineKind, LinkNames, Listing, Maintained, Pass, Preflight, PreviewOptions,
    RestoreScope, Retained, SnapshotSummary, StoreError, TimelinePage, TimelineQuery, WithheldPath,
    Written, destination_naming, preview,
};
pub use index::INDEX_SCHEMA_VERSION;
pub use keepster_core::WithheldReason;
pub use registry::{CONTAINER_VERSION, Registry, RegistryEntry, RegistryError, RootRecord};
