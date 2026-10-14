# 85: Distribution — packages and the release pipeline

**Blocked by:** 68 (Install, uninstall, and erasing only on request), 69 (The performance harness), 70 (The report
layer), 71 (Capturing a repository's own git configuration), 72 (A Root's record survives a power cut on Windows),
73 (A configuration file caught half-saved), 74 (Paths as bytes), 75 (Cloud placeholders), 76 (Windows links),
77 (A Root registered through a link), 78 (Empty Directories), 79 (The Segment Period), 80 (The optional per-Root disk
cap), 82 (Service-manager conformance on three platforms), 84 (A graceful stop of the daemon on Windows), 86 (The
web UI drops Snapshots that Retention, Forget or an erasing removed)

**Status:** ready-for-agent

**What to build:** a person can install Keepster on Linux, macOS or Windows from a released artifact, and remove it
later with their package manager's own command.

Packaging is tiered over **one** universal mechanism — an archive plus `keepster install` (ticket 68) — with every
package format a wrapper over it rather than a separate install path. Distribution comes last, once there is something
to distribute (split from ticket 68).

- [x] A tag produces archives for the documented targets with shell and PowerShell installers, checksums and a
      release containing them
- [ ] A Homebrew formula in our own tap installs both binaries, ad-hoc signed on macOS
- [ ] deb and rpm packages declare the external `git` prerequisite with each distribution's correct package name, and
      place the user unit and the tray autostart entry in the documented locations
- [ ] One package carries both binaries, so a base install is never silent when the insurance fails
- [ ] The package ships files and the **user** registers: nothing enables a service behind the user's back
- [ ] `keepster uninstall` prints the package manager's own removal command
- [ ] The release pipeline runs on a tag, and the smoke browser test and the bench tripwire are release-time rather
      than merge-time
- [ ] The product, API and store-format versions are documented as independent, and the recorded build hash is full

## Comments

- **Found while planning ticket 68 (2026-10-13), facts only:**
  - `dist` treats a package as one app and names its archives after the package, while `keepster` is built by the
    `keepster-daemon` package and `keepster-tray` by `keepster-tray`.
  - The tray reports `no_bundle` outside an `.app`, and the Homebrew formula `dist` generates installs binaries only:
    its documentation lists app bundles (casks) as unsupported.
  - `repository` in `Cargo.toml` points at Bitbucket, and `dist` takes its release hosting from that field.
  - The GitHub repository is private, and a private repository's release files cannot be downloaded without
    authentication.
  - `Cargo.toml` declares `MIT OR Apache-2.0`, and the repository holds only `LICENSE` (MIT).
  - Neither the smoke browser test (ticket 31) nor `make bench` (ticket 69) exists yet.
  - The full build hash is already recorded (`crates/keepster-daemon/build.rs`) and checked by
    `crates/keepster-daemon/tests/daemon/version.rs`; what is missing is the user-facing documentation.
- **Release 0.1.0 (2026-10-14), by the owner's decision ahead of the blockers above, the workflow only:**
  `.github/workflows/release.yml` builds on a `v*` tag an archive per target from ticket 27 (Linux x86_64 and
  aarch64 static musl, macOS aarch64, Windows x86_64; `.tar.xz`, `.zip` on Windows), each holding `keepster`,
  `keepster-tray` and `LICENSE`, and publishes them with `SHA256SUMS` as a GitHub release; run by hand it builds the
  same archives and publishes nothing. It is hand-written, not `dist`, for the two `dist` facts above. Still open
  here: the shell and PowerShell installers, the Homebrew tap, deb and rpm, the uninstall hint, the release-time
  smoke test and bench tripwire, and the version documentation.
- **The install scripts (2026-10-14):** `scripts/install.sh` (Linux x86_64 and aarch64, macOS arm64) and
  `scripts/install.ps1` (Windows x64) download their release's archive, check it against `SHA256SUMS`, put both
  binaries side by side in `~/.local/bin` or `%LOCALAPPDATA%\Programs\Keepster` and run `keepster install` from there,
  passing their arguments and exiting with its code; PATH is left alone. `release.yml` writes the version into them,
  publishes them beside the archives and first runs them against the archives just built; `merge-gate.yml` runs
  their tests against a faked release (`scripts/test-install.sh`, `scripts/test-install.ps1`). The repository is
  public now, so the release's files download without authentication; the fact above about a private repository no
  longer holds.
