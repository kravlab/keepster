# 86: The web UI drops Snapshots that Retention, Forget or an erasing removed

**Blocked by:** 68 (Install, uninstall, and erasing only on request)

**Status:** migrated

**Migrated to:** #23

**What to build:** when a Root's history loses Snapshots while the web UI shows it, the open view loads that history
afresh, as it already does on `resync`, instead of showing Snapshots that are gone until the page is reloaded.

Today every history view follows the stream through one subscription, `follow` in `ui/src/lib/pages.svelte.ts`, which
asks only for `snapshot.created` (and receives `resync` unasked). Three events that mean "Snapshots of this Root are
gone" pass by unseen:

- `retention.completed`, sent when a Retention pass removed something (ticket 51);
- `job.completed` with `kind` `forget`, after Forget rewrote the history (ticket 81);
- `history.erased`, sent when `keepster purge` erased a Root's history (ticket 68).

A view left open then lists Snapshots the daemon no longer holds, and opening one answers 404. This was read from the
code, not observed in a browser.

- [ ] Each of the three events, for the Root the view shows, reloads that view's history as `resync` does; an event
      for another Root changes nothing
- [ ] What the mode keeps beside the list (a selection, a diff, a file's versions) is dropped where it no longer
      exists, as `onResync` already does for `resync`
- [ ] Tests of the plain modules show each event reloading the list, and one for another Root leaving it alone
- [ ] `ui/dist` is rebuilt and committed

## Comments

- Found by the code review of ticket 68.
