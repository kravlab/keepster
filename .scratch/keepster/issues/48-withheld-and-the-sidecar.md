# 48: Withheld, the metadata sidecar, special files and paths as bytes

**Blocked by:** 41 (Include Rules, the secret denylist, the size cap and `explain`)

**Status:** done

**What to build:** a Snapshot never silently omits anything. A path whose content was deliberately not held is
recorded as Withheld with a reason; a file that was meant to be captured and could not be read marks the Snapshot
incomplete and is named with its errno; and a name that is not valid UTF-8 survives the round trip.

**A Withheld path has no tree entry at all** (ADR 0009) — an absent entry says only "no content here", which is
true, and the sidecar says why. The sidecar is one blob at the tree root and is the single home for every per-path
fact git cannot express: Withheld records with a reason and kind and deliberately no size, incomplete records
carrying `unstable` or the errno, a cloud placeholder's mtime, and a junction marker.

- [x] FIFOs, sockets and devices are Withheld as special files and never read, so a pipe without a writer cannot
      block a capture
- [x] A file over the cap, a reserved name, an unrepresentable name, a path inside another Root or a History Store,
      and an unhydrated cloud placeholder each carry their own reason
- [x] Withheld paths appear in a Snapshot's entry listing with their reason, and restoring one is refused
- [x] A Withheld path makes the Snapshot neither incomplete nor empty-looking, and a growing over-cap file does not
      mint a substantively empty Snapshot per capture
- [x] A file that changes while read is retried; one that never settles carries its previous version forward and
      marks the Snapshot incomplete with `unstable`, and one with no previous version is absent and named
- [x] A read error marks the Snapshot incomplete with the errno, and the path enters the retry queue with backoff
- [ ] ~~Cloud placeholders are never hydrated~~ — moved to ticket 75
- [ ] ~~Names travel as bytes~~ — moved to ticket 74
- [x] The sidecar's record grammar is versioned in its header, an unknown record type is skipped **and reported**,
      and no existing type's meaning may change

## Comments

- 2026-10-11: ticket 47 left the Withheld refusal here. A Withheld path has no tree entry, so a Restore asking
  for it answers `path_not_found` today; once this ticket records Withheld paths, `restore_preflight` in
  `keepster-store` should find the record and `keepster_core::plan` refuse it as `withheld` with its reason,
  beside `type_conflict` and `link_check_failed`. `keepster diff` against the Working Tree already says
  `withheld` for a file the next Snapshot would not hold (ticket 46).
- 2026-10-11: split with the user before building: names as bytes became ticket 74 and cloud placeholders
  ticket 75. Until 74, a name that is not UTF-8 is recorded as `i eilseq` under its lossy spelling — incomplete,
  as before, which keeps its Root retrying at the one-hour ceiling; `unrepresentable_path` is the Windows case
  74 owns.
- 2026-10-11: done. The sidecar (`.keepster-meta`, `keepster_core::Sidecar`) is written only when it says
  something — a record, or a header other than the default — so most trees stay just the Root's files; it is
  never listed, diffed, restored or read as content, and a change to what it says counts as a change of the
  path it names, so a Snapshot written for a new pipe or a file grown past its cap is not empty-looking. A
  top-level `.keepster-meta` of the user's is Withheld `reserved_name`. An unknown record is skipped and
  reported twice: `entries` returns it as `unrecognized` (and the daemon logs it), and a capture that cannot
  carry a later build's records forward logs that it dropped them.
- 2026-10-11: reads are `O_NOFOLLOW | O_NONBLOCK | O_NOCTTY`, streamed into the object database, and count
  only if size, mtime, ctime and `(dev, ino)` held still; three retries, then `unstable`. A file gone by the
  time it is read is deleted, one replaced by a special file is Withheld. Windows compares size and mtime
  only: std exposes no change time, and the file index is unstable — a gap to close where the Windows FFI
  lives. Errors are named by errno constant on Unix and by Win32 name on Windows (`win32_<n>` otherwise).
- 2026-10-11: the retry queue of ADR 0004 is a whole pass rather than a list of paths — every pass walks the
  whole Root since ADR 0014, so the paths would add nothing — recorded with the Trigger `retry`, 30 s doubling
  to 1 h and reset by a complete pass, and only for a Root captured automatically.
- 2026-10-11: `other_root` and `history_store` come from fences the daemon sets on the policy. Only the
  History Store's is built: registration refuses overlapping Roots, so another Root inside this one arises only
  through a followed link — ticket 49's. A Restore asking by name for a Withheld path, or one beneath a
  Withheld directory, is refused as a `withheld` 404 for the whole request rather than per path in the plan;
  inside a directory asked for, each is passed over with the issue `withheld` and its reason. A diff names a
  path becoming Withheld; one ceasing to be reads as an addition.
