//! The command surface. It is a stable contract in its own right — subcommands, flags, exit codes and
//! `--json` shapes — because the CLI is one rendering of the API rather than a convenience wrapper
//! (ticket 07).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// `keepster`, the one shipped command.
#[derive(Debug, Parser)]
#[command(name = "keepster", version, about = "Editor-independent file history")]
pub struct Cli {
    /// The daemon's control channel. Defaults to this platform's per-user runtime path; an explicit
    /// path serves an unusual layout and the test harness.
    #[arg(long, global = true, value_name = "PATH")]
    pub control_path: Option<PathBuf>,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands this build understands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the daemon in the foreground. Started by the service manager, not by hand: ticket 05 keeps
    /// exactly one kind of running daemon, the supervised kind.
    Daemon,

    /// Report whether work is being captured right now.
    Status(StatusArgs),
}

/// Options of `keepster status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Print the daemon's status payload as JSON, verbatim.
    #[arg(long)]
    pub json: bool,
}
