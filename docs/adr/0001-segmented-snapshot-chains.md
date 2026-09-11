# Snapshots are stored as segmented chains, one ref per retention period

A History Store holds thousands of Snapshots and must be able to discard them cheaply, but git commits are
immutable and parent-linked, so removing one from the middle of a single long chain means recreating every commit
after it. We therefore store each retention period as its own Segment: one ref per period, the Segment's first
Snapshot being a parentless commit and the rest chained within it. Dropping a whole period is then a ref deletion,
and thinning a period rewrites only that period.

## Considered Options

- **One chain, one ref.** Natural `git log`, but any pruning except from the newest end rewrites the entire suffix
  of history.
- **One ref per Snapshot, independent root commits.** Pruning is a ref deletion, but the ref count grows without
  bound — rejected by the user as unacceptable operationally.
- **A manifest commit holding every Snapshot as a parent.** One ref, deletion rewrites one object, but git's
  behaviour at thousands of parents is unproven and the shape is unidiomatic.

## Consequences

- Ordering across Segments is ours to maintain: there is no single revision walk over all history, so the timeline
  is assembled by enumerating Segment refs (named to sort chronologically) and walking each one.
- `SnapshotId` must be a stable surrogate recorded in a commit trailer, never the commit hash, because thinning a
  Segment recreates its commits.
- Reflogs must stay off in the History Store (`core.logAllRefUpdates`). A bare repository already defaults to
  false, so this is an invariant to assert and test, not a step to perform — but if anything ever turns it on,
  deleted refs stay reachable through the reflog, garbage collection frees nothing, and Retention becomes purely
  cosmetic.
- `core.filemode` must be `false` in the History Store. Unlike the reflog invariant this is not a default — git
  probes the filesystem at init and sets it `true` on Linux and macOS — so it is a step to perform at store
  creation and then assert, not merely assert. It stops the on-disk mode bit from making one Working Tree produce
  different trees on different platforms, and keeps the external `git` maintenance binary consistent with the
  Snapshots `git2` writes. Because Snapshots are built from an index we own (ticket 01), the operative rule is that
  Snapshot tree entries carry a normalised mode — one of git's `100644`, `100755` or `120000`, set by us rather than
  probed by git, never the raw `st_mode`. The executable bit is kept that way: read from `stat` on Unix, carried from
  the previous Snapshot on Windows, and put back by a Restore on Unix (ticket 10).
- Deleting a Segment ref only makes its objects unreachable; something must still reclaim the space. Neither
  `git2` nor `gitoxide` implements garbage collection, so that pass is delegated to the external `git` binary,
  which is a mandatory prerequisite of every installation (ticket 01). Retention must therefore keep git
  maintenance and Snapshot writes out of each other's way (ticket 06).
- Segment ref names must not nest: git's files backend cannot hold both `refs/x` and `refs/x/y`, so the naming
  scheme has to keep every Segment ref a leaf.
- Segment granularity is also prune granularity: a coarse period keeps the ref count low but makes thinning
  rewrite more at once. It is a configuration decision, resolved in ticket 06.
