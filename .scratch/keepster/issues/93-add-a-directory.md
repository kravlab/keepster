# 93: Registering a directory from the web UI

**Blocked by:** 92 (A live Root list and the Root management page)

**Status:** ready-for-agent

**What to build:** the management page registers a directory as a Watched Root, chosen by browsing the daemon's
machine or by typing a path. Part of spec 91 (user stories 1, 6–13, 22–25).

A folder browser module over `GET /v1/fs`, kept separate from the Root management module because ticket 90 reuses it
for Export: it starts at the home directory, goes into a subdirectory and up to the parent, leaves hidden folders out
until asked, marks folders that cannot be listed, and accepts a typed or pasted absolute path; `404` and `409`
(`not_a_directory`, `directory_unreadable`) are shown with the daemon's reason. Registering is `POST /v1/roots`; a
refusal (`root_overlaps` naming the Root it overlaps, `not_a_directory`, …) is shown from its Problem Details and
leaves the dialog open. After success the page stays where it is, the new Root appears through the live list, and a
banner says it was added, with a button that opens its history. The banner also carries the CLI's notice when
`GET /v1/status` says store maintenance is unavailable, with its `git` guidance. A directory unwatched earlier
without erasing comes back with its history, as the daemon already does.

- [ ] The folder browser lists the home directory by default, moves down and up, hides hidden folders until asked,
      marks unlistable ones and accepts a typed absolute path
- [ ] Registering sends `POST /v1/roots` with the chosen path and shows the daemon's refusal with its reason
- [ ] After success nothing is opened or switched; the "added" banner's button opens the new Root's history
- [ ] The maintenance notice appears in the banner only when status says maintenance is unavailable
- [ ] Every new text is a key in the strings module
- [ ] Tests of the folder browser module and of the Root management module's register path, at the `fetch` and
      `EventSource` seams
- [ ] `ui/dist` is rebuilt and committed, and the size budgets hold
