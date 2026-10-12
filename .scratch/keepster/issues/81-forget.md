# 81: Forget a path from all history

**Blocked by:** 54 (The job resource and the manual Maintenance Pass)

**Status:** ready-for-agent

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

- [ ] Forgetting a glob removes every version of every matching path from every Segment and every Pin — the
      sidecar's records included (ADR 0006) — and a subsequent read of those paths finds nothing
- [ ] Every `SnapshotId` survives the rewrite, so Pins, saved links and the timeline stay valid; only the index's
      commit column is refreshed
- [ ] A Snapshot left empty by the removal is kept and marked rather than deleted
- [ ] `POST /v1/roots/{id}/forget` must carry the glob, `confirm: true` and an explicit choice between excluding the
      path from now on and continuing to watch it; anything missing is `400 confirmation_required` — the daemon
      never asks, the CLI does
- [ ] Choosing to keep watching warns when the live file still matches what was removed, because the next Snapshot
      would re-capture the secret
- [ ] Excluding a path that is on the secret opt-in list is refused at the door, with the request, never by a job
      that fails later (ticket 07)
- [ ] Forget never touches the Working Tree, takes no Safety Snapshot, and says plainly that the secret is still on
      disk and still needs rotating
- [ ] Its job's result reports Snapshots touched, blobs removed, and the warning above, and it reports its progress —
      Segments and Pins rewritten of those to rewrite — on `job.progress`, the first job to count its steps
- [ ] `keepster forget <glob>` asks interactively for the confirmation and for `--exclude` or `--keep-watching`,
      refuses without them when not interactive, and follows the job as ticket 54's `keepster maintain` does
