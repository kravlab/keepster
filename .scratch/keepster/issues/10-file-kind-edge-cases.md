# 10 — Binary files, symlinks, permissions and path edge cases

Type: grilling
Status: open
Blocked by: 01

## Question

Snapshotting an arbitrary directory means meeting files that a source tree usually does not contain. What does
v1 do with each?

Settle:

- large files: a size cap, and what the user sees when a file is skipped for exceeding it;
- binary files: stored as-is, or skipped by default;
- symlinks: stored as links, followed, or ignored — and what restore does with them;
- file modes: which permission bits survive a snapshot and a restore, given Windows has no equivalent. Taken as
  given: the History Store sets `core.filemode = false` (ADR 0001), so no mode bit reaches history through git's
  own filesystem probing. What is left to settle is whether the executable bit is captured out of band — in the
  index we build, or as Snapshot metadata — so that a Restore can put it back, or whether v1 accepts losing it and
  says so;
- case-insensitive filesystems, and paths that are valid on one platform but not another;
- files that vanish between the event and the snapshot, and files that are being written while read;
- restore of a file that no longer exists in the working tree, and restore into a directory that was deleted.
