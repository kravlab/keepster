# 78: Empty Directories

**Blocked by:** 50 (Permissions)

**Status:** migrated

**Migrated to:** #17

**What to build:** a restored tree still has the directories the tools expect. An Empty Directory — one the
Snapshot holds no content under, including one whose contents were all excluded or Withheld — is recorded, because a
directory vanishing without a trace is exactly the silent omission this product refuses.

Split out of ticket 50, which captures the modes these records carry. Ticket 25's answer is the design: a `d` record
in `.keepster-meta` per level of a chain, detected from the sorted index by a prefix comparison at no system call's
cost, with Reconciliation as the backstop.

- [ ] An Empty Directory is recorded per level of a chain, each keeping its own mode, and detection costs no extra
      system call
- [ ] Recording and recreating are two independent keys, `empty_dirs` and `restore.empty_dirs`, both on by default
- [ ] A restored directory takes its recorded mode where one exists, and the umask only where none does — parents
      created for a file included
- [ ] A Restore of a directory puts back the recorded mode of every directory already there whose mode differs,
      asked about as `modes_differ` like a file's — ticket 50 captures directory modes, and a diff and the
      timeline show a directory's change of mode, but its Restore writes files only, so `chmod -R` is undone
      for files alone until this lands
- [ ] `entries` lists them from their `d` records, and a diff shows one appearing or disappearing
