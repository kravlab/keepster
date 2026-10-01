# 08 — Workspace structure and module boundaries

Type: grilling
Status: resolved
Blocked by: 01, 02

## Question

What crates exist, and what is each forbidden from knowing?

Settle:

- the split: domain core, git-backed store implementation, watcher, snapshot policy (triggers, debounce, rate
  limit), daemon, CLI, embedded UI assets;
- `SnapshotStore` as the single owner of git plumbing — its interface, and the guarantee that no other crate names
  a git concept. This is what keeps the storage model swappable;
- where policy lives: the core decides *when* a snapshot is warranted, the daemon supplies events and clock;
- what the core must never depend on: HTTP, the filesystem watcher, any DI framework, any UI;
- how the pieces are wired at startup, given the core exposes factory functions only;
- testability: the fakes required (in-memory store, virtual clock, temp-dir fixtures) and which behaviours are
  tested against a real git store instead.

Consult the `codebase-design` skill for this one.

## Answer

Eight crates, and the enforcement is Cargo's, not a convention. In Rust a crate is the only mechanism that can
actually forbid knowing something: a module can always write `use crate::store::…`, so a module split would leave
every rule in this ticket as a comment. The lines below are therefore crate boundaries.

```
keepster-core     std              domain types, policy, retention, include/exclude rules
                                   (also `ulid`, `jiff` and `regex`: value types and pure
                                   predicates, none of which is I/O or a port)
keepster-api      serde            /v1 DTOs, RFC 9457 Problem, error-code table
keepster-store    core             git2 + rusqlite; all git plumbing and the derived index
keepster-watch    core             notify -> core::Event
keepster-app      core + store     use-cases, synchronous
keepster-service  —                systemd unit / LaunchAgent / Windows per-user service
keepster-ui       —                embeds a pre-built ui/dist; MIME, caching, SPA fallback
keepster-daemon   everything       tokio + axum, wiring, two listeners, jobs, SSE
keepster-cli      api + service    clap + HTTP client
```

Forbidden, and checked by the dependency graph rather than by review:

- **`core`** knows no git, no SQL, no HTTP, no tokio, no `notify`, no filesystem. It may name `Path`; it may not
  read one.
- **`store`** knows no HTTP, no tokio, no `notify`.
- **`api`** knows not even the domain — see the additive-only argument below.
- **`cli`** knows no git, no SQL, no `core` and no `store`. The CLI binary does not link `git2` or `rusqlite` at
  all, which turns "UI must not own persistence contracts" from a discipline into a build fact.

### The core owns no ports

`keepster-core` is a **pure decision kernel**: domain types and functions over them, no traits, no I/O.
`retention::plan(segments, now, ladder) -> RetentionPlan`, `rules::verdict(path, rules) -> Verdict`, and the
trigger machine below. Above it, `keepster-app` holds the use-cases (`CaptureSnapshot`, `RestoreFile`,
`RunRetention`, `Forget`, `Reconcile`), each holding the **concrete** `SnapshotStore`. Callers go
UI → use-case → core; nothing above `store` names a git concept.

There is deliberately no `trait SnapshotStore`. The reasoning is recorded as
[ADR 0003](../../../docs/adr/0003-no-ports-in-core.md), because a reader who expects hexagonal architecture will
otherwise read its absence as an oversight.

One correction to this ticket's own framing: it asked how the pieces are wired "given the core exposes factory
functions only". After this decision the core exposes nothing constructible at all, so wiring is a sequence of
constructor calls and the question collapses into failure isolation, answered below.

### "Swappable" means locally replaceable, not trait-substitutable

The ticket motivated the `SnapshotStore` seam as "what keeps the storage model swappable". With no trait, that
guarantee is weaker and must be stated honestly: **no crate outside `store` names a git concept**, so replacing
the storage engine is a change confined to one crate — not a change absorbed by an interface. Ticket 01 rejected
`gitoxide` "for now", but a trait shaped around git2's index/tree/ref model would not have survived that swap
anyway; the seam would have been fiction.

### `SnapshotStore` owns git *and* the derived index

One type, domain vocabulary only: `SnapshotId`, `RootId`, `SegmentId`, paths, byte streams. `git2::Oid`, `Commit`,
`Tree` and `Reference` appear in no public signature, and `StoreError` is a domain enum
(`SnapshotNotFound`, `SegmentNotFound`, `StoreLocked`, `MaintenanceUnavailable`, …) carrying `git2::Error` only as
an opaque `#[source]` that no caller matches on.

The decisive reason it also owns the SQLite index is ticket 03: that store is safe because **the git commit and
ref update always precede the index write**, and a per-Segment watermark turns a killed daemon into a
millisecond catch-up. Had `app` sequenced the two writes, that ordering would live in policy code where a future
use-case can get it wrong, silently. Inside one type it cannot be violated from outside. The same argument
protects ADR 0001's invariants — leaf-only ref names, parentless Segment starts, the `SnapshotId` trailer,
normalised modes — which a thin `write_tree`/`commit`/`update_ref` wrapper would have exported to its callers
while leaving `store` a shallow module.

### Policy is a state machine over values

```rust
pub fn decide(state: PolicyState, event: Event, now: Timestamp) -> (Vec<Action>, PolicyState)
```

`PolicyState` (debounce window, rate-limit history, accumulated paths, last Snapshot time) is plain data owned by
the daemon; `Action` is `Capture { paths, bypass_rate_limit }`, `ArmTimer`, `Reconcile`. Two closed tickets force
this shape. Ticket 02 established that filesystem events are lost, duplicated and reordered, so the load-bearing
test is "given this sequence of events at these times, exactly these Snapshots happen" — which here is a `fold`
over `(event, time)` pairs, with no mocks, no async and no wall-clock waiting. Ticket 05 requires shutdown to
flush pending debounce windows while bypassing the rate limit; that is an `Event::Shutdown` through the same
`decide`, not a second execution path in the stop handler.

Accepted cost: the daemon must store the returned state back, and forgetting to is a silent bug. An engine with
`&mut self` would not have that trap, but would lose replayability of an event sequence as a value.

### No test doubles exist

Time is a parameter, threaded from the daemon into every use-case exactly as it is threaded into `decide`. There
is no `Clock` trait and no `FileSystem` trait; the only test double in the workspace is a real git store in a
temp dir. After the decisions above nothing waits or schedules inside a use-case — scheduling left as
`Action::ArmTimer` — so a parameter covers every use of time that remains.

Tested against a **real** git store, because a fake cannot prove any of it:

- `git gc --prune=now` actually reclaims space after a Segment ref is deleted (ticket 06);
- deleting a ref frees nothing while reflogs are on, and `core.filemode` is `false` at store creation (ADR 0001);
- rebuilding the index from git objects reproduces the same rows (ticket 03);
- a pinned Snapshot survives `gc` (ADR 0002);
- `MaintenanceUnavailable` is raised when the configured `git` path is missing or not executable (ticket 11).

Tested as **values**, with no disk: the Retention ladder, the layered include/exclude matcher with negation, the
trigger machine.

Accepted cost: `keepster-app` tests need `git` on the machine and write to temp dirs, so they are slower than the
core's and will not run in an environment without git.

### `RootRuntime` per Watched Root

The daemon holds `HashMap<RootId, RootRuntime>`, each owning its `SnapshotStore`, its `PolicyState` and its watch
registration. **Failing to open a Root is a value, not a startup error**: `Degraded` (config failed to load, the
previous one is in force — ticket 04) and `Unavailable` (root missing, unmounted or unreadable — ticket 05) are
variants in the map. This is the only shape under which ticket 05's requirement holds that `keepster status` is
the single authority on health and reports per-Root state: an all-or-nothing start has nothing to report exactly
when the report matters most. Lazy construction was rejected for the same reason from the other side — it defers
ticket 02's mandatory startup Reconciliation until someone opens the UI, so on a machine nobody looks at, the
coverage guarantee quietly does not apply. As ticket 05 already specified, an `Unavailable` Root is retried with
backoff and re-attaches on mount.

### UI assets: a crate, and `cargo build` never needs node

`keepster-ui` embeds a pre-built `ui/dist` behind a default-on `embed-ui` feature; building the SPA is a separate
pipeline step (`make ui`), never a `build.rs`. Ticket 07's "one artifact, no runtime dependencies" is a claim
about the shipped binary, not about the build host; conflating the two would make `cargo build` fail without node,
break `cargo install`, and force node into every CI matrix — including the one that only runs core tests, in a
workspace whose CLI has nothing to do with the UI. Committing `dist/` was rejected: minified bundles in every diff
and blame, with no gain in reproducibility, since nothing checks that the committed output matches the sources.

### Settled in passing

CLI selectors (`latest`, `~N`, `2h`) from ticket 07 stay in the CLI and need no `core` dependency: `2h` is
arithmetic on time and `~N` is an index into a timeline the CLI has already fetched, both expressed over
`keepster-api` types. This is recorded so that the apparent tension with "the CLI may not know the domain" is not
later "fixed" by giving the CLI a dependency on `core`.

How the `/v1` contract is published — generated from `keepster-api`'s types or hand-written, and what stops it
drifting — is now sharp enough to be its own question:
[14 — Publishing the `/v1` contract](14-openapi-contract.md).
