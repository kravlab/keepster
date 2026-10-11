# 52: The external `git` prerequisite and the Maintenance Pass

**Blocked by:** 51 (Retention — the ladder, the Segment Period and thinning)

**Status:** done

**What to build:** space that Retention condemned is actually reclaimed, and a user with no external `git` gets a
degraded daemon with a clear reason rather than a broken one. The optional disk cap moved to ticket 80.

Capture runs on the in-process git library; the external binary is a **maintenance-only** prerequisite. Its absence
marks maintenance unavailable with a reason in `status`, which exits non-zero; the one real consequence — Retention
freeing nothing — is bounded by ticket 80's cap. Reclaiming requires pruning immediately, which the exclusive
per-store lock is the precondition for.

- [x] Discovery never reads an inherited `PATH`: an optional but authoritative configured path, then stable absolute
      locations, the Windows registry, and on macOS a `stat` only — the tooling-installation prompt is never raised
- [x] The minimum version is enforced leniently at startup and again on the first real pass, with the failure's
      stderr reaching `status`
- [x] Every invocation is hermetic: no shell, an explicit store directory, system and global configuration disabled,
      a whitelisted environment, correctness keys passed inline; forcing and the bundled maintenance command are
      never used
- [x] A Maintenance Pass reclaims space after Retention deleted a ref, asserted by measuring a real store before and
      after
- [x] Deleting a ref without the pass frees nothing, asserted as the reason the pass exists
- [x] The pass runs at most daily, ~~or immediately when the cap is exceeded,~~ always in an idle window and never
      overlapping a capture — the cap's immediate pass moved to ticket 80
- [x] ~~The optional per-Root cap applies the ladder, then maintenance, then measures, then drops whole Segments
      oldest-first, then maintains again; it floors at the newest Tier, overshoots and reports rather than blocking
      capture~~ — moved to ticket 80
- [x] ~~With maintenance unavailable, capture pauses at the cap instead of filling the disk, and `status` says why~~ —
      moved to ticket 80
- [x] Keepster prints the platform's install command and never runs a package manager

## Comments

- **From ticket 39's review: packing changes the cost of two read paths.** libgit2 cannot stream a packed object,
  so once this ticket's maintenance packs a store, `content` reads the whole blob into memory before handing on
  the requested range, and `entries` inflates every file whole just to test its first 8000 bytes for the binary
  flag — `depth=infinity` would then read essentially the entire Snapshot. `entries` also opens a fresh ODB handle
  per size lookup and per binary probe. Before packing ships: open the ODB once per listing, bound the probe for a
  packed object (or make `binary` lazy or opt-in), and measure the `content` bound on a packed store.
- **Done, split in two.** The disk cap — the cap's own immediate pass, the capture pause without `git`, and the
  size and `store.over_cap` reporting — is ticket 80, which this one unblocks. The struck-out criteria above are
  ticked as moved, not as built.
- **Discovery.** `git.path` is a key of the global configuration's top level only — a per-Root `git` would mean one
  daemon running several, against ticket 11's "one process has one git" — and must be absolute. Set, it is
  authoritative: a missing or broken one is reported, never replaced by a found one. Unset, the stable places are
  tried (`keepster_core::git_candidates`; on Windows the `GitForWindows` `InstallPath` from `HKCU` then `HKLM`, both
  registry views, in `keepster-service`); macOS's `/usr/bin/git` is only `stat`ed and reported as `macos_stub`.
  The daemon looks at start and at each housekeeping look (once a minute) rather than on every configuration
  reload, so that a probe never runs under the Registry lock; a changed `git.path` is picked up within a look, and
  a git that is missing, too old or not executable is looked for again at most once a minute.
- **Validation.** `--version` is parsed leniently at discovery, and again right before the first real pass with
  that git (with a 10 s timeout; a pass has an hour). Until a pass with a given git has succeeded, its failure is the
  git's: maintenance turns `unavailable` for the daemon with reason `failed`, the version, and git's stderr in
  `status`, and it is not looked for again until `git.path` changes or the daemon restarts. After one has
  succeeded, a failure is that store's alone: logged, and retried the next day. A probe's result never overwrites a
  newer finding (each finding has a generation).
- **Hermetic invocation.** Absolute path, no shell, `--git-dir` passed as an `OsString`, environment cleared except
  `SYSTEMROOT`/`WINDIR`/`TEMP`/`TMP`/`TMPDIR`, `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL` an empty file kept in the
  store, `LC_ALL=C`, and the store's correctness keys inline (`core.logAllRefUpdates=false`,
  `core.filemode=false`, shared with `apply_invariants` so the two cannot drift). The pass is
  `gc --prune=now --quiet`, which packs the refs too; never `--force`, never `git maintenance`.
- **Schedule.** Each Root owes a pass once a day, counted from a `keepster-maintained` marker in its store, so a
  restarted daemon does not maintain every store at once; a record more than a day off the clock either way counts
  as stale. A pass runs after the Root's Retention, in its own quiet window (5 min) or past its own ceiling (6 h),
  under the store's lock taken without waiting, and is owed only while there is a usable git.
- **Surfaces.** `status` has `maintenance: available | unavailable` and a `git` object (path, version, minimum,
  reason, detail, install hint); the CLI renders it in one block and exits 11 (`MAINTENANCE_UNAVAILABLE`) when
  maintenance is unavailable and every Root is watching — a Root not watching (10) is the graver news. When the
  daemon found no usable git, the CLI offers the git its own `PATH` finds as a `[git] path = '…'` block to paste,
  never writing it. `keepster watch` prints the same block on standard error whenever maintenance is unavailable —
  on every watch, not only the first, since nothing records a first — and still exits 0. `maintenance.completed`
  is announced per Root. The install hint is printed per platform family; no package manager is ever run.
- **Packed read paths.** A listing opens the ODB once; the binary probe reads a loose object's first 8000 bytes,
  and a packed one only up to 1 MiB (`PACKED_PROBE_LIMIT`) — larger packed blobs report `binary` as unknown
  rather than being inflated whole. `content` from a packed blob still inflates the whole blob before handing on
  the range, so its memory is bounded by the blob's size; the store's capture size cap is what bounds that.
