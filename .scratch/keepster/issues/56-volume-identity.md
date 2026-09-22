# 56: Volume identity, and Roots on network or removable filesystems

**Blocked by:** 44 (Reconciliation — the guarantee, and what counts as evidence of change), 37 (The Registry —
register, deregister and resolve a Watched Root)

**Status:** ready-for-agent

**What to build:** unplugging an external disk does not quietly age a user's real history off the ladder, a Root on a
network share is accepted with an honest warning, and a volume that comes back at a different mount point is followed
automatically.

**An empty tree is never read as a deletion without proof** (ADR 0011): the Registry records the volume's persistent
identity at registration and every capture verifies it. The hazard was never the Snapshot — a mass deletion is the
insured event — but Retention, which over a month of "empty" captures ages the last Snapshot with real content off
the end, silently.

- [ ] The volume's persistent identity is recorded at registration and checked at every capture
- [ ] Where identity cannot be determined and a capture would delete **every** path, the capture is held and the Root
      goes `Degraded` with its reason
- [ ] Mount-point detection is a supporting signal only, never the decision
- [ ] A remounted volume is followed automatically when both the identity and the path within the volume match, with
      the no-overlap rule re-run; this does not count as the user moving a Root
- [ ] A Root on a network filesystem is accepted, and marked `Degraded` exactly where the platform's watcher is
      documented as blind rather than everywhere
- [ ] There is no fallback store: nowhere to write means no capture, and that is reported
- [ ] `Unavailable` explicitly does not mean the history is unreadable, and the tests assert that reading works while
      capture cannot
