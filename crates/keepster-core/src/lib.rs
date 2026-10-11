//! Pure decision kernel: domain types, policy, Retention, Include Rules. No traits, no I/O, no clock.
//!
//! Everything here is a value or a function over values (ADR 0003), which is what lets the policy this
//! product turns on — when a Snapshot is warranted, which Snapshots survive, which paths are captured
//! — be tested without a disk and replayed without a daemon.

mod identity;
mod path;
mod policy;
mod root;
mod rules;
mod segment;
mod snapshot;
mod trigger;

pub use identity::NotAnIdentity;
pub use path::{InvalidPath, RootPath};
pub use policy::{
    CapturePolicy, DEFAULT_MAX_SIZE, Explanation, InvalidSize, SizeRule, Verdict, is_reserved,
    parse_size,
};
pub use root::{RootId, containing, overlapping};
pub use rules::{
    Classification, IGNORE_FILE, InvalidPattern, Kind, Level, PathGlob, Rule, RuleOrigin, RuleSet,
    UnreachableNegation, ignore_file_rules,
};
pub use segment::{SegmentName, UnnameableMoment};
pub use snapshot::{ChangeKind, SnapshotId, Trigger};
pub use trigger::{
    Action, InvalidDuration, PolicyEvent, PolicyState, Triggers, decide, format_duration,
    parse_duration,
};
