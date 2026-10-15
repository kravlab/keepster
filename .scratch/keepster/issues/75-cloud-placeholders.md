# 75: Cloud placeholders are never hydrated

**Blocked by:** 48 (Withheld, the metadata sidecar, special files and incomplete records)

**Status:** done

**What to build:** a capture of a folder synced by OneDrive, iCloud Drive or a similar client never downloads a file
that is only in the cloud, and never loses the version it already holds of one.

Split out of ticket 48, which reserved the sidecar's `p` record for this. Materialisation is disabled where the
platform allows it — Windows' `RtlSetProcessPlaceholderCompatibilityMode` with `PHCM_EXPOSE_PLACEHOLDERS`, macOS's
`setiopolicy_np` with `IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES` — and a placeholder is recognised from the file
attributes the walk reads (`FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` / `FILE_ATTRIBUTE_OFFLINE`, `SF_DATALESS`). The
calls are `unsafe` and live in `keepster-service`.

- [x] Materialisation is disabled for the daemon's process on Windows and macOS before any capture runs
- [x] A placeholder whose size equals its last capture's and whose mtime equals the `p` record carries that version
      forward, with no read
- [x] A placeholder with no version to carry forward — never local, or changed remotely — is Withheld
      `cloud_placeholder`
- [x] The `p` record is written so a file dehydrated after its capture is not Withheld for ever, and it does not
      churn: it changes only when the file does
- [x] Linux, where no such API exists, is decided and documented

## Comments

- 2026-10-11: split from ticket 48 with the user. Hydration may be announced by no event; Reconciliation is the
  backstop (ticket 23).
- 2026-10-15: built with the user's approval of the plan. Correction to the text above: on Windows,
  `PHCM_EXPOSE_PLACEHOLDERS` does not disable hydration. It shows this process placeholders as they are, which
  Microsoft documents as most programs' default, so that the walk sees the attributes. Hydration is avoided only
  by never opening a file carrying `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`, `FILE_ATTRIBUTE_RECALL_ON_OPEN` or
  `FILE_ATTRIBUTE_OFFLINE`. On macOS, `IOPOL_MATERIALIZE_DATALESS_FILES_OFF` makes touching a dataless file's data
  fail with `EDEADLK` (TN3150). `libc` 0.2.189 carries neither `setiopolicy_np` nor `SF_DATALESS`, so both are
  declared from XNU's headers. The daemon sets both at start (`keepster_service::guard_cloud_placeholders`). A
  failure is logged and the daemon starts anyway, because the walk's own check still holds.
- 2026-10-15: a placeholder is recognised from the `stat` the walk takes before any read, and again from the
  open handle before the first byte is read, so a file freed in between is not read either. It is carried
  forward when its size equals the held blob's and its mtime equals the baseline's `p` record, or the stat
  cache's evidence of the read that produced the blob (ADR 0014). Otherwise it is Withheld `cloud_placeholder`.
  The `p` record stays while the path is held, hydrated or not, and counts as no change of its path. A `p` record
  seen for the first time is still something to record, as modes observed for the first time are: the pass that
  first finds a file freed mints one Snapshot with no changed path, even under the daemon's triggers, which
  otherwise record nothing when nothing changed. Later freeing and fetching mint none (ADR 0009). If the index is
  lost after a file is freed and before that pass, the file is Withheld until it is fetched again.
- 2026-10-15: an ignore file that is a placeholder is never read either, since reading it would download it.
  Without its rules nothing beneath its directory can be judged, so the capture carries that directory forward,
  the Root included, and names the ignore file incomplete with `cloud_placeholder`. The watcher leaves its rules
  out and reports more rather than less. The capture and the watcher ask the platform the same way,
  `keepster_core::is_cloud_placeholder`.
- 2026-10-15: a directory carried forward — one that cannot be listed since ticket 05, or one whose ignore file is
  a placeholder — now keeps the baseline's `w`, `i` and `p` records beneath it, decided with the user. Until now
  the sidecar held only the walk's own records, so a carried directory lost them: a Withheld path beneath it was
  no longer named, and a `p` record went, leaving the file Withheld after a lost index.
- 2026-10-15: Linux, decided with the user: nothing is recognised as a placeholder there, and every file is read
  as any other. A FUSE sync client fetches on read and marks nothing beforehand. A dataless directory on macOS
  whose listing fails is treated as any directory that cannot be listed: what the Snapshot held beneath it is
  carried forward and named incomplete. No macOS run has confirmed that the listing fails. Capture Preview cannot
  yet tell a carried placeholder from a Withheld one, and reads a placeholder ignore file: ticket 103. Restore
  and Export still hash, open and read what is at a destination, a placeholder included: ticket 104.
