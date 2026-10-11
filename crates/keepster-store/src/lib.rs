//! All git plumbing and the derived index; the only crate that names a git concept.
//!
//! It also owns the Registry, which is not a git concept but is persistence: ticket 03 splits the
//! daemon's state by whether git can reproduce it, and this crate is where both halves live so that no
//! use-case above it can write them in the wrong order.

mod history;
mod index;
mod registry;

pub use history::{
    CaptureRequest, CapturedSnapshot, Content, DIFF_MAX_SIZE, Depth, DiffLine, DiffTarget, Entry,
    EntryKind, FileChange, FileDiff, HistoryStore, Hunk, LineKind, Pass, SnapshotSummary,
    StoreError, TimelinePage, TimelineQuery, WithheldPath, WithheldReason,
};
pub use registry::{Registry, RegistryEntry, RegistryError, RootRecord};
