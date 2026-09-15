# Volume identity, verified at every capture, gates whether a Snapshot may be taken

The Registry records each Root's **persistent volume identity** when it is registered, and every capture verifies
it before writing anything. If the identity does not match, or cannot be found, no Snapshot is taken and the Root
becomes `Unavailable`. A reader will find a volume check on the hot path of an operation that appears to be about
files, not disks, and will reasonably ask why it is worth a syscall per capture.

## The failure it exists to prevent

An unmounted mount point is an ordinary empty directory. Nothing about reading it fails, so a capture of a Root
whose disk is not plugged in records that every file was deleted — truthfully, as far as the filesystem is
concerned.

That Snapshot is not itself the damage: the previous Snapshot still holds everything, and a genuine mass deletion
is the event this tool exists to insure against, so refusing to capture one would be refusing the product.

The damage is done by **Retention**. ADR 0001's ladder keeps everything for 24 h, hourly to 7 d, daily to 30 d. A
disk left unmounted for a month, with the daemon dutifully capturing "empty", ages the last Snapshot holding real
content off the end of that ladder. It happens **silently**, because from Retention's point of view this is
ordinary history ageing ordinarily — and the result is that the insurance destroys precisely what it insured.

## Considered Options

- **A threshold on the proportion of paths deleted.** Rejected outright: a capture that refuses to record a mass
  deletion refuses the insured event. The question worth asking is not *how much vanished* but *is this tree
  reachable at all*.
- **Mount-point detection as the primary test.** Rejected because Linux cannot answer it. There is no kernel object
  for "a mount point with nothing mounted on it" — such a directory has no mount ID, no `/proc/self/mountinfo`
  record and no `STATX_ATTR_MOUNT_ROOT`; no syscall distinguishes `/mnt/backup` from `/mnt/typo`, and only `fstab`
  or a `.mount` unit expresses intent, both being declarations rather than state. macOS can answer it, through
  `getattrlist`'s `ATTR_DIR_MOUNTSTATUS` and `DIR_MNTSTATUS_TRIGGER`, but a signal present on one platform of three
  cannot carry the rule. It survives as a corroborating signal where it is cheap.
- **A sentinel file inside the Root.** Rejected twice over. It contradicts the posture ticket 27 settled — Keepster
  asks for nothing inside the watched directory — and there is no convention to lean on: every standardised marker
  in this space, from the Cache Directory Tagging Specification's `CACHEDIR.TAG` through restic's and Borg's
  `--exclude-if-present`, means *skip this because a marker is present*. Nothing anywhere means *distrust this
  because a marker is absent*.

## Why this is the field's answer, not an invention

Time Machine is the one tool surveyed that actually solves this hazard, and it solves it this way. `tmutil(8)`, on
volume exclusions: *"These track volumes based on file system UUID, which is persistent across volume name and
mount path changes."* Keying the identity of the protected thing to the volume rather than to a path is what makes
"the volume moved, or is not mounted" a representable state instead of a mass deletion.

The others stop short. rsync has the closest interlock — *"If the sending side detects any I/O errors, then the
deletion of any files at the destination will be automatically disabled. This is to prevent temporary filesystem
failures (such as NFS errors) … from causing a massive deletion of files"* — plus the blunt `--max-delete`.
Duplicati's `allow-missing-source` defaults to false; restic 0.19.0 began exiting non-zero when a source path does
not exist, after excludes were found able to produce an empty snapshot. All of these are triggered by an error or
by absence of the path, never by the path being present and wrong.

## Consequences

- **Two identities per platform, because they answer different questions.** On Linux, `statx` with
  `STATX_MNT_ID_UNIQUE` is cheap and unique within a boot — the same ID space `statmount(2)` and fanotify's mount
  events use — while the filesystem UUID, via `/proc/self/mountinfo`'s mount source or `STATMOUNT_SB_SOURCE`,
  survives reboots. macOS uses DiskArbitration's `DADiskCreateFromVolumePath`, scheduled with
  `DASessionSetDispatchQueue` so it works from a daemon with no GUI session. Windows resolves
  `GetVolumePathNameW` → `GetVolumeNameForVolumeMountPointW` to a `\\?\Volume{GUID}\` path.
- **One Windows hazard must be guarded explicitly**: `GetVolumePathNameW` silently returns the boot volume's drive
  letter for a relative or NT-namespace path, so a watched path on a vanished volume can resolve to `C:\` with no
  error at all.
- **A narrow fallback where identity is unavailable.** Capture proceeds, but one that would delete *every* path is
  held and the Root goes `Degraded` pending confirmation. This is far narrower than the rejected threshold: total
  disappearance only, and only where the primary evidence could not be obtained.
- **A remounted volume is followed rather than lost.** When identity matches and the path within the volume is
  unchanged, the Root's path attribute is updated automatically. Ticket 07's "a move is only ever explicit" governs
  a move the *user* makes; a removable disk reappearing at a different mount point was the operating system's
  choice. ADR-adjacent: ticket 13 made the surrogate id the identity and the path a mutable attribute, so this is
  that decision applied rather than an exception to it. The no-overlap rule is re-run on every such change.
- **`Unavailable` does not mean the history is unreadable.** With the tree gone but the store present, the
  timeline, diffs and export all keep working; only capture and Restore stop.
