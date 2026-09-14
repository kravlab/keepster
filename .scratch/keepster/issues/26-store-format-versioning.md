# 26 — Versioning the on-disk store format and its upgrade path

Type: grilling
Status: needs-triage

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
