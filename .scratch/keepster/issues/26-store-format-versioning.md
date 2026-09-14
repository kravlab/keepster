# 26 — Versioning the on-disk store format and its upgrade path

Type: grilling
Status: resolved

## Question

The History Store's format is now fully determined, and that is what makes this question answerable. It spans
ADR 0001 (segment refs and parentless commits), ticket 03 (per-Root registry files plus the derived SQLite index),
ADR 0002 (pins as `refs/keepster/pins/<SnapshotId>`), ticket 12 / ADR 0004 (the write path), ticket 17 / ADR 0006
and tickets 23 and 25 (the `.keepster-meta` sidecar, now carrying `m`, `w`, `i`, `p`, `j` and `d` records under a
versioned header), plus ticket 11's minimum external `git` of 2.32.0.

Nothing yet says what happens when any of that changes. The sidecar already carries a format version in its first
line, which is a promise the spec has not defined the meaning of.

Settle:

- **What is versioned, and where the version lives.** The sidecar has its own header version, but the ref layout,
  the commit trailer keys and the registry files have none. Is there one store-level version marker, a version per
  artifact, or both — and which file holds the store-level one, given that ticket 03 made the registry the only
  non-derived metadata?
- **Which changes require a bump**: a new sidecar record type is additive and an old reader can skip it, whereas a
  changed meaning for an existing record is not. State the compatibility rule rather than enumerating cases, the
  way ticket 14 stated "additive-only" for `/v1`.
- **Reading an older store with a newer daemon**, and the reverse. A user who upgrades, dislikes it and reinstalls
  the previous version must not find their history unreadable — and if that is not supported, the spec must say so
  where the user will see it, not in a changelog.
- **Whether migration happens, when, and who consents.** On first open, in the Maintenance Pass, or never. A
  rewrite of every Segment is the same machinery `forget` already needs (ticket 06), so the cost is known; the
  question is whether the daemon may do it unasked to a store that is the user's only copy.
- **The derived index needs no migration** (ticket 03 lets it be deleted and rebuilt at any time) — so the rule
  should be stated explicitly as the reason schema changes there are free, and the index should carry a schema
  version whose mismatch triggers a rebuild rather than a migration.
- **What a version the daemon does not understand does**: refuse to watch the Root, watch it read-only, or fail
  the whole daemon. Ticket 05's `Degraded` and `Unavailable` already exist and one of them may fit; `keepster
  status` must say which and why.
- **The relationship to ticket 11's git floor.** Raising the minimum git version is a compatibility change of the
  same kind, and the answer should say whether it travels with the store format version or stands apart.

## Answer

**There is no migration of history, because the content format is a property of the Segment.** New Segments are
written in the new format, old ones stay readable, and the Retention ladder washes them out on its own. Four
version numbers exist, each governing something disjoint, and only one of them can ever require a rewrite — of a
small file that is not history.

### Four numbers, four scopes

The question began as "how many versions", and the answer split once the migration question was asked. *History
is not migrated; the container is; the index is rebuilt.*

| Where | Governs | On a bump |
|---|---|---|
| `root.toml` | The **container**: directory shape, the `refs/keepster/…` namespace, and the registry file's own shape | Rewritten automatically (below) |
| The Segment's parentless root commit, as a trailer | The **content**: trailer key meanings and how Snapshots are laid out and chained inside *this* Segment | Nothing — new Segments use the new value |
| `.keepster-meta`'s header line | The **sidecar's record grammar** only, per Snapshot | Effectively frozen at 1 (below) |
| SQLite `user_version` | The **index schema** | Delete and rebuild |

They are not redundant. The sidecar's header version is what lets a `.keepster-meta` blob be read in isolation —
from a pin, or under `git show` while debugging — without walking up to a Segment root commit to learn its grammar.

**One version was rejected outright.** A store holds Snapshots written by many daemon versions over months, and
nobody rewrites them on upgrade, so a single store-wide number would make any format change require rewriting the
whole history to stay self-consistent — which is the thing this ticket is trying to avoid.

### The compatibility rule: add a kind of thing, never change one

Same shape as ticket 14's "additive-only" promise for `/v1`, and for the same reason.

**The record-type alphabet is append-only.** A reader that meets a record type it does not know **skips it and
says so** — `entries` and `status` report that a Snapshot holds records this build does not understand — and
changing the *meaning* of an existing type is forbidden; you introduce a new type instead. Silent skipping was
rejected as exactly the quiet degradation this design refuses everywhere else; treating an unknown record as fatal
was rejected because it punishes a user for downgrading by taking their whole history away.

This is why the sidecar's header version is effectively frozen: under an append-only alphabet there is nothing
left for it to express except a change to the header line itself, which is why it survives as an escape hatch
rather than as a number anyone expects to move.

The asymmetry that makes skipping safe is worth stating plainly: **an old daemon that skips a new record type
behaves exactly as it always did.** It does not corrupt anything; it merely does not know about a capability it
never had. The dangerous change is the opposite one — redefining an existing type — because then an old reader
confidently misreads. Hence the rule bites on meaning, not on addition.

### No migration of history exists

Recorded as `docs/adr/0010-no-store-migration.md`: a future reader will look for the migration path every storage
system has, and the absence needs to read as a decision rather than an omission.

Two decisions already on the map make this available rather than clever:

- **ADR 0001 made a Segment self-contained** — its own ref, a parentless commit at the head of its chain — so it
  can be dropped or thinned without touching any other Segment.
- **Ticket 06 already established the pattern of a Segment property fixed at creation, of which a store may hold a
  mix indefinitely.** `CONTEXT.md` says it of the Segment Period: "a store may hold Segments of differing periods
  indefinitely."

Making the content format such a property means **no migration operation exists at all.** New Segments are written
in the current format; existing ones are read in theirs; and the default ladder — everything for 24 h, hourly to
7 d, daily to 30 d — retires an old-format Segment within about a month with nobody doing anything.

The alternatives both fail on the same point. **Automatic migration on first open** is the machinery ticket 06
needs for `forget` — a rewrite of every Segment — applied without asking to the user's only copy of their history,
and it is a one-way door: once migrated, the previous daemon can no longer read the store, so the user is punished
for trying to downgrade. **An explicit `keepster migrate`** restores consent but leaves the Root idle until it is
run, which means the insurance is off while the user has not yet read the release notes.

**`root.toml` is the one thing rewritten automatically, and silently.** It is small, daemon-owned, replaced
atomically (ticket 03: temp file, fsync, rename), and it is not history — ticket 03 already establishes that
everything in it is recoverable except the watched path. Rewriting it carries none of the risk that rewriting
Segments does.

**The cost, stated honestly:** a reader must understand every content format still present in a store. The ladder
bounds that at roughly a month — except for **pins**, which survive Retention unconditionally (ADR 0002), so a
pinned Segment in an old format can persist for years. The saving grace is its shape: ADR 0002 re-creates a pinned
Snapshot as a single parentless commit sharing the original's tree, so the old reader path needed for a pin is one
commit and one tree, not a chain.

### What an unknown version does, by level

Each refusal is as narrow as its level allows.

- **Container newer than the daemon** (the `root.toml` version). The directory shape and ref namespace are
  unknown, so neither writing nor reading is safe. The Root is not watched at all, and is reported as
  **`Unavailable` with the code `store_too_new`.**
- **Segment newer than the daemon** (the root-commit trailer). The container is understood, so the daemon keeps
  capturing into a **new Segment of its own format** and simply cannot read that one. The Root stays `Watching`,
  and the unreadable Segment is reported as such wherever it would otherwise be listed. This is the downgrade
  case, and under this rule the insurance keeps working forward while one month of history is temporarily opaque.
- **Unknown record type.** Skipped and reported, per the rule above.
- **Index schema mismatch.** Delete and rebuild, with no user-visible state at all — ticket 03 already permits
  deleting the index at any time, which is precisely why schema changes there are free. The rebuild is the
  procedure ticket 03 specified: enumerate Segment refs newest-first, walk each chain reading `SnapshotId`
  trailers, tree-diff each commit against its parent, then record watermarks.

**`Unavailable` widens slightly, and `CONTEXT.md` is updated.** It was defined through an unreachable directory,
but its operative meaning in ticket 05 is the one that matters here: nothing can be captured, the Root is never
deregistered, and its History Store is kept — all three true of a store this build cannot open. `Degraded` was
rejected because it promises that watching continues on worse terms, and here it does not continue at all.

### Ticket 11's git floor does not travel with the format

Raising the minimum external `git` above 2.32.0 changes no byte on disk. It is a property of the tool, not of the
store, so it must not share a version with one: it surfaces as ticket 11's daemon-global `maintenance:
unavailable` row in `keepster status`, which already carries a reason, and never as a refusal to read history.
Coupling them would mean a user with an old `git` losing access to their Snapshots, when the only thing they
actually lose is the ability to reclaim disk.
