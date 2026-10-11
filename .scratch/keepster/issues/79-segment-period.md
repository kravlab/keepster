# 79: The Segment Period — configurable, fixed per Segment

**Blocked by:** 51 (Retention — the ladder and thinning)

**Status:** ready-for-agent

**What to build:** a user can choose how much history one Segment holds, and a store holding Segments of several
periods is a supported steady state, never a migration to finish.

Split out of ticket 51, whose planner already reads each Segment's span from its name and so handles mixed periods.
Ticket 06's answer is the design: the period governs Segment *creation* and nothing else, hourly is the default
because it equals the finest Tier of the default ladder, and Tier ages should be whole multiples of it or Retention
lags by up to one period.

- [ ] The Segment Period is a key of the user's files, a per-Segment property fixed at creation; reconfiguring it
      affects only new Segments, and existing ones keep the span they were created with
- [ ] A store holding several periods is captured into, listed, thinned and read like any other — sibling refs such
      as `…/2026-09-09T14` and `…/2026-09-09` both sort chronologically, and none nests in another
- [ ] `status` reports the set of Segment Periods as a per-Segment fact and never as one store-wide period
- [ ] A Tier age that is not a whole multiple of the period is warned about, since Retention then lags by up to one
      period
