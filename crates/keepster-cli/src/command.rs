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

    /// Show what changed between a Snapshot and another Snapshot or the Working Tree.
    Diff(DiffArgs),

    /// Put a Snapshot's versions of files back over the Working Tree.
    Restore(RestoreArgs),

    /// Say whether a path is captured, and which rule at which level decided it.
    Explain(ExplainArgs),

    /// Show a Root's effective configuration, and the level that set each key.
    Config(ConfigArgs),

    /// Compare a Root's Working Tree against its latest Snapshot now, and capture what differs.
    Reconcile(ReconcileArgs),

    /// Pin a Snapshot, so that it survives Retention unconditionally until it is unpinned.
    Pin(PinArgs),

    /// Give a pinned Snapshot back to Retention.
    Unpin(UnpinArgs),

    /// List a Root's pinned Snapshots, newest first.
    Pins(PinsArgs),
}

/// Options of `keepster pin`.
#[derive(Debug, Args)]
pub struct PinArgs {
    /// Which Snapshot, as `cat -s` takes it: `latest` (the default), `~N`, an identity, a moment or a
    /// duration ago.
    #[arg(value_name = "SELECTOR")]
    pub snapshot: Option<String>,

    /// Which Root, as a full identity — of a Root no longer watched too — an unambiguous prefix of one,
    /// or a path. Defaults to the Root holding the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,
}

/// Options of `keepster unpin`.
#[derive(Debug, Args)]
pub struct UnpinArgs {
    /// Which Snapshot, as `cat -s` takes it: `latest`, `~N`, an identity, a moment or a duration ago.
    #[arg(value_name = "SELECTOR")]
    pub snapshot: String,

    /// Which Root, as a full identity — of a Root no longer watched too — an unambiguous prefix of one,
    /// or a path. Defaults to the Root holding the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,
}

/// Options of `keepster pins`.
#[derive(Debug, Args)]
pub struct PinsArgs {
    /// Which Root, as a full identity — of a Root no longer watched too — an unambiguous prefix of one,
    /// or a path. Defaults to the Root holding the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Print the pinned Snapshots as JSON, as the timeline items are.
    #[arg(long)]
    pub json: bool,
}

/// Options of `keepster reconcile`.
#[derive(Debug, Args)]
pub struct ReconcileArgs {
    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Read every file, whatever the last pass observed — for a filesystem whose mtimes cannot be
    /// trusted.
    #[arg(long)]
    pub verify: bool,

    /// Print the daemon's answer as JSON, verbatim.
    #[arg(long)]
    pub json: bool,
}

/// Options of `keepster config`.
#[derive(Debug, Args)]
pub struct ConfigArgs {
    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Print the daemon's answer as JSON, verbatim.
    #[arg(long)]
    pub json: bool,
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
    #[arg(long, value_name = "WHEN", conflicts_with = "follow")]
    pub before: Option<String>,

    /// Only Snapshots taken after this moment, in the same forms as `--before`.
    #[arg(long, value_name = "WHEN")]
    pub after: Option<String>,

    /// Print the Snapshots as one JSON array of the daemon's own items — or, with `--follow`, one item
    /// per line.
    #[arg(long)]
    pub json: bool,

    /// Print the last Snapshots oldest first (10 unless `-n` says otherwise), then keep printing each new
    /// one as it is taken, until interrupted.
    #[arg(short = 'f', long)]
    pub follow: bool,
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

/// Options of `keepster diff`.
#[derive(Debug, Args)]
pub struct DiffArgs {
    /// Only this file, as unified hunks; without it, a summary of every changed path. It also decides
    /// the Root, unless `--root` does.
    pub path: Option<PathBuf>,

    /// The older side, as `cat -s` takes it: `latest` (the default), `~N`, an identity, a moment or a
    /// duration ago.
    #[arg(long, value_name = "SELECTOR")]
    pub from: Option<String>,

    /// The newer side: `working` (the default) for the Working Tree as the next Snapshot would hold it,
    /// or a Snapshot as `--from` takes one.
    #[arg(long, value_name = "SELECTOR")]
    pub to: Option<String>,

    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the path, or the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Print every changed file's unified hunks rather than the summary.
    #[arg(short = 'p', long, conflicts_with = "json")]
    pub patch: bool,

    /// Print the daemon's structured answer.
    #[arg(long)]
    pub json: bool,

    /// Write the output to this file, byte for byte, rather than to standard output — which a shell
    /// such as Windows PowerShell 5.1 would re-encode.
    #[arg(short = 'o', long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

/// Options of `keepster restore`.
///
/// Restore overwrites and creates; it never deletes. The whole request is checked first, and a Safety
/// Snapshot of the current state is taken before anything is written, so the restore can be undone.
#[derive(Debug, Args)]
pub struct RestoreArgs {
    /// The files or directories to put back, as paths on this machine — they need not exist any more.
    /// The Root itself means every file of the Snapshot.
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,

    /// Which Snapshot, as `cat -s` takes it: `latest` (the default), `~N`, an identity, a moment or a
    /// duration ago.
    #[arg(long, value_name = "SELECTOR")]
    pub from: Option<String>,

    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the first path.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Print what would happen to every file, and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Create directories that are not there.
    #[arg(long)]
    pub create_parents: bool,

    /// Replace something of another kind: a file where a directory is needed, a link or a special file.
    #[arg(long)]
    pub replace_type: bool,

    /// Replace a file that has other names; those names keep the old content.
    #[arg(long)]
    pub break_hardlinks: bool,

    /// Overwrite a file whose current content the Safety Snapshot cannot hold.
    #[arg(long)]
    pub overwrite_unrecoverable: bool,

    /// All four of the above. It answers no question about names.
    #[arg(long)]
    pub force: bool,

    /// Of the files that are one file here, write this one and skip the others.
    #[arg(long, value_name = "PATH")]
    pub pick: Vec<PathBuf>,

    /// Write the file at PATH under NAME, in the same directory, where its own name cannot exist.
    #[arg(long, value_name = "PATH=NAME")]
    pub rename: Vec<String>,

    /// Where a name in another case is already there: `keep` it, or use the `snapshot`'s.
    #[arg(long, value_name = "PATH=keep|snapshot")]
    pub name_case: Vec<String>,

    /// Where a file's permission bits on disk are not the Snapshot's: take the `snapshot`'s, or `keep`
    /// those on disk — for every such file, or, as PATH=snapshot|keep, for one. Defaults to the
    /// Root's `restore.modes` setting, which asks.
    #[arg(long, value_name = "snapshot|keep|PATH=snapshot|keep")]
    pub modes: Vec<String>,

    /// Skip each file still in the way, with its reason, and restore the rest.
    #[arg(long)]
    pub skip_unresolvable: bool,

    /// Never ask: a confirmation or a resolution still missing is reported, and the exit code is 5.
    #[arg(long)]
    pub no_input: bool,

    /// Print the daemon's answer.
    #[arg(long)]
    pub json: bool,
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
