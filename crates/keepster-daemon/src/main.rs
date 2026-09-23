//! The one shipped binary. The daemon and the CLI are a single artifact, and this crate owns it
//! because ticket 08 makes it the wiring crate; `keepster-cli` stays a library with no store
//! dependency, which `cargo tree -p keepster-cli` is the check for.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use keepster_cli::{Cli, Command, exit};
use keepster_daemon::DaemonConfig;

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

    let code = match &cli.command {
        Command::Daemon => run_daemon(control_path).await,
        Command::Status(args) => keepster_cli::status(args, &control_path).await,
    };

    ExitCode::from(code)
}

/// Run the daemon in the foreground until the supervisor stops it.
///
/// Ticket 61 turns this into the full stop sequence — drain the capture in flight, then flush every
/// pending debounce window — and makes the pid file's lock the single-instance authority.
async fn run_daemon(control_path: PathBuf) -> u8 {
    let daemon = match keepster_daemon::start(DaemonConfig { control_path }).await {
        Ok(daemon) => daemon,
        Err(error) => {
            eprintln!("keepster: could not start the daemon: {error}");
            return exit::OTHER;
        }
    };

    if let Err(error) = tokio::signal::ctrl_c().await {
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
