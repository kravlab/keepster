# 95: Unwatching a Root, optionally erasing its history

**Blocked by:** 92 (A live Root list and the Root management page)

**Status:** migrated

**Migrated to:** #31

**What to build:** each Root's row on the management page can stop watching it, and on explicit consent erase its
history too, as `keepster unwatch [--purge]` does. Part of spec 91 (user stories 26–32).

A modal dialog says the history is kept and its last state pinned. An option in it chooses to also erase the history;
when it is chosen, a second, separate acknowledgement that erasing is irreversible is required, and the button —
reading "unwatch" or "erase history and unwatch" — is live only once every acknowledgement the choice needs is given,
as in the Restore dialog. The request is `DELETE /v1/roots/{id}`, with `purge=true` only when erasing was chosen and
acknowledged. A failure keeps the dialog open with the daemon's reason; success leaves the page where it is, and the
live list drops the Root.

- [ ] Without the erase option the request carries no `purge`
- [ ] With the option and without the acknowledgement nothing can be sent; with both, `purge=true` is sent
- [ ] The button's wording follows the choice
- [ ] A refusal keeps the dialog open with its reason
- [ ] Every new text is a key in the strings module
- [ ] Tests of the consent rule and the request at the Root management module's `fetch` seam
- [ ] `ui/dist` is rebuilt and committed, and the size budgets hold
