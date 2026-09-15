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

- **Configuration**: two TOML levels — a global daemon config and a `.keepster.toml` in the watched directory,
  unrestricted *within capture policy* — merged by key class (pattern lists layer with negation, sets and scalars
  replace). Two classes never reach the Root level: keys describing the daemon (the HTTP port and the loopback
  listener, per ticket 24) and keys that make the daemon **execute** something (`ui.editor`). Reload is
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
  The rename also settled one identifier ticket 07 had left behind — config provenance, now
  `default / global / global-pattern / global-root / root` after the per-Root override relocation below. See
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

- **File kinds are settled around a new term, `Withheld`**: a Snapshot records a path whose content it
  deliberately does not hold — over the 100 MiB default cap, a FIFO, socket or device, a reserved `.git` name, an
  unrepresentable name, a cloud placeholder — with a reason, apart from `incomplete`, which stays "meant to capture,
  could not read". Binaries are stored as-is; symlinks are stored as links, with an opt-in `follow_symlinks` pattern
  list covered by periodic Reconciliation of the targets rather than extra watches; the executable bit lives in the
  tree mode and fuller permissions are deferred to ticket 17; cloud files are never hydrated; paths travel as bytes,
  with `path_bytes` on the wire. Refines ticket 07's Restore: a whole-request preflight with `dry_run`, per-reason
  confirmations plus `force`, per-path resolutions for name collisions and invalid names, `skip_unresolvable`, and a
  refusal for hardlinked files. See
  [10 — Binary files, symlinks, permissions and path edge cases](issues/10-file-kind-edge-cases.md).

- **The external `git` is a maintenance-only prerequisite, and its absence degrades instead of blocking**: capture
  runs on in-process `git2`, so a missing, too-old, non-executable or macOS-stub git marks a daemon-global
  `maintenance: unavailable` (with reason) in `keepster status`, which exits non-zero, and the one real consequence
  — Retention frees nothing without `gc` — is bounded by pausing capture at the disk cap rather than filling the
  disk. Discovery never reads an inherited `PATH`: an optional but unconditionally authoritative `git.path`, then a
  list of *stable* absolute paths (Nix/Guix profile symlinks, not hashed store paths), the Windows registry with
  `cmd\git.exe`, and on macOS `stat` only — `/usr/bin/git` and `xcrun` are never invoked because they raise the
  Command Line Tools dialog. Validation is a leniently parsed `--version` at startup plus the first real
  maintenance pass, whose stderr reaches `status`. Minimum version **2.32.0** (where `GIT_CONFIG_GLOBAL`/`SYSTEM`
  arrived), which excludes nothing still supported, while 2.41.0 would have cost Ubuntu 22.04 LTS and Debian 12 for
  an efficiency-only gain. Every invocation is hermetic — no shell, explicit `--git-dir`, system and global config
  disabled, whitelisted environment, correctness keys as `-c` — and `--force` and `git maintenance` are forbidden.
  Keepster prints the platform's install command and never runs a package manager. See
  [11 — External git prerequisite](issues/11-git-prerequisite.md).

- **The snapshot baseline is re-seeded from the previous Snapshot's tree, and the daemon holds no index between
  Snapshots**: `add_frombuffer` requires an owning repository, so every entry must carry an `Oid` we obtained
  ourselves — which makes "rebuild per Snapshot" mean re-hashing the whole Root, not re-inserting entries. The
  vehicle stays `git2::Index`, because `read_tree` works on an owner-less index and fills the TREE cache, so
  `write_tree` rewrites only the changed paths' ancestry (0.162 s against 0.83 s on 100k entries) — the benefit the
  ticket had attributed to `TreeBuilder` alone. A surviving map was rejected as state that goes stale silently
  (ticket 02's unannounced losses), and it could not have been a `git2::Index` in any case, which is neither `Send`
  nor `Sync`. Consequences: an unreadable file's path enters a retry queue with backoff, because its carried-forward
  entry now looks unchanged; hardlink grouping by `(dev, ino)` belongs to Reconciliation, the only pass that still
  walks; a capture meeting the maintenance lock neither blocks nor drops but coalesces into one Snapshot once the
  lock is released; entries must be applied in lexicographic order (quadratic otherwise); and the previous
  Snapshot's tree must be readable at every capture, including the first of a new Segment. See
  [12 — Snapshot write path](issues/12-snapshot-write-path.md) and
  `docs/adr/0004-baseline-from-previous-tree.md`.

- **The `/v1` document is generated from `keepster-api`'s types, and the error-code set stays open**: the orphan
  rule leaves no way to generate schemas outside the crate owning the DTOs, so ticket 08's "serde alone" becomes
  "serde plus an optional, default-off `openapi` feature" — the forced alternative being a mirror of every DTO,
  i.e. the drift this ticket exists to remove. `utoipa-axum`'s `routes!` makes the axum route and the documented
  path one literal (a renamed handler fails to compile; a handler never routed is caught as dead code), and
  `openapi.json` is committed at the repo root — OpenAPI 3.1.0, `info.version = "1"` — with a `#[test]` comparing
  it byte for byte. "Additive-only" means `oasdiff breaking --fail-on WARN` against the **last released tag**, not
  the PR base, because the promise was made to clients and a client has only seen what shipped. The daemon serves
  the document at `GET /v1/openapi.json`, generated at runtime and before authentication, under the invariant that
  it depends on the build and never on the instance; no HTML viewer is embedded. `code` is one `ErrorCode` enum
  rendered as a `string` with a generated description rather than a closed schema enum — a closed set would freeze
  a list that has already grown twice and would fight the gate — see
  `docs/adr/0005-error-codes-are-an-open-set.md` — and ticket 07's "`1` everything else" becomes an exhaustive
  `match` with no wildcard. SSE events are typed in `components` today; OAS 3.2's `itemSchema` waits for a utoipa
  release and lands additively. See [14 — Publishing the `/v1` contract](issues/14-openapi-contract.md).

- **The history browser is one screen with three view modes**, defaulting to a Snapshot rail, with a
  file-first tree-and-scrubber mode and an activity feed alongside it. Three screens were rejected because
  the prototype showed a `?variant=` switch reading as continuity once the selected file *and* Snapshot were
  carried — which is what a view mode is — and because three screens make every user first answer "which one
  is the right one". Restore's presentation is deliberately **not** per-mode: a mode decides navigation, not
  destructive affordances, so all three reach the same preflight dialog. Prototype on branch
  `prototype/web-ui` (`70c3c64`), `ui/timeline.html?variant=A|B|C`. See
  [15 — Web UI: timeline, view modes and restore presentation](issues/15-web-ui-timeline-prototype.md).

- **Two themes, and the daemon owns the preference**: light and dark, defaulting to `system` (live
  `prefers-color-scheme`) with a pin, no user-supplied themes, and a theme that owns the diff colours and the editor
  palette so `@codemirror/merge`'s barely-visible defaults and `oneDark` are both dropped. The preference lives in a
  daemon-side UI-preferences store reached through a new public `GET`/`PUT /v1/preferences` — not `.keepster.toml`,
  which travels to every clone, and not the read-only global config, which would leave the in-UI control unable to
  make anything stick. Three daemon-wide keys: `theme`, `syntax_line_limit` (20,000, settling ticket 09) and
  `wrap_lines`, all editable from the UI with no settings screen. `localStorage` survives only as a paint cache, so
  losing it costs one flash rather than the setting, and `index.html` stays static. See
  [16 — Web UI themes and appearance settings](issues/16-web-ui-themes.md).

- **Full POSIX modes are captured, in a metadata sidecar inside the Snapshot tree**: all twelve bits, Unix only,
  held as deviations from the Root's modal defaults in one blob, `.keepster-meta`, at the tree root — the SQLite
  index was excluded by ticket 03's derived-cache invariant, a blob addressed by a trailer is physically
  unreachable and would be collected by `gc`, and the trailer text itself crosses ticket 23's "wrong for thousands
  of paths" line on the very `chmod -R 777 .` case the ticket exists for. Ownership, ACLs, xattrs and Windows
  security descriptors are out: the daemon is never elevated (ticket 05), so it could not restore them, and the ACL
  crates would add `libacl` as a runtime dependency. A permission-only change is now a real change — Reconciliation
  gets the mode from the `stat` it already performs, and the watcher, which cannot tell a `chmod` from a `touch`
  (`notify` collapses `IN_ATTRIB` to `Modify(Metadata(Any))`), `stat`s the path on an attribute event. Restore
  asks by default and is answered by one `modes: "snapshot" | "keep"` scalar that `--force` deliberately does not
  resolve, defaulting from a `restore.modes` config key; the Safety Snapshot needs no new mechanism, because
  capture now records the bits it is about to overwrite. Refines tickets 10, 12 and 06. See
  [17 — Full permission capture and restore](issues/17-full-permission-capture.md) and
  `docs/adr/0006-modes-in-a-snapshot-metadata-sidecar.md`.

- **Export is a daemon-side write to a path, not a download**: `POST /v1/roots/{r}/export` takes a `dest` the daemon
  writes itself — the browser can supply one because it always runs on the daemon's machine, so the endpoint stays
  unprivileged and a web client picks the directory through a new `GET /v1/fs?path=` (directories only,
  unrestricted, since a cookie holder can already watch `/` and read the disk through `entries`). A streamed zip64
  `archive` endpoint survives for the Downloads gesture and for handing someone one file, with its size shown before
  the click, but it can never resume. Preflight is synchronous (`dry_run`, totals) and the write is a job on ticket
  07's existing `job.*` events. Ticket 10's preflight is reused minus the three Restore-only rules, with overwriting
  off by default behind a new `overwrite` confirmation; a destination inside a Root is allowed behind `dest_in_root`
  and takes a Safety Snapshot when it overwrites, while a History Store destination is refused. The body mirrors
  Restore (`{ from, paths, dest }`), a diff stays outside export on the existing `/v1/diff` (plus `keepster diff -o`,
  because PowerShell 5.1 rewrites redirected output as UTF-16LE), a `Withheld` path is absent and reported as
  `skipped` with its reason rather than a zero-byte lie, and exported files carry the Snapshot's modes. See
  [19 — Export: saving a version into a directory of the user's choosing](issues/19-export-to-directory.md).

- **A Capture Preview classifies a directory without capturing it**, for a registered Root and for one that is not:
  `GET /v1/preview?path=` streamed as NDJSON (not a job — ticket 07 made `forget` a job because a dropped connection
  strands an irreversible operation, and preview is the inverse), never opening the History Store, taking no lock,
  counting as no Trigger, and aborting when the client disconnects. It reports **content to capture** and refuses to
  guess store growth, which is unknowable without hashing every file — that being the capture. Output groups by *the
  rule that decided*, so it is ticket 07's `explain?path=` mapped over a walk rather than a second engine, with
  top-N largest on both sides and `--depth infinity` for the detailed view. The ticket's second question is demoted
  to `pending_paths` plus an oldest-pending age on `status`, because a listing of a debounce queue is stale as it
  prints. The unregistered case honours `.keepster.toml` exactly as registration would and *shows* what it sets,
  which makes preview ticket 04's unbuilt trust prompt; `keepster watch --dry-run` is an alias for it. Refines
  tickets 04, 07 and 10. See [20 — Capture Preview](issues/20-capture-dry-run.md).

- **Pruning wins over negation**: an excluded directory is never entered, so nothing beneath it can be re-included —
  git's rule, adopted because ticket 02 already applies Include Rules *before* installing watches (~10k watches per
  100k-file tree against a 133,264 budget), so a pruned directory can produce no events for a re-included file to
  arrive on. Full semantics would discard exactly that measurement; prefix analysis is defeated by any unanchored
  negation. Git's silence is not adopted with it: `explain?path=` names the pruning ancestor, and config load warns
  for every negation an ancestor makes unreachable — a warning, not a `Degraded` Root. Recorded as
  `docs/adr/0007-pruning-wins-over-negation.md`. Closes a gap ticket 04 left
  open and refines ticket 02. See [20 — Capture Preview](issues/20-capture-dry-run.md).

- **The tray ships on all three platforms as a separate process, and it is the only carrier of desktop
  notifications** — reversing ticket 05's Out-of-scope line, which argued notifications "deliver nowhere on the
  lingering headless daemon". On Windows there was never a choice: a per-user service gets a non-interactive
  `Service-0xZ1-Z2$` window station, and Microsoft's own documented remedy is a separate process in the user's
  session over IPC; on macOS `UNUserNotificationCenter` needs a `CFBundleIdentifier`, so the tray is an `.app`.
  It is a `/v1` client over the CLI's socket (a live indicator cannot poll) and is governed by one invariant —
  **a view, never a source**: everything it shows is derivable from `GET /v1/status` and the event stream, and no
  notification carries a tray-only action. The daemon decides, the tray delivers, on every platform, which is
  cheap because on Linux the tray process notifies even where stock GNOME cannot draw its icon. Alarms fire only
  when the insurance stops (`Unavailable`, `Degraded`, capture halted at the disk cap, an unexpected daemon death,
  plus a failed user-initiated job) on transition, coalesced, with recovery replacing rather than adding — and
  muting is reported by `status`, so it never becomes a silent failure. Icon and notifications are two channels
  that fail independently, so `status` carries two capability rows, neither affecting the exit code. macOS v1 is
  ad-hoc signed through a Homebrew formula, with Developer ID and notarisation recorded as the precondition of any
  other channel. Recorded as `docs/adr/0008-tray-is-a-separate-process.md`. Refines tickets 05, 07 and 11; the CLI
  gains `service restart | start | stop`, and ticket 08's workspace a ninth crate. See
  [21 — Tray icon and notifications](issues/21-tray-and-notifications.md).

- **A Withheld path has no tree entry at all, and `.keepster-meta` becomes the one home for every per-path fact git
  cannot express.** A zero-byte blob was rejected as the lie that propagates furthest — export would create an empty
  file exactly where ticket 19 promised `skipped` with a reason — and a marker blob (or a marker symlink) as one
  that charges filtering at five levels and collides with ticket 10's symlinks-as-symlinks rule. An absent entry
  says only "no content here", which is true, and the sidecar says why; `entries` joins the two for free, because it
  already reads the sidecar for modes. The `w` record carries a reason and a kind and **deliberately no size**: a
  size would make a growing over-cap log mint one substantively empty Snapshot per capture, and ticket 20's Capture
  Preview already answers the live version of that question. Ticket 17's sidecar gains three further record types
  because two facts had no carrier at all — `i` (incomplete, with `unstable` or the errno), since ticket 10's
  "name what it omits" cannot live in ticket 03's rebuildable index; `p` (a placeholder's mtime), since a git tree
  has no mtime and without it a successfully captured file becomes `cloud_placeholder` forever once the OS
  dehydrates it; and `j`, which finally gives ticket 10's junction marker a named home. **Withheld gets no retry
  queue** — unlike `incomplete`, it is a classification of a successful observation, so it changes only when the
  path changes, which is an event — but a config reload touching the classification inputs and a Registry change
  now queue the Root for Reconciliation, refining ticket 04. Recorded as
  `docs/adr/0009-withheld-has-no-tree-entry.md`. Refines tickets 04, 10, 12 and 19. See
  [23 — How a Withheld path is represented in the git tree](issues/23-withheld-tree-representation.md).

- **The loopback listener is switchable, defaults to on, and lives only in the global daemon config** — because
  ticket 05 established that loopback TCP is not user-scoped on any platform, so the port is the one surface a
  CLI-only user has reason to decline, bounded by a `0600` token rather than by OS permissions. Default-off with
  on-demand binding was rejected for adding a second lifecycle (a bind during operation, a port collision surfacing
  at request time, a `status` row that changes with no config change) while degenerating into "from first use until
  restart", since the daemon never learns the browser closed. With the listener off the **token path disappears
  entirely** — no mint, no `0600` file, `ui token rotate` failing with the same code as `keepster ui` — so `status`
  is the only place that says whether a network surface exists. `embed-ui` stays a **separate** switch: a build
  without the SPA keeps the listener on, because ticket 07 made `/v1` public precisely for third-party clients.
  `keepster service install --no-web-ui` writes the key only when it *creates* the config file, preserving ticket
  07's refusal to round-trip an existing TOML while giving a provisioning script one step instead of four. En route
  it fixed a scope bug: ticket 04 classified the **HTTP port** as Root-level, which would let a cloned
  `.keepster.toml` relocate the daemon's only network surface — the durable form being a class test, *a key that
  describes the daemon rather than a Root is never settable at the Root level*. Refines tickets 04, 05, 07, 08
  and 21. See [24 — A CLI-only install](issues/24-cli-only-install.md).

- **Empty directories are captured by default, as a `d` record in the sidecar, and "empty" means the Snapshot holds
  no content under it** — so `logs/` containing only excluded `*.log` files counts, because git-unrepresentability
  is the actual problem. Capturing won on internal consistency: `Withheld` and `incomplete` both exist to refuse
  silent omissions, and a directory vanishing without a trace is that failure exactly, while a Restore that drops
  one hands back a working tree broken for everything needing a directory to exist. Both objections in the ticket
  dissolved on inspection — the directories that churn are excluded and, after ADR 0007, never entered, and a
  directory is empty only in the `mkdir`-to-first-write window that ticket 04's debounce collapses; detection costs
  **no system call**, being a prefix comparison on ADR 0004's sorted index. Every level of a chain gets its own
  record, keeping its own mode through ticket 17's `m` mechanism. Two keys rather than one tri-state, because
  recording is read at capture and recreating at restore: `empty_dirs` (per-Root, default on) and
  `restore.empty_dirs` (default on), beside `restore.modes`. Export creates them regardless — a fresh copy missing
  its directories is quietly wrong. Supersedes ticket 17's "empty directories stay invisible", and refines ticket
  10: a directory created during Restore takes its recorded mode where one exists, the umask only where none does.
  See [25 — Empty directories](issues/25-empty-directories.md).

- **Three keys leave `.keepster.toml` and their per-Root capability moves into the global config.** Ticket 24's
  audit asked whether a key describes the daemon or a Root; `ui.editor`, the History Store location and the secret
  opt-in list all pass it, being genuine per-project choices, so all three were left Root-settable. The test was
  too weak. Two further classes cannot come from a file that arrives in someone else's repository whatever they
  describe: **keys that make the daemon execute a command**, and **keys that decide where the user's data goes or
  what may enter it** — a Root file could otherwise put the store where `git clean -xdf` destroys it, or opt `.env`
  in past the secret denylist, and since reload is automatic an ordinary `git pull` would do either silently.
  Nothing is lost: the global config gains `[roots.<RootId>]` blocks (the id is the identity, so a move never
  detaches them) and `[[roots]]` path-glob blocks (readable, and they cover Roots not yet registered), with id
  outranking pattern — and those blocks accept the **whole** schema, so **`.keepster.toml` becomes optional**: it
  is now the way to share rules through a repository, not the way to configure a Root, and a user who never creates
  one is not exposed to the clone vector at all. Levels become
  `default / global / global-pattern / global-root / root`, which is also the new provenance enum ticket 13 owns.
  The Root file stays outermost for the keys it may still set, and one setting an excluded key is ignored with a
  warning rather than made `Degraded`. Amends tickets 04, 07, 13, 17
  and 24.

- **There is no migration of history: the content format is a property of the Segment.** ADR 0001 made a Segment
  self-contained and ticket 06 already established the pattern — a Segment property fixed at creation, of which a
  store may hold a mix indefinitely, as it does with the Segment Period — so new Segments are written in the new
  format, old ones stay readable, and the ladder retires them within about a month unaided. Automatic migration
  was rejected as `forget`'s rewrite machinery applied unasked to the user's only copy, and as a one-way door that
  punishes a downgrade; an explicit `keepster migrate` was rejected for leaving the insurance off until the user
  reads the release notes. Four version numbers govern disjoint things — the container in `root.toml` (the one
  thing rewritten automatically, being small, atomic and not history), the content in the Segment's root-commit
  trailer, the sidecar's record grammar in its header (frozen in practice), and the index schema, whose every
  mismatch is a rebuild and whose changes are therefore free. **The record alphabet is append-only**: an unknown
  type is skipped *and reported*, and changing the meaning of an existing type is forbidden — an old daemon that
  skips a new type behaves exactly as it always did, whereas a redefined type makes it confidently misread. An
  unknown version is refused as narrowly as its level allows: a newer container makes the Root `Unavailable`
  (`store_too_new`), a newer Segment leaves capture running in a new Segment of the daemon's own format, an
  unknown record is skipped. `Unavailable` widens from "directory unreachable" to "nothing can be captured", and
  ticket 11's git floor deliberately does not travel with the format. Recorded as
  `docs/adr/0010-no-store-migration.md`. Refines tickets 03, 05, 06, 11 and 17. See
  [26 — Versioning the on-disk store format](issues/26-store-format-versioning.md).

- **Packaging is tiered, and the universal mechanism is an archive plus `keepster install`** — every package format
  is a wrapper over it rather than a separate install path. Tier 1 is archives with shell/PowerShell installers
  plus a Homebrew formula in **our own tap** (homebrew-core rejects third-party prebuilt binaries); tier 2 is deb
  and rpm, the only formats that can declare ticket 11's `git` prerequisite (`git` on Debian, **`git-core`** on
  Fedora); AUR, nixpkgs, Scoop and Chocolatey are explicitly not ours, and owe only a stable tarball and a
  committed `Cargo.lock`. **Flatpak, Snap and AppImage are ruled out on blocking facts**: Flatpak cannot export a
  systemd user unit at all, its background portal *kills* an app whose permission is NO, host `git` needs a
  documented sandbox escape, and Flathub's policy rejects all four categories we fall into; strict Snap sees only
  non-hidden files, so the store is unreachable; AppImage has no dependency field and permits one `.desktop`.
  **No MSI**, because after the two decisions below a Windows installer has no work left. **One package carries
  both binaries**, since ticket 21's `ksni` choice already removed the GTK/X11 weight that usually justifies a
  split, and splitting would make the base install silent when the insurance fails. **No install or uninstall path
  ever deletes history** — ticket 26 made downgrade a supported path and on several channels a downgrade *is*
  remove-then-install — so erasing is only ever an explicit command: `uninstall [--history|--roots|--purge]` with
  `--yes` as separate consent, `purge <root>|--all`, and `unwatch --all`. **The package ships files; the user runs
  `keepster install`**, which also enables linger (polkit allows `set-self-linger` unprompted, and no package could
  do it for anyone). `ui/dist` is committed so `cargo build` needs no node, while the SPA stays embedded; and the
  product, API and store-format versions are **independent by explicit statement**. Amends ticket 05 and ticket 21.
  See [27 — Packaging and distribution per OS](issues/27-packaging-and-distribution.md).

- **Windows autostart is a Scheduled Task, not a per-user service** — amending ticket 05. `CreateService` does not
  document `SERVICE_USER_OWN_PROCESS`, `sc.exe` exposes no `userown`, Microsoft documents per-user services only as
  its own templates, MSI cannot express the type and MSIX forbids it; every SCM write needs elevation. And it never
  delivered its purpose, since a per-user service is deleted at sign-out, so Windows had macOS's logout gap anyway.
  The Task Scheduler supplies restart-on-failure, so ticket 05's "a running daemon is always a supervised one"
  survives — and **installation now needs administrator rights on none of the three platforms**. See
  [27 — Packaging and distribution per OS](issues/27-packaging-and-distribution.md).

## Not yet specified

- Observability leftovers after ticket 05: log rotation and retention per platform, and whether a
  diagnostics/support-bundle command exists to collect state for a bug report. The health surface itself is
  settled — `keepster status`, live state, non-zero exit.
- Testing strategy for filesystem-heavy and time-dependent code.
- Search across snapshots (by content or by path) as a UI feature.
- The CI pipeline itself: format, clippy, the platform matrix, node for `make ui`, and which of the `make` targets
  ticket 14 defined run where. Ticket 14 deliberately added no CI files, so **`make check-api` is a manual release
  gate until a pipeline exists** — the one commitment that decays silently if nothing records it.

## Out of scope

- Encryption of the history store at rest.
- Any remote: pushing, syncing or backing up history off the machine.
- Being a backup tool. Durability against losing the machine is a separate concern served by separate tools, and
  chasing it here would trade away the read latency this tool exists for.
- A Zed extension. The whole point of the design is to sit below the editor, not inside it.
- Editing files inside the web UI. It reads history and restores; it is not an editor.
- Any interaction with the watched directory's own `.git` (its index, HEAD, branches or stash).
- Mounting a Snapshot, or a Root's history, as a read-only filesystem. Post-release by user decision: the mechanism
  differs on all three platforms and none is cheap (FUSE; a macFUSE kernel extension or FSKit on macOS 15+; ProjFS,
  which is not enabled by default, or a third-party driver on Windows), and v1's "look at an old version outside the
  Working Tree" need is served by export instead. Kept with its platform constraints in
  [18 — Mounting a Snapshot as a filesystem](issues/18-mount-snapshots.md).
- A TUI client. Post-release by user decision: v1's answer for a machine without a browser is the CLI, and a second
  interactive surface has to justify itself against the web UI rather than duplicate it. Kept with its reasoning in
  [22 — A TUI client](issues/22-tui-client.md).
