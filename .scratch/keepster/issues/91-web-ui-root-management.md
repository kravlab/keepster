# 91: Managing Roots in the web UI

**Status:** migrated

**Migrated to:** #28

## Problem Statement

A developer who uses the web UI can browse, diff, search and restore history, but cannot change which directories are
under history. Registering a directory, unwatching one, and recording that a Root's directory moved all need a
terminal and `keepster watch`, `unwatch` or `move`. With no Root at all the web UI says only "No Roots are
registered." and offers nothing. A Root whose directory was moved shows a bare `unavailable` with no reason and no way
forward. And the list of Roots the web UI shows is read once, when the page opens: a Root registered, unwatched or
moved from the CLI or another tab stays invisible, or stays listed, until the page is reloaded.

## Solution

The web UI gets a Root management page, reached from the header next to the Root picker and from the empty state. It
lists every Watched Root with its path, its state and the reason behind a state that is not `Watching`, and lets the
developer register a directory, unwatch a Root (optionally erasing its history), and record a Move — the same
operations, with the same consents, as the CLI's `watch`, `unwatch [--purge]` and `move`, through the same `/v1`
endpoints.

A directory is chosen by browsing the daemon's machine through `GET /v1/fs`, or by typing or pasting an absolute path.
Before registering, a Capture Preview runs in the background exactly as `keepster watch` runs one, and the developer
is asked to confirm only when the directory is past what one Root is claimed to hold.

The Root list, on the management page and in the header picker, follows the event stream, so changes made from the
CLI or another tab appear at once. The web UI never opens a screen or switches the Root shown by itself: where an
action or an event makes another place relevant, it shows a banner with a button that goes there.

## User Stories

1. As a developer with no Roots, I want the empty web UI to offer to add a directory, so that history can start
   without a terminal.
2. As a developer, I want a Root management page reached from the header, so that every Root and its state are in one
   place.
3. As a developer, I want the management page to list every Watched Root with its path and state, so that I see at a
   glance what is covered.
4. As a developer, I want a Root that is not `Watching` to show the reason in plain words, so that `degraded` or
   `unavailable` is not a riddle.
5. As a developer whose Root's directory is no longer found, I want its row to say so and suggest recording a Move,
   so that a moved directory has an obvious fix.
6. As a developer, I want to add a directory by browsing folders on the daemon's machine, so that I need not know its
   absolute path by heart.
7. As a developer, I want the folder browser to start in my home directory, so that the usual places are one or two
   clicks away.
8. As a developer, I want hidden folders left out of the browser until I ask for them, so that the list stays
   readable.
9. As a developer, I want folders I cannot list shown as such, so that a dead end is visible before I click it.
10. As a developer, I want to go up to the parent directory in the browser, so that I can reach any folder.
11. As a developer, I want to type or paste an absolute path instead of browsing, so that a path copied from a
    terminal is enough.
12. As a developer, I want a typed path that is not absolute, not a directory or not readable refused with the
    daemon's reason, so that I know what to fix.
13. As a developer, I want a directory that overlaps an existing Root refused with the Root it overlaps named, so
    that I understand why it cannot be added.
14. As a developer, I want the web UI to check what watching a directory would capture before registering it, as the
    CLI does, so that a huge directory is not watched by surprise.
15. As a developer, I want that check to run in the background with visible progress, so that I know the web UI is
    working and not frozen.
16. As a developer adding a directory within what one Root is claimed to hold, I want it registered without any
    further question, so that the common case is one action.
17. As a developer adding a directory past what one Root is claimed to hold, I want the preview's summary shown and
    to be asked whether to watch it anyway, so that the decision is mine and informed.
18. As a developer, I want to decline at that question and have nothing registered, so that backing out is safe.
19. As a developer, I want the preview's failure not to block registering, as in the CLI, so that a preview problem
    does not stop history from starting.
20. As a developer, I want a preview refused because another one is walking to stop the add and say to try again, as
    the CLI does, so that a directory is never watched unseen.
21. As a developer, I want to cancel an add while its preview is walking, so that I can change my mind about a slow
    directory.
22. As a developer who just added a directory, I want to stay on the management page, with the new Root appearing in
    the list, so that the web UI does not move me somewhere I did not ask to go.
23. As a developer who just added a directory, I want a banner saying it was added, with a button that opens its
    history, so that going there is one click when I want it.
24. As a developer who adds a directory while the daemon cannot maintain its stores, I want that banner to warn that
    space will not be reclaimed and how to install `git`, as `keepster watch` does, so that I learn it at the moment
    it matters.
25. As a developer re-adding a directory I unwatched without erasing, I want its earlier history back, so that
    unwatching was not a loss; the daemon already does this.
26. As a developer, I want to stop watching a Root from its row, so that I can end coverage without a terminal.
27. As a developer, I want the unwatch dialog to say the history is kept and its last state pinned, so that I know
    unwatching loses nothing.
28. As a developer, I want an option in the same dialog to also erase the history, so that I can remove a Root
    completely when I mean to.
29. As a developer, I want erasing to need a separate, explicit acknowledgement that it is irreversible, kept apart
    from choosing it, as `--purge` is kept apart from `--yes`, so that one stray click cannot destroy history.
30. As a developer, I want the dialog's button to say what it will do — unwatch, or erase and unwatch — so that the
    consequence is read before it happens.
31. As a developer, I want the button inactive until every acknowledgement the chosen action needs is given, as in
    the Restore dialog, so that an incomplete consent cannot be sent.
32. As a developer, I want a failed unwatch to leave the dialog open with the daemon's reason, so that I can retry or
    back out.
33. As a developer, I want to record a Move from a Root's row, so that a directory I moved keeps its history.
34. As a developer, I want the Move dialog to say that Keepster moves no files and only records where the directory
    is now, so that I do not expect it to move anything.
35. As a developer, I want to choose the new path for a Move with the same folder browser and path field as for
    adding, so that there is one way to name a directory.
36. As a developer, I want a Move to a path that is not a directory, or that overlaps another Root, refused with the
    daemon's reason, so that I know what to fix.
37. As a developer, I want a Root that was `unavailable` because its directory was missing to recover once its Move is
    recorded, so that the fix is visibly a fix.
38. As a developer with the CLI open beside the browser, I want a Root registered, unwatched or moved there to appear
    in the web UI at once, so that the screen is never stale.
39. As a developer with two tabs open, I want an action in one to show in the other, so that both tell the truth.
40. As a developer, I want a Root's state and reason to update live in the list and the header, so that a change in
    coverage is seen when it happens.
41. As a developer, I want the web UI to reload the Root list after the event stream resynchronises, so that missed
    events cannot leave the list wrong.
42. As a developer viewing a Root's history when it is unwatched, from here or elsewhere, I want its history to stay
    on screen marked "no longer watched", so that what I was reading does not vanish.
43. As a developer viewing that history, I want Restore disabled with the reason given, because the daemon restores
    only into a Watched Root, so that a button never promises what will fail.
44. As a developer viewing that history, I want reading, diffs between Snapshots and search to work as before, so that
    the kept history is as usable as the daemon allows.
45. As a developer viewing that history, I want the Root picker to keep showing it, marked, until I choose another
    Root, so that I am not switched away.
46. As a developer viewing a Root's history when it is erased, I want to stay where I am and see a banner saying the
    history was erased, so that I understand why the screen emptied.
47. As a developer whose erased Root is still watched, I want that banner to offer a refresh that shows the history
    starting over, so that I can follow it.
48. As a developer whose erased Root is no longer watched, I want that banner to offer the way to the management
    page, so that I can choose what to look at next.
49. As a developer, I want every text of these screens to come from the web UI's strings, so that wording is changed
    in one place.
50. As a developer, I want the management page operable by keyboard with a visible focus ring, as the rest of the web
    UI is, so that it is usable without a mouse.

## Implementation Decisions

- **No daemon change.** Every operation uses an existing `/v1` endpoint: `GET /v1/roots`, `POST /v1/roots`,
  `DELETE /v1/roots/{id}` with `purge`, `PATCH /v1/roots/{id}`, `GET /v1/fs`, `GET /v1/preview`, `GET /v1/status`,
  and the reading endpoints, which already serve a Root no longer watched by its identity. The embedded SPA gets no
  privileged endpoint, as the spec requires. The OpenAPI document does not change.
- **Vocabulary.** The page and its code speak the domain's terms: register (the UI's "add"), Unwatch, erase history
  (the API's `purge`), Move (the UI's "change path"). `Move` was added to `CONTEXT.md` for this work.
- **A Root management module** (a plain Svelte rune module beside the existing timeline module) owns:
  - the Root list, loaded from `GET /v1/roots` and kept current from the event stream: `root.registered`,
    `root.unregistered`, `root.moved` and `root.state_changed` update it, and `resync` reloads it whole;
  - the Root shown in the History screen when it stops being watched: it stays shown and is marked no longer watched
    until the developer picks another Root; it is not in `GET /v1/roots`, so the module keeps its last known record;
  - the erased state of the Root shown, from `history.erased`, which decides the banner and its button (refresh for a
    Root still watched, the management page for one no longer watched);
  - the add flow as a small state machine: choosing a path, previewing (background `GET /v1/preview` at its default
    depth, which sends the summary alone), asking (only when the summary's `past_ceiling` is not empty), registering,
    done or failed. A preview that fails other than as busy goes on to register, as the CLI does; a busy preview
    (`409`, `preview_busy`) stops with "try again". Cancelling closes the preview stream, which stops the walk;
  - after a successful register, one read of `GET /v1/status`: when store maintenance is unavailable, the "added"
    banner carries the CLI's notice and its `git` guidance;
  - Unwatch with its consent: choosing to erase history and acknowledging that it is irreversible are two separate
    inputs, and the request may go only when the acknowledgement matches the choice — the same separation the CLI
    keeps between `--purge` and `--yes`, and the same "live only when everything is answered" rule as Restore;
  - Move: `PATCH /v1/roots/{id}` with the new path; no content check, because a Move is never inferred and the daemon
    deliberately does no heuristic.
- **A folder browser module** over `GET /v1/fs`, separate because ticket 90 reuses it to choose an Export
  destination: current directory, its parent, its subdirectories, hidden ones left out until asked, unlistable ones
  marked, and a typed absolute path accepted as the place to go or to choose. Its `404` and `409`
  (`not_a_directory`, `directory_unreadable`) are shown as the daemon's reason.
- **Errors** are shown from the Problem Details the daemon returns (`root_overlaps` naming the overlapping Root,
  `not_a_directory`, `root_not_found`, …), through the web UI's existing failure rendering, never by HTTP status
  alone; an unknown code reads as its status, as for every client.
- **A Root's state reason** is rendered from `state_code` through the strings module; `root_missing` gets the
  "not found here — did you move it?" wording and puts the Move action first in its row.
- **Navigation rule.** The web UI never opens a screen or switches the Root shown on its own. After an add the page
  stays where it is; the "added" banner has a button to open the new Root's history. The same banner pattern serves
  the erased history.
- **Restore on a Root no longer watched** is disabled in all three view modes through Restore's shared presentation,
  with the reason shown; the daemon answers `404` for it.
- **Header.** The Root picker gains the entry to the management page and keeps a Root no longer watched, marked, while
  it is the one shown.
- **Strings.** Every new text is a key in the web UI's strings module.
- **`ui/dist`** is rebuilt and committed, and the size budgets (eager ≤ 150 KiB gzip, total ≤ 400 KiB gzip) still
  hold.

## Testing Decisions

- A good test drives a module through its public interface and observes what a person or the daemon would: the
  requests sent (method, URL, body), the list and banners exposed, and which actions are allowed. It does not assert
  on internal fields or call order beyond what the daemon sees.
- **Two seams**, both with `fetch` stubbed and the event stream's `EventSource` driven by hand:
  - the Root management module: the list following each `root.*` event and `resync`; an add within the claim
    registering without a question; an add past the claim asking, registering on yes and registering nothing on no;
    a failed preview still registering; a busy preview stopping; cancel stopping the preview; the maintenance notice
    present only when status says maintenance is unavailable; Unwatch sending no `purge` without the choice, refusing
    to send `purge` without the acknowledgement, and sending it with both; Move sending the new path and surfacing
    `root_overlaps`; the shown Root marked no longer watched on `root.unregistered`, Restore then not allowed; the
    erased banner choosing refresh or the management page; an event for another Root changing nothing about the shown
    one;
  - the folder browser module: listing the home directory by default, going to a subdirectory and to the parent,
    hidden folders left out until asked, a typed path listed, and `404`/`409` surfaced with the daemon's reason.
- **Prior art:** the timeline module's tests (fetch stub plus a hand-driven `EventSource`), the API client's tests
  (asserting the URLs asked), and the Restore logic's tests (consent and readiness as plain functions).
- No component tests, visual snapshots or browser E2E, as the spec rules out; the release browser smoke test is
  unchanged.

## Out of Scope

- Everything else the CLI can do and the web UI cannot yet: ticket 90 (status, `unwatch --all`, `purge` without
  unwatching, the list of Roots no longer watched, snapshot, reconcile, diff against the Working Tree, export, explain,
  preview as a screen, configuration view, pins, maintenance and jobs, Forget, token rotation, log level, mute,
  diagnostics).
- Editing a Root's configuration from the web UI; ticket 90 records it as needing a spec decision first.
- A check that the new path of a Move holds the same tree.
- Any daemon or `/v1` change.
- Reloading an open history on Retention or Forget; that is ticket 86. This ticket covers `history.erased` only for
  the banner it decides.

## Further Notes

- Decided in the `/grill-with-docs` session of 2026-10-14; the owner's UI rule — no self-navigation, a banner with a
  button instead — applies to every later web UI ticket too.
- `GET /v1/fs` is documented as choosing where an Export writes; this ticket uses it unchanged to choose a directory to
  register or move to.
- The diff against the Working Tree does not exist in the web UI yet; ticket 90 adds it, disabled for a Root no longer
  watched.

## Comments
