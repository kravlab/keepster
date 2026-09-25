# 37: The Registry — register, deregister and resolve a Watched Root

**Blocked by:** 36 (Workspace skeleton, the daemon and `keepster status`)

**Status:** done

**What to build:** a user can register any directory as a Watched Root, list their Roots, deregister one, and see
every Root in `keepster status`. Nothing is captured yet; this ticket establishes identity and the Registry.

A Root is identified by a generated ULID that survives renaming or moving the directory, and its path is a mutable
attribute. The Registry is a small daemon-owned file per Root beside its History Store location, replaced
atomically, because the watched path is the one fact git could never reproduce. Roots may not overlap.

- [x] `keepster watch <dir>` registers a Root and prints its human name (`basename` plus a short id); the API
      accepts the same operation
- [x] Registering a path inside an existing Root, or containing one, is refused with the overlap error naming the
      offending Root; the same rule applies to a move
- [x] `keepster roots` and the API list Roots; resolving a path to a Root is its own query
- [x] Deregistration keeps the History Store by default; erasing it requires the explicit purge form with its own
      confirmation
- [x] Moving a Root is an explicit operation that changes the path and costs the history nothing
- [x] The CLI resolves the Root from the nearest registered ancestor of the working directory, accepts a full id, an
      unambiguous id prefix or a path, and errors with a pointer to `watch` when nothing matches
- [x] Registry files are written atomically and survive a kill mid-write; a rewritten file is never partially
      readable
- [x] `status` lists each Root with its state, and the exit code becomes non-zero as soon as a Root is not
      `Watching`

## Comments

- **Where the Registry lives**: one directory per Root under a per-user store base, holding `root.toml`
  written atomically (temporary file, flush, rename, flush the directory). Enumerating Roots is a
  directory listing, so nothing has to be kept consistent with an index. Deregistering removes only the
  record and keeps the directory — which makes a directory without a record exactly the orphaned store
  ticket 53 teaches Retention to decay.
- **Every Root reports `degraded` with `capture_unavailable`**, and `keepster status` therefore exits
  non-zero as soon as one is registered. This is deliberate and temporary: no Trigger and no capture
  exist until tickets 43 and 44, and reporting `watching` would be the comfortable lie ticket 05
  forbids.
- **`?path=` filters the collection** rather than returning a different shape, and a path under no Root
  is an empty array rather than a 404 — the rule ticket 07 states for the timeline, applied here, which
  also keeps the response type single for ticket 40's generated document.
- **A Root's identity is a ULID minted from a passed-in moment**, so ADR 0012 holds: the daemon has
  exactly one wall-clock read, at the one point where a moment enters the domain, behind an explicit
  allow.
- **The path is made absolute but not canonicalised.** Resolving symlinks here would pre-empt ticket 49,
  which owns whether a Root registered through a link follows the link or its target.
- **Paths are UTF-8 only for now.** A path reaches the daemon as text, so a non-UTF-8 directory cannot
  be registered through the API at all; ticket 48 adds the byte-exact representation and with it that
  case.
- **Atomicity is asserted, not assumed**: a test rewrites one record three hundred times while a reader
  lists the Registry in a loop, and every read must parse and see exactly one Root — which is the
  property that matters, since a half-written record is what a reader would otherwise see. A kill
  *during* the write is not simulated; the mechanism that makes it safe (temporary file, flush, rename,
  flush the directory) is what the test exercises.
- **A review of this work found one design fault worth naming**: `Registry::list` aborted on the first
  record it could not read, and the daemon turned that into a `500` for both `/v1/roots` and
  `/v1/status` — so one bad file took the whole report down, which is precisely the all-or-nothing shape
  ticket 08 rejects ("failing to open a Root is a value, not a startup error") and which left
  `RootState::Unavailable` unreachable. The listing now carries each record's own failure, and such a
  Root is reported as `Unavailable` with `store_too_new` or `record_unreadable`; its identity is always
  known, being the directory's name. A Root whose directory has vanished is `Unavailable` with
  `root_missing`, and the human output prints the exact `keepster move` command, as ticket 07 requires.
- **Exit codes are keyed on `code`, not on the HTTP status** (ticket 07 makes the status the coarse layer
  and the code the precise one), with an unknown code falling back to its status exactly as ADR 0005
  requires of any client.
- **`?path=` refuses a relative path** with `path_invalid` instead of resolving it against the *daemon's*
  working directory, which is meaningless to the client that sent it.
- **Registering and moving hold a lock over list-then-check-then-insert.** Without it two requests
  arriving together would both pass the overlap check and produce the nested Roots ticket 07 forbids.
- **The MSRV leg earned its place immediately**: it caught `let`-chains, which are stable only from 1.88,
  in code written against the declared floor of 1.85. Rewritten rather than raising the floor, since
  raising it would exclude users for a syntax convenience.
- Verified: 47 tests, `cargo fmt --check`, `clippy -D warnings` on the host and for
  `x86_64-pc-windows-msvc`, `cargo deny check licenses bans`, and `cargo check` on the declared MSRV
  (1.85.1). No test has run on Windows or macOS.
- **Not done, and correct not to be**: ticket 07's "at unwatch, the latest Snapshot is pinned" has
  nothing to pin — capture arrives with ticket 38 — so ticket 53, which owns pins and orphan decay, must
  add it together with the pin mechanism.
