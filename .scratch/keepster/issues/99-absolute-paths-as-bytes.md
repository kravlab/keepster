# 99: Absolute paths that are not UTF-8 — a Root, a folder, an export destination

**Blocked by:** 74 (Paths as bytes — a name that is not UTF-8 survives the round trip)

**Status:** needs-triage

**What to build:** a folder whose own path is not valid UTF-8 can be watched, picked, exported to and previewed like
any other, instead of being shown lossily or left out.

Ticket 74 makes the names inside a Root bytes on every endpoint. The absolute paths of places on the machine stay
text: the Root's own path is registered and reported through `String` (`Root.path` is filled with
`to_string_lossy`, `crates/keepster-daemon/src/roots.rs`), the folder picker leaves out a directory whose name is not
UTF-8 (`crates/keepster-daemon/src/directories.rs`), and an export's `dest` and the Capture Preview's `path` are read
as text. A Root under such a folder is reported under a name with U+FFFD, which no request can name back.

Found while studying ticket 74.

- [ ] A Root whose path is not UTF-8 can be registered, listed, moved and unwatched, by the CLI and the web UI
- [ ] The folder picker lists such a directory and the web UI can register it
- [ ] An export can be written to such a destination, and the Capture Preview can preview such a folder
- [ ] On the wire such a path carries the same `path_bytes` companion ticket 74 gives names inside a Root
