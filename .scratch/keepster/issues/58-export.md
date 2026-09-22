# 58: Export a version into a directory of the user's choosing

**Blocked by:** 47 (Restore — whole-request preflight, confirmations and the Safety Snapshot), 48 (Withheld, the
metadata sidecar, special files and paths as bytes)

**Status:** ready-for-agent

**What to build:** a user can put an old version, or a whole Snapshot, into a directory they choose — so they can
look at an old state beside the current one without overwriting anything — and can also download one file from the
browser.

Export is a daemon-side write to a destination the client names: the browser may name one because it always runs on
the daemon's machine, which keeps the endpoint unprivileged. A streamed archive endpoint survives for the download
gesture and for handing someone one file, with its size shown before the click, and it can never resume.

- [ ] Exporting writes the chosen paths into the given destination, preserving the Snapshot's permission bits
- [ ] A synchronous preflight reports totals and supports a dry run; the write itself is a job on the existing job
      resource
- [ ] The preflight reuses Restore's rules minus the three that are Restore-only, and adds an explicit overwrite
      confirmation, off by default
- [ ] A destination inside a Root is allowed only behind its own confirmation and takes a Safety Snapshot when it
      overwrites; a destination inside a History Store is refused
- [ ] A Withheld path is absent from the output and reported as skipped with its reason, never as a zero-byte file
- [ ] A directory-picking browse endpoint lists directories only, so a web client can choose a destination
- [ ] The streamed archive endpoint serves the browser download gesture, states its size beforehand, and handles
      large files
- [ ] `keepster export` mirrors it, and a diff written to a file is unaffected by the shell's own encoding
