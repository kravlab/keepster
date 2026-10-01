//! The `/v1` wire types.
//!
//! This crate deliberately knows nothing of the domain: it holds the shapes the daemon serves and
//! the CLI and third-party clients parse, so that the contract can stay additive without dragging
//! domain changes onto the wire (ticket 08).

use serde::{Deserialize, Serialize};

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
pub struct Status {
    /// The daemon itself.
    pub daemon: DaemonStatus,
    /// One entry per Watched Root, in registration order.
    pub roots: Vec<Root>,
}

/// The daemon's own rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct RegisterRoot {
    /// The directory to watch. Made absolute by the daemon; not resolved through symlinks.
    pub path: String,
}

/// A Snapshot, as the API serves it.
///
/// Ticket 39 widens this with what the timeline needs — the Trigger that fired, the incomplete flag,
/// the changed-path count — each of which arrives with the behaviour that can determine it. Adding a
/// field is an additive change inside `/v1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The Snapshot's stable identity, which outlives the commit that holds it.
    pub id: String,
    /// The Segment it belongs to, named for the period it falls in.
    pub segment: String,
    /// When it was taken, in UTC, as RFC 3339.
    pub taken_at: String,
    /// Whether some file that was meant to be captured could not be read.
    ///
    /// A Snapshot is taken anyway — an honest partial Snapshot beats no Snapshot (ticket 05) — so every
    /// shape that returns one has to be able to say that it is not whole, and to name what it omitted.
    pub incomplete: bool,
    /// The paths that could not be read, each one the reason this Snapshot is incomplete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<String>,
}

/// A failure, in the one shape every endpoint uses.
///
/// RFC 9457 Problem Details plus a `code` extension: a standard shape is what a third-party client's
/// library already parses, and `code` — not the prose of `title` or `detail` — is the stable part
/// clients branch on. The code set is deliberately **open** (ADR 0005): a client must treat a code it
/// does not know as its HTTP status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    /// A short, human-readable summary of the problem type.
    pub title: String,
    /// The HTTP status code, repeated in the body as RFC 9457 prescribes.
    pub status: u16,
    /// A human-readable explanation of this occurrence.
    pub detail: String,
    /// The stable machine-readable code. Contract; the prose above is not.
    pub code: String,
    /// Machine-readable particulars — which path, which id, which limit — so a client composes its own
    /// message instead of parsing ours.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub details: std::collections::BTreeMap<String, String>,
}

/// What a Root's state means for the insurance it provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
