# 27 — Packaging and distribution per OS

Type: grilling
Status: resolved

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

## Answer

**Keepster is an open project with tiered channels, and the universal install mechanism is an archive plus
`keepster install`.** Every package format is a convenience wrapper over that one mechanism rather than a separate
install path with its own behaviour. One package carries both binaries; nothing a package does ever touches
history.

### The audience, and why tiers

v1 is published, but a channel is a per-release obligation rather than a one-off. **Tier 1** is what we build and
verify every release; **tier 2** is what we publish best-effort; and a third list exists that we deliberately do
not own. Full distribution — every package manager, ours, signed, every release — was rejected as turning each
release into a multi-platform operation before the project has its first user. The door stays open, since a tier
is a promise that can be raised.

### Windows: the per-user service does not exist — this amends ticket 05

Ticket 05 chose a **Windows per-user service** for autostart. That choice rests on nothing:

- `CreateService`'s documented `dwServiceType` values do **not** include `SERVICE_USER_OWN_PROCESS` (0x50); the
  type appears only on the read side, in `SERVICE_STATUS`. `sc.exe create` documents no `userown` either. Microsoft
  describes per-user services solely as **its own templates**, instantiated per logon, with nothing said about
  third parties.
- Any SCM write requires administrator rights, so registration would need elevation.
- MSI cannot express it at all — the `ServiceInstall` table permits only 0x10, 0x20 and 0x100 — and MSIX forbids
  it outright: *"MSIX doesn't support per-user Windows services… Instead of a user Windows service, use a
  background task."*

And it does not deliver what it was chosen for: a per-user service **stops and is deleted at sign-out**, so the
logout coverage gap ticket 05 accepted on macOS exists on Windows too.

**Autostart on Windows is a Scheduled Task registered at logon**, created by `keepster install` **without
elevation**. Supervision comes from the Task Scheduler's restart-on-failure, so ticket 05's rule that a running
daemon is a supervised one survives. The `Run` key was rejected for offering no supervision at all, and an
ordinary service running as the user was rejected because it requires storing the user's password. The gain is
real: **installation now requires administrator rights on none of the three platforms**, and the logout gap has
one wording in `status` instead of two.

### Nothing that installs or uninstalls ever deletes history

Package removal takes the binaries, the unit and the autostart entry, and stops. This is both policy and the
platforms' own convention: Debian removes only files it shipped, Fedora forbids packages from touching `$HOME` at
all (*"no Fedora package can contain files or directories or modify files under `/home/$USER`"*), and Inno Setup's
manual says it outright — *"users usually don't appreciate having their data files… deleted without warning… So
again, DON'T DO THIS!"*

The deciding argument is ours, though: ticket 26 made **downgrade** a supported path, and on several channels a
downgrade *is* remove-then-install. An uninstall that erased history would destroy the property ticket 26 just
bought, at the exact moment the user is most likely to want it.

**The rule is therefore promoted above any one format: no install or uninstall path deletes history; only an
explicit command that says so can.** The command set:

- **`keepster install`** — registers the service (Scheduled Task, LaunchAgent, `systemd --user`), enables linger
  where it applies, and installs the tray autostart. `--no-tray` skips the tray; `--no-web-ui` (ticket 24) passes
  through; on a machine with no graphical session it says so rather than silently skipping.
- **`keepster uninstall`** — removes exactly what `install` created, plus the runtime directory. History,
  registry and configuration are untouched. It prints the package manager's own removal command, because on a
  packaged install the binaries are not ours to remove.
- **`keepster uninstall --history` / `--roots` / `--purge`** — additionally erase the History Stores, the
  Registry, or everything including configuration.
- **`--yes`** supplies consent separately from selection, because ticket 07 already required explicit `confirm`
  for `forget`, the tool's other irreversible command, and one mistyped flag should not erase history.
- **`keepster purge <root>` and `keepster purge --all`** — erase history while leaving the Root registered and
  watched, which is "start the history over". Mechanically free: the next capture opens a fresh Segment from a
  parentless commit, as ADR 0001 already provides.
- **`keepster unwatch --all [--purge]`** — the existing per-Root command gains an all-Roots form.

`--roots` without `--history` leaves orphaned stores, which needs no new mechanism: ticket 07 already pins the last
Snapshot at deregistration and lets Retention decay the store. With the daemon uninstalled, Retention simply never
runs and they sit untouched, which is the honest outcome.

### The package ships files; the user registers

No registration happens behind the user's back. A system package installs as root and cannot know which user it is
acting for: Debian's debhelper enables a user unit through `deb-systemd-helper --user`, which writes
`/etc/systemd/user/*.wants` — that is `systemctl --global enable`, i.e. **for every user on the machine**,
including those who never heard of Keepster. Fedora cannot even do that: its user-unit preset is
`/usr/lib/systemd/user-preset/99-default-disable.preset` containing `disable *`, owned by `fedora-release`, and a
third-party package cannot change it. So "the package enables it" is both wrong in posture and inconsistent in
practice.

A daemon that reads every version of every watched file and can overwrite the working tree should not start
existing because a package was installed. Ticket 21 already made "service never installed" a state distinct from
"stopped", so `keepster status` on a fresh install says exactly that and names the command.

**Linger is why this lands well rather than merely acceptably.** polkit's
`org.freedesktop.login1.set-self-linger` is `allow_any: yes`, so `keepster install` can enable linger for the
invoking user with no prompt, while `set-user-linger` is `auth_admin_keep` — a package could not do it for anyone
even if it tried.

### One package, both binaries

The usual reason to split a GUI component into its own package is dependency weight, and ticket 21 removed that
when it chose `tray-icon` with `default-features = false, features = ["ksni"]` plus `notify-rust` on zbus: the
Linux tray links **no GTK, no GLib and no X11**, only D-Bus. Splitting would instead mean the base install is
silent when the insurance fails, because ticket 21 made the tray the sole carrier of notifications — the cost
ticket 21 refused to pay when it required `status` to report a muted channel.

`dist` agrees mechanically: two `[[bin]]` in one package is one "App", and both binaries land in every archive and
installer, with *"no way to override this behaviour"*.

### What ships

**Tier 1, built and verified every release:**

- **Archives** (`.tar.xz`, `.zip`) for Linux x86_64 and aarch64, macOS, Windows x86_64 — plus the shell and
  PowerShell installers `dist` generates.
- **A Homebrew formula in our own tap.** Not homebrew-core, which rejects third-party prebuilt binaries. This
  keeps ticket 21's macOS decision intact, since a tap formula is as unquarantined as any other formula.

**Tier 2, published best-effort:** **deb and rpm**, because they are the only formats that can declare the `git`
prerequisite ticket 11 requires — `Depends: git` on Debian and **`Requires: git-core`** on Fedora, where `git` is
a metapackage and `git-core` owns `/usr/bin/git`; `Requires: /usr/bin/git` is the more robust spelling. The user
unit goes to `/usr/lib/systemd/user/`, which is the documented home for *"user units installed by the
distribution package manager"*, and the tray's autostart entry to `/etc/xdg/autostart/`.

**No MSI.** After the two decisions above, a Windows installer has no work left: the service is a Scheduled Task
the user creates, and the Start-menu shortcut carrying the AUMID — which is what makes a toast display at all —
is one `IShellLink` + `IPropertyStore` + `IPersistFile` sequence that `keepster install` performs itself, on
Microsoft's own documented per-user path. An MSI would add a Windows CI runner and WiX v3 to produce a shortcut we
already create.

**Ruled out, each on a blocking fact rather than taste:**

- **Flatpak.** `build-finish` exports exactly eight subdirectories and `lib/systemd/user` is not among them, so a
  systemd user unit cannot be installed at all. The background portal compares compositor state against running
  instances **every minute** and *"if the permission is NO, we kill it"*. Host `git` requires
  `--talk-name=org.freedesktop.Flatpak`, a documented total sandbox escape. And Flathub's written policy rejects
  console software, tray-only applications, host-dependent applications and system utilities — all four apply.
- **Snap, strict.** The `home` interface grants only **non-hidden** files, so a store under `~/.local/share` is
  unreachable; there is no `flatpak-spawn` equivalent for the host's `git`; interfaces are static, so a directory
  the user names later cannot be granted; and `daemon-scope: user` still sits behind an experimental flag the user
  must set. Classic confinement works but costs mandatory Canonical review and full parity only on Ubuntu.
- **AppImage.** No mechanism for a systemd unit, no dependency field at all — the format's rule is to bundle
  everything, which contradicts an external `git` — no autostart concept, and **exactly one** `.desktop` file
  permitted in the root, so daemon and tray cannot both be expressed.

**Community, explicitly not ours:** AUR, nixpkgs, Scoop and Chocolatey. What upstream owes them is small and we
provide it anyway: a stable release tarball and a **committed `Cargo.lock`** (both the Arch Rust guidelines and
nixpkgs' `buildRustPackage` require it, the latter failing the build if it is out of sync).

**One licensing alarm is cancelled.** Ticket 21 flagged `ksni`'s `Unlicense` as a packaging constraint. It is not:
`Unlicense` is a current SPDX identifier, `isOsiApproved: true` and `isFsfLibre: true`, Fedora lists it as
`allowed`, and Debian main already ships it (ripgrep, aho-corasick). Debian binary packages carry no `License:`
field at all — licence lives in `debian/copyright` — while Fedora's `License:` is an SPDX expression and nixpkgs
has `lib.licenses.unlicense`.

### Build inputs: `ui/dist` is committed

Ticket 08 requires that `cargo build` never need node. Of the three shapes available, only one keeps that promise
for a plain `git clone`:

- **Committing the built `ui/dist`** — the choice. Regeneration is an explicit `make ui`, with CI verifying the
  committed output matches a fresh build. Precedent includes rust-lang's own mdBook, which commits node-built
  assets so that `cargo install mdbook` needs no node.
- Gitignoring `dist/` and listing it in `include` with `cargo package --allow-dirty` (the `ring` pattern) keeps the
  repository clean but leaves a fresh checkout unbuildable — the promise half-kept.
- Building or **downloading** in `build.rs` is disqualified by the packaging this ticket exists for: Debian Policy
  §4.9 states that *"required targets must not attempt network access to other hosts"*, and Fedora's mock disables
  build-chroot networking by default.

**The SPA stays embedded in the binary.** Serving it from a directory was considered and rejected: it would add
asset-path resolution across three platforms with a dev/installed split, and create a failure class that does not
exist today — assets disagreeing with the API they talk to, which is real because ticket 14 pins the `/v1`
contract and ticket 16 added `/v1/preferences`. Downloading assets at install time fails offline installs and
distro packaging alike. Committing `dist/` and embedding it are answers to different questions: the first makes a
source build work, the second makes a running binary self-consistent.

One mandatory detail: `rust-embed` must enable **`deterministic-timestamps`**. By default it records each file's
modification time into the binary, which reproducible-builds.org names explicitly as a reproducibility trap.

### Versions are independent, and that is stated rather than implied

The project now carries a product version plus git hash (ticket 05's `status`), an API version fixed at `1`
(ticket 14), and ticket 26's four on-disk format versions. **The product version is the only one a user cites;
every other version is independent by design, and a product major implies nothing about the API or the format.**
The spec says this outright instead of leaving it to be inferred, because the confusion runs both ways: a user
seeing "Keepster 2.0" would assume `/v2` or an unreadable store, and a maintainer would hesitate to raise a
product major for fear of implying a break. Tying them was rejected because ticket 14 promised `/v1` would be
additive and long-lived, so the product would be pinned at major 1 forever or the promise would break.

`status` prints them as labelled rows — build, API, store format — not a column of bare numbers; the store row is
needed anyway for ticket 26 to explain `store_too_new`. The git hash is recorded **in full**: reproducible-builds
warns that an abbreviated hash's length depends on repository size and shallow-clone state, while a full hash is
recommended. `vergen` honours `SOURCE_DATE_EPOCH` and offers an idempotent mode, and no build timestamp is
captured.

### Two build facts worth carrying forward

- **A fully static musl build is now clean**, because `git2` 0.21.0 changed its default features from
  `["ssh","https"]` to `[]`. Keepster needs no libgit2 networking — every remote is out of scope on the map — so
  OpenSSL leaves the build entirely, and with it the one hard obstacle to static linking alongside vendored
  libgit2 and bundled SQLite.
- **macOS binaries must be built on macOS.** The tooling can cross-compile and even notarise from Linux, but
  linking against `MacOSX.sdk` — which vendored libgit2 and bundled SQLite require — is governed by the Xcode SDK
  agreement, which authorises use *"only for execution on an Apple-branded product running macOS"*. Also relevant
  to the release matrix: `x86_64-apple-darwin` was demoted to Tier 2 in Rust 1.90.0, and GitHub no longer offers
  free Intel macOS runners.
