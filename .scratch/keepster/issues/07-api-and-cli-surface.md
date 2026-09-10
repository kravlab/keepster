# 07 — Daemon HTTP API and CLI surface

Type: grilling
Status: open
Blocked by: 03, 04

## Question

The daemon is the only component that touches the store, so both the CLI and the web UI are clients of one API.
What is that contract?

Settle:

- the endpoints and their shapes: project registry (list, watch, unwatch, move), timeline for a project, snapshots
  touching a given path, file listing within a snapshot, file content, diff (snapshot vs previous, snapshot vs
  working tree), restore, forget;
- how a `SnapshotId` appears in URLs and CLI arguments;
- the CLI commands that mirror them, and which are CLI-only (`service install`, `status`);
- the error model shared by both surfaces;
- authentication: how the token is minted, passed and rotated, and what stops another local process from calling
  the API;
- whether the timeline needs streaming or long-polling so an open UI reflects new snapshots live.

## Context from ticket 05

[05 — Daemon lifecycle and autostart on three platforms](05-daemon-lifecycle.md) already fixed part of this
contract; do not reopen it here:

- **Transport is settled**: one HTTP API served on two listeners — a permission-protected Unix socket / named pipe
  for the CLI (no token, OS permissions are the boundary) and 127.0.0.1 plus token for the browser. The
  authentication bullet above therefore only concerns the *TCP* listener; "what stops another local process from
  calling the API" is answered for the CLI path by socket permissions.
- **`status` is not a free design**: it must expose live daemon state as the payload described in 05 — daemon
  liveness and uptime, autostart, linger, the git prerequisite row owned by ticket 11, and per Project its state
  (`Watching` / `Degraded` / `Unavailable`), last Snapshot age, Snapshot count and any "coverage not guaranteed"
  flag — with a non-zero exit whenever any Project is not `Watching`, and `--json` emitting the same shape the web
  UI renders. This ticket owns its wire format, not its content.
- **CLI-only commands** now include `service install | uninstall | status` alongside `status`.
- A Snapshot may be **marked incomplete** (see `CONTEXT.md`), so every shape that returns a Snapshot needs a place
  to say so and to list the omitted paths.

- the commands ticket 06 introduced and did not place: `pin`/`unpin`, `forget <glob>` (with its `--exclude` /
  `--keep-watching` choice and its confirmation), and a manual maintenance trigger, plus how the over-cap and
  Segment-Period facts surface in `status --json`.
