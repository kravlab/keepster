# 76: Windows links — junctions, mount points and links that cannot be created

**Blocked by:** 49 (Symlinks — stored as links, optionally followed)

**Status:** migrated

**Migrated to:** #15

**What to build:** Windows' several kinds of link surrogate each behave predictably: a junction is captured and
restored as a junction, a volume mount point or a WSL symlink is Withheld rather than misread, and a symlink the
account cannot create is refused with its own reason instead of being written as a plain file.

Split out of ticket 49, which stores links as links on every platform. What remains is Windows-only and needs the
reparse-point FFI that lives with the rest of the workspace's `unsafe` in `keepster-service` (ticket 10's
"Windows links" section is the design).

- [ ] A junction — reported by Rust std as a symlink — is stored as a link with a `j` record in the sidecar (ADR
      0009 reserved the letter), obeys the same follow rules, and is restored as a junction, which needs no privilege
- [ ] Volume mount points and WSL symlinks, whose target `read_link` cannot read, are Withheld `special_file` with
      kind `mount_point` / `wsl_symlink`
- [ ] A symlink that cannot be created (no Developer Mode, no `SeCreateSymbolicLinkPrivilege`) is refused with
      `symlink_unsupported` and never written as a plain file; its flavour comes from the Snapshot (the target is a
      directory in it), then from the target on disk, else file

## Comments

- 2026-10-11: from ticket 49's review. Until this ticket, Rust std reports junctions and mount points as
  symlinks: a junction is stored as a link with its target text, but a mount point's target cannot be read,
  so it is named `i <error>` on every pass and keeps its Root retrying at the one-hour ceiling. A link is
  restored with `symlink_file` whatever it points at, so a directory link comes back as a file link; the
  flavour rule above is the fix.
