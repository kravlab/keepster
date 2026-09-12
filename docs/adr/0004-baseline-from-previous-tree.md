# The snapshot baseline is re-seeded from the previous Snapshot's tree, so no index survives between Snapshots

Writing a Snapshot needs a `path → (mode, OID)` baseline, and the obvious optimisation is to keep it in the daemon
and let the watcher mutate only what changed — the whole point of an incremental watcher. We do the opposite: the
baseline is read back out of the previous Snapshot's tree at every capture, into an owner-less `git2::Index` that
is dropped when the capture ends. The daemon holds no such state at all. A reader expecting the incremental design
would otherwise read this as a missed optimisation, which is why it is written down.

## Considered Options

- **A surviving map, mutated by watcher events.** Cheapest per capture, and the conventional shape. Rejected
  because it is state that disagrees with reality *silently*: ticket 02 enumerates losses no OS announces — a
  populated directory moved in (F-L3), a write through another hardlink (F-L5), `mmap` writes and network mounts
  (F-L7), macOS permission filtering (F-M6), a watcher that stops without saying so (#937). A stale entry produces
  a Snapshot claiming a file is unchanged when it is not, which is the loss this tool exists to prevent. It also
  cannot be a `git2::Index`, which implements neither `Send` nor `Sync` and would pin each Root's capture to one
  thread for the daemon's lifetime, so the option additionally means writing and owning our own map; and it costs
  ≈ (130 + path length) bytes per entry resident — 17 MB per 100k files, 170 MB per 1M.
- **Rebuilding from a Working Tree walk on every Snapshot.** The failure mode above cannot exist. Rejected on cost,
  which the ticket had understated: `git_index_add_from_buffer` requires an owning repository (`index.c:1529`), so
  every entry must carry an `Oid` we obtained ourselves, and the only sources are the file or the previous tree.
  Rebuilding without the tree therefore re-reads and re-hashes the entire Root on every Snapshot.
- **Our own map plus `TreeBuilder` on the previous tree.** Proposed as the way to rebuild only the changed paths'
  ancestry. Rejected because its premise is false — see the first consequence below — and because `TreeBuilder`
  knows nothing about `/`-separated paths, so every ancestor level would be walked and rewritten by hand.

## Consequences

- **The stock index already gives the ancestry-only rebuild.** `Index::read_tree` works on an owner-less index
  (`index.c:3341-3392`) and fills the TREE extension cache; `Index::add` invalidates only the root and the changed
  path's ancestors (`tree-cache.c:28-50`); `write_tree` reuses every still-valid subtree (`tree.c:600-604`) and
  short-circuits entirely when nothing was mutated (`tree.c:705-708`). Measured on 100k entries: 0.83 s with no
  cache, **0.162 s** after one changed path, 0.0036 s with no change.
- **Git is the only source of truth about what the last Snapshot held.** Nothing derived can drift from it, because
  nothing derived persists. This is the same invariant ticket 03 states for the SQLite index and ADR 0002 for pins,
  applied to the write path.
- **The carry-forward cases stop being special.** Ticket 10's Windows executable bit ("carried from the same path's
  entry in the previous Snapshot"), an unstable or unreadable file, an unavailable symlink target and a matching
  cloud placeholder all reduce to "leave the baseline entry alone", because the baseline *is* the previous tree.
- **A new obligation: an unreadable file must be retried.** Its carried-forward entry is indistinguishable from
  "unchanged" at the next capture and no second filesystem event is coming, so the daemon keeps failed paths and
  retries them with a growing backoff. This is a work queue in `PolicyState`, not a baseline — it holds only
  failures and may be lost on restart, because startup Reconciliation covers the same ground.
- **Hardlink grouping moves to Reconciliation.** An ordinary capture performs no walk, so research 02's R8 option
  of grouping by `(dev, ino)` "during the snapshot walk" survives only where a walk still happens. An inode-keyed
  cache was rejected independently: an atomic save replaces the inode with no event of its own (research 02:511).
- **Ordering is now load-bearing.** `git_index_add` keeps its vector sorted by insertion, so entries must be
  applied in lexicographic order: measured 22.74 s for 400k entries out of order against 0.86 s in order.
- **The previous Snapshot's tree must be readable at every capture, including the first of a new Segment.** ADR
  0001's parentless Segment start constrains commit parents, not the reachability of the previous Segment's tip, so
  this holds as designed — but it is now depended upon and must be tested rather than assumed.
- **Seeding must come from the store being written to.** Cached subtree OIDs are copied out without an existence
  check (`tree.c:601-603`), so a cross-store seed can yield a root tree referencing subtrees the target lacks.
- **The cost of `read_tree` itself is unmeasured**, as is every other `git2` figure in this project: ticket 01's
  numbers are the `git` CLI on Linux and its owner-less index round trip was never run. The expectation is low
  hundreds of milliseconds per 100k files with no filesystem I/O, against a default floor of one Snapshot per 60 s.
  If that expectation fails at some Root size, this is the decision to revisit.
