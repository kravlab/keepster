//! The identity of a Snapshot, and what a Snapshot records about itself.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

impl SnapshotId {
    /// Mint the identity of the Snapshot that follows `previous`, taken at `now`.
    ///
    /// The timeline's order *is* the order of these identities, so two Snapshots taken within one
    /// millisecond must not be ordered by the random half of a ULID — which would show the later one
    /// first half of the time. Within the previous Snapshot's millisecond the identity is therefore its
    /// successor. A clock that went backwards is not hidden the same way: the identity keeps the moment
    /// it was given, and the derived index notices the disorder and rebuilds rather than inventing a
    /// moment that never happened.
    pub fn following(previous: Option<SnapshotId>, now: SystemTime) -> Self {
        let candidate = Self::generate(now);

        match previous {
            Some(previous)
                if previous.0.timestamp_ms() == candidate.0.timestamp_ms()
                    && candidate <= previous =>
            {
                previous.0.increment().map(Self).unwrap_or(candidate)
            }
            _ => candidate,
        }
    }

    /// The moment this Snapshot was taken, to the millisecond its identity carries.
    pub fn moment(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.0.timestamp_ms())
    }

    /// The smallest identity of any Snapshot taken strictly after `moment`.
    ///
    /// A ULID orders by its millisecond first, so "taken at or before" and "taken after" a moment are
    /// both one comparison against this value — which is how a timeline filter by time stays a plain
    /// range over identities with no second ordering to keep in step.
    pub fn first_after(moment: SystemTime) -> Self {
        let millisecond = moment.duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });

        Self(::ulid::Ulid::from_parts(millisecond.saturating_add(1), 0))
    }
}

/// How one path differs between a Snapshot and the Snapshot before it.
///
/// A permission-only change is a modification: ticket 10 makes it a real change, so a timeline filtered
/// by a path must show it rather than hide it behind an unchanged content hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChangeKind {
    /// The path holds content here and held none in the Snapshot before.
    Added,
    /// The path's content or mode differs from the Snapshot before.
    Modified,
    /// The path held content in the Snapshot before and holds none here.
    Deleted,
}

impl ChangeKind {
    /// The kind's name, as the wire and the derived index both spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }

    /// The kind a name spells, if it spells one.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "added" => Some(Self::Added),
            "modified" => Some(Self::Modified),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }
}

impl fmt::Display for ChangeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Which Trigger caused a Snapshot.
///
/// Each is a variant of its own, so a new one is additive on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    /// A person or a script asked for this Snapshot.
    Manual,
    /// The watcher saw a change, and the Root then stayed quiet for the debounce window — or the daemon
    /// was stopping with that window still counting down.
    Watcher,
    /// The periodic floor fell due.
    Periodic,
    /// A Reconciliation: the whole Working Tree compared against the latest Snapshot — at the daemon's
    /// start, after the platform lost events, after the rules changed, or because someone asked.
    Reconcile,
    /// A Restore is about to write: this is its Safety Snapshot, the state the Restore can be undone to.
    Restore,
    /// The pass before could not read everything it meant to, and the Root is passed over again: a file
    /// kept at its previous version looks unchanged, and no second event is coming (ADR 0004).
    Retry,
}

/// How a pass decides whether a file it walks must be read.
///
/// The `stat` filter is never the verdict — the object id is — so a false positive costs one hash and a
/// false negative the user's work (ticket 35). Where mtimes cannot be trusted, hashing everything is the
/// user's to choose, never automatic: on a network Root it means reading the whole Root over the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Verify {
    /// Read a file only when its size or mtime differs from what the last pass observed — a mode alone
    /// changing is seen from the same `stat`, with nothing read.
    #[default]
    Stat,
    /// Read every file.
    Hash,
}

impl Verify {
    /// The name the configuration and the wire spell it with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stat => "stat",
            Self::Hash => "hash",
        }
    }

    /// The mode a name spells, if this build knows it.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "stat" => Some(Self::Stat),
            "hash" => Some(Self::Hash),
            _ => None,
        }
    }
}

impl Trigger {
    /// The Trigger's name, as the commit trailer, the derived index and the wire all spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Watcher => "watcher",
            Self::Periodic => "periodic",
            Self::Reconcile => "reconcile",
            Self::Restore => "restore",
            Self::Retry => "retry",
        }
    }

    /// The Trigger a name spells, if this build knows it.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "manual" => Some(Self::Manual),
            "watcher" => Some(Self::Watcher),
            "periodic" => Some(Self::Periodic),
            "reconcile" => Some(Self::Reconcile),
            "restore" => Some(Self::Restore),
            "retry" => Some(Self::Retry),
            _ => None,
        }
    }
}

impl fmt::Display for Trigger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
