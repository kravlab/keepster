# 47: Restore — whole-request preflight, confirmations and the Safety Snapshot

**Blocked by:** 39 (Read a version back — the derived index, entries, content and the timeline), 42 (Configuration —
two levels, provenance, automatic reload, `Degraded`)

**Status:** ready-for-agent

**What to build:** a user puts an old version back over their Working Tree, sees the whole plan before anything is
written, is asked before anything structural happens, and can undo the restore itself.

Restore **only overwrites and creates; it never deletes** — a "make identical" mode would delete everything the
Include Rules hide, which the Safety Snapshot could not bring back. The whole request is checked before the first
byte is written; if any path needs a confirmation or a resolution, nothing is written and every path is reported with
its reason. One request takes one Safety Snapshot, so one operation has one undo.

- [ ] Restoring a path writes it atomically (temp file plus rename) and reports a per-path outcome: restored,
      created, unchanged or failed with its error
- [ ] A Safety Snapshot of the current state is taken first, and its id is returned, so the restore can be rolled
      back
- [ ] The dry-run form returns the per-path plan without writing anything and without taking a Safety Snapshot
- [ ] A missing parent, a type change, a hardlinked file and an overwrite whose current version cannot enter the
      Safety Snapshot each require their own explicit confirmation; a blanket force covers all four
- [ ] Name collisions, names invalid on the destination and case-only differences are resolved per path by the user,
      and the blanket force deliberately does not resolve them
- [ ] Restoring a directory over a directory, a Withheld path, or a path whose link check failed is refused with its
      reason
- [ ] The skip-unresolvable form proceeds and reports each unresolved path as skipped
- [ ] A restored file keeps the replaced file's permission bits, with the Snapshot's executable bit applied; a newly
      created file starts from the umask
- [ ] `keepster restore` mirrors every confirmation and resolution as flags, asks interactively on a terminal, and
      exits with the state-conflict code when prompts are disabled and a resolution is missing
