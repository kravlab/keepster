# 03 — Where the Root registry and the snapshot index live

Type: research
Status: resolved

## Question

Two pieces of state are not git objects: the registry of Watched Roots (id, path, per-Root config) and the
index mapping `SnapshotId` to its segment ref and commit. Where do they live — SQLite, plain files inside the
history store, or git objects (trailers, notes) with an in-memory index built on demand?

Weigh against:

- crash safety: the daemon can be killed mid-snapshot, and the store must not be left inconsistent;
- concurrent access: the daemon writes while CLI and HTTP handlers read;
- recoverability: since `SnapshotId` also lives in the commit trailer, the index must be rebuildable from git alone
  if the metadata file is lost or corrupted — confirm this holds for the chosen option;
- the queries the timeline needs: list snapshots for a Root newest-first across segments, and list only the
  snapshots in which a given path changed;
- cost of the dependency on three platforms.

Deliver a recommendation with evidence, and state explicitly what the recovery procedure is when the metadata is
destroyed.

## Answer

Split the state rather than picking one store. The **Registry** lives in plain files — one small daemon-owned
`root.toml` per Root beside its History Store, replaced atomically (temp file + fsync + rename) — because
it is the only state not derivable from git and therefore the only state that must never sit in something a
backup tool or a network filesystem can corrupt. The **SnapshotId → (Segment ref, commit) index**, the
path-change index and the cross-Segment ordering live in **SQLite via rusqlite** (`bundled`, WAL,
`synchronous=NORMAL`, daemon as sole writer), declared explicitly as a *derived cache*: it may be deleted at any
time and rebuilt from git objects. Git notes are rejected outright (a note is bound to a commit SHA, and ADR 0001
thinning recreates commits; every note edit also creates a commit on an unpruned ref), and an index built only on
demand is rejected because "which Snapshots changed this path" has no answer in git short of a full tree-diff
walk — and `git log -- <path>` is documented as deliberately incomplete anyway.

Two invariants make this safe: git commit + ref update always precede the index write, so the index can only lag
and never point at a commit that does not exist; and a per-Segment watermark (ref → last indexed tip) turns
"the daemon was killed" into a millisecond catch-up over the same code path as a full rebuild. SQLite's own
documentation settles the crash-safety bullet: "Transactions are durable across application crashes regardless of
the synchronous setting or journal mode."

Recovery when the metadata is destroyed but git survives: delete the `.sqlite`/`-wal`/`-shm` set together,
enumerate Segment refs newest-first, walk each chain reading `SnapshotId` trailers, tree-diff each commit against
its parent to rebuild the path rows, then record watermarks. The Registry recovers separately and only partly —
the Root id is recoverable from the store directory and a `Keepster-Root` commit trailer, but the
watched directory *path* is recoverable under no option, so the user re-runs `keepster watch <dir>` and the
daemon reattaches the existing store by id.

Full comparison, evidence and the step-by-step recovery procedure:
[`research/03-metadata-store.md`](../research/03-metadata-store.md)
