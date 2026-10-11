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

    /// Put a directory under history.
    Watch(WatchArgs),

    /// List the Watched Roots.
    Roots,

    /// Stop watching a directory. Its history is kept unless a purge is asked for.
    Unwatch(UnwatchArgs),

    /// Record that a Watched Root's directory has moved.
    Move(MoveArgs),

    /// Capture the Root's current contents now.
    Snapshot(SnapshotArgs),

    /// List a Root's Snapshots, newest first — or the ones that changed a path.
    Log(LogArgs),

    /// Write one captured version of a file to standard output.
    Cat(CatArgs),

    /// Say whether a path is captured, and which rule at which level decided it.
    Explain(ExplainArgs),
}

/// Options of `keepster explain`.
#[derive(Debug, Args)]
pub struct ExplainArgs {
    /// The path, as a path on this machine; it need not exist.
    pub path: PathBuf,

    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the path.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Print the daemon's answer as JSON, verbatim.
    #[arg(long)]
    pub json: bool,
}

/// Options of `keepster log`.
#[derive(Debug, Args)]
pub struct LogArgs {
    /// Only the Snapshots that changed this file. It also decides the Root, unless `--root` does.
    pub path: Option<PathBuf>,

    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the path, or the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Show at most this many Snapshots.
    #[arg(short = 'n', long, value_name = "N")]
    pub limit: Option<usize>,

    /// Only Snapshots taken at or before this moment: a timestamp, a local date and time, or a
    /// duration ago such as `2h`.
    #[arg(long, value_name = "WHEN")]
    pub before: Option<String>,

    /// Only Snapshots taken after this moment, in the same forms as `--before`.
    #[arg(long, value_name = "WHEN")]
    pub after: Option<String>,

    /// Print the Snapshots as one JSON array of the daemon's own items.
    #[arg(long)]
    pub json: bool,
}

/// Options of `keepster cat`.
#[derive(Debug, Args)]
pub struct CatArgs {
    /// The file, as a path on this machine; it need not exist any more.
    pub path: PathBuf,

    /// Which Snapshot: `latest` (the default), `~N` for N Snapshots before it, a Snapshot's full
    /// identity, a timestamp or local date and time, or a duration ago such as `2h` — the last two
    /// meaning the Snapshot current at that moment.
    #[arg(short = 's', long, value_name = "SELECTOR")]
    pub snapshot: Option<String>,

    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the file.
    #[arg(short = 'r', long)]
    pub root: Option<String>,
}

/// Options of `keepster snapshot`.
#[derive(Debug, Args)]
pub struct SnapshotArgs {
    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,
}

/// Options of `keepster unwatch`.
#[derive(Debug, Args)]
pub struct UnwatchArgs {
    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Also erase the Root's captured history.
    #[arg(long)]
    pub purge: bool,

    /// Consent to an irreversible operation. Kept separate from selecting one.
    #[arg(long)]
    pub yes: bool,
}

/// Options of `keepster move`.
#[derive(Debug, Args)]
pub struct MoveArgs {
    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Where the Root's directory is now.
    pub path: PathBuf,
}

/// Options of `keepster watch`.
#[derive(Debug, Args)]
pub struct WatchArgs {
    /// The directory to watch. Relative paths are resolved against the working directory.
    pub path: PathBuf,
}

/// Options of `keepster status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Print the daemon's status payload as JSON, verbatim.
    #[arg(long)]
    pub json: bool,
}
