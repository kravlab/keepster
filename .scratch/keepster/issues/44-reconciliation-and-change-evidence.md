# 44: Reconciliation — the guarantee, and what counts as evidence of change

**Blocked by:** 43 (Triggers — the policy kernel, the watcher, debounce and the rate limit)

**Status:** done

**What to build:** changes the operating system never announced are still captured. Reconciliation compares a Working
Tree against its latest Snapshot in full, runs at daemon start and after every rescan signal, and is the mechanism
the coverage promise actually rests on — the watcher is only a latency optimisation.

The comparison that decides whether to read a file is a **filter, not a verdict**: the verdict is always the object
id, so a false positive costs one hash and a false negative costs the user's work. Compared are size and mtime **by
inequality**, plus the mode from the same `stat`; inode, device and ctime are excluded. The inputs live in a
stat-cache table in the derived index, derived from *observation* rather than from git (ADR 0014), so losing the
index costs one full hashing pass. Git's racy rule applies to entries touched during a pass.

- [x] A change made while the daemon was stopped is captured by the pass at the next start
- [x] A populated directory moved into the tree, a write through a second hardlink, and an `mmap` write are all
      captured by a pass, each proven with no watcher installed at all
- [x] A permission-only change is detected from the `stat` the pass already performs, with no hash
- [x] A file restored with its original timestamp and identical size is detected, because the comparison is by
      inequality rather than by "newer"
- [x] An entry whose mtime is not strictly older than the pass's start is re-hashed on the next pass
- [x] The `verify = "hash"` configuration and the one-off flag hash everything; a Root on a filesystem with coarse
      mtime granularity reports that weakness in `status`, and escalation is never automatic
- [x] Each occasion walks the right area: the whole Root at start, on a reload and on a remount; the named subtree
      after a rescan signal that names one, the whole Root where it does not
- [x] A Root whose startup walk is switched off is reported as "coverage not guaranteed"
- [x] Exceeding the daemon-wide watch ceiling moves the next Root onto the periodic pass with the reason in `status`
- [x] Deleting the index and running one pass restores full coverage, and that pass is the only one that hashes
      everything

## Comments

- Ticket 42 built the queue this ticket drains: `keepster_app::config::Configuration::take_reconcile_queue()`
  returns every Root whose classification inputs changed since it was last asked (values only, not where they
  were written). Nothing takes it yet; the reload occasion here is "take the queue and walk each Root in it",
  and the end-to-end test for ticket 42's last criterion — a reload changing the rules queues a full
  comparison — belongs here.
- Ticket 43 hands over two more seams: `keepster_watch::WatchEvent::Rescan` (a platform overflow or a watch
  error) is today mapped to an ordinary change, and should become a Reconciliation of the named subtree or
  the whole Root; and a Root captured automatically reports `coverage_not_guaranteed`, which this ticket
  turns into `Watching` once the startup walk runs. The policy kernel has no `Reconcile` action yet.
- **Decisions taken with the user before implementation.**
  - *One capture path.* Every capture — the watcher's, the periodic floor's, a manual one and a
    Reconciliation — is one pass over the whole Root through the `stat` filter (`HistoryStore::pass`); a
    Reconciliation is that pass recorded as Trigger `reconcile`. So each occasion walks the whole Root,
    including a rescan signal that names a subtree: a superset of the area ticket 35 asks for, never less.
  - *Coarse mtimes are detected by observation*: a pass whose every mtime fell on a whole second reports
    `coarse_mtime`, and the Root is `Degraded`/`mtime_coarse` unless `reconcile.verify = "hash"`. A tree
    unpacked from an archive that keeps whole seconds reads as coarse too; it is a warning, never an
    escalation.
  - *The watch ceiling is here*: half of Linux's `max_user_watches`, read at start; a Root whose watches
    would exceed it is not watched, is reconciled at the periodic floor's interval instead, and reports
    `watch_ceiling`. `DaemonConfig.watch_ceiling` overrides it for tests; macOS and Windows have no ceiling.
  - *On demand*: `keepster reconcile [--verify]` → `POST /v1/roots/{id}/reconcile` `{verify?: "stat"|"hash"}`,
    answering `{snapshot?, hashed, observed, coarse_mtime}`. Nothing is written when nothing changed.
- **The evidence** is the `observed` table of the derived index (schema 2; an older index is rebuilt, ADR
  0010): path, size, mtime in nanoseconds (as text, past 64 bits) and the object id read for them. A file is
  vouched for only when size and mtime equal the row *and* the row's object id is the baseline's — any doubt
  reads. A mode differing alone is a new entry with the old object id, nothing read. Only the rows that
  changed are written, so a pass over a quiet tree writes nothing; a vanished path loses its row in the pass
  that saw it go, and the rows are kept even when the timeline's catch-up fails.
- **The racy rule** compares a file's mtime with the daemon's clock at the pass's start, not with the
  filesystem's own time — reading that would mean writing a probe into the user's tree. So the start is moved
  back by two seconds, FAT's tick, which also covers whole-second filesystems, coarse kernel clocks and modest
  skew: a file written in the two seconds before a pass is read once more by the next. Ticket 35 rejected a
  fixed window as too large; two seconds bounds it to the files written in that window.
- **Occasions**: the start (the daemon settles the Registry before it serves, so the start's pass, which
  `reconcile.at_start = false` switches off, is told apart from the pass a registration or a move owes, which
  always runs — as long as something captures the Root automatically: with every Trigger off the runtime
  starts no pass of its own, since an automatic Snapshot would contradict `capture_unavailable`, and owes the
  held-back one once a Trigger is switched on); a platform overflow or watch error (`WatchEvent::Rescan` → `PolicyEvent::Reconcile`); a reload
  that changed the classification inputs (ticket 42's queue, drained on every reload); and on demand. A Root
  is `Watching` only once a Reconciliation has completed since its runtime started, and otherwise
  `coverage_not_guaranteed`. The remount occasion is ticket 56's.
- **Not covered by a test**: the rescan path (no platform overflow can be produced on demand), and a Root
  over the ceiling is not re-armed when others release watches — only when its configuration changes or the
  daemon restarts.
- Found while making the tests deterministic: a configuration file saved by truncating and rewriting in place
  can be read empty in between, and an empty file is a valid configuration — so the defaults can briefly become
  the known-good. Editors save atomically, and the tests now do too; see ticket 73.
- `HistoryStore::capture_if_changed` is gone: `pass` with `record_unchanged` replaces it. The tests that take
  manual Snapshots run under `keepster_testkit::MANUAL` — no Trigger, no start pass — so a Snapshot the daemon
  takes by itself never races the one a test took.

