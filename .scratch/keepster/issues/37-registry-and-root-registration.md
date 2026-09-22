# 37: The Registry — register, deregister and resolve a Watched Root

**Blocked by:** 36 (Workspace skeleton, the daemon and `keepster status`)

**Status:** ready-for-agent

**What to build:** a user can register any directory as a Watched Root, list their Roots, deregister one, and see
every Root in `keepster status`. Nothing is captured yet; this ticket establishes identity and the Registry.

A Root is identified by a generated ULID that survives renaming or moving the directory, and its path is a mutable
attribute. The Registry is a small daemon-owned file per Root beside its History Store location, replaced
atomically, because the watched path is the one fact git could never reproduce. Roots may not overlap.

- [ ] `keepster watch <dir>` registers a Root and prints its human name (`basename` plus a short id); the API
      accepts the same operation
- [ ] Registering a path inside an existing Root, or containing one, is refused with the overlap error naming the
      offending Root; the same rule applies to a move
- [ ] `keepster roots` and the API list Roots; resolving a path to a Root is its own query
- [ ] Deregistration keeps the History Store by default; erasing it requires the explicit purge form with its own
      confirmation
- [ ] Moving a Root is an explicit operation that changes the path and costs the history nothing
- [ ] The CLI resolves the Root from the nearest registered ancestor of the working directory, accepts a full id, an
      unambiguous id prefix or a path, and errors with a pointer to `watch` when nothing matches
- [ ] Registry files are written atomically and survive a kill mid-write; a rewritten file is never partially
      readable
- [ ] `status` lists each Root with its state, and the exit code becomes non-zero as soon as a Root is not
      `Watching`
