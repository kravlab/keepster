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
        Command::Daemon => {
            let Some(store_base) = keepster_service::default_store_base() else {
                eprintln!("keepster: this platform has no per-user data directory for the store");
                return ExitCode::from(exit::OTHER);
            };

            run_daemon(
                control_path,
                store_base,
                keepster_service::default_config_path(),
            )
            .await
        }
        Command::Status(args) => keepster_cli::status(args, &control_path).await,
        Command::Watch(args) => keepster_cli::watch(args, &control_path).await,
        Command::Roots => keepster_cli::roots(&control_path).await,
        Command::Unwatch(args) => keepster_cli::unwatch(args, &control_path).await,
        Command::Move(args) => keepster_cli::move_root(args, &control_path).await,
        Command::Snapshot(args) => keepster_cli::snapshot(args, &control_path).await,
        Command::Log(args) => keepster_cli::log(args, &control_path, now()).await,
        Command::Cat(args) => keepster_cli::cat(args, &control_path, now()).await,
        Command::Diff(args) => keepster_cli::diff(args, &control_path, now()).await,
        Command::Restore(args) => keepster_cli::restore(args, &control_path, now()).await,
        Command::Explain(args) => keepster_cli::explain(args, &control_path).await,
        Command::Config(args) => keepster_cli::config(args, &control_path).await,
        Command::Reconcile(args) => keepster_cli::reconcile(args, &control_path).await,
    };

    ExitCode::from(code)
}

/// Run the daemon in the foreground until the supervisor stops it.
///
/// Ticket 61 turns this into the full stop sequence — drain the capture in flight, then flush every
/// pending debounce window — and makes the pid file's lock the single-instance authority.
async fn run_daemon(
    control_path: PathBuf,
    store_base: PathBuf,
    config_path: Option<PathBuf>,
) -> u8 {
    let daemon = match keepster_daemon::start(DaemonConfig {
        control_path,
        store_base,
        config_path,
        watch_ceiling: None,
        retry_after: None,
    })
    .await
    {
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
