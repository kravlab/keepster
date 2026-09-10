# 12 — Snapshot write path: index lifetime and change tracking

Type: grilling
Status: open

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
  in the Project, and makes the map ours to shape; the former is already written and knows how to emit nested
  trees;
- how this interacts with a Snapshot marked **incomplete** (`CONTEXT.md`): a file that could not be read must not
  leave behind an entry that looks valid to the next Snapshot;
- memory cost of a long-lived map for a large Project, and whether there is a size past which the answer flips;
- the mode recorded in each entry, which ticket 10 needs: `core.filemode = false` (ADR 0001) stops git probing the
  filesystem, so whatever mode the entry carries is the one that reaches history.
