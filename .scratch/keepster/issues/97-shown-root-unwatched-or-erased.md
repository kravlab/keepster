# 97: The open history when its Root is unwatched or erased

**Blocked by:** 92 (A live Root list and the Root management page)

**Status:** migrated

**Migrated to:** #33

**What to build:** when the Root whose history is open stops being watched, or loses its history, from this tab, another
tab or the CLI, the History screen stays where it is and says so. Part of spec 91 (user stories 42–48).

On `root.unregistered` for the Root shown, its history stays on screen marked "no longer watched": the reading
endpoints still serve it by identity, so the timeline, content, diffs between Snapshots and search work as before. The
Root picker keeps it, marked, until the developer chooses another Root; it is no longer in `GET /v1/roots`, so the
Root management module keeps its last known record. Restore is disabled with the reason given in all three view modes,
through Restore's shared presentation, because the daemon restores only into a Watched Root. On `history.erased` for
the Root shown, the screen stays and a banner says its history was erased: its button refreshes the history, which
starts over, while the Root is still watched, and leads to the management page once it is not. Nothing switches the
Root shown on its own.

- [ ] `root.unregistered` for the Root shown marks it no longer watched and keeps its history and its picker entry
- [ ] Restore is disabled with its reason for a Root no longer watched, in every view mode
- [ ] `history.erased` for the Root shown shows the banner, with refresh for a watched Root and the management page
      otherwise
- [ ] An event for another Root changes nothing on the screen shown
- [ ] Every new text is a key in the strings module
- [ ] Tests of the Root management module at the `fetch` and `EventSource` seams for each case
- [ ] `ui/dist` is rebuilt and committed, and the size budgets hold
