# 74: Paths as bytes — a name that is not UTF-8 survives the round trip

**Blocked by:** 48 (Withheld, the metadata sidecar, special files and incomplete records)

**Status:** done

**What to build:** a file whose name is not valid UTF-8 is captured, listed, read back, diffed and restored like any
other, instead of being reported as unreadable and leaving every Snapshot of its Root incomplete.

Split out of ticket 48, whose other half built the sidecar this one writes non-UTF-8 paths into. `RootPath` is a
`String` today, so the change runs through every crate: the path type, the walk, the derived index's path columns
(a schema bump is a free rebuild, ADR 0010), the sidecar's path field (C-quoted as git quotes such names), and every
endpoint's path in and out.

- [x] Tree names, the sidecar and the derived index hold a name's exact bytes; a Unix name that is not UTF-8 is
      captured like any other and no longer marks the Snapshot incomplete
- [x] The `path` query parameter carries percent-encoded bytes on every endpoint that takes one
- [x] JSON carries `path` as lossy UTF-8 plus `path_bytes` (base64url) whenever the name is not UTF-8, in every
      shape that names a path — Restore's `resolutions` keys included
- [x] On Windows, a name holding an unpaired surrogate is Withheld `unrepresentable_path` rather than stored
- [x] The CLI accepts and prints such names, and `keepster cat` and `keepster restore` reach them

## Comments

- 2026-10-11: split from ticket 48 with the user. Ticket 10 decided the wire form (lines ~145-149); the internal
  representation of `RootPath` and whether the index's path columns become `BLOB` are this ticket's to decide.
  libgit2 passes raw bytes at the C level but `bytes2path` panics on Windows, so `add_all`, `remove_all` and
  `update_all` stay unused (ticket 12).
- 2026-10-15: done. `RootPath` holds bytes, ordered bytewise; the index's path columns are `BLOB` (schema 7, a
  free rebuild); the sidecar writes a byte that is not UTF-8 as `\ooo`, as git quotes a name. `path=` is read by its
  own extractor from the raw query, so `%FF` reaches the handler as the byte. Every JSON field naming a path gains an
  additive `_bytes` companion, present only where the text cannot carry the bytes: a scalar `X` gains `X_bytes`, a
  list gains one entry per path (`null` for UTF-8), `resolutions` gains `resolutions_bytes` keyed by base64url, and
  `Problem.details` gains `path_bytes`; a request's companion wins over its text. A zip entry with such a name drops
  bit 11. Rules match a path as its lossy text: a pattern cannot name the stray bytes, and `*`, `?` and `[!…]`
  match their U+FFFD. The CLI takes paths and `PATH=` flags as `OsStr` bytes and prints names as their bytes on
  Unix; the web UI holds a path as one string with each stray byte as the lone surrogate U+DC80+(byte−0x80).
  oasdiff reads the optional `path_bytes` on the two NDJSON streams' untitled `oneOf` records as new variants;
  `scripts/check-api-ignore.txt` ignores those two false positives. Absolute paths that are not UTF-8 (a Root's own
  path, an Export's `dest`) are ticket 99.
