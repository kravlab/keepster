//! The CLI: a thin, stable client over the daemon's HTTP API.
//!
//! This crate knows no git, no SQL and no domain — ticket 08 makes that a fact of the dependency
//! graph rather than a convention, so `cargo tree -p keepster-cli` is the check. The shipped binary
//! lives in `keepster-daemon`, which is the wiring crate; this one stays a library.

pub mod client;
pub mod command;
pub mod config;
pub mod events;
pub mod explain;
pub mod history;
pub mod reconcile;
pub mod render;
pub mod resolve;
pub mod roots;
pub mod selector;
pub mod snapshots;
pub mod status;

pub use client::{ClientError, DaemonClient, Response};
pub use command::{
    CatArgs, Cli, Command, ConfigArgs, ExplainArgs, LogArgs, MoveArgs, ReconcileArgs, SnapshotArgs,
    StatusArgs, UnwatchArgs, WatchArgs,
};
pub use config::config;
pub use events::{EventStream, SseEvent};
pub use explain::explain;
pub use history::{cat, log};
pub use reconcile::reconcile;
pub use render::{write_human, write_json, write_roots};
pub use resolve::resolve;
pub use roots::{move_root, roots, unwatch, watch};
pub use snapshots::snapshot;
pub use status::{Outcome, exit_code_for, outcome, status};

/// The process exit codes, which are contract: a script must be able to tell one failure from
/// another, so `1` is a catch-all and never the whole table (ticket 07). Each code appears here once
/// something can return it — clap emits its own code for a misused command line, and the codes for
/// not-found, state conflicts, partial success and a version mismatch arrive with the operations that
/// raise them.
pub mod exit {
    /// The command did what was asked.
    pub const SUCCESS: u8 = 0;
    /// The command line itself was wrong, including an ambiguous Root selector.
    pub const USAGE: u8 = 2;
    /// No daemon is listening on the control channel.
    pub const DAEMON_UNREACHABLE: u8 = 3;
    /// What was asked for does not exist.
    pub const NOT_FOUND: u8 = 4;
    /// The daemon refused because of the state something is in, not because of the request.
    pub const STATE_CONFLICT: u8 = 5;
    /// `status` found at least one Root that is not `Watching`.
    pub const ROOT_NOT_WATCHING: u8 = 10;
    /// Anything else, deliberately narrow.
    pub const OTHER: u8 = 1;
}
