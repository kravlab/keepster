# 100: Windows names Withheld under one lossy text — two records at one path

**Blocked by:** 74 (Paths as bytes — a name that is not UTF-8 survives the round trip)

**Status:** needs-triage

**What to build:** on Windows, names that cannot be held because they contain an unpaired surrogate never put two
records at the same path in a Snapshot, and never collide with a file the Snapshot holds under the same text.

Ticket 74 Withholds such a name as `unrepresentable_path` under its lossy text, as ticket 10 decided: the walk takes
`to_string_lossy` of the name and records it (`crates/keepster-store/src/history.rs`, the walk's `name_bytes` branch).
Different names can produce the same text. `\uD800x.t` and `\uD801x.t` both become `�x.t`, and a real file can
be named `�x.t` too. `Walked::withhold` pushes its record without checking for a collision. A directory can
therefore produce two Withheld records at one path, or a held file and a Withheld record at the same path. What the
sidecar, the listing and the diff then do with that has not been checked: these names exist only on Windows, so
only CI can test it.

Found by the closing review of ticket 74.

- [ ] Two such names in one directory produce one Withheld record at their shared lossy path, or another stated
      outcome, never two
- [ ] Such a name next to a held file with the same text leaves the held file's entry, content and history intact
- [ ] A `#[cfg(windows)]` test covers both cases
