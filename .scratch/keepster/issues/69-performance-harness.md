# 69: The performance harness — fixtures, the tripwire and the first baseline

**Blocked by:** 38 (The first Snapshot — History Store invariants and the write path), 44 (Reconciliation — the
guarantee, and what counts as evidence of change)

**Status:** done

**What to build:** the numbers the design rests on become measurable and defended, so a regression that makes capture
five times slower fails a command instead of being discovered by a user.

Two layers with different audiences: an **outcome budget** that states what the product promises, reviewed at release
and published per platform with two platforms marked unverified; and an **operation tripwire** that promises nothing
and fails mechanically. A tripwire line exists **only** where it protects a named invariant or mirrors an outcome
line.

- [x] A seeded generator, committed as code, emits a tree **and** a change stream, with a declared file-size
      distribution, directory fan-out and content compressibility, in a small profile and a ceiling profile
- [x] The generator's bench parameters live in a frozen profile guarded by a digest test, so a fixture's shape cannot
      move without the thresholds moving in the same commit
- [x] Degenerate fixtures are separate and small: a near-limit path, a non-UTF-8 name, symlinks, empty directories, a
      file past the cap
- [x] Thresholds are committed with 2× headroom, and a breach fails the bench command with a non-zero exit
- [x] Tripwire lines cover the named invariants: the baseline seeded from the previous tree, lexicographic entry
      order, validation left off on an uncached index, an excluded directory never entered, and rules applied before
      watches
- [x] Reading a tree is **measured for the first time** and recorded as a baseline rather than gated; if the estimate
      behind the capture budget fails, that is reported rather than absorbed
- [x] Every recorded number carries its environment — processor, memory, filesystem, kernel — and the fixture never
      lives on a memory-backed filesystem
- [x] The published outcome table carries the per-platform unverified marks, and the read-side lines are included

## Comments

- Three of the five invariant lines are baselines rather than gated, each saying why in the gate's report:
  - `capture.one_changed_path` (ADR 0004): with the index unseeded, a capture of the small profile grew only 1.2×,
    since the walk and the rebuilt tree both grow with the files, so no fixture size lets it breach a 2× limit;
  - `capture.first_flat_tree` (ticket 12's order): reversed insertion of 20k entries adds about 24 ms to a 1.36 s
    capture, and would double it only near 800k files;
  - `capture.first_uncached` (strict validation): the invariant is not in the code; ticket 87 decides it and gates
    the line.

  The figures come from the uncommitted change that broke each invariant, not from the reference machine.
  Ticket 30's 5.1× and 26.4× were measured on `write_tree` and index insertion alone, not on a whole capture.
  The two exclusion lines breach when their invariant is broken, and are gated. A capture is gated all the
  same: `capture.step`, a step of the small profile's churn, mirrors the Capture outcome line at small scale.
