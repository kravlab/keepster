//! Pure decision kernel: domain types, policy, Retention, Include Rules. No traits, no I/O, no clock.
//!
//! Everything here is a value or a function over values (ADR 0003), which is what lets the policy this
//! product turns on — when a Snapshot is warranted, which Snapshots survive, which paths are captured
//! — be tested without a disk and replayed without a daemon.

mod event;
mod git;
mod identity;
mod path;
mod policy;
mod restore;
mod retention;
mod root;
mod rules;
mod segment;
mod sidecar;
mod snapshot;
mod trigger;

pub use event::EventId;
pub use git::{GIT_DOWNLOADS, GIT_MINIMUM, GitVersion, PackageFamily, Platform, git_candidates};
pub use identity::NotAnIdentity;
pub use path::{InvalidPath, RootPath};
pub use policy::{
    CapturePolicy, DEFAULT_MAX_SIZE, Explanation, FollowStop, InvalidSize, SizeRule, Verdict,
    is_reserved, parse_size,
};
pub use restore::{
    Ancestry, Confirmation, Confirmed, Existing, Found, Issue, ModeChoice, Modes, NameCase, Naming,
    PathPlan, Plan, Resolution, Step, Unrecoverable, plan, target_of,
};
pub use retention::{
    InvalidLadder, Keep, Ladder, Newest, RetentionPlan, SegmentContents, Thinning, Tier,
    plan_retention,
};
pub use root::{RootId, containing, overlapping};
pub use rules::{
    Classification, IGNORE_FILE, InvalidPattern, Kind, Level, PathGlob, Rule, RuleOrigin, RuleSet,
    UnreachableNegation, ignore_file_rules,
};
pub use segment::{SegmentName, UnnameableMoment};
pub use sidecar::{
    Header, Incomplete, ModeKind, ModeTable, Parsed, Record, SIDECAR, SIDECAR_VERSION, Sidecar,
    SidecarError, SpecialKind, WithheldReason,
};
pub use snapshot::{ChangeKind, SnapshotId, Trigger, Verify};
pub use trigger::{
    Action, InvalidDuration, PolicyEvent, PolicyState, RETRY_AFTER, RETRY_CEILING, Triggers,
    decide, format_duration, parse_duration,
};
