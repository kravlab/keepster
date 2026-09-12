# 11 — External git prerequisite: discovery, validation and absence

Type: grilling
Status: resolved

## Question

Store maintenance is delegated to an external `git` binary (ticket 01), and nothing is bundled, so every
installation depends on a binary we do not control. How is that dependency made safe and legible?

Settle:

- **Discovery**: how the daemon locates `git`. Inheriting `PATH` from whatever session launched it is both
  unreliable for a background service and a hijacking risk, so decide between an absolute path in configuration,
  a platform-specific search list, or a first-run detection recorded in the Registry.
- **Validation**: the minimum supported version and how it is checked, plus which plumbing behaviour we depend on
  that has changed across versions.
- **The macOS stub**: `/usr/bin/git` exists even without Command Line Tools and triggers a GUI installer prompt
  when invoked. A background daemon must never cause that, so it has to distinguish the stub from a real git
  before ever running it.
- **Absence**: what happens when git is missing, too old, or not executable — refuse to start, or run Snapshots
  normally while reporting that maintenance is unavailable and the store will grow. Whatever the answer, the user
  must be able to see the state without reading logs.
- **Offering to install it**: when git is absent, whether Keepster offers to install it rather than only
  reporting the gap — and if so, how far that goes. A printed command the user runs themselves, an offer to invoke
  the platform package manager (winget, Homebrew, apt/dnf), or nothing at all. Weigh what it costs: a background
  daemon must never install software silently, package managers need elevation and differ per distribution, and a
  failed half-install is worse than a clear instruction. The `wizard` skill exists for exactly this shape of
  human-in-the-loop step and may be the right answer instead of code.
- **Windows specifics**: how a user is guided to install Git for Windows, and whether a per-user install in a
  non-standard location is discoverable.
- **Documentation**: where the prerequisite is stated so that it is met before the first Snapshot, not after the
  first failed maintenance pass.

## Answer

The external `git` stays a mandatory prerequisite, but it is a *maintenance* prerequisite: capture runs through
in-process `git2` and never needs it. Every decision below follows from that asymmetry — the dependency may cost
housekeeping, never history.

### Discovery: configuration, then a list of stable paths, never `PATH`

An inherited `PATH` is neither reliable nor safe for a service, so it is never read. It is not the login `PATH` in
the first place — `systemd --user` overwrites it (`default_user_PATH()` in systemd's `src/core/manager.c`, expanding
to `/usr/local/bin:/usr/bin` plus `sbin` variants, with `DEFAULT_USER_PATH` a distribution build option), launchd
hands out a minimal one, and a Windows service gets the system environment without `HKCU\Environment\Path`. Reading
it would also make any `git` dropped into a user-writable directory arbitrary code running as the user.

1. `git.path` from configuration — **optional**, absolute, and unconditionally authoritative. If it is set and does
   not work, that is a configuration error, not a reason to silently pick a different git.
2. Otherwise a platform list of **stable** absolute locations. Stability, not existence, is the criterion: on NixOS
   the real binary is `/nix/store/<hash>-git-<ver>/bin/git` and the hash changes on every upgrade, so the list holds
   the documented profile symlinks instead — `/run/current-system/sw/bin`, `~/.nix-profile/bin` (Nix),
   `/run/current-system/profile/bin`, `~/.guix-profile/bin` (Guix) — alongside `/usr/bin`, `/usr/local/bin`,
   `/opt/homebrew/bin`, `/home/linuxbrew/.linuxbrew/bin`, `/opt/local/bin` and `/snap/bin`.
3. **Windows** does not use that list. The path comes from the registry — `HKCU\Software\GitForWindows\InstallPath`
   first, `HKLM` second, both registry views — and the binary invoked is `<InstallPath>\cmd\git.exe`. The Git for
   Windows maintainer states that `cmd\git.exe` is the only supported entry point and that `mingw64\bin\git.exe` is
   not (git-for-windows/git#2283); the registry key was added expressly so third-party installers could find git.
   This keeps a per-user install discoverable even though it writes `HKCU` only, which the service environment does
   not see. A fresh Windows is assumed to have no git at all (Microsoft Learn: "Git isn't yet a default option on
   computers").
4. **macOS** resolves by `stat` alone and never executes a candidate to test it: `/opt/homebrew/bin/git`,
   `/usr/local/bin/git`, `/opt/local/bin/git`, `/Library/Developer/CommandLineTools/usr/bin/git`, then
   `<xcode-select -p>/usr/bin/git`. `/usr/bin/git` is never invoked, and neither is `xcrun`: both are `/usr/bin`
   shims that raise the GUI dialog *"The \"git\" command requires the command line developer tools"*, which a
   background daemon must never trigger. `xcode-select -p` is safe (it is an OS tool, not a CLT trampoline) and the
   `stat` test is exactly what the Homebrew installer uses.

Resolution runs at daemon start and on config reload. Nothing is persisted: the resolved path is reproducible, a
stored one goes stale across a git reinstall, and persisting it would need new global state — which also keeps
ticket 03's rule intact, that stored metadata holds only what git cannot reproduce. The resolved path and version
appear in `keepster status`. As a last resort for exotic setups the CLI, which *does* run in the user's session with
a correct `PATH`, resolves git there and prints a ready-to-paste `git.path = "…"` line. It never writes it: the
shell's knowledge enters the trusted set only through a key the human put there.

### Validation: the version number at startup, reality afterwards

At startup the resolved absolute path is run with `--version` and parsed leniently: require the `git version `
prefix, take the second whitespace token, read the leading digits of its first three dot-separated components, and
discard everything else. `GIT-VERSION-GEN` pipes `git describe` through `sed -e 's/-/./g'`, so non-release builds
legitimately carry four or five components (`2.56.0.rc0.123.g1a2b3c4`, `…dirty`), Git for Windows appends
`.windows.N`, and Apple appends a parenthesised `(Apple Git-137.1)` whose own number contains a dot. A missing or
non-numeric third component is therefore treated as unknown, never as an error, and the parenthesised tail is never
parsed.

No synthetic probe runs. A `gc` rehearsal on a temporary repository would prove git works on *someone else's* data
while saying nothing about our store — its permissions, its size, or concurrent access — so it adds a second source
of truth about the same question instead of replacing the first real pass. The authoritative runtime signal is the
first actual maintenance pass: if it fails, the daemon flips to `maintenance: unavailable` and **the captured stderr
is surfaced in `keepster status`**, not only logged, so the claim is checkable.

### Absence: degrade, with a disk floor

Missing, too old, not executable, or a macOS stub never stops the daemon and never blocks `keepster watch`.
Capture continues; maintenance is marked unavailable with a reason (`missing`, `too_old`, `not_executable`,
`macos_stub`). The state is **global to the daemon**, not a per-Root `Degraded`/`Unavailable` from ticket 05, because
one process has one git. `keepster status` reports the reason, the required minimum, whatever version was found and
the install command, and exits non-zero.

The one real consequence is bounded explicitly: without `gc` nothing is freed, so ticket 06's Retention frees
nothing and its optional disk cap becomes unenforceable by segment deletion. In this state the cap is enforced the
only way left — **capture for that Root pauses** at the cap instead of filling the disk. Refusing to start was
rejected because it trades all history for the absence of a housekeeping tool, and the outage would be silent
exactly when it matters.

### Minimum version: 2.32.0

2.32.0 is where our own requirement lands: `GIT_CONFIG_SYSTEM` and `GIT_CONFIG_GLOBAL`, which the hermetic
invocation below depends on, were introduced there (git RelNotes 2.32.0). It excludes nothing still supported — the
oldest git inside any supported lifetime is 2.34.1 in Ubuntu 22.04 LTS (to May 2027, with no `git` in
`jammy-backports`), followed by Debian 12 bookworm's 2.39.5 (LTS to 2028-06-30, likewise no backport). Only
already-EOL Debian 11 (2.30.2) and contract-only RHEL 8 EUS minors (8.6 = 2.31.1) fall below.

A 2.41.0 floor was rejected: it buys cruft packs by default and costs exactly those two long-lived distributions.
Cruft packs change efficiency, not correctness — `gc --prune=now` expands to `repack --cruft
--cruft-expiration=now` (`builtin/gc.c`), so unreachable objects are freed immediately either way, and pre-2.41 the
same pass merely explodes them into loose objects first. Trading Debian 12 for a faster idle-time batch job is a bad
exchange. The other version temptations cost nothing either: `--no-detach` (2.47) is unnecessary because an explicit
`gc` without `--auto` never daemonises (`builtin/gc.c` applies `gc.autoDetach` only under `--auto`);
`pack-refs --include/--exclude` (2.42) is unnecessary because we pack everything; and `safe.bareRepository`
(2.38, whose default becomes `explicit` in Git 3.0) is already satisfied because every invocation passes
`--git-dir`.

Apple documents the git version in the Command Line Tools nowhere, so compliance on macOS is **measured** by the
startup check, never assumed from a table.

### Invocation: hermetic, and the store is a private data structure

The store is our internal data structure; maintaining it must not change meaning because of someone's
`~/.gitconfig`. A user's `gc.pruneExpire=2.weeks.ago` would silently make ticket 06's Retention free nothing, and
`core.fsmonitor`, `core.pager`, `alias.*`, `core.hooksPath` and `include`/`includeIf` can execute commands. So every
invocation is: the absolute resolved path, **no shell**, an explicit `--git-dir`, system and global configuration
disabled (`GIT_CONFIG_NOSYSTEM=1` plus `GIT_CONFIG_GLOBAL` pointed at an empty file — an empty file rather than
`/dev/null`, whose behaviour on Windows is unverified), and a whitelisted environment so `GIT_DIR`, `GIT_WORK_TREE`
and `GIT_ALTERNATE_OBJECT_DIRECTORIES` cannot leak in from whatever launched the service.

Correctness-critical keys are passed as explicit `-c` flags on **every** invocation, because the command line
outranks both configuration files and environment variables, is immune to later edits of the store's config file,
and makes each logged command self-explanatory. The same values are additionally written into the store's own
`config` when the store is created, so a human who runs `git gc` inside the store by hand gets identical behaviour;
the duplication must live in one place in the code. `core.logAllRefUpdates=false` is written explicitly rather than
relying on the bare-repository default, since ticket 06 depends on reflogs being off for ref deletion to free
objects.

Two prohibitions: `git gc --force` is never used, because it disables the `gc.pid` lock that protects a concurrent
`gc`; and `git maintenance` is not used at all, because its defaults kept moving as late as 2.52 and 2.54 (the
`geometric` strategy became the default), whereas `gc`, `repack` and `pack-refs` are stable.

### Offering to install: print the command, run nothing

When git is absent, Keepster detects the platform family (apt/dnf/pacman/zypper/apk, Homebrew, winget,
`xcode-select --install`) and prints a ready-to-run command plus a link. It never invokes a package manager. An
`install-git` subcommand was rejected: it buys one keystroke at the price of a distribution × package-manager ×
elevation matrix we would own indefinitely, a half-finished install is worse than a clear instruction, and it is the
only option that grants a file-history tool the right to install software. `winget install --id Git.Git -e` is
printed with the caveat that it prompts for elevation in practice even for a user-scope install. When the family is
unrecognised (Nix, Guix, exotic), the output is a link to git-scm.com/downloads plus the reminder that `git.path`
exists. The detected family affects the wording of the hint only, never behaviour.

The `wizard` skill is not the answer here: it earns its place when a human must be walked through a multi-step
third-party dashboard, and this is one command.

### Where the prerequisite is stated

`keepster service install` is **not** a privileged moment — the service may be installed by an exe, msi, dmg or
distribution package, with the CLI never running. So the prerequisite is declared in two layers:

- **Declaratively, wherever the packaging format supports it** (deb/rpm `Depends:`/`Requires:`, Homebrew
  `depends_on "git"`, AUR `depends`). This is the only channel that *solves* the problem instead of reporting it: the
  user never learns git was needed.
- **In the running product**, precisely because dmg and a bare exe cannot express a dependency at all: the
  `status` surface above, a first-run banner in the web UI (the natural destination for someone who installed a
  dmg), and a notice on the first `keepster watch`. One shared renderer produces this block everywhere, including
  `service install`, which prints it and still exits zero — a gate there would be a second policy for the same
  condition, and its inevitable `--ignore-missing-git` escape hatch would be an admission that the gate was wrong.

Documentation states the prerequisite with the minimum version as a number in the README next to installation, on
the per-OS install pages, and in the configuration reference under `git.path`. Which distribution formats v1 ships
belongs to the packaging question, which this decision constrains but does not settle.

### Handed to other tickets

- **Ticket 12 (snapshot write path)**: `git-gc` documents that `--prune=now` "increases the risk of corruption if
  another process is writing to the repository concurrently" — and that other process is our own `git2` hot path.
  Ticket 06 already makes an exclusive per-store lock the precondition for `gc`, but what a capture arriving while
  that lock is held actually does (wait, defer, or coalesce) is ticket 12's to decide. `gc.pid` is not a substitute:
  it guards `gc` against `gc`, and `--force`, which would bypass it, is forbidden above.
- **Ticket 01 (git engine)**: reftable becomes the default ref format for new repositories in Git 3.0 (git RelNotes
  2.51.0), and libgit2/`git2` support for it is unverified. Our stores are created by `git2` with the `files`
  backend, so nothing breaks today, but the hybrid architecture needs that checked before Git 3.0 lands.
