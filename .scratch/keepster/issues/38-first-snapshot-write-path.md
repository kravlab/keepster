# 38: The first Snapshot — History Store invariants and the write path

**Blocked by:** 37 (The Registry — register, deregister and resolve a Watched Root)

**Status:** ready-for-agent

**What to build:** a user can ask for a Snapshot of a Watched Root and get one, stored in a History Store that obeys
the storage invariants every later ticket depends on. Triggers, rules and reading come later; this is the write path
and the store's shape.

The store is segmented: one ref per Segment, each starting from a parentless commit, Snapshots chained within it,
leaf-only ref names, reflogs off so a deleted ref can actually free objects, git's own file-mode probing disabled,
and the `SnapshotId` and Root id carried as commit trailers (ADR 0001). The baseline for each capture is re-seeded
from the previous Snapshot's tree, so the daemon holds no index between Snapshots (ADR 0004). Capture takes the
exclusive per-store lock. Only the built-in reserved-name rule applies for now: a `.git` component is recorded as
Withheld and never descended into.

- [ ] A manual capture produces a Snapshot with a ULID `SnapshotId`, and the id appears in the commit trailer
- [ ] A Segment is created lazily for the period its first Snapshot falls in, and its start commit is parentless
- [ ] Reflogs are off and file-mode probing is disabled at store creation, both asserted against a real store
- [ ] The baseline is read back from the previous Snapshot's tree at every capture; entries are applied in
      lexicographic order, and the tree write reuses unchanged subtrees
- [ ] A second capture with no changes produces a Snapshot whose tree is identical to its parent's, cheaply
- [ ] A `.git` component (and its platform aliases) is recorded as Withheld with the reserved-name reason and is
      never entered
- [ ] Capture holds the exclusive per-store lock; a concurrent capture coalesces rather than blocking or dropping
- [ ] Every timestamp the store records derives from the injected `now`, proven by a test that writes a Snapshot
      dated a month ago
