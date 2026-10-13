# 68: Install, uninstall, and erasing only on request

**Blocked by:** 61 (Autostart on three platforms, and one supervised daemon), 64 (The web UI's foundation — embedded
assets, the session, the rail and the diff), 67 (The tray process, and notifications when the insurance stops)

**Status:** done

**What to build:** a person registers autostart themselves with one command, removes it later with another, and never
loses their history unless they ask for that explicitly.

The universal install mechanism is an archive plus `keepster install`; every package format will be a wrapper over it
(ticket 85). **No install or uninstall path ever deletes history**, because a downgrade is a supported path and on
several channels a downgrade is remove-then-install.

- [x] `keepster install` and `keepster uninstall` create and remove exactly the autostart entries and the runtime
      directory, leaving history, the Registry and configuration untouched; `keepster service` keeps only `start`,
      `stop`, `restart` and `status`
- [x] Nothing registers behind the user's back: a machine where `keepster install` never ran reports "service never
      installed" naming that command
- [x] On Windows `keepster install` creates a Start-menu shortcut named `Keepster.lnk` in the per-user `Programs`
      folder, carrying the AppUserModelID `Keepster.Tray` (the tray, ticket 67, shows toasts only while that shortcut
      exists, and reports `no_app_id` otherwise), and `keepster uninstall` removes it
- [x] Erasing is only ever explicit: the uninstall flags for history, Roots and everything, the purge command per Root
      and for all, and the all-Roots unwatch form, each with consent supplied separately from selection
- [x] Erasing reaches a Root that is no longer watched, as the API already allows

## Comments

- **Split (2026-10-13).** By the owner's decision distribution comes last, once there is something to distribute.
  Packages, the release pipeline, the smoke browser test and the bench tripwire at release, printing the package
  manager's removal command, and documenting the versions moved to ticket 85.
