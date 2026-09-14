# 27 — Packaging and distribution per OS

Type: grilling
Status: needs-triage

## Question

What ships, in what format, on each of the three platforms — and what each format declares. This was fog from the
charting session; it is answerable now because every constraint that shaped it has been settled elsewhere.

There are **two artifacts**, not one: the daemon/CLI binary and the tray (ticket 21), the second being a separate
process on every platform and an `.app` bundle on macOS.

Constraints already fixed, which the answer must honour rather than revisit:

- **The external `git` prerequisite** (ticket 11) must be declared as a hard dependency in every format that can
  express one — deb/rpm `Depends`, Homebrew `depends_on` — because a dmg and a bare `.exe` cannot, and those fall
  back to ticket 11's runtime detection surface.
- **macOS**: the tray `.app` is ad-hoc signed and distributed through a **Homebrew formula only** in v1, because
  formulae and bottles carry no quarantine attribute. Any other channel — dmg, cask — requires Developer ID,
  hardened runtime and notarisation, hence Apple Developer Program membership; from 2026-09-01 Homebrew stops
  supporting casks that fail Gatekeeper.
- **Windows**: the installer must create a Start-menu shortcut carrying an AUMID, which is what makes a toast
  display at all (ticket 21), and SmartScreen will prompt on an unsigned binary until reputation accrues — EV
  certificates stopped bypassing that in 2024.
- **Licensing**: `ksni` is `Unlicense` rather than MIT/Apache-2.0, and every bundled dependency's licence must be
  expressible in whatever manifest each format uses.

Settle:

- **Which formats v1 actually ships** per platform, and which are deferred. Shipping fewer, well, beats shipping
  many badly.
- **How the two artifacts relate in each format**: one package containing both, two packages with the tray
  depending on the daemon, or an optional extra.
- **Whether `keepster service install` and `keepster tray install` are run by the package** or left to the user
  after installation. A package that installs a service unit behind the user's back is a different posture from
  one that prints the command.
- **Uninstall**, including whether the History Store survives it. Ticket 07 made `unwatch --purge` the only way to
  erase history; an uninstaller that deletes the store silently would contradict that.
- **Build inputs**: ticket 08 requires `cargo build` never to need node, while `make ui` does. State how a release
  build is produced so that a distributor packaging from source is not forced into a node toolchain.
- **Where version numbers come from**, given ticket 14 pins `info.version = "1"` for the API and ticket 05's
  `status` reports a build version and hash.
