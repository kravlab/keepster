# 69: The performance harness — fixtures, the tripwire and the first baseline

**Blocked by:** 38 (The first Snapshot — History Store invariants and the write path), 44 (Reconciliation — the
guarantee, and what counts as evidence of change)

**Status:** ready-for-agent

**What to build:** the numbers the design rests on become measurable and defended, so a regression that makes capture
five times slower fails a command instead of being discovered by a user.

Two layers with different audiences: an **outcome budget** that states what the product promises, reviewed at release
and published per platform with two platforms marked unverified; and an **operation tripwire** that promises nothing
and fails mechanically. A tripwire line exists **only** where it protects a named invariant or mirrors an outcome
line.

- [ ] A seeded generator, committed as code, emits a tree **and** a change stream, with a declared file-size
      distribution, directory fan-out and content compressibility, in a small profile and a ceiling profile
- [ ] The generator's bench parameters live in a frozen profile guarded by a digest test, so a fixture's shape cannot
      move without the thresholds moving in the same commit
- [ ] Degenerate fixtures are separate and small: a near-limit path, a non-UTF-8 name, symlinks, empty directories, a
      file past the cap
- [ ] Thresholds are committed with 2× headroom, and a breach fails the bench command with a non-zero exit
- [ ] Tripwire lines cover the named invariants: the baseline seeded from the previous tree, lexicographic entry
      order, validation left off on an uncached index, an excluded directory never entered, and rules applied before
      watches
- [ ] Reading a tree is **measured for the first time** and recorded as a baseline rather than gated; if the estimate
      behind the capture budget fails, that is reported rather than absorbed
- [ ] Every recorded number carries its environment — processor, memory, filesystem, kernel — and the fixture never
      lives on a memory-backed filesystem
- [ ] The published outcome table carries the per-platform unverified marks, and the read-side lines are included
