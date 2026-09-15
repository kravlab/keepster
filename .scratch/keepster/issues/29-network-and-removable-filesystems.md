# 29 — Network and removable filesystems, and directories that disappear

Type: grilling
Status: resolved

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

## Answer

**The daemon never infers "the user deleted everything" from an empty tree. It verifies that the tree in front of
it is the tree that was registered, by volume identity recorded at registration, and refuses to capture when it is
not.** A Root on a network filesystem is accepted and marked `Degraded` where the watcher is known ineffective; a
removable volume that reappears elsewhere is followed by identity; and there is no fallback store.

### The hazard is Retention, not the Snapshot

Capturing an empty tree is harmless in itself — the previous Snapshot still holds everything, and a genuine mass
deletion is precisely the event this tool insures against, so refusing to capture it would be refusing the
product's purpose.

The damage is done by ticket 06's ladder: everything for 24 h, hourly to 7 d, daily to 30 d. A disk left unmounted
for a month, with the daemon dutifully capturing "empty" the whole time, ages the last Snapshot holding real
content off the end of the ladder — and it does so *silently*, because from Retention's point of view that is
ordinary history ageing ordinarily. **The insurance destroys exactly what it insured.**

A threshold on the proportion deleted was rejected on the same reasoning: it would refuse to capture the insured
event. The question to answer is not "how much vanished" but "is this tree reachable at all".

### The evidence is volume identity, and the field already has the precedent

Recorded as `docs/adr/0011-volume-identity-gates-capture.md`: a volume check sits on the hot path of an operation
that looks like it is about files, and a reader deserves to know why it earns a syscall per capture.

**The Registry records the volume's persistent identity at registration, and every capture verifies it.** Identity
matches → this is our tree, and an empty tree means deletion. Identity absent or different → nothing is mounted
there, or something else is: `Unavailable`, no Snapshot, nothing for Retention to thin.

**Time Machine is the one tool in the field that solves this hazard, and it solves it this way.** `tmutil(8)`, on
volume exclusions: *"These track volumes based on file system UUID, which is persistent across volume name and
mount path changes."* Keying the identity of the backed-up thing to the volume rather than to a path makes "the
volume moved or is not mounted" a representable state rather than a mass deletion.

Per platform:

- **macOS** — `statfs` gives `f_mntfromname`; DiskArbitration's `DADiskCreateFromVolumePath` (10.7+) yields the
  volume UUID, and `DASessionSetDispatchQueue` makes it usable from a daemon with no GUI session.
- **Windows** — `GetVolumePathNameW` → `GetVolumeNameForVolumeMountPointW` gives the `\\?\Volume{GUID}\` path, the
  persistent volume identity, with `GetVolumeInformationW`'s serial as a cross-check. One hazard to guard
  explicitly: `GetVolumePathNameW` **silently returns the boot volume's drive letter** for a relative path or an NT
  namespace path, so a watched path on a vanished volume can resolve to `C:\` without an error.
- **Linux** — `statx` with `STATX_MNT_ID_UNIQUE` (5.8/6.8) identifies the mount within this boot and is the ID
  space `statmount(2)` and fanotify's mount events consume; the *persistent* identity is the filesystem UUID,
  reached through the mount source in `/proc/self/mountinfo` field 10 or `statmount`'s `STATMOUNT_SB_SOURCE`
  (6.13). Both are needed: one is cheap and per-boot, the other survives reboots.

**Mount-point detection is a supporting signal only, and the research says why it cannot be the primary one.** On
Linux there is no kernel object for "a mount point with nothing mounted on it" — such a directory is simply an
ordinary directory, with no mount ID, no mountinfo record, and no `STATX_ATTR_MOUNT_ROOT`. No syscall
distinguishes `/mnt/backup` from `/mnt/typo`; only `fstab` or a `.mount` unit expresses intent, and both are
declarations rather than state. macOS does better — `getattrlist`'s `ATTR_DIR_MOUNTSTATUS` exposes
`DIR_MNTSTATUS_TRIGGER` alongside `DIR_MNTSTATUS_MNTPOINT`, so `TRIGGER && !MNTPOINT` is "armed but empty" (use
`FSOPT_NOFOLLOW`, or the `stat` fires the trigger) — but a signal available on one platform of three cannot carry
the rule.

**A sentinel file inside the Root was rejected twice over.** It contradicts ticket 27's posture that Keepster asks
for nothing in the watched directory — and the research found that no absence-based sentinel convention exists
anywhere. Every standardised marker in this space, from `CACHEDIR.TAG` through restic's and borg's
`--exclude-if-present`, answers "skip this directory *because* a marker is present". Nothing answers "distrust this
directory *because* a marker is absent"; there is no convention to lean on, only a private one to invent.

**Where identity cannot be determined**, capture proceeds normally, but a capture that would delete **every single
path** is held and the Root goes `Degraded` pending confirmation. This is deliberately far narrower than the
threshold rejected above: it fires only on total disappearance and only where the primary evidence is unavailable.
The nearest prior art is rsync's interlock — *"If the sending side detects any I/O errors, then the deletion of any
files at the destination will be automatically disabled. This is to prevent temporary filesystem failures (such as
NFS errors) on the sending side from causing a massive deletion of files on the destination"* — and its blunt
cousin `--max-delete`. Both are error- or count-triggered rather than identity-based, which is why they are the
fallback here and not the rule.

### Network filesystems: accepted, and `Degraded` where the watcher is genuinely blind

A Root on a network filesystem is accepted rather than refused — a home directory on NFS is a real configuration,
and its owner is exactly who wants insurance — and marked `Degraded` with a machine-readable reason. That state
already means what is needed: ticket 05 defined it as watching continuing while the coverage guarantee is off.

**The research refines ticket 02's blanket claim, which was Linux-shaped.** Blindness is not uniform, so the
trigger must not be either:

- **Linux is blind, and documented as such.** `inotify(7)`: *"Inotify reports only events that a user-space program
  triggers through the filesystem API. As a result, it does not catch remote events that occur on network
  filesystems."* `fanotify(7)` repeats it word for word. Worse for FUSE: `fuse_reverse_inval_inode()` and
  `fuse_reverse_inval_entry()` contain **no `fsnotify_*` calls at all**, so even a FUSE filesystem that knows about
  a remote change and correctly invalidates its cache emits nothing. sshfs is blind by construction.
- **macOS is partly sighted.** FSEvents sees only local writes, but `smbfs` implements `VNOP_MONITOR` and maps it
  onto SMB change-notify (`smbfs_start_change_notify`), so kqueue's `EVFILT_VNODE` **does** receive server-driven
  notifications — for directories only; a non-directory returns `ENOTSUP`. NFS implements the same hook by polling.
- **Windows SMB is documented as working.** `ReadDirectoryChangesW`'s support table marks SMB 3.0 as supported, and
  the widely-quoted "does not detect changes made by other machines" line is **not in the current
  documentation** — that caveat belongs to `FindFirstChangeNotificationW`. The real remote constraint is a
  documented 64 KB buffer cap: a larger buffer over the network fails with `ERROR_INVALID_PARAMETER`.

So `Degraded` is raised where coverage is actually reduced — every network and FUSE mount on Linux, files rather
than directories on macOS SMB — and not where the platform documents the watcher as working. Two consequences
follow: **`notify`'s `RecommendedWatcher` does not fall back to polling on its own**, so `PollWatcher` must be
selected deliberately for a blind Root; and because the Reconciliation interval *is* the capture latency there, the
user tightens it through the per-Root Trigger set ticket 04 already provides, rather than through a new mode.

### A remounted volume is followed by identity, not by guesswork

Unplugged → the path is unreadable or the identity does not match → `Unavailable`. Replugged at the same path →
identity matches → `Watching`, with no human involved.

Replugged **elsewhere** — which for removable media is the norm, `/run/media/user/DISK1` where `/media/DISK` used
to be — is followed automatically **when the volume identity matches and the path relative to the volume root is
unchanged**. Ticket 07's rule that "a move is only ever explicit" governs a move the *user* performs; here the
mount point was chosen by the operating system and the user did nothing. Ticket 13 made the surrogate id the
identity and the path a mutable attribute, so updating the attribute when the same volume is positively identified
at a new address is that decision applied, not an exception to it. There is no risk of adopting a different disk,
because identity is the test rather than a heuristic.

**Ticket 07's no-overlap rule is re-run on every such path change**, since the new mount point may well fall inside
another Root.

Mount and unmount are observable on all three platforms, with different precision: Linux can poll `/proc/…/mounts`
for a namespace-wide edge (documented for `mounts`, implemented but undocumented for `mountinfo` — arm on
`EPOLLPRI|EPOLLERR` only, or it spins) and, from **6.15**, receive per-mount `FAN_MNT_ATTACH`/`FAN_MNT_DETACH`
carrying the unique mount id; macOS has FSEvents' `kFSEventStreamEventFlagMount`/`Unmount`, which carry the path,
plus DiskArbitration on a dispatch queue; Windows broadcasts `WM_DEVICECHANGE` to top-level windows, which a
service cannot rely on, so the windowless `CM_Register_Notification` is the supported path — with its documented
register-then-enumerate contract, since it delivers no notification for interfaces that already exist.

### An unreachable store stops capture; it does not relocate it

The store may sit on removable or network storage (ticket 04 permits it, and after the `ui.editor` correction it is
chosen in the global config's per-Root block), so it can vanish independently of the watched tree.

**There is no fallback store.** If there is nowhere to write, there is no capture: the Root goes `Unavailable` with
its own reason code and recovers by itself when the store returns. Falling back to the global store was rejected
because it splits one Root's history across two stores, and merging them is an unbuilt feature — and because
writing a user's history somewhere they did not choose is the class of behaviour ticket 24 spent its effort
removing.

Three consequences worth stating plainly:

- **`Unavailable` does not mean "the history is gone".** When the tree is unreachable but the store is present,
  everything readable stays readable: the timeline, diffs, and export to any destination (ticket 19). Only capture
  and Restore stop, the latter for want of somewhere to write. Without saying this, the state name will be read as
  total loss.
- **A Root path replaced by a file or a symlink is `Unavailable`** with its own code — the path exists but is not a
  tree. And a rule that had never been written down: **Keepster never deletes or replaces the Root path itself**,
  during Restore or anything else.
- **A store inside a Root on removable media disappears with the tree**, which is already one of the three
  obligations ticket 04 requires the spec to state for that placement.
