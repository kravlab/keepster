# 07 — Daemon HTTP API and CLI surface

Type: grilling
Status: resolved
Blocked by: 03, 04

## Question

The daemon is the only component that touches the store, so both the CLI and the web UI are clients of one API.
What is that contract?

Settle:

- the endpoints and their shapes: Root registry (list, watch, unwatch, move), timeline for a Root, snapshots
  touching a given path, file listing within a snapshot, file content, diff (snapshot vs previous, snapshot vs
  working tree), restore, forget;
- how a `SnapshotId` appears in URLs and CLI arguments;
- the CLI commands that mirror them, and which are CLI-only (`service install`, `status`);
- the error model shared by both surfaces;
- authentication: how the token is minted, passed and rotated, and what stops another local process from calling
  the API;
- whether the timeline needs streaming or long-polling so an open UI reflects new snapshots live.

## Context from ticket 05

[05 — Daemon lifecycle and autostart on three platforms](05-daemon-lifecycle.md) already fixed part of this
contract; do not reopen it here:

- **Transport is settled**: one HTTP API served on two listeners — a permission-protected Unix socket / named pipe
  for the CLI (no token, OS permissions are the boundary) and 127.0.0.1 plus token for the browser. The
  authentication bullet above therefore only concerns the *TCP* listener; "what stops another local process from
  calling the API" is answered for the CLI path by socket permissions.
- **`status` is not a free design**: it must expose live daemon state as the payload described in 05 — daemon
  liveness and uptime, autostart, linger, the git prerequisite row owned by ticket 11, and per Root its state
  (`Watching` / `Degraded` / `Unavailable`), last Snapshot age, Snapshot count and any "coverage not guaranteed"
  flag — with a non-zero exit whenever any Root is not `Watching`, and `--json` emitting the same shape the web
  UI renders. This ticket owns its wire format, not its content.
- **CLI-only commands** now include `service install | uninstall | status` alongside `status`.
- A Snapshot may be **marked incomplete** (see `CONTEXT.md`), so every shape that returns a Snapshot needs a place
  to say so and to list the omitted paths.

- the commands ticket 06 introduced and did not place: `pin`/`unpin`, `forget <glob>` (with its `--exclude` /
  `--keep-watching` choice and its confirmation), and a manual maintenance trigger, plus how the over-cap and
  Segment-Period facts surface in `status --json`.

## Answer

### The contract is HTTP, and it is public

`/v1` is a first-class, documented contract with a real compatibility policy: additive changes only inside `/v1`,
breaking changes only through `/v2` served alongside it for a deprecation window. The CLI is a **thin client over
the same API** and is stable too — subcommands, flags, exit codes and `--json` shapes.

Treating the API as private was rejected once third-party clients were considered. A third-party web or native UI
cannot be served by shelling out to the CLI: it needs per-interaction latency, ranged blob reads, server-computed
diffs and a live event feed. Such an author would use the HTTP surface regardless — blind, by reverse engineering,
and broken by the next release. Designing it as public costs almost nothing up front (a prefix, an error model) and
cannot be retrofitted without a breaking change. The discipline pays inwards as well: **the embedded SPA gets no
privileged endpoints**, so everything it can do, any client can do.

Both surfaces stay stable at roughly single cost, because the semantics are defined once in the API and the CLI is
one rendering of it.

### Addressing

**A Root is a ULID**, and the URL segment takes nothing else: `/v1/roots/{rootId}/…`. Path-as-key is impossible —
the map already fixed identity as a surrogate id with the path a mutable attribute — and accepting *both* forms in
one segment was rejected because it gives one resource two canonical URLs, so a bookmarked link starts depending on
which form produced it. Resolving a path is its own query: `GET /v1/roots?path=/abs/path`. ULID over UUIDv4 for
lexicographic = chronological ordering, and over an autoincrement so a store survives being moved to another
machine.

CLI ergonomics live entirely on the client: the Root defaults to the nearest registered ancestor of the cwd (a
git-style walk up; none found is an error pointing at `keepster watch .`), and `-r/--root` accepts a full id, an
unambiguous id prefix, or a path it resolves itself. Ambiguity is therefore resolved by the human in the terminal
and never leaks into a URL. Human-readable output names a Root as `<basename> (<first 8 chars of id>)`.

**A SnapshotId is a ULID** too, and again only the canonical form appears in a URL. Ordering matters here beyond
cosmetics: Snapshots live in different Segments and cross-Segment ordering is exactly the query ticket 03 put in
SQLite; a lexicographic id order that equals chronological order survives `forget`, which rewrites commits but
preserves ids, and the embedded timestamp gives a cheap Segment candidate when the index is rebuilt from git alone.

Selectors are not an API mechanism. `latest`, `~N`, ISO timestamps and `2h`/`3d` are CLI conveniences that reduce to
the timeline query that exists anyway (`?limit=1`, `?before=<ts>&limit=1`), so third-party clients get the same
power rather than a surrogate of ours. `latest` in a URL was rejected outright: it makes a saved link show a
different Snapshot tomorrow while looking like a link to a specific one.

### Paths and reading

**One form of path parameter across every endpoint**: `path=` as a query parameter, always relative to the Root,
always `/`-separated (clients normalise Windows separators), with leading `./`, `..` and absolute paths rejected as
`400 path_invalid`. Path-as-URL-tail is prettier for exactly one endpoint and breaks on `#`, `?` and `%` in
filenames, cannot express the two paths a rename diff needs, and makes `/files/src/` ambiguous between a directory
and an empty file. **How a non-UTF-8 path is represented on the wire is ticket 10's decision**; this ticket only
fixes that the representation is defined once and used by all endpoints.

- `GET /v1/roots/{r}/snapshots/{s}/entries?path=<dir>&depth=1|infinity` — `depth=1` by default, because a 50 000-file
  tree is expanded lazily in a UI anyway; `infinity` serves the CLI and scripts. Each entry: path, kind
  (file/dir/symlink), size, mode, binary flag, and — where the Snapshot is incomplete — the skip marker with its
  errno, since ticket 05 requires incompleteness to be visible wherever a Snapshot is served.
- `GET /v1/roots/{r}/snapshots/{s}/content?path=…` — **raw bytes**, with `Content-Length`, `ETag` and `Range`.
  Base64-in-JSON was rejected: +33 % volume, no streaming, no ranged reads (a UI showing the head of a 200 MB file
  must not fetch all of it), and the JSON envelope adds nothing because the metadata already lives in `entries`.
- `GET /v1/roots/{r}/diff?from=<SnapshotId>&to=<SnapshotId|working>&path=<optional>&format=json|unified` — **the
  daemon computes the diff**. It holds both blobs locally and ticket 01 already made `git2` tree-to-tree diff the hot
  path, so the implementation exists once instead of once per client; shipping two 5 MB blobs to produce a
  three-line diff is the wrong direction, and binary detection, rename detection and the "too large to diff"
  threshold are decisions clients must not each make differently. `to=working` covers Snapshot-vs-Working-Tree, and
  "vs previous" stops being a mode because the client already obtains the previous id from the timeline. Without
  `path`: the changed-path summary; with it: hunks. `format=unified` exists so the CLI does not reassemble unified
  text out of JSON.

### Timeline

`GET /v1/roots/{r}/snapshots?path=&before=&after=&limit=&cursor=` — **one endpoint**. "Which Snapshots changed this
path" is the same list filtered through the path-change index ticket 03 already specified; a separate URL would
return the same shape and force clients to render one table with two code paths.

Pagination is **cursor-based**, the cursor being the last returned `SnapshotId`. The list grows at one end and
erodes at the other (Retention, plus the decay of orphaned stores below), so an offset shifts under the reader in
both directions and paging backwards yields gaps and repeats. ULID ordering makes "after this cursor" a plain value
comparison with no server-side state, and a cursor pointing at an already-pruned Snapshot is not an error — the
comparison still yields the right boundary. Item: `SnapshotId`, timestamp, the Trigger that fired, incomplete flag,
changed-path count, and — when filtering by `path` — the kind of change to that path. `limit` defaults to 100, caps
at 1000; the response carries `next_cursor`. No global cross-Root feed in v1: a UI picks a Root first, and "what is
happening system-wide" is the event stream.

### Registry mutations

- **No overlap.** `POST /v1/roots` with a path inside an existing Root, or containing one, returns `409
  root_overlaps` naming the offending Root; `PATCH` enforces the same rule, or the ban is bypassed in two steps.
  Nesting would break the cwd walk-up the whole CLI ergonomics rest on (a file in two Roots makes `keepster log
  file.rs` ask which one, and the same question repeats in restore, diff and forget) and would capture every change
  twice into two stores. Auto-excluding a nested Root from its parent was rejected too: it silently edits the
  parent's Include Rules, which ticket 04 deliberately made explicit and layered, and leaves a hole in the history
  exactly where the user asked for *more* attention. Symlinked overlap is ticket 10's problem; this rule is stated
  over real paths.
- **`DELETE /v1/roots/{id}` deregisters and keeps the History Store**; `?purge=true` (CLI `unwatch --purge`, with
  explicit confirmation) is the only way to erase it. Unwatching is often temporary, and the history is the
  insurance the whole tool exists to provide.
- **At unwatch, the latest Snapshot is pinned** (ADR 0002: re-created as a parentless commit), and **Retention keeps
  running on the orphaned store**. This is what makes "the history survives" true without holding disk forever: the
  store decays down the normal ladder until only the pinned last state remains — one commit, its tree and blobs — so
  space is reclaimed by the mechanism that already exists, with no freeze/thaw branch anywhere. Freezing the store
  instead was rejected as holding space the ladder had already condemned; letting Retention run *without* the pin was
  rejected because at the default ladder it empties the store within 30 days, turning `unwatch` into a silent,
  unconfirmed `purge`. The pin is an ordinary Pin: listed, `unpin`-able (after which the store does decay to
  nothing, by explicit user action), and it carries the incomplete marking if the last Snapshot had one. A Root that
  never captured anything has nothing to pin. User pins are untouched, the per-Root disk cap still applies with the
  pinned Snapshot as its floor, and re-watching needs no warning because decay was continuous rather than deferred.
- **Moving is explicit only**: `PATCH /v1/roots/{id}` with the new path (CLI `keepster move -r <id> <newpath>`).
  To the daemon a move is indistinguishable from a deletion — ticket 02 lists directory moves among the changes no
  platform reliably announces — so any auto-detection is a basename or content heuristic that will one day graft a
  stranger's history onto yours. A marker file inside the tree was rejected as well: the store is global by default,
  the marker would land in Snapshots, in the user's `git status` and in every clone, where it would present an
  identical id on another machine. Instead `status` shows the `Unavailable` Root with its last known path and prints
  the exact `move` command. A move costs the history nothing: no `SnapshotId` changes.

### Restore

`POST /v1/roots/{r}/restore`, body `{ from: <SnapshotId>, paths: [...] }` — a directory means everything beneath it,
omitting `paths` means the whole Snapshot. **Restore only overwrites and creates; it never deletes.**

An "make the Working Tree identical to the Snapshot" mode was rejected on a decisive point: a Snapshot contains only
what passed the Include Rules, so `node_modules`, build output and — with the default `respect_gitignore = false` —
every path no rule covers are absent from it. "Make identical" deletes all of that, and the Safety Snapshot cannot
undo it, because the Safety Snapshot is taken through the same rules and therefore does not contain it either. An
operation whose worst case is irreversible precisely in the region the tool cannot see does not belong in v1; the
motivating scenario ("an agent deleted half the Working Tree") needs deleted files back, not extra files removed.

Restoring one path per call was rejected too: it means N Safety Snapshots and no single undo, which is the point of
the Safety Snapshot.

The response carries the Safety Snapshot's `SnapshotId` plus a per-path outcome (`restored` / `created` /
`unchanged` / `failed` with its error). Per-path rather than all-or-nothing, because atomicity here is per file
(temp + rename) and no cross-file transaction exists; promising one would be a lie, so the client is told exactly
what happened and holds the id that rolls it back.

### Error model

**RFC 9457 Problem Details** (`application/problem+json`) plus a `code` extension. A standard shape is what a
third-party client's library already parses, and a bespoke envelope would differ from it only by being incompatible
with everything.

**`code` is the stable part, not `title`/`detail`** — prose is edited and translated freely; `code` is contract,
and it is what clients branch on. The set: `root_not_found`, `snapshot_not_found`, `path_not_found`, `path_invalid`,
`root_overlaps`, `root_unavailable`, `root_degraded`, `store_locked`, `config_invalid`, `git_missing` (owned by
ticket 11), `too_large`, `unsupported_entry`, `confirmation_required`, `unauthorized`, `daemon_unreachable`,
`version_mismatch`. HTTP status stays the coarse layer (`400/401/404/409/413/423/500`), `code` the precise one, and
`details` carries machine-readable fields (which path, which id, which limit) so the CLI composes its message
instead of parsing prose. `daemon_unreachable` and `version_mismatch` are raised by the client — no daemon exists to
send them — but belong to the same set so the CLI has one table.

CLI exit codes are a table, not `1` for everything, or a script cannot tell "no such Snapshot" from "the daemon is
dead": `0` success, `2` usage error, `3` daemon unreachable, `4` not found, `5` state conflict (`store_locked`,
`root_unavailable`), `6` partial success (some restore paths failed), `7` incompatible version, `1` everything else.
`status` is not a call error and takes its own code `10` for "some Root is not `Watching`" (ticket 05).

### Authentication

The socket needs no token: OS permissions are the boundary (ticket 05), and a process running as the same user can
reach it anyway. The real threat on the TCP listener is **the browser** — any page the user has open can post to
`127.0.0.1`, and via DNS rebinding can claim to do so from its own origin.

- **A token per daemon start**, in a `0600` file in the runtime directory.
- **The browser never receives it in a URL.** `keepster ui` opens `http://127.0.0.1:<port>/?bootstrap=<code>`; the
  daemon exchanges the single-use code for an `HttpOnly`, `SameSite=Strict` cookie, burns the code and `303`s to a
  clean URL. A token in the address bar is a token in browser history, in `Referer` on any outbound link, and in
  `window.opener`. A long-lived token in config was rejected because it lands in dotfiles and in `.keepster.toml`
  — which ticket 04 left unrestricted and which lives *inside* the watched tree, so it would be captured into
  Snapshots and shipped to every clone — and because nothing revokes it but a hand edit. Short-lived TTL tokens were
  rejected as machinery for a single local session; a daemon restart already rotates, and `keepster ui token
  rotate` gives explicit revocation.
- **Cookies do not stop rebinding**, so every request is checked twice: `Host` must be `127.0.0.1:<port>` or
  `localhost:<port>` and nothing else, and `Origin`, when present, must be in the allowed list. The default list is
  our own origin; `ui.allowed_origins` lets the machine's owner add e.g. `http://localhost:3000` for a third-party
  UI. Never `Access-Control-Allow-Origin: *`.

### Live updates

**SSE**: `GET /v1/events`, `text/event-stream`. The flow is one-way — the daemon informs, the client acts through
ordinary requests — so WebSocket buys bidirectionality nobody needs and pays for it with a second protocol whose
authentication is separate: **CORS does not apply to WebSocket at all**, so the `Origin` check would have to be
re-implemented by hand at the handshake instead of remaining one shared layer. SSE is an ordinary HTTP request, so
cookies, `Host`/`Origin` and the error model apply unchanged, and it works identically over the socket, which is
what lets `keepster log --follow` consume the very endpoint the SPA uses. Polling was rejected as trading either
latency or constant wakeups against a daemon ticket 06 tells to wait for idle windows.

**Gaps are the substance of the design.** Event ids are ULIDs; a reconnecting client sends `Last-Event-ID` and the
daemon replays from a bounded ring buffer (order of the last thousand events, or an hour). When the requested id has
already been evicted the daemon neither stays silent nor pretends: it emits `resync`, and the client refetches the
timeline — the same honesty ticket 05 demands of `status`.

Events: `snapshot.created` (Root id, `SnapshotId`, incomplete flag, changed-path count), `root.state_changed`,
`root.registered`, `root.unregistered`, `root.moved`, `config.reloaded`, `config.invalid`, `retention.completed`,
`maintenance.completed`, `store.over_cap`, `restore.completed`, `job.started`, `job.progress`, `job.completed`.
Payloads are deliberately thin — ids and flags, never content — so a client wanting detail fetches it through the
one endpoint that already returns that shape.

### Pins, forget, maintenance: sync vs job

`pin`/`unpin` are synchronous; `forget` and a manual Maintenance Pass are **jobs**.

`forget` rewrites every Segment and ends in `gc --prune=now` under the exclusive store lock, and it is irreversible
by design (ticket 06). If a synchronous connection drops — a closed laptop, a sleeping tab, a timeout — the client
cannot learn whether the deletion happened, because an operation without identity has no state to query. That is not
an acceptable outcome for the one irreversible command in the tool. A job also lets `202` say honestly "queued
behind the running capture". Wrapping `pin` — a single object write — in a job would add two states and a poll to an
operation that is already finished.

- `PUT /v1/roots/{r}/pins/{SnapshotId}` and `DELETE` — idempotent by construction, since the ref name is
  deterministic (ADR 0002); `GET /v1/roots/{r}/pins` lists them.
- `POST /v1/roots/{r}/forget` → `202` + `job_id`. The body must carry `glob`, `confirm: true` and `on_exclude:
  "exclude" | "keep-watching"`; anything missing is `400 confirmation_required`. **The daemon does not ask, it
  refuses** — ticket 06's interactive prompt lives in the CLI alone. `--exclude` on a path in the secret opt-in list
  is refused at the door, as ticket 06 requires.
- `POST /v1/roots/{r}/maintenance` → `202` + `job_id`.
- `GET /v1/jobs/{id}` — state (`queued`/`running`/`succeeded`/`failed`), progress, result (for `forget`: Snapshots
  touched, blobs removed, and ticket 06's "the live file still matches what was removed" warning), error in the shape
  above.

The CLI waits and renders progress off the same stream by default; `--detach` prints the `job_id` and exits.

### Status

`GET /v1/status` is a global resource, and **`status --json` emits its payload verbatim** — passthrough, not a
parallel schema, which is what ticket 05 meant by "the same shape the web UI renders". Two schemas for one meaning
would both be stable under this ticket's compatibility promise, doubling the versioning cost and guaranteeing drift
at the first added field. The consequence is explicit: fields added to `GET /v1/status` change `status --json` too,
so changes here are additive like everywhere in `/v1`. Human-readable rendering is the CLI's own.

Contents: a daemon section (build version, uptime, pid, autostart, linger — reading `unavailable — coverage ends at
logout` on macOS — and the git prerequisite row owned by ticket 11) and a Root array (id, current path, state, the
machine-readable `code` behind a `Degraded`, last Snapshot id and age, Snapshot count, "coverage not guaranteed"
flag, store size, over-cap flag, and the set of Segment Periods **as a per-Segment fact**, which ticket 06 forbids
presenting as a single store-wide period), plus the orphaned-store section. A Root's own state is also part of `GET
/v1/roots/{id}` rather than duplicated into a second per-Root health endpoint. The exit code stays outside the body.

### Configuration

**Read-only, with provenance**: `GET /v1/roots/{r}/config` returns the effective merged configuration and, per key,
the level that set it (`default / global / global-pattern / global-root / root` — the middle two are the global
config's per-Root blocks, see ticket 04's Comments). Edits are made in the TOML files, which the daemon already
reloads automatically (ticket 04).

Write support was rejected for v1: the daemon would be rewriting a file the user keeps in git and edits by hand, and
would have to preserve comments, key order and formatting or make the first UI change produce a whole-file diff.
That is real work (TOML round-tripping) for a file edited rarely, and "edit in your editor, see it in the UI a
second later" already works. It can be added additively later without breaking `/v1`.

Provenance is not decoration: ticket 04 made pattern lists layered with negation ("last matching pattern wins"), and
an effective list is unreadable without knowing which level contributed each rule. For the same reason there is
`GET /v1/roots/{r}/explain?path=…` — the analogue of `git check-ignore -v` — answering whether a path is captured
and which rule at which level decided it. The rule engine exists regardless, and this removes the largest class of
"why wasn't my file saved" questions. An invalid configuration surfaces here as the known-good config plus the parse
error in the standard error shape, with the Root `Degraded`.

### Open in editor

`POST /v1/roots/{r}/open` with `{ path, line, column, from? }` — **the daemon launches the configured editor**.
Ticket 04 already decided `ui.editor` is a command template with `{file}:{line}:{column}`, which describes a process
launch, not a URL; custom URL schemes cover a handful of editors, need a registered handler, and are unavailable to
the CLI.

Conditions this endpoint cannot ship without:

- **No shell.** The template is split into argv when the config is parsed and substitution goes into separate
  arguments. `sh -c` with an interpolated path would turn a filename into executable code. This was written when
  `ui.editor` could come from a `.keepster.toml` arriving in another person's clone; that key is now global config
  only (see ticket 04's Comments), so the rule survives as defence in depth rather than as the boundary, and a
  path under the Root is still attacker-influenced text.
- The path is confined to the Root after canonicalisation; anything escaping it is `400 path_invalid`.
- The process is spawned detached, its output is not collected (an editor outlives the daemon), and the response is
  `204`, not the editor's exit status.
- Rate-limited: the endpoint spawns processes, and a cheap guard is warranted even behind the `Origin`/cookie checks.

`from=<SnapshotId>` opens a **historical** version: the daemon materialises the blob into a `0600` read-only temp
file under the runtime directory and opens that, cleaning such files at shutdown. Without it, "open in editor" can
only open the version already on disk. CLI mirror: `keepster open <path> [--from <SnapshotId>]`.

### CLI-only, and the version handshake

CLI-only: `daemon` (foreground run, invoked by the service manager), `service install | uninstall | status`, `ui`,
`ui token rotate`. The reason is necessity, not taste: `service install` must work **while the daemon is not
running**, and a daemon installing its own supervisor over its own HTTP is circular. Reading stays in the API —
`GET /v1/status` already carries the autostart and linger rows — so a UI displays the state and a human changes it.

`GET /v1/version` is served on both listeners and, on TCP, **before authentication**, or a client cannot distinguish
a bad token from a version mismatch. It returns the daemon build version, the API version (`v1`) and the build hash;
the `Host`/`Origin` checks still apply, the cookie does not.

**Refusal only on an API major mismatch** (`version_mismatch`, exit `7`). A differing build with a matching API
version is a warning, not a refusal — two installs on one machine (a system package plus `cargo install`) is
ordinary, and since `/v1` changes only additively, refusing an older client would mean not believing our own
compatibility guarantee. The embedded SPA cannot drift, being in the same binary, but performs the same check
through the same code path so the privileged-client asymmetry stays absent.

### Terminology consequence

`Project` was replaced by **`Watched Root`** (short form `Root`) while settling addressing: the entity is a
*registration* whose path is a mutable attribute, so a folder-shaped word asserts the opposite of what was decided,
and `folders/{id}/entries?path=<folder>` would use one word for two things. `Project` additionally implies code and
development for a tool that will watch `~/Documents/notes`. The rename touches `CONTEXT.md`, tickets 03–06, both
ADRs, the registry file name (`project.toml` → `root.toml`) and the `Keepster-Project` commit trailer, and is
tracked as [13 — Rename Project to Watched Root](13-rename-project-to-root.md).
