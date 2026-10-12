# 81: Forget a path from all history

**Blocked by:** 54 (The job resource and the manual Maintenance Pass)

**Status:** done

**What to build:** a user who leaked a secret into history can remove every version of that path from the whole
history, as a queryable job rather than a connection they must keep open, with the tool telling them plainly what
is still on disk.

Split out of ticket 54, which builds the job resource this runs as. Forget rewrites every Segment and Pin
containing a matching path, preserves every `SnapshotId`, and ends with an immediate prune under the exclusive
lock. It is affordable precisely because the ladder bounds the store (ticket 06).

Decided when the ticket was split: excluding a path from now on writes the pattern to a **file the daemon owns**,
beside the Root's record in the Registry, never to a TOML file the user edits (ticket 07 keeps configuration
read-only). It is a rule layer of its own, above every user level, so no negation in a user's file can bring the
secret back, and `explain` and `config` show it at a level of its own (`forget`).

- [x] Forgetting a glob removes every version of every matching path from every Segment and every Pin — the
      sidecar's records included (ADR 0006) — and a subsequent read of those paths finds nothing
- [x] Every `SnapshotId` survives the rewrite, so Pins, saved links and the timeline stay valid; only the index's
      commit column is refreshed
- [x] A Snapshot left empty by the removal is kept and marked rather than deleted
- [x] `POST /v1/roots/{id}/forget` must carry the glob, `confirm: true` and an explicit choice between excluding the
      path from now on and continuing to watch it; anything missing is `400 confirmation_required` — the daemon
      never asks, the CLI does
- [x] Choosing to keep watching warns when the live file still matches what was removed, because the next Snapshot
      would re-capture the secret
- [x] Excluding a path that is on the secret opt-in list is refused at the door, with the request, never by a job
      that fails later (ticket 07)
- [x] Forget never touches the Working Tree, takes no Safety Snapshot, and says plainly that the secret is still on
      disk and still needs rotating
- [x] Its job's result reports Snapshots touched, blobs removed, and the warning above, and it reports its progress —
      Segments and Pins rewritten of those to rewrite — on `job.progress`, the first job to count its steps
- [x] `keepster forget <glob>` asks interactively for the confirmation and for `--exclude` or `--keep-watching`,
      refuses without them when not interactive, and follows the job as ticket 54's `keepster maintain` does

## Comments

- **The rewrite.** `HistoryStore::forget` filters every tree of every Segment and Pin — trees shared between
  Snapshots once, keyed by where they sit so an anchored pattern matches as it should — drops matching files, whole
  matching directories and directories left empty, and the sidecar's records of matching paths (a record is matched
  as what the tree holds at its path, so a pattern for directories leaves a file's record alone; lines a later build
  wrote are kept). Every commit is made again with its own author, committer and message, so every `SnapshotId`
  stays; Pins stay parentless; each ref moves only from the tip read. A Segment holding commits this product did not
  write is rewritten too — what a secret is in matters less than that it goes — and a ref naming no commit is
  reported in the result (`left_alone`) rather than passed over. The index finds the rewritten sources by their
  watermarks and reads them again.
- **The mark** (decided): every Snapshot Forget took a path from carries `Keepster-Forgotten: <count>`, raised by
  each Forget, and its `Keepster-Omitted` count loses the not-read records that went; the API has
  `forgotten_paths`, `log` shows `[N forgotten]`. One left empty is kept, and that count is why it is empty.
- **Excluding** (decided at the split): `forgotten.toml` beside the Root's record, written by the job — under the
  store's lock, and only once nothing opted in stands in its way — never at the door, so a job that fails or never
  starts leaves no exclusion behind. Its rules are a level of their own, `forget`, above every user level, shown by
  `explain` and `config`; a file that cannot be read leaves the Root uncaptured rather than capture the secret back.
  A manual capture and a Restore's Safety Snapshot read their rules holding the store, so one that waited behind a
  Forget cannot take back what it excluded.
- **The opt-in refusal** (decided): on the paths the glob matches in the history — asked at the door without the
  lock and without writing anything, and again by the job under the lock, so a capture in between changes nothing.
  `409 secret_opted_in` names the path. A file created since the last Snapshot is not among them; it is in no
  history to forget yet.
- **The request and the job.** `POST /v1/roots/{id}/forget` takes `glob`, `confirm: true` and `on_exclude`
  (`exclude` or `keep-watching`), refusing anything missing with `400 confirmation_required` and `details.missing`;
  a negation is `400 glob_invalid`, since it names what to keep. It is refused with `409 maintenance_unavailable`
  without a usable `git`, because it ends with a Maintenance Pass under the same lock, which is what takes the bytes
  off the disk. The job reports progress — the Segments and Pins rewritten of those to rewrite, then the pass — and
  its result: Snapshots touched, blobs removed, the paths removed, `still_on_disk`, `excluded`, the bytes reclaimed.
  Two Forgets of different globs are two jobs; the same one asked twice while it waits is one.
- **What is still on disk.** The Working Tree is only read: a removed path whose file still holds a removed version
  is named in `still_on_disk`. The CLI always says the secret is still on disk and still needs rotating, and warns
  that the next Snapshot captures it again only when it is kept watched. No Safety Snapshot is taken.
- **The CLI.** `keepster forget <glob> (--exclude | --keep-watching) [--yes] [--no-input] [--detach]` asks at a
  terminal for what the request must carry and exits 2 without it otherwise, then follows the job as `keepster
  maintain` does.
