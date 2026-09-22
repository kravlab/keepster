# 51: Retention — the ladder, the Segment Period and thinning

**Blocked by:** 42 (Configuration — two levels, provenance, automatic reload, `Degraded`), 38 (The first Snapshot —
History Store invariants and the write path)

**Status:** ready-for-agent

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

- [ ] The ladder is planned as a pure function over Segments, a `now` and the Tier list, tested as values
- [ ] A Segment straddling a Tier boundary is left untouched until it has fully crossed, and a second pass over the
      same store changes nothing
- [ ] Thinning within a period keeps the last Snapshot and re-chains the Segment, preserving every `SnapshotId`
- [ ] A Tier coarser than the Segment Period prunes by ref deletion alone
- [ ] The Segment Period is a per-Segment property fixed at creation; reconfiguring it affects only new Segments, and
      a store holding several periods is a supported steady state
- [ ] `status` reports the set of Segment Periods as a per-Segment fact and never as one store-wide period
- [ ] Retention is checked once per Segment rollover, waits for an idle window with a hard ceiling, and never
      overlaps a capture
- [ ] A month of history is built through the real capture path at injected times, and the pass's effect on it is
      asserted against the real store
