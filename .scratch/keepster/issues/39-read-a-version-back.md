# 39: Read a version back — the derived index, entries, content and the timeline

**Blocked by:** 38 (The first Snapshot — History Store invariants and the write path)

**Status:** done

**What to build:** a user can list a Snapshot's files, read one version's bytes, and page through a Root's
chronology — including the chronology of a single path. This is the read side the whole product exists for.

The `SnapshotId`, path-change and cross-Segment ordering indexes live in SQLite as a **derived cache** that may be
deleted at any time and rebuilt from git objects; the commit and ref update always precede the index write, and a
per-Segment watermark turns a killed daemon into a millisecond catch-up. The timeline is one cursor-paginated
resource that also answers "which Snapshots changed this path".

- [x] Listing entries under a path works with the default shallow depth and with the full-depth form, reporting each
      entry's path, kind, size, mode and binary flag
- [x] Reading a version returns raw bytes with content length, an entity tag and range support; a partial read of a
      large file does not read the whole blob into the response
- [x] The timeline returns Snapshots newest-first with cursor pagination, a next cursor, and the documented item
      shape; a cursor pointing at an already-pruned Snapshot is not an error
- [x] Filtering the timeline by a path returns only the Snapshots that changed it, with the kind of change
- [x] Deleting the index and restarting rebuilds exactly the same rows from git objects, asserted row for row
- [x] Killing the daemon between the ref update and the index write leaves the index lagging, never pointing at a
      commit that does not exist, and the watermark repairs it at the next start
- [x] `keepster log` and `keepster cat` mirror the timeline and content operations, including the CLI selectors
      (`latest`, `~N`, a timestamp, a duration), which resolve to ordinary timeline queries

## Comments

- **One index per Root, at `index.sqlite` beside `history.git`** in the Root's own directory, which is the place
  ticket 38 reserved for it. Research 03 sketched one database per data directory; per Root keeps a purge the
  removal of one directory, confines a damaged index to one Root, and lets each store and its index be opened
  together for one operation. The schema is three tables — `snapshots`, `changes`, `watermarks` — with
  `user_version = 1`; any mismatch, a file that is not SQLite, or a failed `quick_check` discards the three-file
  set and rebuilds. A busy lock or a full disk is *not* treated as damage: discarding a database another
  connection holds is itself a corruption vector, so those are reported instead.
- **The commit and the ref come first, then the index, in one store function.** `HistoryStore::capture` writes
  git and then calls the same catch-up that recovery uses, so the normal path and the repair path are one loop.
  An index that fails during a capture fails nothing — the Snapshot is durable — and the daemon logs the lag.
  Catch-up runs at every start before the control channel is bound (with `quick_check`), after every capture,
  and lazily on every read; the read-side check costs one ref listing and one small query when nothing lags.
- **"What changed" is measured against the previous Snapshot in `SnapshotId` order, across Segments**, not
  against the git parent — the first Snapshot of an hour is parentless but still has a predecessor (ADR 0004).
  A watermark that is no longer an ancestor of its Segment's tip, a vanished Segment, or a new Snapshot sorting
  before an indexed one each rebuild the whole index, because each invalidates the predecessor of later rows.
  Retention (ticket 51) will thin Segments routinely and may want a narrower rebuild; this one is correct and
  costs about 1,600 commits at the ladder's bound.
- **Two trailers were added to every Snapshot commit**: `Keepster-Trigger` (`manual` until ticket 43) and
  `Keepster-Omitted` (a count). Both are needed by a rebuild from git alone; the count is deliberately not the
  paths, which ticket 48 records per path in the sidecar (ADR 0006). A commit with no Trigger trailer reads as
  `manual`, the only Trigger that existed when it was written.
- **`SnapshotId` is now strictly increasing within a millisecond** (`SnapshotId::following`): two captures in one
  millisecond would otherwise be ordered by the ULID's random half, and the timeline is that order. A clock that
  went backwards is not papered over — the identity keeps its true moment and the index rebuilds into time order.
- **`taken_at` is the identity's own moment, to the millisecond**, in the capture answer as well as the timeline,
  so the two always agree and a rebuilt index reports exactly what the original did. The capture answer used to
  carry nanoseconds from the clock read.
- **Timeline bounds are identities.** `cursor` and `before` are both exclusive upper bounds in one ULID order and
  the tighter one wins; `before=<moment>` means taken *at or before* it (what a selector "as of 14:00" needs),
  `after=<moment>` means strictly after. `limit` is 100 by default and capped silently at 1000; `0` is refused.
  The path filter matches a file path exactly — a directory path matches nothing until a prefix form is wanted.
- **New problem codes**: `snapshot_not_found` and `path_not_found` (404), `not_a_file` and `not_a_directory`
  (409), and `path_invalid`, `limit_invalid`, `cursor_invalid`, `moment_invalid`, `depth_invalid` (400). The CLI
  maps the not-founds to exit 4 and the rest to the usage code 2.
- **Content is streamed from a blocking thread through a four-chunk channel**, so the response never holds the
  version whole. The store streams loose objects through libgit2's ODB reader and only reads up to the end of a
  range; a *packed* object cannot be streamed by libgit2 and is read whole into memory, though the response still
  carries only the range. Nothing packs a store yet — ticket 52's maintenance will, and is where that bound
  should be measured. One range is honoured; several are answered whole (RFC 9110 permits it). The `ETag` is the
  blob id, so equal bytes share a tag; `If-None-Match` answers `304`, which costs nothing given the tag.
- **`entries` reports `mode` as the permission bits this build records** — `0o644` or `0o755` for a file, nothing
  for a directory or symlink — until ticket 50's sidecar holds all twelve. The per-entry skip marker of an
  incomplete Snapshot that ticket 07 describes needs the per-path record and is ticket 48's to add.
- **CLI.** `keepster log [PATH] [-r] [-n N] [--before WHEN] [--after WHEN] [--json]` and
  `keepster cat PATH [-s SELECTOR] [-r]`. Selectors are `latest`, `~N` (counting the Root's Snapshots, as
  `HEAD~N` counts commits), a full `SnapshotId`, an RFC 3339 moment or a local date and time, and a duration
  (`90s`, `15m`, `2h`, `3d`, `1w`) — each reduced to `?limit=…` or `?before=…&limit=1`, paging by cursor when
  `~N` reaches beyond one page. The moment a duration counts back from is read in `keepster-daemon`'s `main.rs`
  and passed in, because ADR 0012 keeps that crate the only clock reader; `keepster-cli` gained `jiff` and still
  links no git, SQLite or core. `log --json` prints one array of the daemon's own items, unparsed, so a field this
  build does not know still reaches a script.
- **Dependencies**: `rusqlite 0.40.2` (bundled SQLite) in the store, `futures-util` without default features in
  the daemon, `jiff` in the CLI. The MSRV still holds: `cargo +1.87 check --workspace --all-features` passes.
- **This container runs as root, which the permission tests cannot see through.** Root reads a mode-`000` file,
  so the four tests that make a file unreadable fail here — three of them already did on `main`. Every test binary
  was therefore also run as `nobody`: 127 passed, none failed (the clock lint, which runs `cargo`, and the version
  test, whose own `git rev-parse` refuses a repository owned by another user, pass as root).
- Verified: `cargo fmt --check`, `clippy --all-targets --all-features -D warnings`, the CLI dependency gate,
  `cargo +1.87 check`, the whole suite as described above. **No test has run on Windows or macOS.**
- **Review, after the first commit: six defects fixed, four transferred.**
  - *A read could rebuild the index backwards.* `catch_up` read the Segment tips before taking the write lock, so
    a read racing a capture compared the capture's fresh watermarks with its own stale tips, took them for a
    rewritten Segment and rebuilt the whole index back to the older state. The tips are now read under the lock,
    where they are at least as new as any committed watermark. No test can force that interleaving without a hook
    in production code; the fix is argued in the comment beside it.
  - *An index that could not be opened refused the capture.* The store opened SQLite eagerly, before git was
    written. It now opens the index on first use, so a capture writes its commit and ref whatever the index does,
    and an index that cannot even be opened shows up as `index_lag` — tested with a directory in the index's place.
  - *A failed count after the commit failed the request.* `changed_paths` is now a `Result` in the store and an
    optional field on the wire, absent only from a capture's own answer when the count failed; the request
    succeeds, so a client does not retry into a second Snapshot. Inducing a diff failure needs a damaged object
    store mid-capture, so this path is not tested.
  - *Two first opens of an empty index raced to create the schema.* Creation now re-checks inside `BEGIN
    IMMEDIATE`. The test — eight readers on a deleted index, twenty times — failed three runs out of three on the
    previous code, and on the way to passing it found two more defects of the same race. Switching a fresh file
    to WAL can answer `SQLITE_BUSY` without running the busy handler, so the switch is now retried briefly, and an
    index already in WAL is only asked, which takes no lock. And the version and the table count were two
    separate reads, which could straddle another connection's commit, call a healthy index foreign and discard it
    from under its readers (`SQLITE_IOERR_FSTAT`); an unversioned file is now judged only under the write lock.
    Thirty consecutive runs pass, as root and as an unprivileged user.
  - *`keepster cat ../notes.md` from a subdirectory was "not inside the Root".* `std::path::absolute` keeps `..`
    on Unix; the CLI now resolves `.` and `..` by name, as the shell does, before resolving the Root.
  - *`keepster log | head` exited 1.* A closed pipe is success for `log` as it already was for `cat`.
  - Transferred: the all-or-nothing rebuild after thinning or a backwards clock step (ticket 51), the cost of
    `entries` and `content` on a packed store (ticket 52), and taking the single-instance lock before index
    recovery (ticket 61). Still open here, as a known cost: `keepster log` without `-n` collects every page before
    printing the first line.

