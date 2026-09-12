# 19 — Export: saving a version into a directory of the user's choosing

Type: grilling
Status: open

## Question

v1 currently writes history back over the working file only: Restore, as settled by tickets 07 and 10. The user
also wants to put a historical version *somewhere else* — this file as it was, a whole subtree as it was, an
entire Snapshot, or a diff — into a directory they name, leaving the Working Tree untouched.

This is the same read as Restore with a different destination, so the decision is mostly about which of Restore's
hard-won rules survive the change of destination, and which are Restore-only.

Settle:

- **What can be exported**: a single path's version, a subtree, a whole Snapshot, a diff between two Snapshots —
  and in what shape (plain files laid out under the destination, or one archive).
- **Which of Restore's preflight rules apply.** Ticket 10 gave Restore a whole-request preflight with `dry_run`,
  per-reason confirmations plus `force`, per-path resolutions for name collisions and unrepresentable names,
  `skip_unresolvable`, and a flat refusal for hardlinked files. Writing into an empty directory outside any Root
  changes the stakes: nothing is being overwritten, so the hardlink refusal and the collision resolutions may be
  unnecessary — but a destination that is *not* empty brings them straight back.
- **Whether export may write into a watched Root.** Exporting into a Root feeds the export back into history and
  can collide with a live capture; refusing outright is simple but would block the obvious "put the old version
  next to the new one" workflow.
- **What a `Withheld` entry exports as** (ticket 10): omitted, a zero-byte placeholder, or a refusal for the whole
  request — and whether that differs from what Restore does.
- **Diff export format**: a unified text diff, the pair of files, or both; and what a diff means for a binary or
  `Withheld` path.
- **The three surfaces.** The `/v1` contract is additive-only (ticket 07), so this is new endpoints, not changed
  ones. The CLI takes a destination argument. The web UI cannot write to a directory from a browser, so its export
  is a download — a genuinely different mechanism from a daemon-side copy, and the contract has to serve both
  without growing two semantics.
- **Who performs the copy**: the daemon can read the store but may be unable to write to the user's chosen
  directory (a sandboxed or differently-confined install), while having the CLI write it means streaming bytes over
  the API. This also decides whether a huge export is a job with progress (like `forget` and Maintenance in
  ticket 07) or a synchronous request.
- **Permissions of exported files**, and whether the executable bit travels (ticket 10 keeps it in the tree mode);
  ticket 17 may widen this.
