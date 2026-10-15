# 101: Updating Keepster

**Status:** migrated

**Migrated to:** #37

**What to build:** a person who has Keepster installed can update it to a newer release, and after the update every
part of Keepster that is running is the new version.

Today Keepster has no update of its own: nothing in the code downloads or installs a newer release. The only way to
update is to install again:

- running the install script again (`README.md`: "Running it again upgrades in place"). With no `KEEPSTER_VERSION`
  it installs the latest release, whose version it reads from `releases/latest/download/SHA256SUMS`
  (`scripts/install.sh`, `scripts/install.ps1`);
- or by hand: replacing both binaries from the archive and running `keepster install` again.

What an install over an earlier one does now:

- **The daemon** is started from the new binary. On Linux the script renames each new binary over the old one, and
  `keepster install` runs `systemctl --user restart` (`crates/keepster-service/src/autostart.rs`, the systemd
  `install`). On macOS `keepster install` unloads and loads the agent again (`reload_agent`). On Windows the script
  stops the daemon and every tray running from the install folder before replacing the files, and `keepster install`
  starts the Scheduled Task.
- **The configuration** is left alone. `--no-web-ui` writes one only where there is none
  (`crates/keepster-daemon/src/supervised.rs`, `declare_no_web_ui`).
- **The history** is not migrated (ADR 0010). At start the daemon rewrites only the Registry records of an older
  container version (`Registry::upgrade`).
- **The tray on Linux stays the old version.** Nothing stops the running tray. `tray::install` starts a new one
  (`crates/keepster-service/src/autostart/tray.rs`, the Linux `install`), which exits at once because the old tray
  holds the tray's lock (`crates/keepster-tray/src/main.rs`, `PidLockError::Held`). `keepster install` still prints
  that the tray is running. The old tray runs until it is quit or the session ends. This holds for the script and for
  an update by hand. Found by reading the code; not reproduced.

Package-manager updates (Homebrew, deb, rpm) depend on the packages of ticket 85, which do not exist yet.

- [ ] How a person updates is decided and documented
- [ ] After an update on Linux, the tray running is the new version
