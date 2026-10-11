# 47: Restore — whole-request preflight, confirmations and the Safety Snapshot

**Blocked by:** 39 (Read a version back — the derived index, entries, content and the timeline), 42 (Configuration —
two levels, provenance, automatic reload, `Degraded`)

**Status:** done

**What to build:** a user puts an old version back over their Working Tree, sees the whole plan before anything is
written, is asked before anything structural happens, and can undo the restore itself.

Restore **only overwrites and creates; it never deletes** — a "make identical" mode would delete everything the
Include Rules hide, which the Safety Snapshot could not bring back. The whole request is checked before the first
byte is written; if any path needs a confirmation or a resolution, nothing is written and every path is reported with
its reason. One request takes one Safety Snapshot, so one operation has one undo.

- [x] Restoring a path writes it atomically (temp file plus rename) and reports a per-path outcome: restored,
      created, unchanged or failed with its error
- [x] A Safety Snapshot of the current state is taken first, and its id is returned, so the restore can be rolled
      back
- [x] The dry-run form returns the per-path plan without writing anything and without taking a Safety Snapshot
- [x] A missing parent, a type change, a hardlinked file and an overwrite whose current version cannot enter the
      Safety Snapshot each require their own explicit confirmation; a blanket force covers all four
- [x] Name collisions, names invalid on the destination and case-only differences are resolved per path by the user,
      and the blanket force deliberately does not resolve them
- [x] Restoring a directory over a directory, a Withheld path, or a path whose link check failed is refused with its
      reason (the Withheld refusal is ticket 48's — see Comments)
- [x] The skip-unresolvable form proceeds and reports each unresolved path as skipped
- [x] A restored file keeps the replaced file's permission bits, with the Snapshot's executable bit applied; a newly
      created file starts from the umask
- [x] `keepster restore` mirrors every confirmation and resolution as flags, asks interactively on a terminal, and
      exits with the state-conflict code when prompts are disabled and a resolution is missing

## Comments

- **Decisions taken with the user before implementation.** The Safety Snapshot is a Snapshot with Trigger
  `restore`; when the Working Tree is identical to the latest Snapshot no second one is written and the latest's
  identity is returned as the Safety Snapshot. Refusing a Withheld path is ticket 48's: a Withheld path has no
  tree entry (ADR 0009) and no record yet, so it answers `path_not_found` until 48 gives it one — 48 already
  lists "restoring one is refused". The naming checks are a pure function over what the destination makes of
  names, and the Windows link names come through `keepster-service`. Test seams: the `/v1` harness, the
  preflight in `keepster-core`, `keepster restore` as a process, and the OpenAPI document. Asking on a terminal
  is not asserted by a test; it was checked by hand through a pty.
- **Where things are.** `keepster_core::plan` decides, from facts, what happens to every file (`Issue`,
  `Confirmation`, `Resolution`, `Step`). `HistoryStore::restore_preflight` gathers the facts from disk —
  ancestry, what is at the target, content compared by object id, links inside the Root by `(dev, ino)` or
  through the daemon-supplied `LinkNames` on Windows, recoverability by the same decision the capture walk makes
  (ignore files included) — and `restore_write` writes. `keepster_app::restore` plans, refuses a blocked plan,
  takes the Safety Snapshot and writes; the daemon holds the store's lock for the whole request.
- **"A directory over a directory"** in the criterion is read as ticket 10's `type_conflict`: a directory where
  a file of the Snapshot would go is refused, whatever is confirmed. Restoring a directory of the Snapshot over
  an existing directory is ordinary: each file beneath it is planned on its own.
- **Writes.** Each file is written to `.keepster-restore.<pid>.<n>` beside it — short, so a long name cannot
  exceed the filesystem's limit — synced, its bits set through the open handle, and renamed into place; the
  directory is synced after. A file replacing one whose bits are known starts `0600`, so private content is
  never briefly readable. Bits: the replaced file's, without setuid, setgid or sticky, with the Snapshot's `x`
  applied where `r` is; a new file starts from the umask. Before each write the facts are gathered again;
  what an earlier file of the same request did (a directory created or put where a file was, a link broken) is
  expected, anything else fails that file and leaves it alone.
- **Not covered, and why.** A link swapped into a directory above a file between its re-check and its write
  could still be followed: closing that window needs `openat`-relative writes, which is a change of its own,
  and the race needs someone who can already write in the Root. The Safety Snapshot is a capture like any
  other, so it trusts the `stat` filter (ADR 0014). A link in a Snapshot is passed over — none are captured
  until ticket 49, which restores them. Resolutions on a path without that question are not refused; a
  `rename_to` that names a taken name or collides with another file's new name is `name_invalid` or
  `name_collision`, where case is significant too. Reaching exit code 6 needs a file that fails mid-request,
  which no test produces on demand.
- **Additions beyond the ticket's words**: the `blocked` outcome and the `target` field on the wire,
  `resolution_invalid` for a malformed answer, `--json`, and the "skip them and restore the rest?" question.
