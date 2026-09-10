# 06 — Retention policy and segment granularity

Type: grilling
Status: resolved
Blocked by: 03

## Question

The storage model makes pruning cheap at segment granularity (see `docs/adr/0001-segmented-snapshot-chains.md`).
What policy actually runs?

Settle:

- segment granularity (hour, day, configurable) and its consequence: dropping a whole segment is free, thinning
  inside one rewrites that segment;
- the default retention ladder, e.g. keep everything for N hours, then one per hour for N days, then one per day —
  and whether "keep everything" is bounded by count, age, or store size;
- how a disk-usage cap interacts with the ladder when both are configured;
- when retention runs, and how it avoids competing with a Snapshot in flight;
- when the external `git` maintenance pass (`gc`, `repack`, packed-refs compaction) runs, and how it is kept away
  from a Snapshot in flight — git's own documentation concedes a corruption risk under concurrent access, so
  exclusion here is a requirement, not a nicety. Writing our own mark-and-sweep is explicitly out of v1 (ticket 01);
- the manual operation "forget this file across all history" (used when a secret leaked in) and what it costs;
- whether a snapshot can be pinned so retention never removes it.

## Answer

### Segment granularity

**Configurable, hourly default.** A Segment is created lazily on the first Snapshot falling in its period, so the
ref count tracks *active* periods, not elapsed ones: a Project touched twice a day costs ~30 refs over a 30-day
window, a continuously-churning one ~720. Both are trivial for `packed-refs`.

Hourly is the default because it makes the Segment period equal to the finest tier the default ladder uses, and
that alignment is what keeps pruning cheap: every tier coarser than the period prunes by pure ref deletion, and
thinning only ever happens inside one period, on a set bounded by the rate limit (default 1/60 s → ≤60 Snapshots).

The period governs Segment *creation* and nothing else. Reconfiguring it applies to new Segments only; existing
Segments keep the span they were created with, and **mixed-period stores are a supported steady state, not a
migration to be finished**. Nothing requires uniformity: the ladder is written in durations, the boundary rule
below is stated per Segment, the disk cap drops whole refs regardless of span, and pins are period-free by
construction. ADR 0001 already forbids nesting ref names, so `…/2026-09-09T14` and `…/2026-09-09` are siblings and
both sort correctly. Consequently `keepster status` reports the period as a per-Segment fact and must not imply
the store has one.

### The retention ladder

An ordered tier list, expressed in durations so it stays correct at any Segment period:

```toml
[[retention.tier]]
age  = "24h"     # Snapshots younger than this…
keep = "all"

[[retention.tier]]
age  = "7d"
keep = "hourly"

[[retention.tier]]
age  = "30d"
keep = "daily"
# past the last tier: discarded
```

That is also the shipped default: **everything for 24 h, one per hour to 7 days, one per day to 30 days, then
discarded.**

**The newest tier is bounded by age alone.** A count or size bound on "keep all" sacrifices the last few hours
first — the exact window the tool exists to serve. Its count is already bounded by the rate limit (≤1440/day at the
default), and git deduplicates every unchanged blob, so the marginal cost of a Snapshot is one tree chain plus what
actually changed. Store size is the disk cap's concern, and the cap eats the *oldest* end.

**The survivor of a thinned period is the last Snapshot in it** — the state the following period started from. It
keeps the chain chronologically continuous and is stable when a coarser tier later thins the same region again.

**Boundary invariant: Retention only ever acts on a Segment lying wholly past a tier boundary.** A Segment
straddling the boundary is left untouched until it has fully crossed. This is what makes thinning happen at most
once per Segment for its whole life, and what makes the pass idempotent — without it, a boundary sweeping through a
live Segment rewrites that Segment on every run. It also gives the validation rule for the period knob: tier ages
should be whole multiples of the Segment period, or Retention lags by up to one period.

### Disk-usage cap

**Opt-in, per Project, off by default.** The ladder already bounds growth; a default cap would silently truncate
history nobody asked to lose. The cap is per Project rather than on the shared global store, because a global cap
lets one busy Project evict another's history — a surprise with no owner.

One physical fact dictates the order of operations: **deleting a Segment ref frees nothing until `gc` runs**, it
only makes objects unreachable. So the cap can never be a check-then-prune loop — that would delete the entire
history and still measure the same size. The sequence is: apply the ladder → maintenance → *measure* → if still
over, prune the old end → maintenance again. The cap is therefore coupled to the maintenance pass by construction,
not by scheduling convenience.

What the cap may eat: **whole Segments from the oldest end, and nothing else.** It never thins within a tier —
that would convert a free ref deletion into a chain rewrite and leave a history whose shape the user can no longer
state. With this restriction the guarantee stays readable: "the last 24 hours, plus as much of the ladder as fits."

The floor is the newest tier. If even "keep all" exceeds the cap, Retention stops there, the store exceeds the cap,
and `keepster status` reports it. **Refusing to capture is explicitly rejected**: the tool would go quiet at
exactly the moment the tree is churning hardest, and ticket 05 already established that it must never look
healthier than it is.

### When the passes run, and mutual exclusion

Ticket 05 put the CLI behind the daemon's own socket, so the daemon is the only process that touches a History
Store and the external `git` is a child it spawns. Exclusion is therefore an in-process **exclusive per-store
lock**, held by either a capture or a maintenance pass and never both. Reads (timeline, diff, blob fetch) stay
unlocked: `gc` only removes objects Retention already made unreachable, so the worst a concurrent reader observes
is a clean "Snapshot no longer exists", never corruption.

That lock is the *precondition* for the prune, not decoration around it. Git's default `gc.pruneExpire` is
`2.weeks.ago`, so a plain `git gc` leaves everything Retention just unlinked on disk for a fortnight — Retention
would appear to free nothing and the disk cap above could never be satisfied. We need `git gc --prune=now`, which
is precisely the mode git documents as unsafe under concurrent access. Holding the exclusive lock as sole writer is
what makes that call safe.

Cadence, split by cost:

- **Retention** has work only when a Segment has wholly crossed a boundary, so it is checked once per Segment
  rollover — hourly by default.
- **`git gc --prune=now` / `repack` / packed-refs compaction** rewrites packs and runs **at most daily**, or
  immediately when the disk cap is exceeded. Git's own `git maintenance` splits cheap-hourly from expensive-daily
  for the same reason.

Neither competes with the user for I/O: Retention is never urgent, so a scheduled pass waits for the first quiet
moment (~5 min without a Snapshot), with a hard ceiling (~6 h) so a permanently busy Project is still maintained
rather than growing forever. Daemon start *schedules* a pass rather than blocking on one — the startup
Reconciliation walk is the latency-critical work there.

### Pinning

**A pin is the Snapshot re-created as a parentless commit under `refs/keepster/pins/<SnapshotId>`.** See
`docs/adr/0002-pins-as-parentless-commits.md`.

Two properties decide it. A pin recorded in the SQLite index would evaporate on a cache rebuild — ticket 03
declares that index wholly derived and deletable — and the next Retention pass would delete the Snapshot the user
explicitly asked to keep; any flag-based design has to carve an exception into the one invariant that makes ticket
03's recovery story work. And a ref makes the pin **self-enforcing**: a reachable commit is one `gc` physically
cannot collect, so the guarantee stops depending on Retention remembering to honour a flag.

Re-creating it *parentless* is what removes all coupling. A pin ref pointing into an existing chain would keep
every ancestor reachable, so one pin would preserve a whole Segment and defeat the cap. As its own root it shares
the tree, costs one commit object, and lets Retention, thinning and the cap stay entirely pin-unaware. The pinned
copy's commit hash differs from the in-Segment copy, which is exactly the case ADR 0001 anticipated by making
`SnapshotId` the public identity; the index treats the pin ref as canonical for that id.

### Forget a file across all history

`keepster forget <glob>` rebuilds every tree containing a matching path, re-chains every affected Segment,
rewrites any matching pin, and ends with `gc --prune=now` — all under the store lock.

**The ladder is what makes this affordable.** At the defaults a maximally-busy Project retains ~1440 commits in the
24 h tier, ~144 in the hourly tier and ~23 in the daily tier: on the order of **1,600 commits**, not the hundreds of
thousands a real repository holds. A `git2` treebuilder pass over that is sub-second CPU plus the object writes, so
no lazy or soft-delete variant is needed. A soft delete was rejected outright: it leaves the secret readable on
disk for up to 30 days to anyone bypassing the tool, which fails the only reason the command exists.

`SnapshotId`s are preserved through the rewrite, so pins, saved links and the timeline all stay valid; only the
index's commit column needs refreshing, over the same watermark catch-up path ticket 03 already specifies. A
Snapshot left empty by the removal is kept and marked — deleting it would rewrite a chronology the user did not ask
to change.

**Exclusion is a per-invocation choice with no default**, because either default is wrong half the time: a `.env`
should never be captured again, while a source file with one leaked line should keep being snapshotted. Both
`--exclude` (adds the pattern to the Project's exclude rules) and `--keep-watching` ship; interactively the command
asks, non-interactively it fails unless one is given. `--exclude` is refused when the path is on the secret opt-in
list, since that list is the user having explicitly said "capture this".

Under `--keep-watching` the command hashes the live file against the blobs it just removed and **warns when they
still match**, because in that case the next Snapshot re-captures the secret and the operation was silently undone.
The user must rotate the secret and edit the file first.

`forget` never touches the Working Tree — the live file is the user's, and the command says plainly that the secret
is still on disk and still needs rotating. It takes **no Safety Snapshot**, since that would re-capture the very
thing being removed. It is irreversible and gated behind explicit confirmation.
