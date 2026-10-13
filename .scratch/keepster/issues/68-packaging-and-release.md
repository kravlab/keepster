# 68: Packaging, install and the release pipeline

**Blocked by:** 61 (Autostart on three platforms, and one supervised daemon), 64 (The web UI's foundation — embedded
assets, the session, the rail and the diff), 67 (The tray process, and notifications when the insurance stops)

**Status:** ready-for-agent

**What to build:** a person can install Keepster on Linux, macOS or Windows from a released artifact, register
autostart themselves, and remove it later without ever losing their history.

Packaging is tiered over **one** universal mechanism — an archive plus the install command — with every package format
a wrapper over it rather than a separate install path. **No install or uninstall path ever deletes history**, because
a downgrade is a supported path and on several channels a downgrade is remove-then-install.

- [ ] A tag produces archives for the documented targets with shell and PowerShell installers, checksums and a
      release containing them
- [ ] A Homebrew formula in our own tap installs both binaries, ad-hoc signed on macOS
- [ ] deb and rpm packages declare the external `git` prerequisite with each distribution's correct package name, and
      place the user unit and the tray autostart entry in the documented locations
- [ ] One package carries both binaries, so a base install is never silent when the insurance fails
- [ ] The Windows installer creates a Start-menu shortcut named `Keepster.lnk`, in the per-user or the all-users
      `Programs` folder, carrying the AppUserModelID `Keepster.Tray`: the tray (ticket 67) shows toasts only while
      that shortcut exists, and reports `no_app_id` otherwise
- [ ] The package ships files and the **user** registers: nothing enables a service behind the user's back, and a
      fresh install reports "service never installed" naming the command
- [ ] `keepster install` and `keepster uninstall` create and remove exactly the autostart entries, leaving history,
      the Registry and configuration untouched, and print the package manager's own removal command
- [ ] Erasing is only ever explicit: the uninstall flags for history, Roots and everything, the purge command per Root
      and for all, and the all-Roots unwatch form, each with consent supplied separately from selection
- [ ] The release pipeline runs on a tag, and the smoke browser test and the bench tripwire are release-time rather
      than merge-time
- [ ] The product, API and store-format versions are documented as independent, and the recorded build hash is full
