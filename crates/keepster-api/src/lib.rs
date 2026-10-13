//! The `/v1` wire types.
//!
//! This crate deliberately knows nothing of the domain: it holds the shapes the daemon serves and
//! the CLI and third-party clients parse, so that the contract can stay additive without dragging
//! domain changes onto the wire (ticket 08).

use std::collections::BTreeMap;

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
    /// The on-disk formats this build writes and reads up to. Absent from a daemon older than the
    /// field, which says nothing rather than something false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_format: Option<StoreFormat>,
}

/// The four version numbers of the store, each governing something disjoint (ADR 0010): only the
/// container's can ever be rewritten, history is never migrated, and the index is rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StoreFormat {
    /// The shape of a Root's directory in the store, recorded in its `root.toml` — the one number
    /// rewritten automatically when it moves. A newer one makes the Root `unavailable` with
    /// `store_too_new`.
    pub container: u32,
    /// How Snapshots are laid out in a Segment, recorded with each Segment. Each keeps the one it was
    /// written in; one newer than this build's is listed as unreadable, and capture carries on in a
    /// Segment of this build's own.
    pub content: u32,
    /// The grammar of a Snapshot's metadata sidecar. Its record types only ever grow: one this build
    /// does not know is skipped and reported.
    pub sidecar: u32,
    /// The derived index's schema: any other one is rebuilt, never migrated.
    pub index: u32,
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
    /// Which build, which API and which store format: the fields the version handshake returns.
    #[serde(flatten)]
    pub version: VersionInfo,
    /// The daemon's process id, informational only — the pid file's lock, not its content, is the
    /// single-instance authority (ticket 05).
    pub pid: u32,
    /// How long this daemon has been serving, measured monotonically.
    pub uptime_seconds: u64,
    /// Whether the daemon comes back at login: its autostart unit is installed and enabled (ticket 61).
    pub autostart: Reported,
    /// Why autostart is unavailable: `not_installed` — a state of its own, never read as stopped — or
    /// `disabled`. An open set, as error codes are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autostart_reason: Option<String>,
    /// Whether coverage survives the user's logout (ticket 61).
    pub linger: Reported,
    /// Why it does not: `logout_gap` where the platform ends a user's agents at logout (macOS,
    /// Windows), or `disabled` where linger could be switched on and is not (Linux). An open set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linger_reason: Option<String>,
    /// Whether store maintenance can run at all, which depends on the external `git` (ticket 52). While
    /// it cannot, nothing Retention removes is reclaimed, and `keepster status` exits non-zero.
    pub maintenance: Reported,
    /// The external `git` maintenance runs with, or why there is none.
    pub git: GitStatus,
    /// Whether the daemon has a network surface: the loopback listener the browser reaches it through
    /// (ticket 60). The only place that says so. Switched off, it is a deliberate configuration rather
    /// than a fault, and changes no exit code. Absent from a daemon older than the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener: Option<ListenerStatus>,
    /// The daemon's log: the level it writes at, and a raise while one lasts (ticket 62). Absent from a
    /// daemon older than the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<LogStatus>,
}

/// The daemon's log, as `status` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LogStatus {
    /// The level written at now: `error`, `warn`, `info`, `debug` or `trace`.
    pub level: String,
    /// The level configured by `log.level`, which a raise returns to by itself.
    pub default: String,
    /// When a raised level ends, as an RFC 3339 moment. Present only while a raise lasts: a state that
    /// changes what the daemon does is never silent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// The file the log is written to, when the daemon writes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// A raise of the log level, which always ends: `PUT /v1/log/level`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LogLevelRequest {
    /// The level to write at: `error`, `warn`, `info`, `debug` or `trace`.
    pub level: String,
    /// How long, as a duration such as `30m` or `2h`: at least a second, at most seven days. Required:
    /// a level left raised by forgetting is not a state this API can reach.
    #[serde(default, rename = "for", skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
}

/// The loopback listener: where it is, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ListenerStatus {
    /// Whether it is listening.
    pub state: ListenerState,
    /// Where it listens, `127.0.0.1:<port>`, while it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Why it could not bind, while it `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Whether the loopback listener exists. Decided when the daemon starts, and only a restart changes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ListenerState {
    /// It is listening, at `address`.
    Listening,
    /// `ui.listener` switched it off: the daemon has no network surface, and mints no token.
    Off,
    /// It is switched on and could not bind — most likely another process holds `ui.port`.
    Failed,
}

/// A way into the browser session, from `POST /v1/ui/bootstrap` (ticket 60).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UiBootstrap {
    /// The address to open: the loopback listener with a single-use code, which it exchanges for a
    /// cookie and redirects away from, so the token itself never appears in a URL.
    pub url: String,
}

/// The web UI's preferences: what `GET /v1/preferences` answers and `PUT /v1/preferences` replaces
/// whole (ticket 66, ticket 16).
///
/// Every key is required in a `PUT` today. A key added inside `/v1` will be optional there, an absent
/// one keeping the value stored, so a client older than the key cannot erase it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Preferences {
    /// The palette: `system`, which follows the operating system's light or dark setting, `light` or
    /// `dark`. A string rather than an enum, so that a palette added inside `/v1` is not a breaking
    /// change to a client that reads this.
    pub theme: String,
    /// The longest line, in characters, a diff is still syntax-highlighted with: 1 to 10 000 000.
    // Signed, so that a negative number reaches the range check and is refused as `preference_invalid`
    // rather than as JSON of the wrong shape.
    #[cfg_attr(feature = "openapi", schema(minimum = 1, maximum = 10_000_000))]
    pub syntax_line_limit: i64,
    /// Whether the diff wraps long lines rather than scrolling sideways.
    pub wrap_lines: bool,
}

/// The external `git` store maintenance runs with (ticket 52). Capture never needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GitStatus {
    /// Where it is: found in configuration (`git.path`) or a stable place — never an inherited `PATH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The version it reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The oldest version maintenance runs with.
    pub minimum: String,
    /// Why maintenance cannot run: `missing`, `macos_stub`, `not_executable`, `too_old` or `failed`. An
    /// open set, as error codes are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What running it said, for `not_executable` and `failed`: its error, word for word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// How to install one, while maintenance cannot run. Keepster never runs it itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<InstallHint>,
}

/// How a person installs `git` here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct InstallHint {
    /// The command for this platform's package manager, when it is recognised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// What to know besides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Where to read more.
    pub link: String,
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
    /// It is in place.
    Available,
    /// It is not, and a field beside it says why.
    Unavailable,
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
    /// How many paths changed since the last Snapshot and wait for their debounce window or the rate
    /// limit. A count rather than a listing, because a listing of a debounce queue is stale as it prints
    /// (ticket 20).
    #[serde(default)]
    pub pending_paths: u64,
    /// How long the oldest of those changes has waited, in seconds; absent when nothing is pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_pending_seconds: Option<u64>,
    /// For a Root that follows links (`files.follow_symlinks`), how their targets are covered: by a
    /// pass on an interval of its own rather than by watches. Its own fact, not a coverage gap. Absent
    /// for a Root that follows none, or is not captured automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_targets: Option<FollowTargets>,
    /// How many records of the last Snapshot's metadata a newer Keepster wrote that this build does not
    /// understand: skipped, and not carried into the Snapshots this build takes (ADR 0010). Absent
    /// while there are none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unrecognized_records: Option<u64>,
}

/// How the targets of a Root's followed links are covered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FollowTargets {
    /// How many links the last pass followed.
    pub count: u64,
    /// When a pass last covered them, in UTC, as RFC 3339; absent before the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reconciled_at: Option<String>,
    /// How often they are passed over, in seconds.
    pub interval: u64,
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
    /// The Root-relative paths that were meant to be captured and are not held as they are, each one
    /// the reason this Snapshot is incomplete.
    ///
    /// Named by the answer to a capture. The timeline carries the flag but not the list: each is a
    /// record of the Snapshot itself, which `entries` lists with why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<String>,
    /// Paths present in the Working Tree whose content this Snapshot deliberately does not hold, and
    /// why. Withholding is policy, not failure, so it does not make a Snapshot incomplete.
    ///
    /// Named by the answer to a capture; each is also a record of the Snapshot itself, which `entries`
    /// lists.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub withheld: Vec<Withheld>,
    /// How many paths differ from the Snapshot before it. Zero is a real answer: nothing changed.
    ///
    /// Absent only from the answer to a capture whose count failed after the Snapshot was already
    /// written — the Snapshot exists, and the timeline carries its count once the index has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_paths: Option<u64>,
    /// How many of `changed_paths` it added: content here, none in the Snapshot before. With
    /// `deleted_paths`, what lets a client say "only deleted files" in words; the rest of
    /// `changed_paths` are modifications. Absent exactly when `changed_paths` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_paths: Option<u64>,
    /// How many of `changed_paths` it deleted: content in the Snapshot before, none here. Absent
    /// exactly when `changed_paths` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_paths: Option<u64>,
    /// With a timeline filtered by `path`, how that path changed in this Snapshot. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
    /// Whether it is pinned, and so survives Retention unconditionally until it is unpinned (ADR 0002).
    #[serde(default)]
    pub pinned: bool,
    /// How many paths Forget took from it; absent for a Snapshot Forget never touched. One with nothing
    /// left in it is kept, and this is what says why it is empty (ticket 81).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forgotten_paths: Option<u64>,
}

/// A path a Snapshot holds no content for, on purpose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Withheld {
    /// Where it is, relative to the Root.
    pub path: String,
    /// Why its content was not held, and for a special file what kind it is.
    #[serde(flatten)]
    pub why: WithheldReason,
}

/// Why a Snapshot holds no content for a path that is there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WithheldReason {
    /// `too_large`, `special_file`, `reserved_name` (the directory's own `.git`, or `.keepster-meta` at
    /// its top), `unrepresentable_path`, `other_root`, `history_store` or `cloud_placeholder`.
    ///
    /// A string rather than a closed enum, for the reason ADR 0005 gives for error codes: the set grows
    /// inside `/v1`, so a client must accept a reason it does not know.
    pub reason: String,
    /// With `special_file`: `fifo`, `socket`, `char_device` or `block_device` — an open set too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// Whether one path is captured, and which rule decided it — the answer of `GET …/explain`.
///
/// The analogue of `git check-ignore -v`, and the answer to "why wasn't my file saved": it names the
/// rule a person wrote, at which level and on which line, rather than describing a computation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Explanation {
    /// The path asked about, relative to the Root.
    pub path: String,
    /// Whether a Snapshot taken now would hold its content.
    pub captured: bool,
    /// What a capture does with it.
    pub decision: Decision,
    /// With `withheld`, why: `too_large`, `reserved_name` (the directory's own `.git`), `special_file`
    /// (a FIFO, socket or device), `history_store` or `other_root`. An open set, as `Withheld::reason`
    /// is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The rule behind the decision: the exclude rule, the pruning ancestor's exclude rule, the
    /// denylist entry, the per-pattern cap — or, for a captured path, the include or opt-in that let it
    /// in. Absent when no rule was involved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleReference>,
    /// With `pruned`, the excluded ancestor directory that is never entered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pruned_by: Option<String>,
    /// The last of the user's rules matching the path itself, reported even when an ancestor prunes it
    /// or the denylist refuses it — so the answer can say what that rule would have done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own_rule: Option<RuleReference>,
    /// What is at the path in the Working Tree right now; absent when nothing is. A path need not exist
    /// to be explained, and one that does not is explained as a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_disk: Option<OnDiskKind>,
    /// Its size, when it is a file on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The size cap that applies to it, in bytes.
    pub max_size: u64,
    /// Where the scalar cap was set — the built-in default, or a configuration file's `max_size`. A
    /// per-pattern cap that applies instead is named in `rule`.
    pub max_size_origin: Origin,
    /// What the configuration defines but almost certainly does not mean — a negation an excluded
    /// ancestor makes unreachable, a key a Root's own file may not set. None of them makes the Root
    /// `Degraded`.
    pub warnings: Vec<ConfigWarning>,
}

/// What a capture does with one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// It enters the Snapshot.
    Captured,
    /// A rule excludes it.
    Excluded,
    /// An excluded ancestor directory is never entered, so no rule beneath it is evaluated.
    Pruned,
    /// The built-in secret denylist refuses it, and no opt-in in the global configuration admits it.
    Secret,
    /// It is there, but its content is not held; `reason` says why.
    Withheld,
}

/// What is at a path in the Working Tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum OnDiskKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link.
    Symlink,
    /// A FIFO, socket or device.
    Special,
}

/// One rule, named the way a person wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RuleReference {
    /// The pattern as written, `!` included where it was written.
    pub pattern: String,
    /// Whether it excludes or re-includes what it matches.
    pub effect: RuleEffect,
    /// Where it was written.
    #[serde(flatten)]
    pub origin: Origin,
}

/// Where a rule or a value was set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Origin {
    /// The level it was set at.
    pub level: ConfigLevel,
    /// The file it was written in, or a description of a built-in value.
    pub source: String,
    /// Its line in that file, where it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// The levels configuration is layered in, from the least specific to the most — the provenance enum
/// of ticket 04, plus the watched directory's own ignore files, which sit beneath everything the user
/// writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ConfigLevel {
    /// Built into Keepster.
    Default,
    /// A watched directory's own `.gitignore`, honoured only on request.
    IgnoreFile,
    /// The global configuration.
    Global,
    /// A pattern block of the global configuration.
    GlobalPattern,
    /// A Root's own block of the global configuration.
    GlobalRoot,
    /// The Root's own `.keepster.toml`.
    Root,
    /// What Forget excluded from now on: written by the daemon, above every level a person writes.
    Forget,
}

/// What a rule does to the paths it matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RuleEffect {
    /// It keeps them out of history.
    Exclude,
    /// It lets them back in.
    Include,
}

/// Something about the configuration that is defined but almost certainly not what was meant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConfigWarning {
    /// What kind of warning: `negation_unreachable` or `key_ignored`. An open set.
    pub code: String,
    /// What it means here, for a person.
    pub message: String,
}

/// What a client sends to restore — `POST /v1/roots/{id}/restore` (ticket 47).
///
/// Restore only overwrites and creates; it never deletes. The whole request is checked before anything is
/// written: a path that needs a confirmation or a resolution, or cannot be restored, stops it whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RestoreRequest {
    /// The Snapshot whose versions are written, by identity.
    pub from: String,
    /// Root-relative paths; a directory means everything beneath it, and none means the whole Snapshot.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Plan only: answer with what would happen to every path, write nothing and take no Safety
    /// Snapshot.
    #[serde(default)]
    pub dry_run: bool,
    /// The structural changes agreed to: a list, or `"all"` for the blanket force — which settles every
    /// confirmation and no resolution.
    #[serde(default)]
    pub confirm: Confirm,
    /// Answers to what only the user can settle, keyed by the Snapshot path they are for.
    #[serde(default)]
    pub resolutions: std::collections::BTreeMap<String, PathResolution>,
    /// Skip each path still in the way, with its reasons, rather than stop the request.
    #[serde(default)]
    pub skip_unresolvable: bool,
    /// For every file whose permission bits on disk are not the Snapshot's (`modes_differ`) and that
    /// has no answer of its own in `resolutions`: take the Snapshot's, or keep those on disk. Absent,
    /// the Root's `restore.modes` setting answers — by default it asks. The blanket force never does
    /// (ticket 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<ModesChoice>,
}

/// Which permission bits a restored file ends up with, where the Snapshot's and the disk's differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ModesChoice {
    /// The Snapshot's.
    Snapshot,
    /// The ones on disk now — without setuid and setgid when the content is replaced, since those were
    /// granted to the old content.
    Keep,
}

/// The confirmations a Restore request gives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum Confirm {
    /// `"all"`: every confirmation.
    All(ConfirmAll),
    /// These.
    These(Vec<Confirmation>),
}

impl Default for Confirm {
    fn default() -> Self {
        Self::These(Vec::new())
    }
}

/// The blanket force's spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum ConfirmAll {
    /// Every confirmation.
    #[serde(rename = "all")]
    All,
}

/// A structural change a Restore must be told it may make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Confirmation {
    /// Create directories that are not there.
    CreateParents,
    /// Replace something of another kind: a file where a directory is needed, a link or a special file.
    ReplaceType,
    /// Replace a file that has other names, which keep the old content.
    BreakHardlinks,
    /// Overwrite a file whose current content the Safety Snapshot cannot hold.
    OverwriteUnrecoverable,
    /// Export: write over what is already at the destination. Outside a Root nothing brings it back.
    Overwrite,
    /// Export: write into a Watched Root, which captures what is written.
    DestInRoot,
}

/// The answer to one path's question; exactly one field is set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PathResolution {
    /// For `name_collision`: write this path, and skip the others that are one file with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<bool>,
    /// For `name_invalid`: write the file under this bare name in the same directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_to: Option<String>,
    /// For `name_case`: keep the name already on disk, or take the Snapshot's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_case: Option<NameCaseChoice>,
    /// For `modes_differ`: take the Snapshot's permission bits, or keep those on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<ModesChoice>,
}

/// Which name a file restored over a name in another case keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum NameCaseChoice {
    /// The name on disk.
    KeepExisting,
    /// The Snapshot's.
    UseSnapshot,
}

/// What a Restore did — or, for a dry run, would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Restored {
    /// The Snapshot to restore from to undo this Restore: taken of the state just before it, or the
    /// latest one when the Working Tree was identical to it. Absent from a dry run. A file this Restore
    /// created is not removed by restoring it, since Restore never deletes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safety_snapshot: Option<String>,
    /// Whether this was only the plan.
    pub dry_run: bool,
    /// Every file of the request, in path order.
    pub paths: Vec<RestoredPath>,
}

/// One file of a Restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RestoredPath {
    /// The file's path in the Snapshot.
    pub path: String,
    /// Where it is written, when that is not `path`: a rename, or a name in another case kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// What happened to it — or, in a dry run, what would.
    pub outcome: RestoreOutcome,
    /// For a `blocked` or `skipped` path, every reason, and what would settle each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<RestoreIssue>,
    /// For a `failed` path, why. What was there before is left as it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What happened to one file of a Restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RestoreOutcome {
    /// A file in place was replaced by the Snapshot's version.
    Restored,
    /// The file was created.
    Created,
    /// The file already held the version.
    Unchanged,
    /// Passed over: a collision's other paths, or what `skip_unresolvable` let go.
    Skipped,
    /// It could not be written; nothing of it was left half-done.
    Failed,
    /// It needs a confirmation or a resolution, or cannot be restored: the request stops.
    Blocked,
}

/// Why a file of a Restore is not simply written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RestoreIssue {
    /// The reason: `parent_missing`, `type_change`, `hardlinked`, `overwrite_unrecoverable`,
    /// `name_collision`, `name_invalid`, `name_case`, `modes_differ`, `type_conflict`,
    /// `link_check_failed` or `withheld`. An open set, as error codes are.
    pub reason: String,
    /// The confirmation that settles it, for the four that one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
    /// The resolution that settles it — `write`, `rename_to`, `name_case` or `modes` — for the four only
    /// the user can answer. Neither this nor `confirm` for a path that cannot be restored at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// For `type_change` and `link_check_failed`: where, Root-relative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// For `hardlinked`: the file's other names inside the Root. For `name_collision`: the other
    /// Snapshot paths that are one file with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// For `hardlinked`: how many more names the file has outside the Root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links_outside: Option<u64>,
    /// For `name_case`: the name on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing: Option<String>,
    /// For `overwrite_unrecoverable`: why — `too_large`, `excluded`, `unreadable` or `unsupported`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// For `modes_differ`: the Snapshot's permission bits, as a number (`384` is `0o600`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_mode: Option<u32>,
    /// For `modes_differ`: the permission bits on disk now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_mode: Option<u32>,
}

/// The `to` of a diff that names the Working Tree rather than a Snapshot.
pub const WORKING_TREE: &str = "working";

/// What changed from a Snapshot to another Snapshot or the Working Tree — the answer of
/// `GET /v1/roots/{id}/diff` (ticket 46).
///
/// Computed by the daemon, which holds both sides, so binary detection, rename detection and the size
/// past which no hunks are computed are decided once and every client shows the same lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Diff {
    /// The older side: a Snapshot's identity.
    pub from: String,
    /// The newer side: a Snapshot's identity, or `working` for the Working Tree as the next Snapshot
    /// would hold it.
    pub to: String,
    /// Without `path`, every changed file in path order and without hunks; with it, that one file with
    /// its hunks.
    pub files: Vec<DiffFile>,
    /// Working Tree files that could not be read. Each keeps its last captured content, as a capture
    /// keeps it, so the comparison is not whole there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<String>,
}

/// One file's part of a diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DiffFile {
    /// Where the file is on the newer side — or, for a deletion, where it was. Relative to the Root. A
    /// directory is named only when its permission bits are all that changed there.
    pub path: String,
    /// Where a renamed file was on the older side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// How it changed; absent for a path asked about that did not change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<FileChange>,
    /// Whether either version is binary, by git's rule: no hunks are computed for it.
    pub binary: bool,
    /// Whether either version is larger than the daemon diffs, 8 MiB: no hunks are computed for it, and
    /// it is not probed for being binary.
    pub too_large: bool,
    /// Why the next Snapshot would not hold a file that is still in the Working Tree — `too_large` in
    /// this version — which is why it reads as deleted, and is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withheld: Option<String>,
    /// The file's permission bits on the older side, all twelve as `entries` gives them in
    /// `permissions` — present, with `new_mode`, when they changed, alone or with the content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_mode: Option<u32>,
    /// The file's mode on the newer side, when it differs from `old_mode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_mode: Option<u32>,
    /// The changed lines with their context. Empty in a summary, and for a binary or too large file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hunks: Vec<DiffHunk>,
}

/// How a file changed between the two sides of a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum FileChange {
    /// Only on the newer side.
    Added,
    /// On both sides, different.
    Modified,
    /// Only on the older side.
    Deleted,
    /// Moved, perhaps edited too: one change, with `old_path` saying where from.
    Renamed,
}

/// One run of changed lines with the context around it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DiffHunk {
    /// The run's first line on the older side, counting from 1.
    pub old_start: u32,
    /// How many lines of the older side it covers.
    pub old_lines: u32,
    /// The run's first line on the newer side, counting from 1.
    pub new_start: u32,
    /// How many lines of the newer side it covers.
    pub new_lines: u32,
    /// Its lines, in order.
    pub lines: Vec<DiffLine>,
}

/// One line of a hunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DiffLine {
    /// Which side it is on.
    pub kind: LineKind,
    /// The line without its ending. Text that is not UTF-8 is shown with replacement characters; the
    /// unified format carries the exact bytes.
    pub text: String,
    /// `false` for a file's last line when the file ends without a newline; absent otherwise.
    #[serde(default = "always", skip_serializing_if = "is_true")]
    pub newline: bool,
}

/// Which side of a diff a line is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    /// On both sides.
    Context,
    /// Only on the newer side.
    Added,
    /// Only on the older side.
    Deleted,
}

fn always() -> bool {
    true
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's `skip_serializing_if` hands the field by reference"
)]
fn is_true(value: &bool) -> bool {
    *value
}

/// One event on the live stream, `GET /v1/events` (ticket 45).
///
/// Payloads are thin on purpose — identities and flags, never content: a client wanting detail fetches it
/// through the endpoint that already serves that shape, so no second rendering of a Root or a Snapshot
/// exists to drift. The set grows inside `/v1` as behaviour is added (retention, maintenance, jobs), so a
/// client must pass over an event whose name it does not know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "event")]
pub enum Event {
    /// A Snapshot was written, by whichever Trigger.
    #[serde(rename = "snapshot.created")]
    SnapshotCreated {
        /// The Root it belongs to.
        root: String,
        /// The Snapshot's identity.
        snapshot: String,
        /// Whether some file meant to be captured could not be read.
        incomplete: bool,
        /// How many paths differ from the Snapshot before it; absent when the count failed after the
        /// Snapshot was written.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        changed_paths: Option<u64>,
    },
    /// A directory became a Watched Root.
    #[serde(rename = "root.registered")]
    RootRegistered {
        /// The new Root.
        root: String,
    },
    /// A Root was deregistered.
    #[serde(rename = "root.unregistered")]
    RootUnregistered {
        /// The Root that is no longer watched.
        root: String,
        /// Whether its History Store was erased with it.
        purged: bool,
    },
    /// A Root's directory was recorded at a new path.
    #[serde(rename = "root.moved")]
    RootMoved {
        /// The Root that moved.
        root: String,
    },
    /// What the daemon can promise for a Root changed.
    #[serde(rename = "root.state_changed")]
    RootStateChanged {
        /// The Root.
        root: String,
        /// Its state now.
        state: RootState,
        /// The reason behind a state that is not `watching`, as `GET /v1/roots/{id}` gives it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state_code: Option<String>,
    },
    /// A Root's configuration in force changed: a file was edited and every file it depends on is
    /// valid.
    #[serde(rename = "config.reloaded")]
    ConfigReloaded {
        /// The Root whose configuration changed.
        root: String,
    },
    /// A file a Root's configuration depends on became invalid, so the last known-good version stays
    /// in force for it — or, without one, the Root is not captured.
    #[serde(rename = "config.invalid")]
    ConfigInvalid {
        /// The Root the invalid file affects.
        root: String,
    },
    /// A Restore wrote what it was asked to, as far as it could.
    #[serde(rename = "restore.completed")]
    RestoreCompleted {
        /// The Root.
        root: String,
        /// Its Safety Snapshot, which undoes it.
        safety_snapshot: String,
        /// How many files were replaced.
        restored: u64,
        /// How many were created.
        created: u64,
        /// How many could not be written.
        failed: u64,
    },
    /// A Retention pass removed Snapshots the ladder no longer keeps (ticket 51). Sent only when it
    /// removed something; a client showing the timeline refetches it.
    #[serde(rename = "retention.completed")]
    RetentionCompleted {
        /// The Root.
        root: String,
        /// How many Segments were deleted whole.
        deleted_segments: u64,
        /// How many were re-chained with fewer Snapshots.
        thinned_segments: u64,
        /// How many Snapshots are gone.
        removed_snapshots: u64,
    },
    /// A Maintenance Pass reclaimed what nothing references any more and packed the rest (ticket 52).
    #[serde(rename = "maintenance.completed")]
    MaintenanceCompleted {
        /// The Root.
        root: String,
    },
    /// A job left the queue and is under way (ticket 54).
    #[serde(rename = "job.started")]
    JobStarted {
        /// The job's identity.
        job: String,
        /// What it does: `maintenance`. The set grows inside `/v1`.
        kind: String,
        /// The Root it works on.
        root: String,
    },
    /// A job got further: `done` of `total` steps, in units of its own.
    #[serde(rename = "job.progress")]
    JobProgress {
        /// The job's identity.
        job: String,
        /// How many steps are done.
        done: u64,
        /// How many there are.
        total: u64,
    },
    /// A job ended; `GET /v1/jobs/{id}` has its result or what went wrong.
    #[serde(rename = "job.completed")]
    JobCompleted {
        /// The job's identity.
        job: String,
        /// What it did.
        kind: String,
        /// The Root it worked on.
        root: String,
        /// How it ended: `succeeded` or `failed`.
        state: JobState,
    },
    /// The events since the client's `Last-Event-ID` cannot be replayed: they were evicted, the daemon
    /// restarted, or the client fell too far behind. Whatever it shows may be stale, so it refetches.
    #[serde(rename = "resync")]
    Resync,
}

impl Event {
    /// The event's name, which is also the SSE `event:` field it is sent under.
    pub fn name(&self) -> &'static str {
        match self {
            Self::SnapshotCreated { .. } => "snapshot.created",
            Self::RootRegistered { .. } => "root.registered",
            Self::RootUnregistered { .. } => "root.unregistered",
            Self::RootMoved { .. } => "root.moved",
            Self::RootStateChanged { .. } => "root.state_changed",
            Self::ConfigReloaded { .. } => "config.reloaded",
            Self::ConfigInvalid { .. } => "config.invalid",
            Self::RestoreCompleted { .. } => "restore.completed",
            Self::RetentionCompleted { .. } => "retention.completed",
            Self::MaintenanceCompleted { .. } => "maintenance.completed",
            Self::JobStarted { .. } => "job.started",
            Self::JobProgress { .. } => "job.progress",
            Self::JobCompleted { .. } => "job.completed",
            Self::Resync => "resync",
        }
    }
}

/// Where a job is in its life: queued, then running, then succeeded or failed — once, in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    /// Accepted, waiting for its store — behind a capture in flight, say.
    Queued,
    /// Under way.
    Running,
    /// Done as asked; `result` says what it did.
    Succeeded,
    /// Ended without doing what was asked; `error` says why.
    Failed,
}

/// A job just accepted, as `202` answers it; `Location` names the same job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JobAccepted {
    /// The job's identity, by which `GET /v1/jobs/{id}` reports it.
    pub job_id: String,
}

/// A long or irreversible operation, as `GET /v1/jobs/{id}` reports it (ticket 54, ticket 07). Jobs live
/// in the daemon's memory: a restart forgets them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Job {
    /// The job's identity.
    pub id: String,
    /// What it does: `maintenance`. The set grows inside `/v1`, so a client must accept a kind it does
    /// not know.
    pub kind: String,
    /// The Root it works on.
    pub root: String,
    /// Where it is in its life.
    pub state: JobState,
    /// How far it got, for a job that counts its steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<JobProgress>,
    /// What it did, once it succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<JobResult>,
    /// Why it failed, in the shape of every other failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Problem>,
}

/// How far a job got: `done` of `total` steps, in units of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JobProgress {
    /// How many steps are done.
    pub done: u64,
    /// How many there are.
    pub total: u64,
}

/// What a job did. Each kind fills in what it measures, so every field is optional and more arrive
/// inside `/v1` as kinds are added.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JobResult {
    /// A Maintenance Pass — and the one Forget ends with: how much smaller the store is on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaimed_bytes: Option<u64>,
    /// Forget: how many Snapshots held a matching path, and hold none now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshots_touched: Option<u64>,
    /// Forget: how many contents no Snapshot holds any more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blobs_removed: Option<u64>,
    /// Forget: every Root-relative path it removed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_paths: Vec<String>,
    /// Forget: the removed paths whose file in the Working Tree still holds what was removed. Forget
    /// never touches the Working Tree, so the secret there still needs rotating — and, unless `excluded`,
    /// the next Snapshot captures it again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub still_on_disk: Vec<String>,
    /// Forget: whether what it removed is kept out of history from now on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded: Option<bool>,
    /// Forget: refs among the Segments' and Pins' names this build could not read, and so left as they
    /// are — naming no commit, or written by a newer Keepster in a content format this build does not
    /// know — named because what they hold was not forgotten, and nothing is passed over in silence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left_alone: Vec<String>,
    /// Export: the directory written into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest: Option<String>,
    /// Export into a Root, overwriting something there: the Snapshot of that Root taken just before,
    /// which what was overwritten can be restored from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safety_snapshot: Option<String>,
    /// Export: every file of the request, in path order, with what happened to it — `restored` meaning
    /// written over what was there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<RestoredPath>,
}

/// A Forget, as `POST /v1/roots/{id}/forget` takes it. Every field must be there: the daemon never asks,
/// it refuses with `confirmation_required`, and the asking is the CLI's (ticket 07).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ForgetRequest {
    /// What to remove from every Snapshot, as an Include Rule pattern relative to the Root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// `true`: this is irreversible, and the caller knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<bool>,
    /// `exclude` keeps the pattern out of history from now on; `keep-watching` goes on capturing it —
    /// right for a file that only held a secret for a while, wrong for one that still does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_exclude: Option<String>,
}

/// What a Reconciliation asked for on demand did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Reconciled {
    /// The Snapshot the pass wrote; absent when the Working Tree matched the latest Snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
    /// How many files the pass read and hashed; the rest were vouched for by what the last pass saw.
    pub hashed: u64,
    /// How many files the pass compared.
    pub observed: u64,
    /// Whether every mtime the pass saw fell on a whole second — a sign the filesystem keeps none finer,
    /// which makes the size-and-mtime filter weaker than it looks.
    pub coarse_mtime: bool,
}

/// One Root's effective configuration: every key of the schema, the value in force, and the level
/// that set it.
///
/// Read-only: the configuration is the TOML files a person edits, which the daemon reloads by itself
/// (ticket 07). While a file is invalid, the values here are the last known-good ones and `errors` says
/// which file is at fault and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RootConfiguration {
    /// The Root's identity.
    pub root: String,
    /// Every key of the schema, in the order the schema lists them.
    pub keys: Vec<ConfigKey>,
    /// What the configuration defines but almost certainly does not mean. None of them makes the Root
    /// `Degraded`.
    pub warnings: Vec<ConfigWarning>,
    /// The files whose current version is invalid and therefore not applied, each in the standard
    /// error shape with the code `config_invalid` and the file as the `path` detail. Any makes the
    /// Root `Degraded`.
    pub errors: Vec<Problem>,
}

/// One key of a Root's effective configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConfigKey {
    /// The key as a person writes it, `table.key`.
    pub key: String,
    /// The value in force, typed as the key is: a switch, a number of bytes, a duration written as text
    /// in the largest whole unit (`3s`, `1m`, `1h`), or a list. A pattern list holds its patterns as
    /// written; `files.max_size_by_pattern` holds `{ pattern, max }` objects; `triggers.active` holds the
    /// names of the active Triggers.
    pub value: serde_json::Value,
    /// Where the value was set: the most specific level that set a scalar — the built-in default when
    /// none did — and, for a list, its last entry's.
    pub origin: Origin,
    /// For a pattern list, every entry and where it was written, from the least specific level to the
    /// most — which is the order the last matching pattern is found in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<ConfigEntry>>,
    /// Whether a file sets this key to something else that only a restart applies. The value above is
    /// the one in force.
    pub restart_required: bool,
}

/// One entry of a layered list, and where it was written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConfigEntry {
    /// The entry, as `ConfigKey::value` types it.
    pub value: serde_json::Value,
    /// Where it was written.
    #[serde(flatten)]
    pub origin: Origin,
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
    /// The Segments, and the Pins as `pins/` and the Snapshot's identity, written by a newer Keepster in
    /// a format this build cannot read. Their Snapshots are on no page; they are not lost, and a build
    /// that reads their format lists them again. Absent while there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
}

/// A Root's pinned Snapshots, as `GET /v1/roots/{id}/pins` serves them: few enough to need no page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Pins {
    /// The pinned Snapshots, newest first.
    pub snapshots: Vec<Snapshot>,
    /// The Pins, as `pins/` and the Snapshot's identity, written by a newer Keepster in a format this
    /// build cannot read: pinned still, and not listed above. Absent while there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
}

/// The paths a Snapshot holds under a directory, as `…/entries` serves them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Entries {
    /// One per path, in byte order of the path.
    pub entries: Vec<Entry>,
    /// Records of the Snapshot's own metadata this build does not know, as they are written — skipped,
    /// and reported, because a later build recorded facts this one cannot show. Absent when there are
    /// none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unrecognized: Vec<String>,
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
    /// A file's tree mode as permission bits, `420` (`0o644`) or `493` (`0o755`): whether it is
    /// executable, on every platform. Absent for anything but a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    /// All twelve permission bits of a file or directory as the Snapshot holds them, as a number
    /// (`384` is `0o600`). Absent where the capture observed none — on Windows, or before ticket 50 —
    /// and for anything else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<u32>,
    /// Whether the content is binary by git's rule. A display flag only: binary files are stored as
    /// they are (ticket 10). Absent for anything but a file, and for a packed file over a megabyte,
    /// which would have to be read whole to tell (ticket 52).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<bool>,
    /// With `withheld`: why the Snapshot holds no content for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withheld: Option<WithheldReason>,
    /// Why it is not held as it was when the Snapshot was taken, which makes the Snapshot incomplete:
    /// `unstable` (it kept changing while read) or the read error's symbolic name — `eacces` on Unix,
    /// `sharing_violation` on Windows. With `omitted` nothing is held; on a file, its previous version
    /// was kept. An open set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete: Option<String>,
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
    /// There when the Snapshot was taken, and deliberately not held; `withheld` says why.
    Withheld,
    /// Meant to be held and not read, with no earlier version to keep; `incomplete` says why.
    Omitted,
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
    /// With `confirmation_required`, every file of the Restore asked for and what it needs; absent
    /// otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<RestoredPath>,
}

/// What a Root's state means for the insurance it provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RootState {
    /// Captured as promised.
    Watching,
    /// Still watched, but not as promised — its configuration failed to load and the last known-good
    /// one is in force, or its coverage guarantee is switched off.
    Degraded,
    /// Nothing can be captured: the directory is missing, unmounted or unreadable, it is on a volume
    /// other than the one it was registered on (`volume_mismatch`), its History Store cannot be written
    /// (`store_unreachable`) or was written by a version this build cannot open, or its configuration has
    /// never had a valid version to fall back on. The history is not unreadable: the timeline, diffs and
    /// content read as ever, and only capture and Restore stop.
    Unavailable,
}

/// One line of a Capture Preview, as `GET /v1/preview` streams them: a `header` first, then an `entry`
/// for every path asked for as the walk classifies it, interleaved with `progress`, then a `summary` — or,
/// if the walk fails, an `error` in its place. One JSON object per line (ticket 57); a record of a type a
/// client does not know is to be passed over.
///
/// A preview captures nothing and says nothing of what the History Store would grow by: that is
/// unknowable without hashing every file, which is the capture. What it measures is content to capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PreviewRecord {
    /// What is previewed, sent before the walk starts.
    Header {
        /// The directory previewed.
        path: String,
        /// The Root it is, or lies inside, when there is one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root: Option<String>,
        /// Where it lies inside that Root, `/`-separated, when it is not the Root itself: classified by the
        /// Root's rules, as a capture of the Root would classify it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        within: Option<String>,
        /// A Root it contains, when it is none: registering it would be refused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        overlaps: Option<String>,
        /// The directory's own `.keepster.toml`, when it has one: honoured exactly as registering it would
        /// honour it, and shown, so that a rule a repository's author wrote is not read as Keepster's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config: Option<PreviewConfig>,
    },
    /// One path, as the walk classified it.
    Entry {
        /// Where, relative to the directory previewed.
        path: String,
        /// `file`, `directory`, `link` or `special`.
        kind: String,
        /// `captured`, `excluded`, `withheld` or `unreadable`.
        class: String,
        /// Why it is Withheld, for a Withheld path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Its apparent size in bytes: a file's length, a link's target text; nothing for a directory.
        bytes: u64,
        /// The rule that decided, where one did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<RuleReference>,
        /// Whether it is an excluded directory the walk did not enter, as a capture never would.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pruned: bool,
    },
    /// How many paths the walk has classified so far, sent now and then whether or not any is listed —
    /// so a reader sees the walk is alive, and the walk sees a reader that went away.
    Progress {
        /// Paths classified so far.
        walked: u64,
    },
    /// Why the walk stopped short, sent instead of the summary.
    Error {
        /// What failed, for a person.
        detail: String,
    },
    /// What the whole walk found.
    Summary {
        /// Content to capture.
        captured: PreviewTotals,
        /// What the rules and the secret denylist leave out.
        excluded: PreviewTotals,
        /// What would be recorded without its content, by reason.
        withheld: BTreeMap<String, PreviewTotals>,
        /// How many paths could not be looked at.
        unreadable: u64,
        /// Every rule that decided something, and what it decided, largest first.
        groups: Vec<PreviewGroup>,
        /// The largest files captured, largest first.
        largest_captured: Vec<PreviewSize>,
        /// The largest files left out, largest first: excluded ones that were measured, and those
        /// withheld without their content.
        largest_left_out: Vec<PreviewSize>,
        /// Which of v1's claims for one Root it is past: `files`, `directories`, `bytes`. Nothing is
        /// refused above them; registering asks for a confirmation.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        past_ceiling: Vec<String>,
    },
}

/// A directory's own `.keepster.toml`, as a preview shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PreviewConfig {
    /// Where it is.
    pub file: String,
    /// Every key it sets, in schema order, with what it sets it to.
    pub sets: Vec<PreviewSetting>,
    /// What it defines that is almost certainly not meant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ConfigWarning>,
}

/// One key a directory's own `.keepster.toml` sets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PreviewSetting {
    /// The key as a person writes it, `table.key`.
    pub key: String,
    /// What the file writes for it, typed as `ConfigKey::value` types a value: one for a scalar, every
    /// entry the file adds for a list.
    pub values: Vec<serde_json::Value>,
}

/// How many files and directories, and how many bytes of files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PreviewTotals {
    /// Files, links and special files.
    pub files: u64,
    /// Directories.
    pub directories: u64,
    /// The files' apparent sizes, summed.
    pub bytes: u64,
    /// Excluded directories not entered, and so not measured.
    #[serde(default)]
    pub pruned: u64,
}

/// Everything one rule — or one reason, or no rule at all — decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PreviewGroup {
    /// `captured`, `excluded`, `withheld` or `unreadable`.
    pub class: String,
    /// Why, for a Withheld group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The rule, absent where none decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleReference>,
    /// How many files it decided.
    pub files: u64,
    /// How many directories.
    pub directories: u64,
    /// The files' apparent sizes, summed.
    pub bytes: u64,
    /// The directories it pruned, named and unmeasured.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pruned: Vec<String>,
}

/// One path and its apparent size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PreviewSize {
    /// Where, relative to the directory previewed.
    pub path: String,
    /// Its apparent size in bytes.
    pub bytes: u64,
}

/// An Export, as `POST /v1/roots/{id}/export` takes it (ticket 58): a Snapshot's versions written into a
/// directory the request names, the Working Tree untouched.
///
/// Restore's request, less what only a Restore needs, and with a destination: the whole request is
/// checked before anything is written, and a path that needs a confirmation or a resolution stops it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExportRequest {
    /// The Snapshot whose versions are written, by identity.
    pub from: String,
    /// Root-relative paths; a directory means everything beneath it, and none means the whole Snapshot.
    #[serde(default)]
    pub paths: Vec<String>,
    /// The directory written into, absolute, on the daemon's machine — which is the client's, since the
    /// daemon listens only there. Each file goes at its Snapshot path beneath it; it is made if missing.
    pub dest: String,
    /// Plan only: answer with what would happen to every path and the selection's size, and write
    /// nothing.
    #[serde(default)]
    pub dry_run: bool,
    /// The changes agreed to: `overwrite`, `replace_type`, `dest_in_root`, `overwrite_unrecoverable`, or
    /// `"all"` for every one — which settles no resolution.
    #[serde(default)]
    pub confirm: Confirm,
    /// Answers to what only the user can settle, keyed by the Snapshot path they are for.
    #[serde(default)]
    pub resolutions: std::collections::BTreeMap<String, PathResolution>,
    /// Skip each path still in the way, with its reasons, rather than stop the request.
    #[serde(default)]
    pub skip_unresolvable: bool,
    /// Where a file already at the destination has other permission bits than the Snapshot's
    /// (`modes_differ`) and no answer of its own: take the Snapshot's, or keep those there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<ModesChoice>,
}

/// What an Export would do, as a dry run answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExportPlan {
    /// The directory it would write into.
    pub dest: String,
    /// How many files the selection holds, Withheld paths aside.
    pub files: u64,
    /// Their sizes, summed.
    pub bytes: u64,
    /// Every file of the request, in path order, with what would happen to it.
    pub paths: Vec<RestoredPath>,
}

/// How much a selection of a Snapshot holds, so its size can be shown before anything is done with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SelectionSize {
    /// How many files, links included.
    pub files: u64,
    /// Their sizes, summed: what an Export writes.
    pub bytes: u64,
    /// The exact size of its `archive`, which stores rather than compresses.
    pub archive_bytes: u64,
    /// The paths it holds no version of, which no export or archive contains.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub withheld: Vec<Withheld>,
}

/// The directories in one directory on the daemon's machine, for choosing where an Export writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DirectoryListing {
    /// The directory listed, absolute.
    pub path: String,
    /// The directory above it, absent at the top.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Every directory in it, by name; no file, no link, no size.
    pub directories: Vec<Subdirectory>,
}

/// One directory of a listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Subdirectory {
    /// Its name.
    pub name: String,
    /// Its path, absolute: what to list next, or to export into.
    pub path: String,
    /// Hidden: a dot name, or the platform's hidden attribute. A picker shows these only when asked.
    pub hidden: bool,
    /// Whether it can be listed in turn.
    pub readable: bool,
}

/// What a path search found, as `GET /v1/search/paths` answers it (ticket 59): every path any Snapshot
/// changed whose name the query finds, in every Root asked — a file long gone from the Working Tree
/// included, which nothing else in the product can reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PathSearch {
    /// Every path found, by Root and then by path.
    pub hits: Vec<PathHit>,
    /// Whether there were more than `limit`; ask for fewer paths with a narrower query.
    pub truncated: bool,
    /// Roots whose history could not be read, and so were not searched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsearched: Vec<Unsearched>,
}

/// One path a path search found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PathHit {
    /// The Root it is in, by identity.
    pub root: String,
    /// The path, Root-relative.
    pub path: String,
    /// How many Snapshots changed it.
    pub snapshots: u64,
    /// The first of them.
    pub first: String,
    /// The last of them.
    pub last: String,
    /// Whether the newest Snapshot still holds it: `false` is a file already gone, which only its
    /// history still has.
    pub exists: bool,
}

/// A Root a search could not read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Unsearched {
    /// The Root, by identity.
    pub root: String,
    /// Why, for a person.
    pub detail: String,
}

/// One line of a content search, as `GET /v1/search/content` streams them (ticket 59): the cost first,
/// then every match and every Withheld path as the walk meets them, progress now and then, and a summary
/// last. A record of a type a client does not know is to be passed over.
///
/// The walk reads each distinct version once, however many Snapshots held it, so a match is a **content
/// version** — a path, and the range of Snapshots its content was current for — never one row per
/// Snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentRecord {
    /// What the search will read, before it reads any of it: per Root, its distinct versions and their
    /// size. No time is promised: the cost is stored content, not a number of Snapshots.
    Plan {
        /// Each Root searched.
        roots: Vec<SearchCost>,
    },
    /// A line of a version that matched.
    Match {
        /// The Root, by identity.
        root: String,
        /// The path, Root-relative.
        path: String,
        /// The version's content identity, opaque: equal for equal content, wherever it is.
        content: String,
        /// The first Snapshot that held this content at this path.
        first: String,
        /// The last.
        last: String,
        /// The line, counted from 1.
        line: u64,
        /// The text around what matched.
        snippet: String,
    },
    /// A path the walk met with no content to search: it was Withheld.
    Withheld {
        /// The Root, by identity.
        root: String,
        /// The path, Root-relative.
        path: String,
        /// Why: `too_large`, `special_file`, and the rest of the Withheld reasons.
        reason: String,
        /// For a special file, what kind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// The first Snapshot it was Withheld in.
        snapshot: String,
    },
    /// How far the walk has got in one Root.
    Progress {
        /// The Root, by identity.
        root: String,
        /// Versions looked at so far.
        done: u64,
        /// Versions there are.
        total: u64,
    },
    /// A Root the walk could not search, and why; the others go on.
    Error {
        /// The Root, by identity.
        root: String,
        /// What failed, for a person.
        detail: String,
    },
    /// What the whole walk looked at: sent last, so that finding nothing is told apart from not looking.
    Summary {
        /// Versions searched as text.
        versions_scanned: u64,
        /// Versions skipped as binary: a NUL among their first bytes.
        binaries_skipped: u64,
        /// Withheld paths met, each named in a `withheld` record.
        withheld: u64,
        /// Versions the store could not read.
        #[serde(default, skip_serializing_if = "is_zero")]
        unreadable: u64,
    },
}

/// What a content search reads in one Root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SearchCost {
    /// The Root, by identity.
    pub root: String,
    /// Its distinct versions under the path asked.
    pub versions: u64,
    /// Their sizes, summed.
    pub bytes: u64,
}

/// Whether a count is nothing, which the wire leaves out.
fn is_zero(count: &u64) -> bool {
    *count == 0
}

/// What the daemon knows that a diagnostics bundle carries, as `GET /v1/diagnostics` serves it on the
/// control channel alone (ticket 63).
///
/// The bundle discloses by inventory and never redacts (ADR 0013): paths are here as they are, because
/// the defects this exists for cannot be diagnosed without them. What is never here is a byte of any
/// file from history — every section is read from status, configuration, the Registry, refs, commit
/// messages and the index, and none from a blob. Each section carries its own failure, so one that
/// cannot be read leaves the others whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Diagnostics {
    /// The health surface, exactly as `GET /v1/status` serves it.
    pub status: Status,
    /// What the daemon runs on and with.
    pub environment: Environment,
    /// The effective configuration with the level that set each key.
    pub configuration: DiagnosticConfiguration,
    /// Every Root the Registry holds, watched or not, with its record as written.
    pub registry: Vec<RegistryRecord>,
    /// What each Root's store holds, one entry per Root the Registry holds.
    pub topology: Vec<RootTopology>,
    /// How far each Root's derived index got, one entry per Root the Registry holds.
    pub watermarks: Vec<RootWatermarks>,
    /// The tail of the daemon's log, and the format it is written in.
    pub log: LogTail,
}

/// What the daemon runs on, and with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Environment {
    /// The operating system, as Rust names it: `linux`, `macos`, `windows`.
    pub os: String,
    /// The processor architecture, as Rust names it: `x86_64`, `aarch64`.
    pub arch: String,
    /// The filesystem under each Root whose record can be read, in registration order.
    pub filesystems: Vec<RootFilesystem>,
    /// The directory watches every Root holds together, against the daemon-wide ceiling.
    pub watches: WatchCounters,
    /// The external `git` maintenance runs with, or why there is none: the discovery result.
    pub git: GitStatus,
    /// How each Root's last Maintenance Pass since the daemon started ended, in registration order. A
    /// Root with no pass since then has no entry: the record is kept in memory, not on disk.
    pub maintenance: Vec<MaintenancePass>,
}

/// The filesystem under one Root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RootFilesystem {
    /// The Root's identity.
    pub root: String,
    /// Where its directory is.
    pub path: String,
    /// The filesystem's type as the platform names it: `ext4`, `nfs4`, `fuse.sshfs`, `apfs`. Absent
    /// when the volume could not be asked, or when the platform does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem: Option<String>,
    /// Whether the platform reports the volume as remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<bool>,
    /// Where the volume is mounted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_point: Option<String>,
    /// Why the volume could not be asked: most often, the directory is not there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The directory watches in use, against the ceiling the daemon keeps under the platform's budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WatchCounters {
    /// How many directory watches every Root's watcher holds together now.
    pub in_use: u64,
    /// How many they may hold together, where the platform has such a budget (Linux's inotify); absent
    /// where it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling: Option<u64>,
}

/// How one Root's last Maintenance Pass ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MaintenancePass {
    /// The Root's identity.
    pub root: String,
    /// When it ended, as an RFC 3339 moment.
    pub finished_at: String,
    /// How: `succeeded`; `failed`, when `git` ran and failed it; `git_unavailable`, when the `git` failed
    /// its check before the pass; `history_unreadable`, when the store could not be opened so `git`
    /// never ran. An open set, as error codes are.
    pub outcome: String,
    /// What a pass that succeeded reclaimed, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaimed_bytes: Option<u64>,
    /// What `git` wrote to its error stream, word for word, when it failed. A pass that succeeded keeps
    /// none: `git` runs quietly, and its error stream is kept only when it fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    /// Why the pass did not run, when it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The effective configuration: the global level on its own, and every Root's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DiagnosticConfiguration {
    /// The defaults and the global file's own table — what every Root starts from, and what the keys
    /// describing the daemon are set by.
    pub global: GlobalConfiguration,
    /// Every watched Root's effective configuration, in registration order.
    pub roots: Vec<RootConfigurationReport>,
}

/// The global level of the configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GlobalConfiguration {
    /// The global file, where there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Every key of the schema, with its value and the level that set it, in the order the schema
    /// lists them. Empty while the global file has never had a valid version.
    pub keys: Vec<ConfigKey>,
    /// The global file, when its current version is invalid and not applied, in the standard error
    /// shape.
    pub errors: Vec<Problem>,
}

/// One Root's effective configuration, or why it has none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RootConfigurationReport {
    /// The Root's identity.
    pub root: String,
    /// Its configuration, as `GET /v1/roots/{id}/config` serves it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<RootConfiguration>,
    /// Why it has none: no file it depends on ever had a valid version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
}

/// One Root the Registry holds, with its record as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RegistryRecord {
    /// The Root's identity, which is its directory's name.
    pub id: String,
    /// `watched`, or `unwatched` for a Root whose history is kept after an unwatch.
    pub standing: String,
    /// Where its directory is, from its record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The identity of the volume it was registered on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
    /// Its place within that volume, `/`-separated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within_volume: Option<String>,
    /// Why its record cannot be read, when it cannot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What one Root's store holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RootTopology {
    /// The Root's identity.
    pub root: String,
    /// The store's shape. Absent, without an error, for a Root nothing was ever captured from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<StoreTopology>,
    /// Why the store could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The shape of one store, read from its refs and commit messages — never a blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StoreTopology {
    /// Every Segment's ref, in name order — which is chronological.
    pub segments: Vec<SegmentTopology>,
    /// Every Pin's ref, in name order.
    pub pins: Vec<PinTopology>,
    /// The bytes the repository takes on disk.
    pub history_bytes: u64,
    /// The bytes the derived index takes on disk; zero when there is none.
    pub index_bytes: u64,
}

/// One Segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SegmentTopology {
    /// The ref's last component: the period, perhaps followed by a sibling's format.
    pub name: String,
    /// The commit at its tip.
    pub tip: String,
    /// The content format its Snapshots are in; absent when it names none this build knows of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<u32>,
    /// How many of its commits hold a Snapshot.
    pub snapshots: u64,
    /// How many hold none: written by something other than Keepster.
    pub foreign: u64,
    /// Its oldest Snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest: Option<SnapshotPoint>,
    /// Its newest Snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest: Option<SnapshotPoint>,
}

/// One Pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PinTopology {
    /// The pinned Snapshot's identity, which is the ref's name.
    pub snapshot: String,
    /// The commit it names.
    pub tip: String,
    /// The content format of the Snapshot it holds; absent when it names none this build knows of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<u32>,
}

/// A Snapshot named by its identity and the moment it carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SnapshotPoint {
    /// Its identity.
    pub id: String,
    /// When it was taken, in UTC, as RFC 3339.
    pub taken_at: String,
}

/// How far one Root's derived index got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RootWatermarks {
    /// The Root's identity.
    pub root: String,
    /// Every source the index tracks or should track, in source order. Empty for a Root nothing was
    /// ever captured from.
    pub sources: Vec<WatermarkReport>,
    /// Why the store or its index could not be read as it is. The index is never repaired to read it:
    /// a report that rebuilt what it reports on would hide what it exists to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// How far the index got with one source: a Segment, or a Pin as `pins/` and its identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WatermarkReport {
    /// The source, as the index names it.
    pub source: String,
    /// The commit at its tip now; absent for a source the index names after its ref went.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip: Option<String>,
    /// The commit the index caught up to; absent for a source not indexed yet. Equal to `tip` once the
    /// index is up to date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed: Option<String>,
}

/// The tail of the daemon's log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LogTail {
    /// The format the daemon writes lines in, `text` or `json`, as `log.format` set it at the start. A
    /// rotated file written before a restart may be in the other.
    pub format: String,
    /// The current file and then each rotated one, newest first, until the tail's budget is spent.
    /// Empty when the daemon writes no log file, or when its log could not be read.
    pub files: Vec<LogFileTail>,
    /// Why the log could not be read, when it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The end of one log file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LogFileTail {
    /// The file's name in the log directory: `daemon.log`, `daemon.log.1`, …
    pub name: String,
    /// The file's whole size, in bytes.
    pub size: u64,
    /// Its last lines, whole lines only.
    pub text: String,
    /// Whether the lines before these were left out.
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::{Event, RootState};

    #[test]
    fn an_events_name_is_the_tag_it_is_serialised_under() {
        let root = || "01K6DX3QR7M2V8C9WZ4PN5YQ1A".to_owned();
        for event in [
            Event::SnapshotCreated {
                root: root(),
                snapshot: root(),
                incomplete: false,
                changed_paths: Some(1),
            },
            Event::RootRegistered { root: root() },
            Event::RootUnregistered {
                root: root(),
                purged: false,
            },
            Event::RootMoved { root: root() },
            Event::RootStateChanged {
                root: root(),
                state: RootState::Watching,
                state_code: None,
            },
            Event::ConfigReloaded { root: root() },
            Event::ConfigInvalid { root: root() },
            Event::RestoreCompleted {
                root: root(),
                safety_snapshot: root(),
                restored: 1,
                created: 0,
                failed: 0,
            },
            Event::RetentionCompleted {
                root: root(),
                deleted_segments: 1,
                thinned_segments: 2,
                removed_snapshots: 30,
            },
            Event::MaintenanceCompleted { root: root() },
            Event::JobStarted {
                job: root(),
                kind: "maintenance".to_owned(),
                root: root(),
            },
            Event::JobProgress {
                job: root(),
                done: 1,
                total: 2,
            },
            Event::JobCompleted {
                job: root(),
                kind: "maintenance".to_owned(),
                root: root(),
                state: super::JobState::Succeeded,
            },
            Event::Resync,
        ] {
            let serialised = serde_json::to_value(&event).expect("JSON");
            assert_eq!(serialised["event"], event.name());
        }
    }
}
