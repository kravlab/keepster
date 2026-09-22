# 44: Reconciliation — the guarantee, and what counts as evidence of change

**Blocked by:** 43 (Triggers — the policy kernel, the watcher, debounce and the rate limit)

**Status:** ready-for-agent

**What to build:** changes the operating system never announced are still captured. Reconciliation compares a Working
Tree against its latest Snapshot in full, runs at daemon start and after every rescan signal, and is the mechanism
the coverage promise actually rests on — the watcher is only a latency optimisation.

The comparison that decides whether to read a file is a **filter, not a verdict**: the verdict is always the object
id, so a false positive costs one hash and a false negative costs the user's work. Compared are size and mtime **by
inequality**, plus the mode from the same `stat`; inode, device and ctime are excluded. The inputs live in a
stat-cache table in the derived index, derived from *observation* rather than from git (ADR 0014), so losing the
index costs one full hashing pass. Git's racy rule applies to entries touched during a pass.

- [ ] A change made while the daemon was stopped is captured by the pass at the next start
- [ ] A populated directory moved into the tree, a write through a second hardlink, and an `mmap` write are all
      captured by a pass, each proven with no watcher installed at all
- [ ] A permission-only change is detected from the `stat` the pass already performs, with no hash
- [ ] A file restored with its original timestamp and identical size is detected, because the comparison is by
      inequality rather than by "newer"
- [ ] An entry whose mtime is not strictly older than the pass's start is re-hashed on the next pass
- [ ] The `verify = "hash"` configuration and the one-off flag hash everything; a Root on a filesystem with coarse
      mtime granularity reports that weakness in `status`, and escalation is never automatic
- [ ] Each occasion walks the right area: the whole Root at start, on a reload and on a remount; the named subtree
      after a rescan signal that names one, the whole Root where it does not
- [ ] A Root whose startup walk is switched off is reported as "coverage not guaranteed"
- [ ] Exceeding the daemon-wide watch ceiling moves the next Root onto the periodic pass with the reason in `status`
- [ ] Deleting the index and running one pass restores full coverage, and that pass is the only one that hashes
      everything
