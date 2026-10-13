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

    /// Stop watching a directory, or every one. Its history is kept unless a purge is asked for.
    Unwatch(UnwatchArgs),

    /// Erase a Root's history, or every Root's, and keep watching: the next capture starts it over.
    Purge(PurgeArgs),

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

    /// Write a Snapshot's versions of files into another directory, or into one zip archive, leaving the
    /// Working Tree untouched.
    Export(ExportArgs),

    /// Find a path by a fragment of its name in every Root's history — a file long deleted included.
    Find(FindArgs),

    /// Find text in every version a Root's history holds, when you remember the code but not the file.
    Grep(GrepArgs),

    /// Say whether a path is captured, and which rule at which level decided it.
    Explain(ExplainArgs),

    /// Show what a Snapshot of a directory would hold right now, grouped by the rule that decided —
    /// for a Root, or for a directory before it is watched. Captures nothing.
    Preview(PreviewArgs),

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

    /// Run a Maintenance Pass over a Root now, reclaiming what Retention removed, as a job.
    Maintain(MaintainArgs),

    /// Report a job: where it is, and how it ended.
    Job(JobArgs),

    /// Remove every version of a path from the Root's whole history, when a secret was captured.
    /// Irreversible; the files on disk are not touched.
    Forget(ForgetArgs),

    /// Open the web UI in a browser, signed in — or, with `token rotate`, end every browser session.
    Ui(UiArgs),

    /// Register the daemon with this platform's service manager — a systemd user unit, a LaunchAgent,
    /// or a Scheduled Task at logon — start it, and check that it answers; and start the tray at every
    /// graphical login. Needs no administrator rights.
    Install(InstallArgs),

    /// Stop the daemon and remove what `keepster install` created. History, Roots and configuration are
    /// left as they are unless a flag below asks, with `--yes`, for them to be erased.
    Uninstall(UninstallArgs),

    /// Start, stop or inspect the daemon through its service manager.
    #[command(subcommand)]
    Service(ServiceCommand),

    /// Show the daemon's log level, raise it for a while, or end a raise. A raise always ends by
    /// itself: `keepster log-level debug --for 30m`.
    LogLevel(LogLevelArgs),

    /// Mute the tray's notifications for a while. Alarms are still raised, and `status` shows them and
    /// the mute; a mute always ends by itself: `keepster mute --for 2h`.
    Mute(MuteArgs),

    /// Lift a mute now.
    Unmute,

    /// Write a diagnostics bundle for a bug report into one local zip, after printing what is in it
    /// and what each file discloses. Paths are included as they are; nothing is ever sent anywhere.
    Diagnose(DiagnoseArgs),
}

/// Options of `keepster diagnose`. There are none that choose what goes in: a bundle trimmed by a
/// flag would look exactly like a complete one (ADR 0013).
#[derive(Debug, Args)]
pub struct DiagnoseArgs {
    /// Print the inventory of what the bundle would hold, and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Where to write the bundle. Defaults to `keepster-diagnose-<moment>.zip` in the current
    /// directory; an existing file is never overwritten.
    #[arg(short, long, value_name = "PATH", conflicts_with = "dry_run")]
    pub output: Option<PathBuf>,
}

/// Options of `keepster log-level`.
#[derive(Debug, Args)]
pub struct LogLevelArgs {
    /// The level to raise to: `error`, `warn`, `info`, `debug` or `trace`.
    #[arg(requires = "duration", conflicts_with = "reset")]
    pub level: Option<String>,

    /// How long the raise lasts, such as `30m` or `2h`; then the configured level is back.
    #[arg(long = "for", value_name = "DURATION", requires = "level")]
    pub duration: Option<String>,

    /// End a raise now.
    #[arg(long)]
    pub reset: bool,
}

/// Options of `keepster mute`.
#[derive(Debug, Args)]
pub struct MuteArgs {
    /// How long, in whole minutes, such as `30m` or `2h`: at most a day. An hour when not given.
    #[arg(long = "for", value_name = "DURATION")]
    pub duration: Option<String>,
}

/// The subcommands of `keepster service`.
#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    /// Ask the service manager to start the daemon.
    Start,

    /// Ask the service manager to stop the daemon. It stays stopped until it is started, or until the
    /// next login.
    Stop,

    /// Ask the service manager to stop the daemon and start it again, as a change only a restart
    /// applies needs.
    Restart,

    /// Say whether autostart is installed, whether the daemon is running, and whether coverage
    /// survives logout.
    Status,
}

/// Options of `keepster install`.
#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Declare that this machine has no web UI: when this creates the global configuration, it writes
    /// the loopback listener off. An existing configuration is never rewritten.
    #[arg(long)]
    pub no_web_ui: bool,

    /// Install no tray: nothing shows the daemon's state at a glance, and nothing notifies when
    /// coverage stops.
    #[arg(long)]
    pub no_tray: bool,
}

/// Options of `keepster uninstall`. No install or uninstall path erases anything on its own (ticket 68):
/// each flag selects what to erase, and `--yes` consents to it separately.
#[derive(Debug, Args)]
pub struct UninstallArgs {
    /// Also erase every Root's captured history. Watched Roots stay registered and start over; a Root
    /// no longer watched, which has nothing but its history, is erased whole.
    #[arg(long)]
    pub history: bool,

    /// Also stop watching every Root. Each one's history is kept, its last state pinned, as `unwatch`
    /// leaves it.
    #[arg(long)]
    pub roots: bool,

    /// Also erase everything Keepster keeps for this user: every Root and its history, the
    /// configuration, the web UI's preferences and the logs.
    #[arg(long, conflicts_with_all = ["history", "roots"])]
    pub purge: bool,

    /// Consent to an irreversible operation. Kept separate from selecting one.
    #[arg(long)]
    pub yes: bool,
}

/// Options of `keepster ui`.
#[derive(Debug, Args)]
pub struct UiArgs {
    /// Print the address to open instead of opening a browser. It holds a single-use code, which the
    /// first browser to open it spends.
    #[arg(long)]
    pub no_browser: bool,

    /// What else to do with the browser's way in.
    #[command(subcommand)]
    pub command: Option<UiCommand>,
}

/// The subcommands of `keepster ui`.
#[derive(Debug, Subcommand)]
pub enum UiCommand {
    /// The loopback listener's token.
    #[command(subcommand)]
    Token(TokenCommand),
}

/// The subcommands of `keepster ui token`.
#[derive(Debug, Subcommand)]
pub enum TokenCommand {
    /// Replace the token, ending every browser session it opened. Restarting the daemon does too.
    Rotate,
}

/// Options of `keepster forget`.
#[derive(Debug, Args)]
pub struct ForgetArgs {
    /// What to remove, as an exclude pattern relative to the Root: `.env`, `secrets/`, `*.pem`.
    pub glob: String,

    /// Keep it out of history from now on — right for a file that still holds the secret.
    #[arg(long, conflicts_with = "keep_watching")]
    pub exclude: bool,

    /// Go on capturing it — right only once the file no longer holds the secret.
    #[arg(long)]
    pub keep_watching: bool,

    /// Confirm without being asked: this cannot be undone.
    #[arg(long)]
    pub yes: bool,

    /// Never ask; refuse instead when the command line does not say everything.
    #[arg(long)]
    pub no_input: bool,

    /// Print the job's identity and end at once, rather than following it to its end.
    #[arg(long)]
    pub detach: bool,

    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,
}

/// Options of `keepster maintain`.
#[derive(Debug, Args)]
pub struct MaintainArgs {
    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Print the job's identity and end at once, rather than following it to its end.
    #[arg(long)]
    pub detach: bool,
}

/// Options of `keepster job`.
#[derive(Debug, Args)]
pub struct JobArgs {
    /// The job's identity, as `--detach` printed it.
    pub id: String,

    /// Print the daemon's answer as JSON, verbatim.
    #[arg(long)]
    pub json: bool,
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

    /// What only the person can settle.
    #[command(flatten)]
    pub resolving: ResolutionArgs,

    /// Print the daemon's answer.
    #[arg(long)]
    pub json: bool,
}

/// The answers a Restore and an Export take to what only the person can settle.
#[derive(Debug, Args)]
pub struct ResolutionArgs {
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
    /// those on disk — for every such file, or, as PATH=snapshot|keep, for one. For a Restore, defaults
    /// to the Root's `restore.modes` setting, which asks.
    #[arg(long, value_name = "snapshot|keep|PATH=snapshot|keep")]
    pub modes: Vec<String>,

    /// Skip each file still in the way, with its reason, and do the rest.
    #[arg(long)]
    pub skip_unresolvable: bool,

    /// Never ask: a confirmation or a resolution still missing is reported, and the exit code is 5.
    #[arg(long)]
    pub no_input: bool,
}

/// Options of `keepster export`.
#[derive(Debug, Args)]
pub struct ExportArgs {
    /// The files or directories to export, as paths on this machine — they need not exist any more.
    /// None, or the Root itself, is the whole Snapshot.
    pub paths: Vec<PathBuf>,

    /// Which Snapshot, as `cat -s` takes it: `latest` (the default), `~N`, an identity, a moment or a
    /// duration ago.
    #[arg(long, value_name = "SELECTOR")]
    pub from: Option<String>,

    /// The directory to write into, made if missing; each file goes at its path in the Root beneath it.
    #[arg(
        long,
        value_name = "DIR",
        required_unless_present = "archive",
        conflicts_with = "archive"
    )]
    pub to: Option<PathBuf>,

    /// Write one zip archive of the selection to FILE instead, as a browser downloads it.
    #[arg(long, value_name = "FILE")]
    pub archive: Option<PathBuf>,

    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the first path, or the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Print what would happen to every file and how much it is, and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Write over what is already in the destination. Off by default: outside a Root nothing can bring
    /// it back.
    #[arg(long)]
    pub overwrite: bool,

    /// Replace something of another kind: a file where a directory is needed, a link or a special file.
    #[arg(long)]
    pub replace_type: bool,

    /// Write into a Watched Root, which captures what is written there.
    #[arg(long)]
    pub dest_in_root: bool,

    /// Inside a Root, overwrite what its Safety Snapshot cannot hold.
    #[arg(long)]
    pub overwrite_unrecoverable: bool,

    /// All four of the above. It answers no question about names.
    #[arg(long)]
    pub force: bool,

    /// What only the person can settle.
    #[command(flatten)]
    pub resolving: ResolutionArgs,

    /// Print the job's identity and end at once, rather than following it to its end.
    #[arg(long)]
    pub detach: bool,

    /// Print the daemon's answer.
    #[arg(long)]
    pub json: bool,
}

/// Options of `keepster find`.
#[derive(Debug, Args)]
pub struct FindArgs {
    /// A fragment of the path, matched anywhere in it — case-insensitively unless it has a capital.
    pub query: String,

    /// Read the query as an Include Rules pattern, as the configuration writes them: `**/*.env`.
    #[arg(long)]
    pub glob: bool,

    /// Search only this Root — a full identity, an unambiguous prefix of one, or a path. Repeatable;
    /// every Root when absent.
    #[arg(short = 'r', long)]
    pub root: Vec<String>,

    /// How many paths to list: 100 unless given, at most 1000.
    #[arg(long)]
    pub limit: Option<usize>,

    /// Print the daemon's answer.
    #[arg(long)]
    pub json: bool,
}

/// Options of `keepster grep`.
#[derive(Debug, Args)]
pub struct GrepArgs {
    /// What to find: a literal, matched case-insensitively unless it has a capital.
    pub pattern: String,

    /// Read the pattern as a regular expression.
    #[arg(long)]
    pub regex: bool,

    /// Which Root, as a full identity, an unambiguous prefix of one, or a path. Defaults to the Root
    /// holding the working directory.
    #[arg(short = 'r', long, conflicts_with = "all_roots")]
    pub root: Option<String>,

    /// Search every Root: every version each one holds is read, which the first line says the size of.
    #[arg(long)]
    pub all_roots: bool,

    /// Search only what is at or beneath this path, a file or directory of the Root.
    #[arg(long, value_name = "PATH", conflicts_with = "all_roots")]
    pub path: Option<PathBuf>,

    /// Print the daemon's stream as it arrives, one JSON object per line, verbatim.
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
    /// Which Root, as a full identity — with `--purge`, of a Root no longer watched too — an
    /// unambiguous prefix of one, or a path. Defaults to the Root holding the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Stop watching every Root instead. With `--purge`, every Root no longer watched is erased too.
    #[arg(long, conflicts_with = "root")]
    pub all: bool,

    /// Also erase the Root's captured history.
    #[arg(long)]
    pub purge: bool,

    /// Consent to an irreversible operation. Kept separate from selecting one.
    #[arg(long)]
    pub yes: bool,
}

/// Options of `keepster purge` (ticket 68).
#[derive(Debug, Args)]
pub struct PurgeArgs {
    /// Which Root, as a full identity — of a Root no longer watched too, which is erased whole — an
    /// unambiguous prefix of one, or a path. Defaults to the Root holding the working directory.
    #[arg(short = 'r', long)]
    pub root: Option<String>,

    /// Erase every Root's history instead. A Root no longer watched, which has nothing but its
    /// history, is erased whole.
    #[arg(long, conflicts_with = "root")]
    pub all: bool,

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

    /// Show what watching it would capture, and watch nothing: `keepster preview`, under this name.
    #[arg(long)]
    pub dry_run: bool,

    /// Watch it without asking, even past what one Root is claimed to hold.
    #[arg(long)]
    pub yes: bool,
}

/// Options of `keepster preview`.
#[derive(Debug, Args)]
pub struct PreviewArgs {
    /// The directory; by default the Root holding the working directory, or else the working directory.
    pub path: Option<PathBuf>,

    /// `infinity` lists every path as well as the summary.
    #[arg(long)]
    pub depth: Option<String>,

    /// List every path of one class: `captured`, `excluded`, `withheld` or `unreadable`.
    #[arg(long)]
    pub only: Option<String>,

    /// Enter excluded directories to measure what they hold, which capture never does.
    #[arg(long)]
    pub measure_excluded: bool,

    /// How many of the largest entries to name on each side.
    #[arg(long)]
    pub top: Option<usize>,

    /// Print the daemon's stream as it arrives, one JSON object per line, verbatim.
    #[arg(long)]
    pub json: bool,
}

/// Options of `keepster status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Print the daemon's status payload as JSON, verbatim.
    #[arg(long)]
    pub json: bool,
}
