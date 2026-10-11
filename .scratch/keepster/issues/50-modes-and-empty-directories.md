# 50: Permissions and Empty Directories

**Blocked by:** 48 (Withheld, the metadata sidecar, special files and paths as bytes), 47 (Restore — whole-request
preflight, confirmations and the Safety Snapshot)

**Status:** done

**What to build:** an accidental `chmod` is undoable, and a restored tree still has the directories the tools expect.

All twelve POSIX bits are captured on Unix, held in the sidecar as deviations from the Root's modal defaults, so a
tree-wide permission change costs one small record rather than thousands (ADR 0006). Ownership, ACLs, extended
attributes and Windows security descriptors are out: the daemon is never elevated and could not restore them. An
Empty Directory — one the Snapshot holds no content under, including one whose contents were all excluded — is
recorded, because a directory vanishing without a trace is exactly the silent omission this product refuses.

- [x] A permission-only change produces a Snapshot: the pass takes the mode from the `stat` it already performs, and
      the watcher `stat`s the path on an attribute event
- [x] A tree-wide permission change is stored as deviations from the modal defaults, not as one record per path
- [x] Restore asks by default what to do about modes, answered by a single scalar that the blanket force
      deliberately does not resolve, defaulting from a configuration key
- [x] The Safety Snapshot needs no new mechanism, because capture records the bits about to be overwritten
- [x] Windows captures and restores no permission bits beyond the executable bit's tree-mode treatment
- [ ] ~~An Empty Directory is recorded per level of a chain, each keeping its own mode, and detection costs no extra
      system call~~ — moved to ticket 78
- [ ] ~~Recording and recreating are two independent keys, both on by default~~ — moved to ticket 78
- [ ] ~~A restored directory takes its recorded mode where one exists, and the umask only where none does~~ — moved
      to ticket 78

## Comments

- 2026-10-11: split with the user before building: Empty Directories became ticket 78, which now also owns putting
  back the mode of a directory already there — this ticket captures directory modes, and the timeline and a diff
  show a directory's change of mode, but a Restore still writes files only.
- 2026-10-11: done, as ticket 17 decided. All twelve bits are taken from the `stat` the pass already makes; an `m`
  record holds a path's mode only where it differs from its kind's default in the header, an executable file's
  default being the file default with `x` wherever it has `r`. The header's modal pair is counted by a
  Reconciliation, and wherever no observed header can be carried — the first Snapshot, the first after one that
  observed none — and carried forward otherwise (ADR 0006); a recount that changes no path's mode writes no
  Snapshot of its own. A tree-wide `chmod` therefore costs a record per path until the next Reconciliation, then
  one header.
- 2026-10-11: a decision of its own, from the review: the header says whether modes were observed (the word
  `modes`), so a Snapshot taken on Windows or before this ticket holds no mode at all rather than the defaults made
  up for every path — which would have let a Restore loosen a private key to `0644`. Every Snapshot taken on Unix
  therefore has a sidecar. From such a Snapshot a Restore keeps the old behaviour: the replaced file's bits, or the
  umask's for a new one, with the Snapshot's executable bit.
- 2026-10-11: Restore raises `modes_differ` (`snapshot_mode`, `current_mode`; resolution `modes`) for a file whose
  bits on disk are not the Snapshot's, answered by a path's `modes: snapshot|keep` resolution, the request's
  `modes`, or `restore.modes = "ask"|"snapshot"|"keep"` (default `ask`, settable by a Root's own file) — never by
  the blanket force. The CLI takes `--modes snapshot|keep` or `--modes PATH=snapshot|keep`, asks on a terminal
  (once for every file where several differ), and exits 5 with `--no-input`. A created file takes the Snapshot's
  bits, not the umask's; a file whose content already matches is chmodded in place; `keep` drops setuid and setgid
  when the content is replaced. The bits are read back after they are set, and a path the system would not give
  them all — setgid into a group the user is not in — fails.
- 2026-10-11: `entries` gains `permissions` (all twelve bits, where observed) beside `mode`, which keeps the tree
  mode's meaning; a diff's `old_mode`/`new_mode` carry all twelve bits, a change of mode alone is listed — a
  directory's too — and the unified patch writes git's `old mode`/`new mode` lines with every bit. A chmod on a file
  with other names changes all of them, as it is one inode, and the Safety Snapshot holds the bits through the name
  inside the Root, so no `break_hardlinks` question is raised for it.
