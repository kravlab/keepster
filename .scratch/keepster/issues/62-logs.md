# 62: Logs — one rotated file per user, and a debug level that expires

**Blocked by:** 42 (Configuration — two levels, provenance, automatic reload, `Degraded`)

**Status:** ready-for-agent

**What to build:** when something goes wrong a user has a readable log in a known place on every platform, and a debug
level left on overnight can neither fill the disk the daemon is protecting nor push the useful lines out of the
window.

The platforms' native sinks lose either the content or the readability, so the log is our own rotated file in a
per-user directory; on Linux the journal still receives startup and fatal lines. Volume is bounded by size-triggered
rotation plus a retained-file count — no library caps total bytes, so a total is only ever "size times files" — and
the level is an operation with an expiry, which is the root-cause answer rather than the compensating one.

- [ ] The daemon writes to a rotated file in the documented per-user directory on each platform, at the configured
      level, defaulting to informational
- [ ] Rotation triggers on size, retains a bounded number of files, and the declared ceiling is a configuration key
- [ ] Raising the level takes a duration and returns to the default by itself; an active temporary level is a row in
      `status`
- [ ] The rotation writer is covered by tests on all three platforms, including the boundary and the semantics of
      renaming an open file on Windows
- [ ] Lines are human-readable with structured fields by default, switchable to one JSON object per line by a global
      key, one file either way
- [ ] The format and the ceiling describe the daemon, so they are never settable per Root
- [ ] On Linux only startup and fatal lines reach the journal, so the volume is not duplicated
