# 92: A live Root list and the Root management page

**Blocked by:** None (can start immediately)

**Status:** done

**What to build:** the web UI's Root list stops being read once at page load and follows the daemon instead, and a
Root management page shows it. Part of spec 91 (user stories 2–5, 38–41, 49, 50).

The list moves out of the application shell into a Root management module (the prefactor the rest of spec 91 builds
on), loaded from `GET /v1/roots` and kept current from the event stream: `root.registered`, `root.unregistered`,
`root.moved` and `root.state_changed` update it, and `resync` reloads it whole. The header's Root picker and the new
page both read it. The page lists every Watched Root with its path, its state and, for a state that is not
`Watching`, the reason rendered from `state_code`; a Root whose directory is missing (`root_missing`) says it is not
found at that path and asks whether it was moved. The page is reached from the header beside the Root picker and from
the empty state, which offers it instead of only saying no Root is registered.

- [x] The Root list follows each `root.*` event and reloads on `resync`; an event never selects another Root on its
      own
- [x] The management page lists every Root with path, state and the reason behind a state that is not `Watching`
- [x] `root_missing` reads as "not found here — moved?"
- [x] The page is reached from the header and from the empty state, and is keyboard operable with a visible focus ring
- [x] Every new text is a key in the strings module
- [x] Tests of the Root management module, with `fetch` stubbed and the `EventSource` driven by hand, cover each event,
      `resync`, and an event for one Root leaving the others alone
- [x] `ui/dist` is rebuilt and committed, and the size budgets hold
