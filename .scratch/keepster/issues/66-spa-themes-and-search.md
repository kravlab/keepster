# 66: The web UI's themes, appearance settings and search entrance

**Blocked by:** 65 (The web UI's three view modes, and Restore in the browser), 59 (Search across Snapshots — by path
and by content)

**Status:** ready-for-agent

**What to build:** the UI matches the rest of the user's desktop, their appearance choices stick without living in a
repository or in browser storage, and they can search from the header and land on the version they were looking for.

Two themes, defaulting to the system preference with a pin, owning the diff colours and the editor palette so the
merge extension's barely-visible defaults are dropped. The preference lives in a daemon-side store reached through a
public preferences resource — not in the in-tree configuration file, which travels to every clone, and not in the
read-only global configuration, which would leave the in-UI control unable to make anything stick.

- [ ] Light and dark themes exist, the default follows the live system preference, and a pin overrides it
- [ ] The theme owns the diff colours and the editor palette; no third-party editor theme ships
- [ ] Three daemon-wide keys — theme, the syntax line limit and line wrapping — are readable and writable through the
      preferences resource, and editable from the UI with no settings screen
- [ ] Browser storage is used only as a paint cache, so losing it costs one flash rather than the setting, and the
      entry page stays static
- [ ] A header search field offers path and content search, with results in an overlay — not a fourth view mode
- [ ] Picking a result closes the overlay and hands the file and Snapshot to the current mode; a result in another
      Root switches the Root visibly
- [ ] A content search shows progress and a cancel control, because it is streamed and outside the read budget
- [ ] The bundle still fits both size budgets, and the accessibility commitment is keyboard operability with a visible
      focus ring
