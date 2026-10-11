# 48: Withheld, the metadata sidecar, special files and paths as bytes

**Blocked by:** 41 (Include Rules, the secret denylist, the size cap and `explain`)

**Status:** ready-for-agent

**What to build:** a Snapshot never silently omits anything. A path whose content was deliberately not held is
recorded as Withheld with a reason; a file that was meant to be captured and could not be read marks the Snapshot
incomplete and is named with its errno; and a name that is not valid UTF-8 survives the round trip.

**A Withheld path has no tree entry at all** (ADR 0009) — an absent entry says only "no content here", which is
true, and the sidecar says why. The sidecar is one blob at the tree root and is the single home for every per-path
fact git cannot express: Withheld records with a reason and kind and deliberately no size, incomplete records
carrying `unstable` or the errno, a cloud placeholder's mtime, and a junction marker.

- [ ] FIFOs, sockets and devices are Withheld as special files and never read, so a pipe without a writer cannot
      block a capture
- [ ] A file over the cap, a reserved name, an unrepresentable name, a path inside another Root or a History Store,
      and an unhydrated cloud placeholder each carry their own reason
- [ ] Withheld paths appear in a Snapshot's entry listing with their reason, and restoring one is refused
- [ ] A Withheld path makes the Snapshot neither incomplete nor empty-looking, and a growing over-cap file does not
      mint a substantively empty Snapshot per capture
- [ ] A file that changes while read is retried; one that never settles carries its previous version forward and
      marks the Snapshot incomplete with `unstable`, and one with no previous version is absent and named
- [ ] A read error marks the Snapshot incomplete with the errno, and the path enters the retry queue with backoff
- [ ] Cloud placeholders are never hydrated: materialisation is disabled where the platform allows it, and a
      placeholder whose size and mtime match its last capture carries that version forward
- [ ] Names travel as bytes: the path query parameter carries percent-encoded bytes, and JSON carries a lossy UTF-8
      path plus the raw bytes whenever the name is not UTF-8
- [ ] The sidecar's record grammar is versioned in its header, an unknown record type is skipped **and reported**,
      and no existing type's meaning may change

## Comments

- 2026-10-11: ticket 47 left the Withheld refusal here. A Withheld path has no tree entry, so a Restore asking
  for it answers `path_not_found` today; once this ticket records Withheld paths, `restore_preflight` in
  `keepster-store` should find the record and `keepster_core::plan` refuse it as `withheld` with its reason,
  beside `type_conflict` and `link_check_failed`. `keepster diff` against the Working Tree already says
  `withheld` for a file the next Snapshot would not hold (ticket 46).
