# 102: Checking for updates

**Status:** needs-triage

**What to build:** Keepster finds out whether a newer release than the one installed exists, and tells the person.

Today nothing in the code checks for a newer release. A person learns the installed version from `keepster
--version`, and clients learn it from `VersionInfo.build_version` (`crates/keepster-api/src/lib.rs`), which the CLI
and the tray already read.

Facts a design has to take into account:

- The daemon's one network surface is the Loopback Listener on `127.0.0.1` (`CONTEXT.md`). A check for updates would
  be Keepster's first outbound connection.
- The install scripts already learn the latest release's version without an API: from the archive names in
  `releases/latest/download/SHA256SUMS` (`scripts/install.sh`, `version_in_sums`).
- How a person then updates is ticket 101.

- [ ] Who checks, when, and whether the check can be switched off are decided
- [ ] Where Keepster tells the person about a newer release is decided
