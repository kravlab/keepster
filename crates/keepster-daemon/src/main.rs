//! The one shipped binary. The daemon and the CLI are a single artifact, and this crate owns it
//! because ticket 08 makes it the wiring crate; `keepster-cli` stays a library with no store
//! dependency, which `cargo tree -p keepster-cli` is the check for.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use keepster_cli::{Cli, Command, exit};
use keepster_daemon::DaemonConfig;

mod diagnose;
mod supervised;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let Some(control_path) = cli
        .control_path
        .clone()
        .or_else(keepster_service::default_control_path)
    else {
        eprintln!(
            "keepster: this platform has no per-user runtime directory; pass --control-path <PATH>"
        );
        return ExitCode::from(exit::OTHER);
    };

    // With no `--control-path`, the daemon is the platform's supervised one: when it is not running,
    // the service manager is asked to start it, and `status` says why it is not (ticket 61). An explicit
    // path names some other daemon, which nothing here may start.
    if cli.control_path.is_none() && !keepster_service::daemon_answers(&control_path) {
        match &cli.command {
            // `diagnose` exists for the moment the daemon will not run: starting it would hide that,
            // so without one the bundle is collected from the disk (ticket 63).
            Command::Daemon | Command::Service(_) | Command::Diagnose(_) => {}
            Command::Status(_) => {
                return ExitCode::from(supervised::unreachable(&control_path));
            }
            _ => {
                if let Err(code) = supervised::reach(&control_path).await {
                    return ExitCode::from(code);
                }
            }
        }
    }

    let code = match &cli.command {
        Command::Daemon => {
            let Some(store_base) = keepster_service::default_store_base() else {
                eprintln!("keepster: this platform has no per-user data directory for the store");
                return ExitCode::from(exit::OTHER);
            };

            let Some(runtime_dir) = keepster_service::default_runtime_dir() else {
                eprintln!(
                    "keepster: this platform has no per-user runtime directory for the daemon"
                );
                return ExitCode::from(exit::OTHER);
            };

            run_daemon(
                control_path,
                runtime_dir,
                store_base,
                keepster_service::default_config_path(),
                keepster_service::default_log_dir(),
            )
            .await
        }
        Command::Status(args) => keepster_cli::status(args, &control_path).await,
        Command::Watch(args) => keepster_cli::watch(args, &control_path).await,
        Command::Preview(args) => keepster_cli::preview(args, &control_path).await,
        Command::Roots => keepster_cli::roots(&control_path).await,
        Command::Unwatch(args) => keepster_cli::unwatch(args, &control_path).await,
        Command::Move(args) => keepster_cli::move_root(args, &control_path).await,
        Command::Snapshot(args) => keepster_cli::snapshot(args, &control_path).await,
        Command::Log(args) => keepster_cli::log(args, &control_path, now()).await,
        Command::Cat(args) => keepster_cli::cat(args, &control_path, now()).await,
        Command::Diff(args) => keepster_cli::diff(args, &control_path, now()).await,
        Command::Restore(args) => keepster_cli::restore(args, &control_path, now()).await,
        Command::Export(args) => keepster_cli::export(args, &control_path, now()).await,
        Command::Find(args) => keepster_cli::find(args, &control_path).await,
        Command::Grep(args) => keepster_cli::grep(args, &control_path).await,
        Command::Explain(args) => keepster_cli::explain(args, &control_path).await,
        Command::Config(args) => keepster_cli::config(args, &control_path).await,
        Command::Reconcile(args) => keepster_cli::reconcile(args, &control_path).await,
        Command::Pin(args) => keepster_cli::pin(args, &control_path, now()).await,
        Command::Unpin(args) => keepster_cli::unpin(args, &control_path, now()).await,
        Command::Pins(args) => keepster_cli::pins(args, &control_path).await,
        Command::Maintain(args) => keepster_cli::maintain(args, &control_path).await,
        Command::Job(args) => keepster_cli::job(args, &control_path).await,
        Command::Forget(args) => keepster_cli::forget(args, &control_path).await,
        Command::Ui(args) => keepster_cli::ui(args, &control_path).await,
        Command::Service(command) => supervised::service(command, &control_path).await,
        Command::LogLevel(args) => keepster_cli::log_level(args, &control_path).await,
        Command::Diagnose(args) => diagnose::diagnose(args, &control_path, now()).await,
    };

    ExitCode::from(code)
}

/// Run the daemon in the foreground until the supervisor stops it.
///
/// Ticket 61 turns this into the full stop sequence — drain the capture in flight, then flush every
/// pending debounce window — and makes the pid file's lock the single-instance authority.
async fn run_daemon(
    control_path: PathBuf,
    runtime_dir: PathBuf,
    store_base: PathBuf,
    config_path: Option<PathBuf>,
    log_dir: Option<PathBuf>,
) -> u8 {
    // Taken over before the daemon starts, so a stop asked for while it recovers its indexes is not a
    // kill: the stop sequence runs as soon as there is a daemon to stop.
    let stop = match StopSignals::listen() {
        Ok(stop) => stop,
        Err(error) => {
            eprintln!("keepster: could not wait for a stop signal: {error}");
            return exit::OTHER;
        }
    };
    let log = log_dir.map(|directory| Arc::new(keepster_daemon::FileLog::new(directory)));
    // A panic is fatal, so it reaches the journal through the default hook's standard error, and the
    // log file through the subscriber.
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        tracing::error!(%panic, "the daemon panicked");
        report(panic);
    }));
    let daemon = match keepster_daemon::start(DaemonConfig {
        control_path,
        runtime_dir,
        loopback_port: None,
        store_base,
        config_path,
        watch_ceiling: None,
        retry_after: None,
        housekeeping: None,
        volumes: None,
        log: log.clone(),
    })
    .await
    {
        Ok(daemon) => daemon,
        // Exited with the code the systemd unit is told not to restart on: a refused second daemon is not
        // a failure to supervise (ticket 61). It is also the CLI's state-conflict code.
        Err(error @ keepster_daemon::StartError::AlreadyRunning { .. }) => {
            eprintln!("keepster: {error}; this one exits");
            return keepster_service::autostart::REFUSED_EXIT;
        }
        Err(error) => {
            eprintln!("keepster: could not start the daemon: {error}");
            return exit::OTHER;
        }
    };

    // Standard error reaches the journal under systemd, so it carries this line and fatal ones alone; the
    // volume goes to the file (ticket 62).
    match log.as_deref().and_then(keepster_daemon::FileLog::file) {
        Some(file) => eprintln!(
            "keepster: the daemon {} is running as pid {}; its log is {}",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            file.display()
        ),
        None => eprintln!(
            "keepster: the daemon {} is running as pid {}, without a log file",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        ),
    }

    if let Err(error) = stop.requested().await {
        eprintln!("keepster: could not wait for a stop signal: {error}");
        return exit::OTHER;
    }

    match daemon.shutdown().await {
        Ok(()) => exit::SUCCESS,
        Err(error) => {
            eprintln!("keepster: the daemon stopped with an error: {error}");
            exit::OTHER
        }
    }
}

/// What asks the daemon to stop: SIGTERM from systemd or launchd — which is how a service manager stops
/// a service — or an interrupt from a terminal.
///
/// Either way the stop sequence follows: drain the capture in flight, then flush every pending debounce
/// window (ticket 05). Task Scheduler ends a task outright, which the store survives as it survives any
/// kill.
struct StopSignals {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}

impl StopSignals {
    /// Take the signals over now, replacing their default of ending the process.
    fn listen() -> std::io::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            terminate: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?,
        })
    }

    /// Wait until a stop is asked for.
    #[cfg(unix)]
    async fn requested(mut self) -> std::io::Result<()> {
        tokio::select! {
            interrupted = tokio::signal::ctrl_c() => interrupted,
            _ = self.terminate.recv() => Ok(()),
        }
    }

    #[cfg(not(unix))]
    async fn requested(self) -> std::io::Result<()> {
        tokio::signal::ctrl_c().await
    }
}

/// The moment a selector such as `2h` is counted back from.
///
/// Read here rather than in `keepster-cli`, because this crate is the one place allowed to read the
/// clock (ADR 0012); the CLI takes the moment as a parameter like everything else below it.
#[allow(
    clippy::disallowed_methods,
    reason = "ADR 0012: the daemon crate is where a moment enters, and the CLI is linked from it"
)]
fn now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}
