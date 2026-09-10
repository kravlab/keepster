# 02 — Reliability limits of cross-platform filesystem watching

Type: research
Status: resolved

## Question

The daemon must not miss changes, and every platform loses events under some condition. What are the real failure
modes for this workload, and what must the daemon do to survive them?

Investigate, per platform and for the `notify` crate as the likely abstraction:

- Linux: `inotify` watch limits (`max_user_watches`), `IN_Q_OVERFLOW`, cost of recursively watching a 100k-file tree;
- macOS: FSEvents coalescing, latency, historical-event replay, and whether directory-level granularity is enough;
- Windows: `ReadDirectoryChangesW` buffer overflow and what is reported when it happens;
- everywhere: how editors actually save (write to a temp file then rename), how that appears as events, plus
  renames, moves in and out of the tree, hardlinks and symlinks.

Deliver: which events can be lost and how that is detected, what a reconciliation pass must re-scan to recover,
and what the daemon must do at startup when it has been down for an unknown period.

## Answer

Every platform loses changes, and only some of that loss is announced. Linux announces queue overflow with
`IN_Q_OVERFLOW` (no path, `wd == -1`), macOS announces coalescing and drops with `MustScanSubDirs` (with a
path), Windows discards its whole `ReadDirectoryChangesW` buffer and reports it only as a zero-byte
completion or `ERROR_NOTIFY_ENUM_DIR`. `notify` maps all three onto `Event::need_rescan()` — but only since
the fix for issue #963, and it discards FSEvents' historical replay by hardcoding `sinceWhen = SinceNow`, so
downtime is a black hole on every platform. Worse, several losses are never announced at all: a populated
directory moved into the tree reports only its own arrival (verified locally — its contents are never named),
a write through one hardlink names no other link, writes via `mmap`, network mounts and unreadable files
produce nothing, and a watcher can stop delivering silently. Editors compound this: an atomic save arrives as
`MOVED_FROM`/`MOVED_TO`, with no modify event on the saved path at all.

Conclusion: the watcher is a latency optimisation, never the correctness mechanism. Reconciliation is the
guarantee, it must be non-disableable at daemon start and after every rescan signal, and the daemon must arm
watches before walking, rescan any directory that appears in the tree, apply Include Rules before installing
watches (measured: ~10k watches and 1-4 s to arm a 100k-file tree, against a per-user budget of 133264), and
disable symlink following so a recursive watch cannot escape the Project.

Full findings, with sources and local measurements:
[`../research/02-filesystem-watching.md`](../research/02-filesystem-watching.md)
