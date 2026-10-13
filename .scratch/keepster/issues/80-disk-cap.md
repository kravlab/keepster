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
- [ ] The cap stays pin-unaware (ADR 0002): it drops Segment refs and never a Pin ref, so every pinned Snapshot —
      the last state of a Root no longer watched included — is part of its floor, and a cap that cannot be met
      because of Pins overshoots and reports it (moved here from ticket 53)
- [ ] `status` reports each Root's store size and whether it is over its cap, and `store.over_cap` is announced

## Comments

- **The paused capture is an alarm (from ticket 67)**: the daemon decides every notification, in its `alarms`
  module (`crates/keepster-daemon/src/alarms.rs`), and the tray only delivers what `alarms.changed` announces.
  Ticket 67 raises no alarm for the cap, because the state does not exist yet. When this ticket introduces capture
  paused at the cap and `store.over_cap`, a paused capture must raise an alarm through that module — a new kind
  alongside `root_unavailable`, `root_degraded` and `job_failed`, cleared when capture resumes — so that the tray
  notifies it and `status` lists it, as the insurance having stopped.
