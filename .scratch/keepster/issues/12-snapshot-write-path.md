# 12 — Snapshot write path: index lifetime and change tracking

Type: grilling
Status: resolved

## Question

Ticket 01 settled *how* a Snapshot is written — an owner-less in-memory `git2::Index` (`Index::new` +
`Index::add` + `write_tree_to`) that never opens any repository's index file — but not how long that object
lives. The two answers give different systems, so this is a decision rather than an implementation detail.

If the index survives between Snapshots, the watcher mutates only the entries whose paths changed and git never
walks the Working Tree at all. That is the whole point of an incremental watcher, but it turns the index into
daemon state that can disagree with reality — and ticket 02 established that filesystem events are lost, several
kinds of loss never being announced by the OS. A stale entry does not fail loudly: it produces a Snapshot that
claims a file is unchanged when it is not, which is silent loss of exactly the version the tool exists to keep.

If the index is rebuilt per Snapshot, that failure mode does not exist, at the cost of re-adding every entry on
every Snapshot.

Settle:

- whether the index (or an equivalent `path → (mode, OID)` map) survives between Snapshots, or is discarded after
  each one;
- if it survives, it is a derived cache of the same class as ticket 03's SQLite index: what invalidates it, and
  whether repairing it is Reconciliation's job — Reconciliation currently compares Working Tree against the latest
  Snapshot, and would now also have to leave the in-memory map correct;
- what happens on daemon restart. Rebuilding from the previous Snapshot's tree is cheap and needs no filesystem
  I/O; rebuilding from a Working Tree walk is the Reconciliation cost. Which one runs, and whether that differs
  from the cold-start path ticket 04 already made configurable;
- whether `git2::Index` is the right vehicle at all, or our own map plus `TreeBuilder` applied to the previous
  Snapshot's tree. The latter rebuilds only the tree objects on the changed paths' ancestry rather than every tree
  in the Root, and makes the map ours to shape; the former is already written and knows how to emit nested
  trees;
- how this interacts with a Snapshot marked **incomplete** (`CONTEXT.md`): a file that could not be read must not
  leave behind an entry that looks valid to the next Snapshot;
- memory cost of a long-lived map for a large Root, and whether there is a size past which the answer flips;
- the mode recorded in each entry, which ticket 10 needs: `core.filemode = false` (ADR 0001) stops git probing the
  filesystem, so whatever mode the entry carries is the one that reaches history. *Settled by ticket 10:*
  `100755` / `100644` from `stat` on Unix, carried from the same path's entry in the previous Snapshot on Windows,
  `120000` for links.
- hardlinks: no platform reports that link *A* changed when the write went through link *B* (research 02, F-L5,
  R8), so an entry refreshed only from events keeps a stale copy of a hardlinked file. Whether the walk or a
  long-lived map groups entries by `(dev, ino)` — on Windows the volume serial number plus file index, through the
  `windows-sys` calls ticket 10 already takes — so that one changed link refreshes every link, or whether this is
  left to Reconciliation. Ticket 10 left this here as change tracking.
- what a capture does while store maintenance holds the exclusive per-store lock (ticket 06): wait, defer to the
  next Trigger, or coalesce into one Snapshot afterwards. `git-gc` documents that `--prune=now` "increases the risk
  of corruption if another process is writing to the repository concurrently", and that other process is this write
  path, so the exclusion has to be real rather than advisory. Git's own `gc.pid` lock is no help — it guards `gc`
  against `gc`, and `--force`, which would bypass it, is forbidden by ticket 11. Left here by
  [11 — External git prerequisite](11-git-prerequisite.md).

## Answer

**The baseline is re-seeded from the previous Snapshot's tree at every capture. No `path → (mode, OID)` map
survives between Snapshots, and the vehicle stays `git2::Index` — seeded by `Index::read_tree`.** Recorded as
`docs/adr/0004-baseline-from-previous-tree.md`.

### The question was mis-framed, in both directions

Rebuilding per Snapshot is not "re-adding every entry". `git_index_add_from_buffer` requires an owning repository
and errors with "Index is not backed up by an existing repository" (`index.c:1529`), so every entry must carry an
`Oid` we obtained ourselves. There are exactly two places to get one: read the file, or read the previous tree.
"Rebuild from scratch" therefore means **re-reading and re-hashing the whole Root on every Snapshot** — not a
cheap re-insertion pass.

And the choice of vehicle was decided by a premise that turns out to be false. The ticket assumed only a hand-rolled
map plus `TreeBuilder` rebuilds just the changed paths' ancestry. In fact `Index::read_tree` works on an
owner-less index (`index.c:3341-3392`; libgit2's own `tests/libgit2/index/cache.c:92-120` does exactly this) and
populates the TREE extension cache; `Index::add` then invalidates only the root and the ancestors of the changed
path (`index.c:1709` → `tree-cache.c:28-50`), and `write_tree` reuses every still-valid subtree
(`tree.c:600-604`), short-circuiting entirely when nothing was mutated (`tree.c:705-708`). Measured on 100k
entries: 0.83 s to write a tree with no cache, **0.162 s** after changing one path, 0.0036 s with no change.
`TreeBuilder` buys nothing over this and costs a hand-written walk of every ancestor level, since its API knows
nothing about `/`-separated paths.

### Why a surviving map was rejected

- **It is daemon state that disagrees with reality silently.** Ticket 02 lists the losses no OS announces: a
  populated directory moved in (F-L3, measured — its contents are never named), a write through another hardlink
  (F-L5), `mmap` writes and network mounts (F-L7), permission-filtered events on macOS (F-M6), and a watcher that
  stops without saying so (#937). A stale entry then produces a Snapshot claiming a file is unchanged when it is
  not, which is the exact loss this tool exists to prevent.
- **It cannot be a `git2::Index`.** `git2::Index` implements neither `Send` nor `Sync`, so a long-lived one pins
  each Root's capture to one thread for the life of the daemon, against ticket 08's `HashMap<RootId, RootRuntime>`
  (`Repository` is `Send`). Choosing survival therefore also means writing and maintaining our own map.
- **It costs resident memory per Root**: ≈ (130 + path length) bytes per entry — 17 MB for 100k files, 170 MB for
  1M. Under this decision that cost is transient, held only for the duration of a capture, and there is no Root
  size at which the answer flips.

The previous tree is needed on every capture anyway: ticket 10 carries the Windows executable bit from "the same
path's entry in the previous Snapshot", and three separate carry-forward paths (an unstable or unreadable file, an
unavailable symlink target, a cloud placeholder whose size and mtime match) are all expressed as "keep what the
last Snapshot held". With the tree as the baseline these are free rather than special cases.

### Daemon restart

No longer a distinct path: there is nothing to rebuild, so a capture after a restart is identical to any other
capture. This is orthogonal to ticket 04's configurable startup Reconciliation, which answers a different question
— *which paths changed while we were not watching* — and is unaffected.

### A file that could not be read is retried

Ticket 10 has an unreadable file carry its previous version forward and mark the Snapshot incomplete. Under this
decision that carried-forward entry is indistinguishable from "unchanged" at the next capture, and no second
filesystem event is coming, so without more the stale version would stand until the next Reconciliation — days,
under the default Trigger set. **The daemon therefore keeps the failed paths and retries them on each subsequent
capture with a growing backoff.** This is a work queue, not a baseline: it holds only failures, its natural home is
the accumulated-paths field of ticket 08's `PolicyState`, and losing it on restart is harmless because startup
Reconciliation covers the same ground.

### Hardlinks are Reconciliation's job

No platform reports that link *A* changed when the write went through link *B* (F-L5 measured; macOS `notify` #374
delivers no callback at all). Research 02's R8 offered the snapshot walk as the place to group entries by
`(dev, ino)` — but after this decision an ordinary capture performs no walk, touching only the changed paths. The
walk survives in exactly one place, Reconciliation, which already `stat`s everything, so grouping there is free and
additionally removes duplicate hashing of one inode reached under several names. An ordinary capture does not chase
hardlink siblings. A persistent inode-keyed map was rejected twice over: it reintroduces the state this decision
removes, and an atomic save replaces the inode with no event of its own (research 02:511), so it would be stale by
construction. Windows identity, where it is needed, goes through the `windows-sys` calls ticket 10 already takes.

### A capture arriving during maintenance coalesces

`git gc --prune=now` is documented as raising the risk of corruption under a concurrent writer, and that writer is
this path, so ticket 06's exclusive per-store lock is real exclusion, not advice. A capture that finds the lock
held **does not block and is not dropped**: its paths stay accumulated in `PolicyState`, and exactly one Snapshot
covering all of them is taken as soon as the lock is released. Waiting would pin the capture task for the length of
a `gc` and let several queue up behind it; deferring to the next Trigger would strand the work indefinitely, since
a watcher-only Trigger set may produce no further Trigger for hours. Maintenance already waits for ~5 minutes of
quiet before starting, so this case means the user resumed work just as it began.

### Mechanics this fixes, beyond the question asked

- **Entries must be added in lexicographic order.** `git_index_add` keeps the vector sorted by insertion, so
  unordered insertion is quadratic: measured 22.74 s for 400k entries out of order against 0.86 s in order
  (~2 µs/entry, linear to 1M). A capture sorts its changed paths before applying them.
- **Blobs are written before the tree is built.** An owner-less index does not validate that an `Oid` exists
  (`index.c:1390-1399` requires an owner), but `write_tree_to` validates every entry against the *target*
  repository (`tree.c:488-504`) and rejects a null OID.
- **Seed only from the store being written to.** Cached subtree OIDs are copied out without an existence check
  (`tree.c:601-603`), so seeding from one store and writing to another can produce a root tree referencing subtrees
  that do not exist there.
- **Strict object validation is a real cost on an uncached index**: 100k entries took 10.19 s with it on (the
  default) against 0.83 s with it off, one `git_odb_read_header` per entry. Seeding from the previous tree keeps
  the cache valid and the validated set small; whether the global
  `GIT_OPT_ENABLE_STRICT_OBJECT_CREATION` is touched is left to implementation, not assumed here.
- **Zeroed `stat` fields are fine.** Only `path`, `mode` and `id` are read on the add → `write_tree` path
  (`tree.c:586-680`); libgit2's own tests build entries with `memset(..., 0, ...)`. Our index is never written to
  disk and never diffed against a working tree, which is what those fields exist for.
- **`mode` must be one of `0100644`, `0100755`, `0120000`, `0160000`**, or `git_index_add` rejects the entry. Note
  that `core.filemode = false` does not touch this path: distrust of the mode applies only to `add_path`, so the
  executable bit we set is the one that reaches history — exactly what ADR 0001 and ticket 10 require.
- **Path validation always applies**, repository or not: `..` and a `.git` component are rejected
  (`GIT_PATH_REJECT_INDEX_DEFAULTS`), which is the same set ticket 10 records as Withheld `reserved_name`. Raw
  non-UTF-8 bytes pass at the C level; the Rust binding's `bytes2path` panics on Windows, so the pathspec callbacks
  (`add_all`, `remove_all`, `update_all`) must not be used.
- **`Repository::blob_path` streams** in 64 KiB chunks in a bare repository (`blob.c:102-137`): with no workdir no
  filters load, so the whole-file-in-memory branch is unreachable for our store.
- **`Oid::hash_file` computes a blob OID without writing the object** (`git2-0.21.0/src/oid.rs:214-255`), available
  if a capture wants to test whether a changed path's content actually differs before writing. It follows symlinks
  and applies no filters.

### Invariant this decision creates

The previous Snapshot's tree must be readable at every capture, **including the first capture of a new Segment**.
ADR 0001's "parentless" constrains the commit's parents, not the reachability of the previous Segment's tip, so
this is satisfiable as designed — but it was nowhere written down, and both the Windows mode carry-forward and this
decision now depend on it.

### Not measured

The cost of `read_tree` itself. It is eager and recursive, one entry per non-tree entry plus one ODB read per
subtree, with no filesystem I/O — expected in the low hundreds of milliseconds for 100k files, against a default
rate limit of one Snapshot per 60 s. No `git2` figure of any kind has ever been measured for this project: ticket
01's numbers are the `git` CLI on Linux, and the owner-less index round trip it proposed was never run. Worth a
benchmark before the write path is called done.
