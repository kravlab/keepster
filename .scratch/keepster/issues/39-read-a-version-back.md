# 39: Read a version back — the derived index, entries, content and the timeline

**Blocked by:** 38 (The first Snapshot — History Store invariants and the write path)

**Status:** ready-for-agent

**What to build:** a user can list a Snapshot's files, read one version's bytes, and page through a Root's
chronology — including the chronology of a single path. This is the read side the whole product exists for.

The `SnapshotId`, path-change and cross-Segment ordering indexes live in SQLite as a **derived cache** that may be
deleted at any time and rebuilt from git objects; the commit and ref update always precede the index write, and a
per-Segment watermark turns a killed daemon into a millisecond catch-up. The timeline is one cursor-paginated
resource that also answers "which Snapshots changed this path".

- [ ] Listing entries under a path works with the default shallow depth and with the full-depth form, reporting each
      entry's path, kind, size, mode and binary flag
- [ ] Reading a version returns raw bytes with content length, an entity tag and range support; a partial read of a
      large file does not read the whole blob into the response
- [ ] The timeline returns Snapshots newest-first with cursor pagination, a next cursor, and the documented item
      shape; a cursor pointing at an already-pruned Snapshot is not an error
- [ ] Filtering the timeline by a path returns only the Snapshots that changed it, with the kind of change
- [ ] Deleting the index and restarting rebuilds exactly the same rows from git objects, asserted row for row
- [ ] Killing the daemon between the ref update and the index write leaves the index lagging, never pointing at a
      commit that does not exist, and the watermark repairs it at the next start
- [ ] `keepster log` and `keepster cat` mirror the timeline and content operations, including the CLI selectors
      (`latest`, `~N`, a timestamp, a duration), which resolve to ordinary timeline queries
