# There is no migration of history: the content format is a property of the Segment

Keepster ships no migration command and performs no migration of Snapshots. When the on-disk content format
changes, new Segments are written in the new format and existing ones are read in theirs; the Retention ladder
retires the old ones within about a month without anyone acting. A future reader will look for the migration
path every storage system has and find none, so the absence is recorded here as a decision.

The version governing content lives in the **Segment's parentless root commit**, as a trailer. It is one of four
numbers with disjoint scopes: the container version in `root.toml` (directory shape and the `refs/keepster/…`
namespace), this one, the sidecar's record grammar in `.keepster-meta`'s header, and the index schema in SQLite.
Only the first can require a rewrite, and what it rewrites is not history.

Two earlier decisions make this available rather than inventive. **ADR 0001 made a Segment self-contained** — its
own ref, a parentless commit at the head of its chain — so it can be dropped or thinned without touching another.
And **ticket 06 already established the pattern**: the Segment Period is a property fixed at creation of which a
store may hold a mix indefinitely. Content format is the same shape of fact.

## Considered Options

- **A single store-wide version.** Rejected first, and it is what forces the others: a store accumulates Snapshots
  written by many daemon versions over months, so one number for the whole store makes any format change require
  rewriting the entire history to stay self-consistent.
- **Automatic migration on first open.** Rejected on two counts. Mechanically it is the rewrite-every-Segment
  machinery ticket 06 needs for `forget` — the tool's one irreversible operation, which ticket 07 wrapped in a job
  with an explicit `confirm: true` for exactly this reason — applied without asking to the user's only copy of
  their history. And it is a one-way door: once migrated, the previous daemon can no longer read the store, so a
  user who upgrades, dislikes the result and reinstalls finds their history gone. Being punished for downgrading
  is the opposite of what an insurance tool should do.
- **An explicit `keepster migrate`, with the Root idle until it runs.** Restores consent and loses coverage: the
  insurance is off for as long as the user has not yet noticed, which for someone who upgraded through a package
  manager and did not read release notes could be weeks.

## Consequences

- **The record-type alphabet is append-only**, and changing the meaning of an existing type is forbidden — a new
  type is introduced instead. This is the same promise ticket 14 made for `/v1`, for the same reason. It also
  freezes the sidecar's header version in practice, leaving it an escape hatch for a change to the header line
  itself rather than a number anyone expects to move.
- **An unknown record type is skipped and reported**, never skipped silently and never treated as fatal. The
  asymmetry that makes this safe: an old daemon skipping a new type behaves exactly as it always did, whereas a
  redefined type would make it confidently misread.
- **Refusal is as narrow as the level allows.** A container newer than the daemon makes the Root `Unavailable`
  with `store_too_new`, because neither reading nor writing is safe when the ref namespace is unknown. A Segment
  newer than the daemon costs only that Segment: capture continues into a new Segment of the daemon's own format,
  so the insurance keeps working forward while some history is temporarily opaque.
- **`Unavailable` widens.** It was defined through an unreachable directory; its operative meaning — nothing can be
  captured, the Root is never deregistered, the store is kept — covers a store this build cannot open just as well.
  `Degraded` does not fit, because it promises that watching continues on worse terms.
- **`root.toml` is rewritten automatically and silently** when the container version moves. It is small,
  daemon-owned, replaced atomically, and not history: ticket 03 establishes that everything in it is recoverable
  except the watched path.
- **The index is never migrated.** A schema mismatch deletes and rebuilds it by ticket 03's procedure, which is
  why schema changes there cost nothing and need no coordination with anything else here.
- **A reader must understand every content format still present.** The ladder bounds that at roughly a month, with
  one exception: a **pin** survives Retention unconditionally (ADR 0002), so a pinned Segment in an old format can
  persist for years. The cost is bounded by its shape — ADR 0002 re-creates a pinned Snapshot as a single
  parentless commit sharing the original's tree, so the legacy reader path for a pin is one commit and one tree,
  not a chain.
- **Ticket 11's minimum `git` does not travel with the format.** Raising it changes no byte on disk; it surfaces as
  the `maintenance: unavailable` row in `keepster status` and never as a refusal to read history. Coupling them
  would cost a user their Snapshots when all they actually lose is the ability to reclaim disk.
