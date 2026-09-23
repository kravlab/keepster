//! The CLI: a thin, stable client over the daemon's HTTP API.
//!
//! This crate knows no git, no SQL and no domain — ticket 08 makes that a fact of the dependency
//! graph rather than a convention, so `cargo tree -p keepster-cli` is the check. The shipped binary
//! lives in `keepster-daemon`, which is the wiring crate; this one stays a library.

pub mod client;
pub mod command;
pub mod render;
pub mod status;

pub use client::{ClientError, DaemonClient, Response};
pub use command::{Cli, Command, StatusArgs};
pub use render::{write_human, write_json};
pub use status::{Outcome, exit_code_for, outcome, status};

/// The process exit codes, which are contract: a script must be able to tell one failure from
/// another, so `1` is a catch-all and never the whole table (ticket 07). Each code appears here once
/// something can return it — clap emits its own code for a misused command line, and the codes for
/// not-found, state conflicts, partial success and a version mismatch arrive with the operations that
/// raise them.
pub mod exit {
    /// The command did what was asked.
    pub const SUCCESS: u8 = 0;
    /// No daemon is listening on the control channel.
    pub const DAEMON_UNREACHABLE: u8 = 3;
    /// `status` found at least one Root that is not `Watching`.
    pub const ROOT_NOT_WATCHING: u8 = 10;
    /// Anything else, deliberately narrow.
    pub const OTHER: u8 = 1;
}
