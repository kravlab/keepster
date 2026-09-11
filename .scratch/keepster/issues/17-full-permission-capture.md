# 17 — Full permission capture and restore

Type: grilling
Status: open
Blocked by: 10

## Question

Ticket 10 settled that a Snapshot keeps only the executable bit, as the git tree entry mode (`100755` / `100644`),
and that a Restore over an existing file carries the replaced file's permission bits across and applies the
Snapshot's executable bit on top. Two consequences were accepted for v1 and flagged for a separate decision:

- a permission-only change other than `x` (`chmod go-r key`, an agent's `chmod -R 777 .`) changes no tree, so it
  produces no Snapshot and is invisible in history;
- a Restore cannot put back the permissions a file had when the Snapshot was taken, only its executable bit, so an
  accidental `chmod 644` on a private key cannot be undone through Keepster.

Should Keepster capture and restore permissions in full, and if so, how?

Settle:

- whether the full permission bits are captured out of band — a commit trailer, a side blob per Snapshot, or
  another carrier — and what Forget, Segment thinning and Pins (ADR 0002) must do to carry that data along;
- whether ownership, ACLs (POSIX ACLs, macOS ACLs, Windows DACLs) and extended attributes are in or out, per
  platform;
- whether a permission-only change is a change that warrants a Snapshot, given the watcher reports it only as an
  attribute event and Reconciliation would then have to compare permissions as well as content;
- where a Restore takes permissions from when both the Snapshot and the replaced file carry them, and what happens
  on Windows, which has no POSIX bits;
- how the Safety Snapshot preserves the permissions a Restore is about to overwrite;
- the interaction with [12 — Snapshot write path](12-snapshot-write-path.md), whose index entries carry the mode
  that reaches history.
