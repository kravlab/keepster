# 75: Cloud placeholders are never hydrated

**Blocked by:** 48 (Withheld, the metadata sidecar, special files and incomplete records)

**Status:** ready-for-agent

**What to build:** a capture of a folder synced by OneDrive, iCloud Drive or a similar client never downloads a file
that is only in the cloud, and never loses the version it already holds of one.

Split out of ticket 48, which reserved the sidecar's `p` record for this. Materialisation is disabled where the
platform allows it — Windows' `RtlSetProcessPlaceholderCompatibilityMode` with `PHCM_EXPOSE_PLACEHOLDERS`, macOS's
`setiopolicy_np` with `IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES` — and a placeholder is recognised from the file
attributes the walk reads (`FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` / `FILE_ATTRIBUTE_OFFLINE`, `SF_DATALESS`). The
calls are `unsafe` and live in `keepster-service`.

- [ ] Materialisation is disabled for the daemon's process on Windows and macOS before any capture runs
- [ ] A placeholder whose size equals its last capture's and whose mtime equals the `p` record carries that version
      forward, with no read
- [ ] A placeholder with no version to carry forward — never local, or changed remotely — is Withheld
      `cloud_placeholder`
- [ ] The `p` record is written so a file dehydrated after its capture is not Withheld for ever, and it does not
      churn: it changes only when the file does
- [ ] Linux, where no such API exists, is decided and documented

## Comments

- 2026-10-11: split from ticket 48 with the user. Hydration may be announced by no event; Reconciliation is the
  backstop (ticket 23).
