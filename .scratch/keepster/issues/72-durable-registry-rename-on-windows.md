# 72: A Root's record survives a power cut on Windows as it does on Unix

**Blocked by:** none

**Status:** ready-for-agent

**What to build:** registering, moving or unwatching a Root on Windows is as durable as on Linux and macOS: once the
command has answered, a power cut cannot bring back the previous `root.toml`.

The Registry replaces a record by writing `root.toml.new`, flushing it, and renaming it over `root.toml`. On Unix it
then flushes the directory, because the rename changes a directory entry, and without that flush a crash right after
it can leave the old name on disk although the new contents reached it. Windows cannot open a directory as a file
(`File::open` answers "Access is denied"), so since `1b01106` the directory flush runs on Unix only. What remains on
Windows is atomic — NTFS journals the rename, so a reader sees the old record or the new one, never a torn one — but
not durable: a power cut within moments of a registration can bring back the previous record, and the operation then
has to be repeated. History is not affected; only the Registry's last change is.

The Windows way to make the rename itself durable is `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING |
MOVEFILE_WRITE_THROUGH`, which returns only once the move has been flushed. It needs a direct Windows API call, which
belongs in `keepster-service` beside the other Win32 code, so every `unsafe` block in the workspace stays in one
module.

- [ ] On Windows, replacing a record renames through `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING |
      MOVEFILE_WRITE_THROUGH`; on Unix the rename and directory flush stay as they are
- [ ] The Win32 call lives in `keepster-service` with its `unsafe` confined there, and `keepster-store` reaches it
      through a safe function
- [ ] The existing registry tests — including the one that reads a record while it is being rewritten — pass on all
      three platforms in the merge gate
- [ ] The comment in `registry.rs` that explains why Windows skips the directory flush says what replaces it

## Comments

- Found while making the merge gate green on Windows (`1b01106`). Durability under power loss cannot be asserted by a
  test without a crash rig; the gate can only show that the write path works and stays atomic, so the evidence for
  the write-through flag is its documented contract.
