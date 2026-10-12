# 54: The job resource and the manual Maintenance Pass

**Blocked by:** 52 (The external `git` prerequisite, the Maintenance Pass and the disk cap), 53 (Pins, and what
happens to a deregistered Root's history)

**Status:** done

**What to build:** a long or irreversible operation — a manual Maintenance Pass now, Forget in ticket 81 — runs as
a queryable job rather than a connection the user must keep open, and the CLI follows it.

A job, because an operation whose connection drops must still have an identity to query; pins stay synchronous for
the opposite reason. Forget itself moved to ticket 81, which this one unblocks.

- [x] ~~Forget, its request, its warning and its exclusion~~ — moved to ticket 81
- [x] The job resource reports queued, running, succeeded or failed with progress and a result, and a manual
      Maintenance Pass is a job through the same resource
- [x] The CLI follows progress off the event stream by default and can detach, printing the job id

## Comments

- **Done, split in two.** Forget is ticket 81, which carries its criteria and the decision taken at the split: an
  `--exclude` goes to a file the daemon owns, as a rule level of its own (`forget`) above every user level.
- **The job resource.** `POST /v1/roots/{id}/maintenance` answers `202` with `job_id` and `Location:
  /v1/jobs/{id}`; `GET /v1/jobs/{id}` reports `id`, `kind`, `root`, `state` (`queued`, `running`, `succeeded`,
  `failed`), and once it ends a `result` or an `error` in the problem shape. The states move only forward
  (`keepster_core::JobState`). `job.started`, `job.progress` and `job.completed` are announced.
- **Jobs live in memory** (decided): the last 256 finished are remembered, an unfinished one always is, and a
  restart forgets them all (`404 job_not_found`). A job's work is idempotent, so asking again is the answer.
  Asking for a pass while one of that Root still waits joins it rather than queuing a second; a job that panics
  ends `failed`; once the daemon is stopping, a job still waiting for its store ends `failed` with
  `job_interrupted`.
- **Maintenance as a job.** Refused at the door with `409 maintenance_unavailable` while no `git` is usable (the
  CLI exits 11, as `status` does); otherwise queued behind any capture of the Root, run as the scheduled pass is
  — the same `git` checks, `maintenance.completed`, and the daily schedule counted from it — and its result says
  how many bytes it reclaimed. A `git` failing its check ends the job `maintenance_unavailable`, a pass `git`
  failed `maintenance_failed` with what it said, a history that cannot be opened `history_unreadable`. A Root no
  longer watched can be maintained too.
- **Progress.** The resource and the stream carry it, but a Maintenance Pass is one `git` command with no steps
  to count, so it reports none; Forget (ticket 81) is the first job that will.
- **The CLI.** `keepster maintain [-r ROOT]` subscribes to the stream before asking, prints the job's start and
  progress on standard error and how it ended on standard output, and falls back to asking `GET /v1/jobs/{id}`
  if the stream goes away or replays nothing; a failed job exits with its problem's code. `--detach` prints the
  job's identity alone. `keepster job <id> [--json]` reports one — the way back to a detached job.
