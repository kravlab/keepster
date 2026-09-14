# 29 — Network and removable filesystems, and directories that disappear

Type: grilling
Status: needs-triage

## Question

A Root's directory may live on something that is not a local disk, or may stop existing while the daemon watches
it. This is a correctness question about capture, not a convenience one, which is why it belongs before the spec.

The surrounding decisions are settled. Ticket 05 created **`Unavailable`** for a Root whose directory cannot be
reached at all — missing, unmounted or unreadable — distinct from `Degraded`, and made clear such a Root is never
deregistered and keeps its History Store, because identity does not depend on path. Ticket 02 established that
`mmap` writes and network mounts are among the changes no platform announces, which is why Reconciliation is the
guarantee. Ticket 10 defined `mount_point` as a `Withheld` `special_file` kind. Ticket 04 made the store location
configurable, including inside the Root.

Settle:

- **What the daemon does when a Root's filesystem is a network mount** — SMB, NFS, SSHFS — where the watcher is
  documented as unreliable. Is a Root there refused, accepted with reduced guarantees and a `Degraded` reason, or
  accepted silently with Reconciliation carrying it?
- **Detection.** Is the filesystem type inspected at registration and at each mount change, and what does the
  daemon do on a platform where that is awkward to determine?
- **Removable media**: a Root on a USB disk that is unplugged and replugged, possibly at a different mount point.
  Ticket 07 already makes a move explicit and forbids overlap; an unplugged disk is `Unavailable`, but the
  re-appearance path and whether it is automatic need stating.
- **Reappearance with different content.** A disk returning with a wholly different tree, or a mount point that is
  empty because the filesystem has not been mounted yet, must not be captured as "the user deleted everything".
  This is the sharpest risk in the ticket: an empty mount point looks exactly like a deletion.
- **A store on removable or network storage**, which ticket 04 permits — including what happens when the store is
  unreachable but the Root is not, and vice versa.
- **The disappearing Root directory**, including the case where it is replaced by a file or a symlink, and how
  that interacts with ticket 10's type rules and ADR 0007's pruning.
