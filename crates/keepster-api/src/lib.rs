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
    /// With a timeline filtered by `path`, how that path changed in this Snapshot. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
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
            Self::Resync => "resync",
        }
    }
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
    /// they are (ticket 10). Absent for anything but a file.
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
    /// Nothing can be captured: the directory is missing, unmounted or unreadable, its History Store
    /// was written by a version this build cannot open, or its configuration has never had a valid
    /// version to fall back on.
    Unavailable,
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
            Event::Resync,
        ] {
            let serialised = serde_json::to_value(&event).expect("JSON");
            assert_eq!(serialised["event"], event.name());
        }
    }
}
