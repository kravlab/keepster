# 51: Retention — the ladder and thinning

**Blocked by:** 42 (Configuration — two levels, provenance, automatic reload, `Degraded`), 38 (The first Snapshot —
History Store invariants and the write path)

**Status:** done

**What to build:** history is bounded by a ladder the user can state and change, the last 24 hours are never thinned,
and a Retention pass is cheap and idempotent.

The ladder is an ordered list of Tiers expressed in durations, so it stays correct at any Segment Period, and the
shipped default is everything for 24 hours, hourly to 7 days, daily to 30 days, then discarded. The survivor of a
thinned period is its **last** Snapshot. **Retention only ever acts on a Segment lying wholly past a Tier
boundary**, which is what makes thinning happen at most once per Segment and the pass idempotent.

```toml
[[retention.tier]]
age  = "24h"
keep = "all"

[[retention.tier]]
age  = "7d"
keep = "hourly"

[[retention.tier]]
age  = "30d"
keep = "daily"
```

- [x] The ladder is planned as a pure function over Segments, a `now` and the Tier list, tested as values
- [x] A Segment straddling a Tier boundary is left untouched until it has fully crossed, and a second pass over the
      same store changes nothing
- [x] Thinning within a period keeps the last Snapshot and re-chains the Segment, preserving every `SnapshotId`
- [x] A Tier coarser than the Segment Period prunes by ref deletion alone
- [ ] ~~The Segment Period is a per-Segment property fixed at creation; reconfiguring it affects only new Segments, and
      a store holding several periods is a supported steady state~~ — moved to ticket 79
- [ ] ~~`status` reports the set of Segment Periods as a per-Segment fact and never as one store-wide period~~ —
      moved to ticket 79
- [x] Retention is checked once per Segment rollover, waits for an idle window with a hard ceiling, and never
      overlaps a capture
- [x] A month of history is built through the real capture path at injected times, and the pass's effect on it is
      asserted against the real store

## Comments

- **From ticket 39's review: the index rebuild is all-or-nothing, and this ticket makes it routine.** The derived
  index computes each Snapshot's changes against its predecessor in `SnapshotId` order, across Segments, so a
  watermark that is no longer an ancestor of its Segment's tip — which every thinning produces — rebuilds the
  whole index under the write lock (`Index::catch_up` in `keepster-store`). The same happens on every catch-up
  after the clock steps backwards, until wall time passes the old maximum, because each new Snapshot sorts below
  the newest indexed one. Both are correct but cost a full re-diff of the history while readers wait up to the
  5 s busy timeout. Narrow it here: rebuild only the thinned Segment plus the first Snapshot after it, and insert
  an out-of-order Snapshot by recomputing just its successor's rows.
- 2026-10-11: split with the user before building: the configurable Segment Period and `status`'s per-Segment
  periods became ticket 79. The planner already reads each Segment's span from its name — an hour, or the day
  ticket 79 will write — so mixed periods need nothing more of it.
- 2026-10-11: done. `[[retention.tier]]` is settable from the user's files only — global, `[root.<id>]` and
  `[[roots]]` — never a Root's own `.keepster.toml`, which is ignored with `key_ignored`: how much history is kept is
  ticket 04's class of keys deciding what becomes of the user's data, and a file in someone else's repository must
  not shorten it to an hour. `keep` is `all`, `hourly`/`daily`/`weekly` (one hour, one day, seven days) or any
  period, counted from the epoch in UTC. A ladder is refused unless its first Tier keeps `all`, its ages increase,
  and each Tier's period holds whole periods of the one before — otherwise a Snapshot one Tier kept as the last of
  its period could be thinned away by the next, and that period lose every Snapshot.
- 2026-10-11: a decision of its own, with the user: Retention never removes the Root's newest Snapshot, even past
  the last Tier — it is the record of the present state and the next capture's baseline. A period's survivor is
  its last Snapshot across every Segment, so a Tier coarser than the Segments deletes the others by ref alone
  when a Tier as fine as the Segments came before it; a Segment reaching the coarser Tier unthinned — a daemon
  that was down for the whole finer Tier — is re-chained there instead. A Segment whose chain holds a commit
  without our trailers is left alone, and a ref is moved only from the tip the pass read.
- 2026-10-11: the index catch-up is narrowed as asked: a Segment rewritten or gone loses its rows and is read
  again, and each Snapshot written or removed has its successor's changes counted again, which also places an
  out-of-order Snapshot without rebuilding anything else. A rebuilt index agrees with a caught-up one, tested.
- 2026-10-11: the daemon looks every minute; a pass is owed once per hourly Segment rollover — and at the start,
  scheduled rather than run — and runs once the Root's newest Snapshot is five minutes old, or six hours after it
  became owed. It takes the store's lock without waiting, so a capture in flight wins and the pass is tried at the
  next look; a stopping daemon lets a pass in flight finish for up to two seconds before its last captures. That
  exclusion is by construction — the harness cannot hold a store's lock — and is not tested on its own. A pass that
  removed something sends `retention.completed` with its counts. Reclaiming the space is ticket 52's maintenance.
