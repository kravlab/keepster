# 54: Forget a path from all history, and the job resource

**Blocked by:** 52 (The external `git` prerequisite, the Maintenance Pass and the disk cap), 53 (Pins, and what
happens to a deregistered Root's history)

**Status:** ready-for-agent

**What to build:** a user who leaked a secret into history can remove every version of that path from the whole
history, with a queryable job rather than a connection they must keep open, and with the tool telling them plainly
what is still on disk.

Forget rewrites every Segment and pin containing a matching path, preserves every `SnapshotId`, and ends with an
immediate prune under the exclusive lock. It is affordable precisely because the ladder bounds the store. It is a
**job**, because an irreversible operation whose connection drops must still have an identity to query; pins stay
synchronous for the opposite reason.

- [ ] Forgetting a glob removes every version of every matching path from every Segment and every pin, and a
      subsequent read of those paths finds nothing
- [ ] Every `SnapshotId` survives the rewrite, so pins, saved links and the timeline stay valid; only the index's
      commit column is refreshed
- [ ] A Snapshot left empty by the removal is kept and marked rather than deleted
- [ ] The request must carry the glob, an explicit confirmation and an explicit choice between excluding the path
      from now on and continuing to watch it; anything missing is refused — the daemon never asks, the CLI does
- [ ] Choosing to keep watching warns when the live file still matches what was removed, because the next Snapshot
      would re-capture the secret
- [ ] Excluding a path that is on the secret opt-in list is refused
- [ ] Forget never touches the Working Tree, takes no Safety Snapshot, and says plainly that the secret is still on
      disk and still needs rotating
- [ ] The job resource reports queued, running, succeeded or failed with progress and a result — Snapshots touched,
      blobs removed, and the warning above — and a manual Maintenance Pass is a job through the same resource
- [ ] The CLI follows progress off the event stream by default and can detach, printing the job id
