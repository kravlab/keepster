# 104: Restore and Export download the cloud placeholders they look at

**Blocked by:** 75 (Cloud placeholders are never hydrated)

**Status:** migrated

**Migrated to:** #40

**What to build:** a Restore or an Export into a folder synced by OneDrive, iCloud Drive or a similar client never
downloads a file that is only in the cloud merely by looking at what is already where it writes.

Ticket 75 keeps the capture and the watcher from ever reading a placeholder. Restore and Export still read what is
on disk at each destination before they write, and every such read of a placeholder downloads it on Windows, and
on macOS — where the daemon refuses materialisation — fails with `EDEADLK`:

- comparing the file there with the version to write hashes it whole: `file_existing` in
  `crates/keepster-store/src/history/restore.rs` and `existing` in `history/export.rs`
  (`git2::Oid::hash_file`);
- deciding whether a file there could be read back opens it: `unrecoverable_file` and the ancestor check in
  `facts` (`restore.rs`), and `unrecoverable` in `export.rs` (`File::open`);
- deciding whether the Safety Snapshot would hold a file reads every `.gitignore` on the way to it:
  `recoverability_in` (`restore.rs`), which Export shares.

The capture now carries a directory whose `.gitignore` is a placeholder forward and names that file, and carries a
placeholder file forward while it is provably the version held, otherwise Withholding it `cloud_placeholder`. What
Restore should conclude about a placeholder at its destination follows from that, and is not yet decided.

Found while building ticket 75.

- [ ] What Restore and Export report and do for a placeholder at a destination is decided: the same version as
      held, a file whose overwrite the Safety Snapshot cannot recover, or a class of its own
- [ ] A placeholder at a destination, or a `.gitignore` on the way to one, is recognised from its metadata and
      never opened, hashed or read
- [ ] On macOS, a destination the daemon cannot read because materialisation is refused is reported as what it
      is, not as unreadable for an unknown reason
