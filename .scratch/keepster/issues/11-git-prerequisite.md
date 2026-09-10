# 11 — External git prerequisite: discovery, validation and absence

Type: grilling
Status: open

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
