# Keepster — wayfinder map

Label: wayfinder:map

## Destination

A buildable v1 spec for Keepster: a cross-platform background daemon that captures snapshots of watched
directories into its own git-backed store, plus the CLI and the localhost web UI for browsing, diffing and
restoring those snapshots. The map is done when nothing is left to decide before `/to-spec` can collapse it into
a spec.

## Notes

- Domain: editor-independent file history ("local history" as in JetBrains/VS Code), insurance against an AI agent
  or a stray command overwriting work before it was committed.
- Backend language is Rust (user requirement, not a decision to revisit).
- Skills every session should consult: `grilling` and `domain-modeling`. Add `codebase-design` whenever module
  boundaries or interfaces are on the table.
- The user's global instructions cap each round at one question. Ask one, wait, then ask the next.
- Plan, don't build: this map produces decisions. Implementation starts after `/to-spec` → `/to-tickets`.
- **Read-optimised, not write-optimised.** The job is to find a file, see its chronology, look at one version and
  restore it after an accidental deletion, edit or breakage — so the store is judged on how cheaply it answers
  "give me (Snapshot, path)" and "diff these two versions", not on durability. That is why git backs it: every
  version is addressable by commit and path, and diffing two commits needs no common ancestor. Backup tools
  optimise the opposite direction — chunked dedup, encryption, surviving the loss of the machine — and reading one
  file out of them means walking a tree or mounting a snapshot.

## Decisions so far

<!-- charting session, 2026-09-08; these were settled in conversation before any ticket existed -->

- **Destination scope**: v1 spec covers both core (daemon + CLI) and web UI, not core alone.
- **Watched unit is any directory**: git presence is not required, so the ignore engine and the Root identity
  are ours, not git's. Git remains a storage engine only.
- **One global daemon + a Root registry**: a single process watches every registered directory; CLI and web UI
  talk to it. Not one process per Root.
- **Platforms**: Linux, macOS and Windows in v1.
- **Snapshot triggers are configurable**: watcher-with-debounce, periodic floor snapshot and full reconciliation
  scan are all available; the user chooses which are active. A rate limit always applies, and its value is the
  user's to set. Refined by ticket 04: Reconciliation defaults to on, disabling it drops only the startup walk, and
  the response to an OS rescan signal is never optional.
- **Ignore model**: our own include/exclude rules are authoritative; honouring a directory's `.gitignore` is an
  optional layer on top, not the default source of truth.
- **Storage model**: segmented chains — one ref per retention period, each segment starting from a parentless
  commit, snapshots chained within a segment. A stable `SnapshotId` (not the commit SHA) is the public identity,
  and the history store must keep reflogs off so that deleting a ref actually frees objects. See
  `docs/adr/0001-segmented-snapshot-chains.md`.
- **Secrets**: refused by default. A built-in denylist (`.env*`, `*.pem`, `id_rsa`, `local.properties`, …) keeps
  them out of history until the user opts a path in explicitly. No encryption in v1; at-rest protection is the
  operating system's job (LUKS/FileVault/BitLocker) plus restrictive permissions on the store.
- **Restore**: writes over the working file atomically (temp file + rename), but always takes a safety snapshot of
  the current state first, so a restore is itself undoable.
- **Web UI delivery**: the daemon serves it over HTTP on 127.0.0.1 with a token; the built SPA is embedded in the
  binary. One artifact, no runtime dependencies.
- **Root identity**: explicit registration (`keepster watch <dir>`), identity is a generated surrogate id;
  the path is a mutable attribute, so moving or renaming a directory keeps its history.
- **Git engine is split by role**: `git2` (libgit2, vendored) owns the hot path — writing a Snapshot from an index
  we own, reading blobs, tree-to-tree diffs — while store maintenance (`gc`, `repack`, packed-refs compaction) is
  delegated to an external `git` binary, which neither library provides. See
  [01 — Which git implementation backs the history store](issues/01-git-engine.md).
- **An external `git` is a mandatory prerequisite** on all three platforms, and nothing is bundled: no MinGit, no
  shipped git binaries, one distribution, no GPLv2 redistribution obligations. The cost is a prerequisite to detect
  and validate before first use — see [11 — External git prerequisite](issues/11-git-prerequisite.md). The benefit
  is that no hand-written mark-and-sweep exists in v1.
- **The watcher is a latency optimisation, not the correctness mechanism**: every platform loses changes, and
  several losses are never announced (a populated directory moved into the tree, a write through another hardlink,
  `mmap` writes, network mounts). Reconciliation is therefore the guarantee and cannot be disabled, whatever
  Trigger set the user picks. See
  [02 — Reliability limits of cross-platform filesystem watching](issues/02-filesystem-watching.md).
- **Metadata is split by derivability**: the Registry lives in per-Root plain files replaced atomically, because
  the watched path is the one fact git cannot reproduce; the `SnapshotId` and path indexes live in SQLite declared
  as a derived cache that may be deleted and rebuilt from git objects at any time. Git notes are rejected outright.
  See [03 — Where the Root registry and the snapshot index live](issues/03-metadata-store.md).

- **Configuration**: two TOML levels — a global daemon config and an unrestricted `.keepster.toml` in the
  watched directory — merged by key class (pattern lists layer with negation, sets and scalars replace). Reload is
  automatic, an invalid config never applies and leaves the Root Degraded, and the store location is itself a
  key: global by default, optionally inside the Root. See
  [04 — Configuration schema and precedence](issues/04-configuration-schema.md).

- **Daemon lifecycle**: one daemon per OS user, running as that user and never elevated, autostarted by a unit
  `keepster service install` writes and can verify (Linux linger; macOS LaunchAgent, whose logout gap is shown
  rather than hidden; Windows per-user service). One HTTP API on two listeners — permission-protected socket/pipe
  for the CLI, loopback plus token for the browser. A flock'd pid file is the single-instance authority, and only
  the service manager ever starts the daemon, so a running daemon is always a supervised one. A new Root state
  `Unavailable` separates "root gone or unreadable" from `Degraded`, and a partly unreadable tree still yields a
  Snapshot, marked incomplete. `keepster status` is the authority on health — live daemon state, non-zero exit,
  `--json` — and logs are demoted to debugging. Shutdown drains the in-flight capture and flushes pending debounce
  windows, and SIGKILL past the stop timeout is survivable. See
  [05 — Daemon lifecycle and autostart on three platforms](issues/05-daemon-lifecycle.md).

- **Retention**: a Segment Period (configurable, hourly default, per-Segment so reconfiguring needs no migration)
  plus a duration-based Tier ladder — everything for 24 h, hourly to 7 d, daily to 30 d — where the survivor of a
  thinned period is its last Snapshot and Retention only ever touches a Segment lying *wholly* past a boundary, so
  thinning happens at most once per Segment and the pass is idempotent. An opt-in per-Root disk cap drops whole
  Segments oldest-first *after* maintenance (ref deletion frees nothing until `gc`), floors at the newest tier and
  never blocks capture. An exclusive per-store lock is the precondition for `git gc --prune=now`, which is required
  because git's two-week `gc.pruneExpire` default would otherwise make Retention free nothing; Retention runs per
  rollover, `gc` at most daily, both deferred to idle. `forget <glob>` rewrites every Segment and pin containing a
  path — cheap (~1,600 commits) precisely because the ladder bounds the store — preserves every `SnapshotId`, never
  touches the Working Tree, and takes no default on whether to also exclude the path. See
  [06 — Retention policy and segment granularity](issues/06-retention-policy.md).

- **Pins are self-enforcing, not a flag**: a pinned Snapshot is re-created as a parentless commit under
  `refs/keepster/pins/<SnapshotId>`, so `gc` physically cannot collect it, it survives an index rebuild, it
  costs one commit object, and Retention, thinning and the disk cap stay entirely pin-unaware. A flag would have
  had to carve an exception into ticket 03's "the index is wholly derived" invariant. See
  `docs/adr/0002-pins-as-parentless-commits.md`.

- **The HTTP API is a public contract, the CLI a thin client over it**: `/v1`, additive-only, breaking changes only
  via `/v2`, and the embedded SPA gets no privileged endpoints — because a third-party web or native UI cannot be
  served by shelling out to the CLI. Addressing is ULIDs for both `Root` and `SnapshotId`, one canonical URL form
  each, with cwd resolution and selectors (`latest`, `~N`, `2h`) living in the CLI; `path=` is one query parameter
  across every endpoint; content is raw bytes with `Range`, diffs are computed by the daemon; Restore only
  overwrites and creates and never deletes (Include Rules make "make identical" irreversibly destructive outside
  what the tool can see); errors are RFC 9457 with a stable `code` plus a CLI exit-code table; the browser gets a
  per-start token through a single-use bootstrap exchanged for an `HttpOnly` cookie, guarded by `Host`/`Origin`
  checks; live updates are SSE with `Last-Event-ID` replay and an honest `resync`; `forget` and Maintenance are
  jobs, pins are synchronous; `status --json` is a verbatim passthrough of `GET /v1/status`; configuration is
  read-only with per-key provenance plus an `explain?path=` diagnostic; `open` launches the configured editor via
  argv without a shell and can materialise a historical version. See
  [07 — Daemon HTTP API and CLI surface](issues/07-api-and-cli-surface.md).
- **Unwatch keeps the history but lets it decay**: deregistering pins the latest Snapshot (ADR 0002) and leaves
  Retention running, so an orphaned store falls down the normal ladder to exactly that pinned last state instead of
  either emptying itself within 30 days (a silent `purge`) or holding disk forever. Refines ticket 06. Roots may not
  overlap, and a move is only ever explicit. See [07](issues/07-api-and-cli-surface.md).
- **`Project` is renamed `Watched Root`** (short `Root`): the entity is a registration whose path is a mutable
  attribute, so a folder-shaped word asserts the opposite, and `Project` implies code for a tool that will watch
  `~/Documents/notes`. The documentation-only pass is done: `CONTEXT.md`, this map, tickets `01`–`12`, the
  registry file (`project.toml` → `root.toml`) and the commit trailer (`Keepster-Project` → `Keepster-Root`).
  The rename also settled one identifier ticket 07 had left behind — config provenance is now
  `default / global / root`. See
  [13 — Rename Project to Watched Root](issues/13-rename-project-to-root.md).

- **Eight crates, and the core owns no ports**: `core` is a pure decision kernel (no traits, no I/O), `app` holds
  use-cases over a *concrete* `SnapshotStore` that owns both git and the derived index, and `api`, `watch`,
  `service` and `ui` are separate so that Cargo — not review — enforces what each may not know; the CLI links
  neither `git2` nor `rusqlite` nor `core`. Policy is `decide(state, event, now) -> (Vec<Action>, PolicyState)`
  over values, a failed Root is a `RootRuntime` variant rather than a startup error, time is a parameter, and the
  workspace contains no fake but a real git store in a temp dir. `cargo build` never needs node. See
  [08 — Workspace structure and module boundaries](issues/08-workspace-structure.md) and
  `docs/adr/0003-no-ports-in-core.md`.

- **The diff editor is CodeMirror 6, fed by the daemon's hunks**: about seven times lighter than a trimmed Monaco
  (96.7 against 671 KiB gzip before first paint), and the only engine that renders hunks computed elsewhere
  (`diffConfig.override`), which ticket 07's daemon-side diff requires, so MergeView's own diff is never used.
  Line wrapping is an opt-in button, and a file with a line over a configurable threshold (20,000 characters by
  default) opens without syntax highlighting. See
  [09 — Web UI diff editor: CodeMirror 6 or Monaco](issues/09-web-ui-prototype.md).

## Not yet specified

- Packaging and distribution per OS (single binary, package managers, code signing on macOS/Windows). The git
  prerequisite is no longer part of this: it graduated into ticket 11.
- Upgrade path for the on-disk history format once it changes.
- Behaviour on network and removable filesystems, and on directories that disappear while watched.
- Performance budget and how it is measured (snapshot latency, daemon idle cost, store growth per day).
- Observability leftovers after ticket 05: log rotation and retention per platform, and whether a
  diagnostics/support-bundle command exists to collect state for a bug report. The health surface itself is
  settled — `keepster status`, live state, non-zero exit.
- Testing strategy for filesystem-heavy and time-dependent code.
- Search across snapshots (by content or by path) as a UI feature.
- The v1 SPA stack. The ticket 09 prototype runs on Vite, Svelte 5 and TypeScript, picked for the prototype and not
  yet confirmed for v1.

## Out of scope

- Proactive desktop notifications when a Root stops being covered: three extra platform
  integrations that deliver nowhere on a headless lingering daemon, and mutable into silence.
  The pull-based `status` surface carries v1 instead. See
  [05 — Daemon lifecycle and autostart on three platforms](issues/05-daemon-lifecycle.md).
- Encryption of the history store at rest.
- Any remote: pushing, syncing or backing up history off the machine.
- Being a backup tool. Durability against losing the machine is a separate concern served by separate tools, and
  chasing it here would trade away the read latency this tool exists for.
- A Zed extension. The whole point of the design is to sit below the editor, not inside it.
- Editing files inside the web UI. It reads history and restores; it is not an editor.
- Any interaction with the watched directory's own `.git` (its index, HEAD, branches or stash).
