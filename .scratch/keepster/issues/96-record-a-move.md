# 96: Recording a Move from the web UI

**Blocked by:** 92 (A live Root list and the Root management page), 93 (Registering a directory from the web UI)

**Status:** ready-for-agent

**What to build:** each Root's row on the management page records a Move — where the Root's directory is now — as
`keepster move` does. Part of spec 91 (user stories 5, 33–37).

The dialog says Keepster moves no files and only records where the directory now is, and chooses the new path with
the folder browser and path field ticket 93 built. There is no content check: a Move is never inferred. The request
is `PATCH /v1/roots/{id}`; a refusal (`not_a_directory`, `root_overlaps`, `root_not_found`) is shown with its reason.
A Root whose directory is missing (`root_missing`) puts the Move action first in its row. After success the page stays
where it is, and the live list shows the new path and the state the daemon reports for it.

- [ ] The dialog states that no files are moved
- [ ] The new path is chosen with the shared folder browser or typed
- [ ] `PATCH /v1/roots/{id}` carries the new path; a refusal is shown with its reason
- [ ] A `root_missing` Root offers the Move first
- [ ] Every new text is a key in the strings module
- [ ] Tests of the Move request and its refusals at the Root management module's `fetch` seam
- [ ] `ui/dist` is rebuilt and committed, and the size budgets hold
