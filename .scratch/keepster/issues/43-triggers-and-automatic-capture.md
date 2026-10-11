# 43: Triggers — the policy kernel, the watcher, debounce and the rate limit

**Blocked by:** 42 (Configuration — two levels, provenance, automatic reload, `Degraded`)

**Status:** done

**What to build:** a user edits a file, stops typing, and a Snapshot appears by itself — without a noisy build
turning into a Snapshot per second, and without a reboot costing them their most recent edits.

Policy is a pure function over values, `decide(state, event, now) -> (Vec<Action>, PolicyState)`, with the state
owned by the daemon (debounce window, rate-limit history, accumulated paths, last Snapshot time) and actions limited
to capturing, arming a timer and reconciling. This shape is required because filesystem events are lost, duplicated
and reordered, so the load-bearing test is "given this sequence of events at these times, exactly these Snapshots
happen" — a fold over `(event, time)` pairs with no mocks and no waiting. Shutdown is an event through the same
function, not a second code path.

- [x] Editing a file yields a Snapshot after the configured debounce window, and repeated edits inside the window
      yield one
- [x] The rate limit caps how often Snapshots are taken, with its value configurable and always in force
- [x] The periodic floor Trigger is available and off by default; which Triggers are active is the user's choice
- [x] An editor's atomic save is captured as an edit, not as a deletion followed by a creation
- [x] The watcher applies the rules before installing watches, so an excluded directory is never watched
- [x] Shutdown stops accepting Triggers, finishes the capture in flight through its ref update, then takes a Snapshot
      for every Root whose debounce window is still counting down, bypassing the rate limit
- [x] Being killed past the stop timeout is survivable: at worst an unreferenced object and a lagging index
- [x] A capture that arrives while the store's lock is held **coalesces into one Snapshot** once the lock is
      released, rather than blocking or being dropped — transferred from ticket 38, which built the lock and left
      coalescing to the policy that owns accumulated paths and the rate limit
- [x] The trigger machine is tested as values over scripted event-and-time sequences; watcher tests assert only that
      an event eventually arrives, never that one did not

## Comments

- **Decisions taken with the user before implementation.**
  - *Keys.* A `[triggers]` table, settable at every level including `.keepster.toml`, all live: `active` — the
    set of active Triggers, `watcher` and `periodic`, replaced wholesale by a more specific level, default
    `["watcher"]`; `debounce` (default `3s`); `rate_limit` (default `1m`, refused at zero, since it is always
    in force); `periodic` (default `1h`, used only while `periodic` is active). Durations take a unit (`ms`,
    `s`, `m`, `h`, `d`) and are served as text in the largest whole unit. The trailer and wire names are
    `watcher` and `periodic`; a stopping daemon's flush is recorded as `watcher`.
  - *An automatic capture that changes nothing writes no Snapshot* (`capture_if_changed`): a touched file or a
    quiet periodic floor leaves no trace, and does not count against the rate limit. A manual Snapshot is always
    written.
  - *State.* While Reconciliation does not exist, a Root captured automatically is `Degraded` with
    `state_code: coverage_not_guaranteed` — the watcher is a latency optimisation, not the guarantee. Ticket 44
    makes it `Watching`. A Root with every Trigger off stays `capture_unavailable`.
  - *Seams.* The policy as values in core, the watcher in `keepster-watch`, and the real daemon.
- **The policy** is `keepster_core::decide(state, event, now)` over a monotonic `Duration` since the daemon
  started. Events: `Changed`, `Timer`, `CaptureFinished`, `CaptureUnchanged`, `CaptureFailed`, `LockBusy`,
  `LockReleased`, `Settings`, `Shutdown`; actions: `Capture` and `ArmTimer` — Reconciling is ticket 44's.
  Two rules beyond the ticket's text, both found in review: the debounce window closes at the latest once the
  oldest pending change has waited as long as the rate limit (`max(debounce, rate_limit)`), so a file written
  more often than every 3 s — a log, a watch-mode build — is still captured once a minute rather than never;
  and a periodic capture that could not happen is owed again *as periodic*, so a switched-off watcher Trigger
  never fires in its place. A flush that fails while stopping is not retried, so a stop always finishes.
- **Coalescing**: a capture finding its store's lock held reports `LockBusy`; the paths stay with the policy and
  the runtime polls the lock every 500 ms without pinning a thread, then one capture covers everything pending.
- **Shutdown**: the server stops accepting requests, the reload tick stops, every runtime gets `Shutdown`; each
  finishes its capture in flight and captures its pending changes at once. The drain is bounded at 8 s, inside
  the service manager's ~10 s stop timeout; past it the runtimes are abandoned.
- **Killed past the stop timeout** relies on what tickets 38 and 39 already prove: git is written before the
  index, an interrupted capture leaves at worst an unreferenced object, and the watermark repairs a lagging
  index at the next start (the store's recovery tests). This ticket adds no state that outlives the process.
- **The watcher** (`keepster-watch`, `notify` 8.2): on Linux one non-recursive inotify watch per directory, armed
  by a walk that applies the rules — the user's, the Root's own `.git`, and the ignore files when they are
  honoured — so an excluded directory is never watched; a directory that arrives is armed and walked, and one
  removed is forgotten so it is watched again if it comes back. macOS (FSEvents) and Windows
  (`ReadDirectoryChangesW`) watch the whole tree with one handle, so there the same rules filter the events
  instead. A platform overflow or a watch error is `WatchEvent::Rescan`, a change somewhere until ticket 44
  answers it with a Reconciliation. `notify` brings `CC0-1.0` and `inotify` `ISC` into `deny.toml`.
- **Capture still walks the whole Root**: the accumulated paths say what prompted a capture, not its scope.
- `pending_paths` and `oldest_pending_seconds` on a Root — ticket 57's status criterion — landed here, because
  the shutdown flush is untestable without seeing that a change is pending.
- **Locks**, in the one order every holder of several keeps: a store's lock, the Registry, the configuration, the
  runtimes. A capture re-checks the Registry once it holds the store's lock, and an erasing deregistration takes
  that lock first, so an erased store is never recreated as an orphan.
