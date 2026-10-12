# 53: Pins, and what happens to a deregistered Root's history

**Blocked by:** 51 (Retention — the ladder, the Segment Period and thinning), 52 (The external `git` prerequisite,
the Maintenance Pass and the disk cap)

**Status:** done

**What to build:** a user can declare that one Snapshot survives Retention unconditionally, and deregistering a Root
leaves its history decaying to exactly its last state rather than vanishing in a month or holding disk forever.

A Pin is the Snapshot **re-created as a parentless commit** under a deterministic ref (ADR 0002): reclamation
physically cannot collect it, it survives an index rebuild, it costs one commit object, and Retention, thinning and
the disk cap stay entirely pin-unaware. Parentless is what removes the coupling — a pin pointing into a chain would
keep every ancestor alive and defeat the cap.

- [x] Pinning and unpinning are synchronous and idempotent, and listing pins works
- [x] A pinned Snapshot survives a Maintenance Pass, asserted against a real store
- [x] A pinned Snapshot survives deleting and rebuilding the index, and the index treats the pin ref as canonical for
      that id
- [x] A pin shares the Snapshot's tree and adds exactly one commit object
- [x] Retention, thinning ~~and the cap~~ need no knowledge of pins~~, and the cap's floor accounts for the pinned
      Snapshot~~ — the cap and its floor moved to ticket 80, which carries this as a criterion
- [x] Deregistering a Root pins its latest Snapshot and leaves Retention running, so the store decays down the ladder
      to that pinned state
- [x] That pin is an ordinary Pin: listed, unpinnable, and carrying the incomplete marking if the last Snapshot had
      one; a Root that never captured anything has nothing to pin
- [x] Re-registering a decayed Root needs no warning, because decay was continuous rather than deferred

## Comments

- **Done, as one ticket.** A Pin is the Snapshot's own commit again — tree, message, signatures — without a
  parent, under `refs/keepster/pins/<SnapshotId>`: one commit object, the same commit however often it is made.
  `PUT`/`DELETE /v1/roots/{id}/pins/{snapshot}` answer `204` whatever the previous state, under the store's lock
  (a Maintenance Pass must not prune between reading the Snapshot and writing its Pin); `GET …/pins` lists them
  newest first. A ref under a Pin's name that is not this Snapshot's Pin is replaced rather than taken at its
  word; one naming no commit, or nested below the leaf, is passed over by the index.
- **The index.** Schema 3 (a rebuild at the first start, ADR 0010) holds every *copy* of a Snapshot — its Segment
  commit and its Pin — each Pin being a source with its own watermark beside the Segments, so pinning and
  unpinning are caught up like any capture. A Snapshot is read from its Pin while it has one and goes once
  neither copy is left; `pinned` is on every Snapshot the API returns, and `log` marks it `[pinned]`.
- **Unwatching.** The Root's newest Snapshot is pinned first, then `root.toml` becomes `unwatched.toml` — a file
  of its own, so a build from before this ticket sees what it always saw there: a directory without a record,
  which it leaves alone. A history that cannot be pinned is not unwatched (a `500`), so a Root never decays
  unpinned; `purge` remains, and now also erases the history of a Root no longer watched. A Root that never
  captured anything has nothing to pin.
- **Decay.** Housekeeping goes on over Roots no longer watched, by the global ladder — the global file and its
  blocks for that Root, never the Root's own file — and without ticket 51's "newest always survives": that
  Snapshot's last state is its Pin's to hold (`Newest::Ages`), so the store decays to exactly the pinned state,
  and, unpinned, to nothing, by the user's own act (ticket 07). Orphaned directories left by earlier builds have
  no Pin and are not decayed: doing so would be the silent purge ticket 07 rejected.
- **Reaching it again.** Reads and `/pins` accept the identity of a Root no longer watched; capture, Restore,
  Reconcile and configuration do not. `keepster watch` of the exact path it was unwatched at re-adopts the Root —
  its identity and its history — with no warning, the newest such Root winning if several were unwatched there.
  `keepster pin [SELECTOR]` (default `latest`), `keepster unpin SELECTOR` and `keepster pins [--json]` take a full
  identity of such a Root as `-r`, and `keepster unwatch` names the command that lists its Pin.
