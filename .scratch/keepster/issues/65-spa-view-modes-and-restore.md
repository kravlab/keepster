# 65: The web UI's three view modes, and Restore in the browser

**Blocked by:** 64 (The web UI's foundation — embedded assets, the session, the rail and the diff), 47 (Restore —
whole-request preflight, confirmations and the Safety Snapshot)

**Status:** ready-for-agent

**What to build:** a user picks the question rather than the screen — "when did this break", "what happened to this
file", "what happened while I was away" — and can restore from any of them through one dialog that never puts an
irreversible action one click away.

Three view modes of **one** screen, defaulting to the rail, carrying the selected file **and** Snapshot across
switches so a switch reads as continuity. Restore's presentation is deliberately not per-mode: a mode decides
navigation, not destructive affordances.

- [ ] The file-first mode offers a tree of the Root and a horizontal scrubber of the Snapshots that touched the chosen
      file
- [ ] The activity feed shows Snapshot cards with their changed paths inline and the diff as an overlay
- [ ] Switching modes carries the file and the Snapshot; arriving with a file but no Snapshot resolves to the newest
      Snapshot that touched it
- [ ] While the per-file filter is on and the selected Snapshot falls out of the list, the selection moves to the
      newest Snapshot the filter still contains
- [ ] A Snapshot that only deleted or only added files says so in words in every mode, never as an empty-looking diff
- [ ] A filter hides routine watcher Snapshots in the rail as well as in the feed
- [ ] All three modes reach the same Restore dialog: the daemon's preflight plan, every confirmation and resolution it
      asks for, and the Safety Snapshot promise on the same panel
- [ ] The file column is a flat list by default with the tree as a view option, and selecting a file the current
      Snapshot never touched says "unchanged here" rather than looking unrelated
- [ ] Restore's preflight handling is unit-tested as a plain module
