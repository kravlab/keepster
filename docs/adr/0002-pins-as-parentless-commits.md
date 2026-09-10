# A pinned Snapshot is re-created as a parentless commit under its own ref

A user must be able to protect one Snapshot from Retention indefinitely. The obvious design — a flag Retention
consults — has nowhere safe to live: the SnapshotId index is declared a wholly derived cache that may be deleted
and rebuilt from git objects at any time (ticket 03), so a pin stored there evaporates on a rebuild and the next
Retention pass deletes exactly the Snapshot the user asked to keep. We therefore store the pin in git, as a ref, so
that reachability itself enforces it: `refs/keepster/pins/<SnapshotId>` points at the Snapshot re-created as a
parentless commit, sharing the original's tree.

## Considered Options

- **A flag Retention honours**, held in the Registry's plain-file layer rather than the derived index. Keeps one
  commit per Snapshot, but the guarantee lives in policy code: thinning, whole-Segment deletion and `gc` must each
  remember to check it, and a miss is silent data loss.
- **A pin ref pointing at the existing in-chain commit.** Self-enforcing and free to create, but it keeps every
  ancestor in the Segment reachable, so one pin preserves a whole period of Snapshots and the disk-usage cap
  (ticket 06) stops working.
- **Pin the whole Segment** — mark it never-thinned and never-dropped. Trivial against ADR 0001, but the cost of
  protecting one version is a whole period, and it blunts the cap unpredictably.
- **No pinning; export instead.** `keepster export` copies a version out to a path the user owns. Smaller v1
  surface, but nothing inside the store can be protected from the ladder.

## Consequences

- Retention, thinning and the disk-usage cap are entirely **pin-unaware**. Thinning rewrites a Segment freely and
  the cap drops Segment refs freely; the pinned Snapshot survives because it holds its own root. Two subsystems
  that would otherwise have to agree are decoupled.
- `gc` physically cannot collect a pinned Snapshot, so the guarantee does not depend on any policy code being
  correct.
- Storage cost is **one commit object**. The tree and every blob beneath it are shared with the in-Segment copy.
- The pinned copy's commit hash differs from the in-Segment copy's. This is the case ADR 0001 anticipated by making
  `SnapshotId`, not the commit hash, the public identity; the index treats the pin ref as canonical for that id
  while both exist.
- Pin refs must remain leaves, like Segment refs, so nothing may ever nest beneath
  `refs/keepster/pins/<SnapshotId>`.
- A pin survives an index rebuild, because refs are enumerable from git alone — pins are part of what the recovery
  procedure in ticket 03 reconstructs.
- `forget` (ticket 06) rewrites pins like any other commit, which is trivial here precisely because each pin is a
  single parentless commit rather than a chain.
