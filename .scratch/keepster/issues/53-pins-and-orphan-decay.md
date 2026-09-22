# 53: Pins, and what happens to a deregistered Root's history

**Blocked by:** 51 (Retention — the ladder, the Segment Period and thinning), 52 (The external `git` prerequisite,
the Maintenance Pass and the disk cap)

**Status:** ready-for-agent

**What to build:** a user can declare that one Snapshot survives Retention unconditionally, and deregistering a Root
leaves its history decaying to exactly its last state rather than vanishing in a month or holding disk forever.

A Pin is the Snapshot **re-created as a parentless commit** under a deterministic ref (ADR 0002): reclamation
physically cannot collect it, it survives an index rebuild, it costs one commit object, and Retention, thinning and
the disk cap stay entirely pin-unaware. Parentless is what removes the coupling — a pin pointing into a chain would
keep every ancestor alive and defeat the cap.

- [ ] Pinning and unpinning are synchronous and idempotent, and listing pins works
- [ ] A pinned Snapshot survives a Maintenance Pass, asserted against a real store
- [ ] A pinned Snapshot survives deleting and rebuilding the index, and the index treats the pin ref as canonical for
      that id
- [ ] A pin shares the Snapshot's tree and adds exactly one commit object
- [ ] Retention, thinning and the cap need no knowledge of pins, and the cap's floor accounts for the pinned
      Snapshot
- [ ] Deregistering a Root pins its latest Snapshot and leaves Retention running, so the store decays down the ladder
      to that pinned state
- [ ] That pin is an ordinary Pin: listed, unpinnable, and carrying the incomplete marking if the last Snapshot had
      one; a Root that never captured anything has nothing to pin
- [ ] Re-registering a decayed Root needs no warning, because decay was continuous rather than deferred
