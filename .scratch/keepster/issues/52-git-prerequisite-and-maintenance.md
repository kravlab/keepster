# 52: The external `git` prerequisite, the Maintenance Pass and the disk cap

**Blocked by:** 51 (Retention — the ladder, the Segment Period and thinning)

**Status:** ready-for-agent

**What to build:** space that Retention condemned is actually reclaimed, a user with an optional disk cap sees it
respected, and a user with no external `git` gets a degraded daemon with a clear reason rather than a broken one.

Capture runs on the in-process git library; the external binary is a **maintenance-only** prerequisite. Its absence
marks maintenance unavailable with a reason in `status`, which exits non-zero, and the one real consequence —
Retention freeing nothing — is bounded by pausing capture at the disk cap rather than filling the disk. Reclaiming
requires pruning immediately, which the exclusive per-store lock is the precondition for.

- [ ] Discovery never reads an inherited `PATH`: an optional but authoritative configured path, then stable absolute
      locations, the Windows registry, and on macOS a `stat` only — the tooling-installation prompt is never raised
- [ ] The minimum version is enforced leniently at startup and again on the first real pass, with the failure's
      stderr reaching `status`
- [ ] Every invocation is hermetic: no shell, an explicit store directory, system and global configuration disabled,
      a whitelisted environment, correctness keys passed inline; forcing and the bundled maintenance command are
      never used
- [ ] A Maintenance Pass reclaims space after Retention deleted a ref, asserted by measuring a real store before and
      after
- [ ] Deleting a ref without the pass frees nothing, asserted as the reason the pass exists
- [ ] The pass runs at most daily, or immediately when the cap is exceeded, always in an idle window and never
      overlapping a capture
- [ ] The optional per-Root cap applies the ladder, then maintenance, then measures, then drops whole Segments
      oldest-first, then maintains again; it floors at the newest Tier, overshoots and reports rather than blocking
      capture
- [ ] With maintenance unavailable, capture pauses at the cap instead of filling the disk, and `status` says why
- [ ] Keepster prints the platform's install command and never runs a package manager
