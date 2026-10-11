# 80: The optional per-Root disk cap

**Blocked by:** 52 (The external `git` prerequisite and the Maintenance Pass)

**Status:** ready-for-agent

**What to build:** a user with an optional disk cap sees it respected — by dropping the oldest history first, never
by thinning within a Tier, and never by refusing to capture while maintenance can still free space.

Split out of ticket 52, which reclaims what Retention condemned. Ticket 06's answer is the design: deleting a ref
frees nothing until a Maintenance Pass runs, so the cap can never be a check-then-prune loop; it is opt-in and per
Root, because a global cap lets one busy Root evict another's history.

- [ ] The optional per-Root cap applies the ladder, then maintenance, then measures, then drops whole Segments
      oldest-first, then maintains again; it floors at the newest Tier, overshoots and reports rather than blocking
      capture
- [ ] A cap exceeded runs a Maintenance Pass at once rather than waiting for the daily one, still in an idle window
      and never overlapping a capture
- [ ] With maintenance unavailable, capture pauses at the cap instead of filling the disk, and `status` says why
- [ ] `status` reports each Root's store size and whether it is over its cap, and `store.over_cap` is announced
