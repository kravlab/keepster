# 18 — Mounting a Snapshot as a filesystem

Type: grilling
Status: wontfix

## Question

Instead of copying a historical version out of the store, expose a Snapshot — or a whole Root's history — as a
read-only filesystem, so the user can browse, `grep` and diff it with ordinary tools.

Settle, if this is ever picked up:

- **The platform mechanism**, which differs on all three and is cheap on none:
  - **Linux**: FUSE, which is the well-trodden path but still a second process and a mount namespace to manage.
  - **macOS**: macFUSE is a kernel extension, and Apple has been closing that door for years (user approval, reduced
    security posture, per-release breakage). FSKit is the sanctioned replacement but is macOS 15+ only, so it would
    either raise the platform floor or require both implementations.
  - **Windows**: ProjFS (Windows Projected File System) is an optional feature that is not enabled by default and
    is absent on some editions; third-party drivers such as WinFsp exist, but their licensing and redistribution
    terms must be checked before assuming they can ship (ticket 11 already made "no bundled binaries" a decision for
    `git`, and the same question applies here).
- **What gets mounted**: one Snapshot, a Root as a time-tree (`<root>/<snapshot>/<path>`), or a single namespace
  covering every Root.
- **How `Withheld` entries appear** (ticket 10): a Snapshot deliberately holds no content for some paths, and a
  filesystem has no vocabulary for "deliberately absent" — a zero-byte file, an absent entry and an `EIO` all lie
  differently.
- **Mount lifetime and ownership**: who holds the mount (the daemon, or a helper process), where mount points live,
  whether they survive daemon restart, and what cleans up a stale mount after a crash. A dead FUSE process leaving
  an unresponsive directory in the user's home is a worse failure than no feature.
- **Read-only enforcement**: how writes are refused, and whether the mount can be made immune to a tool that
  "helpfully" tries to write (an editor writing a swap file into a mounted historical tree).
- **The relationship to export** ([19 — Export](19-export-to-directory.md)): whether mount replaces the export path,
  complements it, or is simply a different presentation of the same read.

## Comments

Deferred past v1 by user decision (2026-09-11). Mounting is a post-release feature, so it is ruled out of this
map's scope and recorded here rather than as a single line on the map, so that the platform constraints above
survive for whoever picks it up. Out-of-scope work does not graduate: if this returns, it returns as a fresh
effort, not as a resumption of this map.
