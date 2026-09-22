# 43: Triggers — the policy kernel, the watcher, debounce and the rate limit

**Blocked by:** 42 (Configuration — two levels, provenance, automatic reload, `Degraded`)

**Status:** ready-for-agent

**What to build:** a user edits a file, stops typing, and a Snapshot appears by itself — without a noisy build
turning into a Snapshot per second, and without a reboot costing them their most recent edits.

Policy is a pure function over values, `decide(state, event, now) -> (Vec<Action>, PolicyState)`, with the state
owned by the daemon (debounce window, rate-limit history, accumulated paths, last Snapshot time) and actions limited
to capturing, arming a timer and reconciling. This shape is required because filesystem events are lost, duplicated
and reordered, so the load-bearing test is "given this sequence of events at these times, exactly these Snapshots
happen" — a fold over `(event, time)` pairs with no mocks and no waiting. Shutdown is an event through the same
function, not a second code path.

- [ ] Editing a file yields a Snapshot after the configured debounce window, and repeated edits inside the window
      yield one
- [ ] The rate limit caps how often Snapshots are taken, with its value configurable and always in force
- [ ] The periodic floor Trigger is available and off by default; which Triggers are active is the user's choice
- [ ] An editor's atomic save is captured as an edit, not as a deletion followed by a creation
- [ ] The watcher applies the rules before installing watches, so an excluded directory is never watched
- [ ] Shutdown stops accepting Triggers, finishes the capture in flight through its ref update, then takes a Snapshot
      for every Root whose debounce window is still counting down, bypassing the rate limit
- [ ] Being killed past the stop timeout is survivable: at worst an unreferenced object and a lagging index
- [ ] The trigger machine is tested as values over scripted event-and-time sequences; watcher tests assert only that
      an event eventually arrives, never that one did not
