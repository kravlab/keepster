# Keepster

Editor-independent file history: a background daemon captures snapshots of watched directories into its own
git-backed store, so that work overwritten by an agent, a script or a stray command can be recovered without
relying on the directory's own version control.

## Language

**Watched Root** (short form **Root**):
A directory registered for history, identified by a surrogate id that survives renaming or moving the directory.
_Avoid_: project, repository, workspace, folder, directory

**Registry**:
The daemon's record of every registered Root and its current path.
_Avoid_: database, index, root list

**Working Tree**:
The live files in a Root's directory, as opposed to any captured version of them.
_Avoid_: source, current version, disk state

**Include Rules**:
A Root's own rules deciding which files a Snapshot captures. They are authoritative; a directory's `.gitignore`
is an optional layer beneath them. An excluded directory is never entered, so nothing beneath it can be
re-included: where a rule and an ancestor's rule disagree, the ancestor wins.
_Avoid_: gitignore, filters, exclusions

**Snapshot**:
The captured state of a Root's included files at one moment. Normally complete; where some included files could
not be read, the Snapshot is still taken and marked incomplete, naming what it omits, so that it never claims a
completeness it does not have.
_Avoid_: commit, version, backup, checkpoint

**Withheld**:
A path a Snapshot records as present in the Working Tree but whose content it deliberately does not hold, together
with the reason it was withheld. Withholding is policy, not failure: it does not make a Snapshot incomplete, which is
reserved for files that were meant to be captured and could not be read.
_Avoid_: skipped, omitted, excluded, ignored

**SnapshotId**:
The stable public identity of a Snapshot, deliberately not the underlying commit hash, so that pruning may rewrite
commits without invalidating anything that refers to a Snapshot.
_Avoid_: hash, SHA, revision

**Segment**:
The group of Snapshots covering one retention period, self-contained so it can be dropped or thinned without
touching any other Segment.
_Avoid_: bucket, chain, branch

**History Store**:
The storage owned by Keepster that holds a Root's Snapshots, separate from and invisible to the version
control inside the watched directory.
_Avoid_: backup, cache, repo

**Trigger**:
A rule that decides when a Snapshot is taken. Which Triggers are active is the user's choice — except
Reconciliation, which runs whenever the guarantee needs it and is recorded as a Trigger of its own.
_Avoid_: schedule, hook, watcher event

**Debounce window**:
How long a Root must stay quiet after its last change before the watcher Trigger captures it, so that a burst of
saves becomes one Snapshot. A change waits no longer than the rate limit would make it wait anyway.
_Avoid_: delay, grace period

**Rate limit**:
The shortest time between two Snapshots of one Root, always in force whatever the Triggers. A stopping daemon's
flush of pending changes is the one capture that ignores it.
_Avoid_: throttle, cooldown

**Pending change**:
A change the watcher reported that no Snapshot holds yet — waiting for its debounce window, the rate limit, or a
store lock. Reported as a count and the age of the oldest, never as a listing.
_Avoid_: queue, backlog

**Reconciliation**:
A full comparison of a Working Tree against its latest Snapshot, used to recover changes whose filesystem events
were never delivered.
_Avoid_: full scan, resync, refresh

**Capture Preview**:
A classification of a directory's current contents against the Include Rules that would govern it, without
capturing anything. Distinct from Reconciliation, which compares a Working Tree against its latest Snapshot; a
Capture Preview compares nothing, and is defined for a directory that is not yet a Root.
_Avoid_: dry-run, simulation, scan, audit

**Empty Directory**:
A directory a Snapshot holds no content under, whether because nothing is there or because everything in it was
excluded or Withheld. Narrower than the everyday sense in one direction and wider in the other: a directory full of
excluded files is empty here, because the history holds nothing beneath it.
_Avoid_: dangling directory, placeholder directory

**Retention**:
The rules deciding which Snapshots survive and which are discarded.
_Avoid_: cleanup, expiry, garbage collection

**Segment Period**:
The span of time one Segment covers. A property of the Segment, fixed when it is created, not of the History Store:
changing the configured period affects only Segments created afterwards, so a store may hold Segments of differing
periods indefinitely.
_Avoid_: segment size, granularity, bucket width

**Retention Ladder**:
The ordered list of Tiers that expresses a Root's Retention, from the newest and densest to the oldest and
sparsest.
_Avoid_: policy, schedule, rotation

**Tier**:
One rung of the Retention Ladder: an age, and how densely Snapshots are kept below it. Stated in durations, so it
is independent of the Segment Period.
_Avoid_: level, band, bucket

**Pin**:
A user's declaration that one Snapshot survives Retention unconditionally. Held as its own self-contained copy of
the Snapshot, so that Retention need not know pinned Snapshots exist.
_Avoid_: lock, keep, favourite, star

**Maintenance Pass**:
The periodic reclamation of a History Store's disk space, distinct from Retention: Retention decides which
Snapshots survive, the Maintenance Pass makes the space of the others actually available again. It cannot overlap
with capturing a Snapshot.
_Avoid_: gc, cleanup, compaction, vacuum

**Forget**:
Removing every version of a path from a Root's whole history, used when a secret was captured. Distinct from
Retention, which discards Snapshots wholesale by age; Forget removes one path from Snapshots that otherwise
survive, and never touches the Working Tree.
_Avoid_: purge, scrub, delete, redact

**Degraded**:
The state of a Root the daemon is still watching but cannot serve as promised — its configuration failed to
load and the last known-good one is in force, or its coverage guarantee has been switched off or does not
exist yet.
_Avoid_: broken, error state, unhealthy

**Unavailable**:
The state of a Root that cannot be captured at all — its directory is missing, unmounted or unreadable, its
History Store was written by a version this one cannot open, or its configuration has never had a valid version
to fall back on. Distinct from Degraded, where watching continues on
worse terms; an Unavailable Root is never deregistered and its History Store is kept, because the Root's identity
does not depend on its path.
_Avoid_: offline, missing, lost, disconnected

**Restore**:
Writing a Snapshot's version of a file back over the Working Tree.
_Avoid_: revert, rollback, checkout

**Safety Snapshot**:
The Snapshot taken of the current state immediately before a Restore, which is what makes a Restore itself
undoable.
_Avoid_: backup, pre-restore copy
