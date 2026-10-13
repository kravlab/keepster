// A tray is no console program: on Windows it gets no console window of its own.
#![cfg_attr(windows, windows_subsystem = "windows")]

//! `keepster-tray`, started at the user's graphical login by `keepster install` (tickets 67 and 68).

use std::process::ExitCode;

use clap::Parser;
use keepster_tray::log::TrayLog;
use keepster_tray::{model, platform, run};
use tokio::sync::mpsc;

/// `keepster-tray`: no options besides `--help` and `--version`; it finds the daemon where `keepster`
/// does.
#[derive(Debug, Parser)]
#[command(name = "keepster-tray", version, about = "Keepster's tray")]
struct Args {}

/// Take the tray's lock and log, then run the platform's shell with the connection until Quit.
fn main() -> ExitCode {
    Args::parse();
    let (Some(control_path), Some(runtime_dir)) = (
        keepster_service::default_control_path(),
        keepster_service::default_runtime_dir(),
    ) else {
        eprintln!("keepster-tray: this platform has no per-user runtime directory");
        return ExitCode::FAILURE;
    };
    if let Err(error) = std::fs::create_dir_all(&runtime_dir) {
        eprintln!("keepster-tray: {}: {error}", runtime_dir.display());
        return ExitCode::FAILURE;
    }
    let _lock =
        match keepster_service::lock_pid_file(&runtime_dir.join(keepster_service::TRAY_PID_FILE)) {
            Ok(lock) => lock,
            Err(keepster_service::PidLockError::Held { .. }) => return ExitCode::SUCCESS,
            Err(keepster_service::PidLockError::Io(error)) => {
                eprintln!("keepster-tray: the tray's lock could not be taken: {error}");
                return ExitCode::FAILURE;
            }
        };
    // A tray that cannot write its log still shows the insurance: the log only explains it.
    let log = keepster_service::default_log_dir().and_then(|directory| {
        TrayLog::install(&directory, model::DEFAULT_LOG_LEVEL)
            .map_err(|error| eprintln!("keepster-tray: the log could not be opened: {error}"))
            .ok()
    });
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        pid = std::process::id(),
        "the tray is starting"
    );

    let (actions, chosen) = mpsc::unbounded_channel();
    platform::shell(actions).run(Box::new(move |adapter| {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(run::run(control_path, adapter, chosen, log.as_ref())),
            Err(error) => tracing::error!(%error, "the tray's runtime could not be started"),
        }
    }));

    ExitCode::SUCCESS
}
