# 56: Volume identity, and Roots on network or removable filesystems

**Blocked by:** 44 (Reconciliation — the guarantee, and what counts as evidence of change), 37 (The Registry —
register, deregister and resolve a Watched Root)

**Status:** done

**What to build:** unplugging an external disk does not quietly age a user's real history off the ladder, a Root on a
network share is accepted with an honest warning, and a volume that comes back at a different mount point is followed
automatically.

**An empty tree is never read as a deletion without proof** (ADR 0011): the Registry records the volume's persistent
identity at registration and every capture verifies it. The hazard was never the Snapshot — a mass deletion is the
insured event — but Retention, which over a month of "empty" captures ages the last Snapshot with real content off
the end, silently.

- [x] The volume's persistent identity is recorded at registration and checked at every capture
- [x] Where identity cannot be determined and a capture would delete **every** path, the capture is held and the Root
      goes `Degraded` with its reason
- [x] Mount-point detection is a supporting signal only, never the decision
- [x] A remounted volume is followed automatically when both the identity and the path within the volume match, with
      the no-overlap rule re-run; this does not count as the user moving a Root
- [x] A Root on a network filesystem is accepted, and marked `Degraded` exactly where the platform's watcher is
      documented as blind rather than everywhere
- [x] There is no fallback store: nowhere to write means no capture, and that is reported
- [x] `Unavailable` explicitly does not mean the history is unreadable, and the tests assert that reading works while
      capture cannot

## Comments

- **The identity** (decided): `keepster-service` answers which volume a path is on — Linux by the mount
  `/proc/self/mountinfo` names for the path's device, identified by the `/dev/disk/by-uuid` link resolving to it,
  the mounted part of the filesystem included so two btrfs subvolumes or bind mounts are never taken for each other;
  macOS by `statfs` and `getattrlist`'s `ATTR_VOL_UUID` (never through an automount trigger); Windows by
  `GetVolumePathNameW` on the resolved path — a relative path refused and a volume the path is not beneath an error,
  never the boot volume — then the `\\?\Volume{…}\` GUID path. A network share is identified by its source
  (`remote:`). One persistent identity per platform, not the ADR's two: the per-boot mount id was never needed for a
  decision, and `getattrlist` reads the same UUID DiskArbitration does without a dispatch queue.
- **The record**: `root.toml` (and `unwatched.toml`) gain optional `volume` and `within_volume`, additive, read as
  absent by an older build. Recorded at registration, at a move the person makes, and when a Root is watched again;
  a Root registered before this ticket has none, and is treated as one whose volume could not be identified.
- **The gate** (decided), at every capture and Restore, holding the store's lock: the same volume at the same place
  is ours; another, or none that can be named, is `Unavailable` `volume_mismatch` — 409 for a capture or a Restore,
  while the timeline and content read as ever (the harness asserts both). Mount points are never the decision: a
  volume is followed only when exactly one of its mount points has a directory at the Root's place within it, and
  asking that directory's volume confirms it.
- **Following** (decided): at a capture, a Restore and every housekeeping look (each under the store's lock, and
  re-checked under the Registry's lock so a move the person made meanwhile wins), through
  `keepster_app::roots::follow`, which re-runs the no-overlap rule. It publishes `root.moved` — clients must re-read
  the path either way — and the log says it was followed.
- **Unproven** (decided): where the volume could not be identified, a pass that would delete every path the previous
  Snapshot held is held (`CaptureRequest::hold_emptied`, `Pass::held`) for every trigger but `manual`: a capture the
  person asks for is the confirmation. The Root is `Degraded` `capture_held`, kept by the daemon across runtime
  restarts, with `keepster status` naming the command that records it.
- **The watcher**: blind on Linux network and FUSE mounts (not `fuseblk`), on every macOS network mount (FSEvents),
  never on Windows — `Watch::Blind`, reconciled periodically as a failed watch is, `Degraded` `watch_blind`.
- **No fallback store**: a store that cannot be opened is 409 `store_unreachable` and `Unavailable`
  `store_unreachable`, from a manual or an automatic capture alike, and nothing is written anywhere else.
- **ADR 0003 amended**: the daemon asks `trait Volumes`, and `keepster-testkit` answers with `FakeVolumes` — the one
  stand-in in the workspace, since no test can unplug a disk; the platform's own answers are tested for real in
  `keepster-service` on every CI platform. `CONTEXT.md` gains Volume Identity, and Unavailable says the history
  stays readable.
- **Not done**: mount and unmount events (fanotify, FSEvents, `CM_Register_Notification`) — a remount is noticed at
  the next capture or housekeeping look, within a minute; and a store kept inside the Root, which no build writes yet.

