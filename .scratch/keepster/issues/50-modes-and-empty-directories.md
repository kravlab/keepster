# 50: Permissions and Empty Directories

**Blocked by:** 48 (Withheld, the metadata sidecar, special files and paths as bytes), 47 (Restore — whole-request
preflight, confirmations and the Safety Snapshot)

**Status:** ready-for-agent

**What to build:** an accidental `chmod` is undoable, and a restored tree still has the directories the tools expect.

All twelve POSIX bits are captured on Unix, held in the sidecar as deviations from the Root's modal defaults, so a
tree-wide permission change costs one small record rather than thousands (ADR 0006). Ownership, ACLs, extended
attributes and Windows security descriptors are out: the daemon is never elevated and could not restore them. An
Empty Directory — one the Snapshot holds no content under, including one whose contents were all excluded — is
recorded, because a directory vanishing without a trace is exactly the silent omission this product refuses.

- [ ] A permission-only change produces a Snapshot: the pass takes the mode from the `stat` it already performs, and
      the watcher `stat`s the path on an attribute event
- [ ] A tree-wide permission change is stored as deviations from the modal defaults, not as one record per path
- [ ] Restore asks by default what to do about modes, answered by a single scalar that the blanket force
      deliberately does not resolve, defaulting from a configuration key
- [ ] The Safety Snapshot needs no new mechanism, because capture records the bits about to be overwritten
- [ ] Windows captures and restores no permission bits beyond the executable bit's tree-mode treatment
- [ ] An Empty Directory is recorded per level of a chain, each keeping its own mode, and detection costs no extra
      system call
- [ ] Recording and recreating are two independent keys, both on by default
- [ ] A restored directory takes its recorded mode where one exists, and the umask only where none does
