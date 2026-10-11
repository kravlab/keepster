//! The `/v1` wire types.
//!
//! This crate deliberately knows nothing of the domain: it holds the shapes the daemon serves and
//! the CLI and third-party clients parse, so that the contract can stay additive without dragging
//! domain changes onto the wire (ticket 08).

use serde::{Deserialize, Serialize};

mod code;

pub use code::ErrorCode;

/// Percent-encode a path for the `path=` query parameter, which is how every endpoint takes a path.
///
/// Paths travel as bytes (ticket 10), so this encodes bytes rather than characters: a name that is not
/// UTF-8 survives the trip. The unreserved set of RFC 3986 plus `/`, which is a path separator here and
/// needs no escaping.
pub fn encode_path_query(path: &str) -> String {
    path.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The API version this build serves. It is part of the URL prefix and of every version handshake.
pub const API_VERSION: &str = "v1";

/// What a client learns before authenticating: which build it is talking to, and which API it speaks.
///
/// Served on both listeners and, on the loopback listener, before authentication — otherwise a client
/// cannot tell a rejected token from a version mismatch (ticket 07).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VersionInfo {
    /// The product version of this build. The only version a user cites; it implies nothing about
    /// the API version or the store format, which move independently (ticket 27).
    pub build_version: String,
    /// The full commit hash of this build, never abbreviated — an abbreviation's length depends on
    /// repository size and clone depth. `unknown` where the build had no repository to read.
    pub build_hash: String,
    /// The API version, `v1` for this build.
    pub api_version: String,
}

/// The whole health surface, as `GET /v1/status` serves it and as `keepster status --json` prints it
/// verbatim.
///
/// One payload, not two: a parallel CLI schema would double the versioning cost and drift at the
/// first added field (ticket 07). It is built from live daemon state and never from logs, because a
/// dead daemon writes nothing and that is indistinguishable from a quiet healthy one (ticket 05).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Status {
    /// The daemon itself.
    pub daemon: DaemonStatus,
    /// One entry per Watched Root, in registration order.
    pub roots: Vec<Root>,
}

/// The daemon's own rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DaemonStatus {
    /// Which build and which API, the same three fields the version handshake returns.
    #[serde(flatten)]
    pub version: VersionInfo,
    /// The daemon's process id, informational only — the pid file's lock, not its content, is the
    /// single-instance authority (ticket 05).
    pub pid: u32,
    /// How long this daemon has been serving, measured monotonically.
    pub uptime_seconds: u64,
    /// Whether autostart is installed. Filled by ticket 61.
    pub autostart: Reported,
    /// Whether coverage survives logout. Filled by ticket 61, and on macOS it reads as unavailable
    /// because a LaunchAgent dies at logout.
    pub linger: Reported,
    /// Whether store maintenance can run at all, which depends on the external `git`. Filled by
    /// ticket 52.
    pub maintenance: Reported,
}

/// A row whose owning behaviour has not been built yet.
///
/// It exists so that `status` can say "not known" instead of a comfortable default: ticket 05 forbids
/// the tool to look healthier than it is. Each variant below is added by the ticket that can actually
/// determine the fact, and adding one is an additive change to `/v1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Reported {
    /// This build cannot determine the fact yet.
    Unknown,
}

/// A Watched Root, as both the registry endpoints and `status` serve it.
///
/// One shape, not two: ticket 07 keeps a Root's state on the Root itself rather than duplicating it
/// into a second per-Root health resource, so a client learns a Root's health from the same object it
/// learned the Root from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Root {
    /// The Root's identity, which survives renaming or moving its directory.
    pub id: String,
    /// Where its directory is right now. A path is a mutable attribute of the registration.
    pub path: String,
    /// What the daemon can currently promise for it.
    pub state: RootState,
    /// The machine-readable reason behind a state that is not `Watching`, so a client can branch on it
    /// rather than on prose. Absent while the Root is `Watching`.
    pub state_code: Option<String>,
}

/// What a client sends to register a directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RegisterRoot {
    /// The directory to watch. Made absolute by the daemon; not resolved through symlinks.
    pub path: String,
}

/// A Snapshot, as the API serves it — from a capture and from the timeline alike.
///
/// One shape for both, so a client renders the Snapshot it just took with the same code as the ones it
/// pages through. Adding a field is an additive change inside `/v1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Snapshot {
    /// The Snapshot's stable identity, which outlives the commit that holds it.
    pub id: String,
    /// The Segment it belongs to, named for the period it falls in.
    pub segment: String,
    /// When it was taken, in UTC, as RFC 3339, to the millisecond its identity carries.
    pub taken_at: String,
    /// The Trigger that caused it: `manual` for one a person or a script asked for. The set grows
    /// inside `/v1` as Triggers are added, so a client must accept a name it does not know.
    pub trigger: String,
    /// Whether some file that was meant to be captured could not be read.
    ///
    /// A Snapshot is taken anyway — an honest partial Snapshot beats no Snapshot (ticket 05) — so every
    /// shape that returns one has to be able to say that it is not whole.
    pub incomplete: bool,
    /// The paths that could not be read, each one the reason this Snapshot is incomplete.
    ///
    /// Named by the answer to a capture. The timeline carries the flag but not the list, which ticket
    /// 48 makes a record of the Snapshot itself and `entries` the place to read it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<String>,
    /// How many paths differ from the Snapshot before it. Zero is a real answer: nothing changed.
    ///
    /// Absent only from the answer to a capture whose count failed after the Snapshot was already
    /// written — the Snapshot exists, and the timeline carries its count once the index has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_paths: Option<u64>,
    /// With a timeline filtered by `path`, how that path changed in this Snapshot. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
}

/// How one path differs between a Snapshot and the Snapshot before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// The path holds content here and held none before.
    Added,
    /// Its content or its mode differs from before.
    Modified,
    /// It held content before and holds none here.
    Deleted,
}

/// One page of a Root's chronology, as `GET /v1/roots/{id}/snapshots` serves it.
///
/// Newest first, and paged by cursor rather than offset: the list grows at one end and erodes at the
/// other, so an offset would shift under the reader in both directions (ticket 07).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Timeline {
    /// The Snapshots on this page, newest first.
    pub snapshots: Vec<Snapshot>,
    /// The `cursor` that fetches the next, older page; absent on the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// The paths a Snapshot holds under a directory, as `…/entries` serves them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Entries {
    /// One per path, in byte order of the path.
    pub entries: Vec<Entry>,
}

/// One path a Snapshot holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Entry {
    /// Where it is, relative to the Root, `/`-separated.
    pub path: String,
    /// What it is.
    pub kind: EntryKind,
    /// Its size in bytes. Absent for a directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Its permission bits, as a number (`420` is `0o644`). Absent where nothing is recorded; until
    /// ticket 50 a file carries only the executable bit's distinction, `0o644` or `0o755`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    /// Whether the content is binary by git's rule. A display flag only: binary files are stored as
    /// they are (ticket 10). Absent for anything but a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<bool>,
}

/// What kind of thing a Snapshot holds at a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link, held as a link.
    Symlink,
}

/// A failure, in the one shape every endpoint uses.
///
/// RFC 9457 Problem Details plus a `code` extension: a standard shape is what a third-party client's
/// library already parses, and `code` — not the prose of `title` or `detail` — is the stable part
/// clients branch on. The code set is deliberately **open** (ADR 0005): a client must treat a code it
/// does not know as its HTTP status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Problem {
    /// A short, human-readable summary of the problem type.
    pub title: String,
    /// The HTTP status code, repeated in the body as RFC 9457 prescribes.
    pub status: u16,
    /// A human-readable explanation of this occurrence.
    pub detail: String,
    /// The stable machine-readable code. Contract; the prose above is not.
    ///
    /// A string rather than an [`ErrorCode`], because the set is open: a code this build does not know
    /// must still parse, so that a client can fall back to `status` (ADR 0005).
    #[cfg_attr(feature = "openapi", schema(schema_with = code::code_schema))]
    pub code: String,
    /// Machine-readable particulars — which path, which id, which limit — so a client composes its own
    /// message instead of parsing ours.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub details: std::collections::BTreeMap<String, String>,
}

/// What a Root's state means for the insurance it provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RootState {
    /// Captured as promised.
    Watching,
    /// Still watched, but not as promised — its configuration failed to load and an older one is in
    /// force, or its coverage guarantee is switched off.
    Degraded,
    /// Nothing can be captured: the directory is missing, unmounted or unreadable, or its History
    /// Store was written by a version this build cannot open.
    Unavailable,
}
