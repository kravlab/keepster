//! Pure decision kernel: domain types, policy, Retention, Include Rules. No traits, no I/O, no clock.
//!
//! Everything here is a value or a function over values (ADR 0003), which is what lets the policy this
//! product turns on — when a Snapshot is warranted, which Snapshots survive, which paths are captured
//! — be tested without a disk and replayed without a daemon.

mod root;

pub use root::{NotARootId, RootId, containing, overlapping};
