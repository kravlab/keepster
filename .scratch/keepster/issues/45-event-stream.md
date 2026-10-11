# 45: The live event stream

**Blocked by:** 43 (Triggers — the policy kernel, the watcher, debounce and the rate limit)

**Status:** done

**What to build:** an open client — the web UI later, `keepster log --follow` now — sees Snapshots and state changes
as they happen, and is told honestly when it missed some rather than shown a stale screen.

Server-sent events over the same HTTP surface, so cookies, the host and origin checks and the error model apply
unchanged and the stream works identically over the socket. Event ids are ULIDs; a reconnecting client sends the last
id it saw and the daemon replays from a bounded ring buffer. When the requested id has been evicted the daemon
neither stays silent nor pretends: it emits a resynchronise event and the client refetches.

- [x] The stream delivers the event set for the behaviour that exists, with thin payloads — ids and flags, never
      content
- [x] Reconnecting with the last seen id replays the events that followed it
- [x] Reconnecting with an evicted id yields a resynchronise event instead of silence or a gap
- [x] The ring buffer is bounded and documented, and the endpoint is described in the OpenAPI document
- [x] `keepster log --follow` consumes the very same endpoint over the socket
- [x] Server-side replay is tested through the harness; nothing about client-side reconnection is asserted here

## Comments

- **Decisions taken with the user before implementation.** `keepster log --follow` prints the tail oldest
  first — 10 unless `-n` says otherwise, as `tail -f` does — then each new Snapshot below it; `--json` with
  `--follow` is one object per line. When the daemon goes away, follow waits for it and reconnects with the
  last id it saw; a `resync` or a reconnect makes it refetch. Test seams: `/v1/events` through the harness,
  the in-memory replay buffer, `keepster log --follow` as a real process, and the OpenAPI document.
- **The event set** is the one ticket 07 lists, as far as the behaviour exists: `snapshot.created` (Root,
  Snapshot, incomplete, changed-path count), `root.registered`, `root.unregistered` (with `purged`),
  `root.moved`, `root.state_changed` (state and code), `config.reloaded`, `config.invalid`, and `resync`.
  Retention, maintenance, `store.over_cap`, restore and job events arrive with their tickets (51, 52, 47,
  54). Payloads are `keepster_api::Event`, a serde enum tagged by `event`; the SSE `event:` field repeats it.
- **The replay buffer** keeps the last 1000 events, none older than an hour (`events::REPLAY_BOUND`), and is
  a leaf in the lock order. Replay goes by the order kept, not by comparing ids: two events minted in one
  millisecond have none. A `Last-Event-ID` that is not kept — evicted, from before a restart, not text, or
  not ours — opens the stream with `resync`, which carries the newest id still kept so the next reconnect
  resumes from there. A stream that falls behind the broadcast channel (as deep as the buffer) gets
  `resync` too. An empty header is a client that saw nothing: no replay, no `resync`.
- **State and configuration changes are found by comparing**, because a Root's state is computed when asked:
  every reload pass — once a second, and at once after a Registry change — compares each Root's state and
  configuration (values only, so a comment moving a line is not a reload) with what was announced last. So
  `root.state_changed` and `config.*` follow a change within about a second, and a Root's first state is not
  announced; `root.registered` stands for it. `config.invalid` is a new invalid file; `config.reloaded` is
  changed values, or the last invalid file fixed.
- **Shutdown.** An open stream would hold axum's graceful stop forever, so the stop signal closes the bus
  first and every stream ends.
- **OpenAPI.** `Event` is in the components as a `oneOf` whose shapes are told apart by their `event` enum.
  There is no `discriminator` object: utoipa emits one only for variants that are schemas of their own. The
  stream's media type deliberately carries no schema — 3.1 can only describe a body as a whole, and binding
  `Event` with 3.2's `itemSchema` later must stay additive (ticket 14). The bound and the `resync` semantics
  are prose in the endpoint's description.
- **Follow** fetches what is new through the timeline rather than trusting the event's payload, so a path
  filter is applied by the daemon and a `resync` or reconnect takes the same path. "New" is by Snapshot
  identity, the timeline's own order; a clock stepped backwards gives a Snapshot an older identity, which
  the index rebuilds around but follow does not print. It ends when its Root is deregistered (exit 0 on the
  event, `not found` when it learns of it on a refetch), and stops retrying when the daemon refuses the
  stream. Reconnection is not asserted by a test, as the ticket asks; it was checked by hand across a
  restart.
- **Not covered by a test through the harness**: true eviction (a thousand events or an hour) — the unit
  tests of the buffer cover both bounds, and the harness covers an id from before a restart; a lagging
  stream; a non-text `Last-Event-ID`, which the client cannot send.
- Found on the way: the glossary avoids "resync" for Reconciliation; the stream's `resync` is a different
  thing and the entry now says so.
