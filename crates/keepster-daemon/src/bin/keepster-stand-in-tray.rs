//! `keepster-stand-in-tray`: what `keepster-tray` does with the tray's lock and nothing more, for the
//! daemon's own tests of `keepster install` replacing a running tray (issue 49). Built only with the
//! `test-stand-ins` feature, and never released.
//!
//! Like the tray, it takes the lock on `tray.pid` in the runtime directory and writes its pid there, and
//! leaves at once, successfully, when another holds it. Two variables steer it, for the tests:
//! `KEEPSTER_STAND_IN_STARTS` names a file each start appends its pid to, before the lock is tried, so a
//! test can count the trays started; `KEEPSTER_STAND_IN_IGNORE_TERM` makes it ignore `SIGTERM`, as a tray
//! that will not end when asked. `--version` only answers, so that a test can wait until a copy just
//! written can be run.

use std::process::ExitCode;

/// How long the stand-in holds the lock: longer than any test that starts one, which kills it at its
/// end.
#[cfg(unix)]
const HOLD: std::time::Duration = std::time::Duration::from_secs(60);

/// Record this start, then — inside a runtime, so that `SIGTERM` can be caught — take the tray's lock
/// and hold it for [`HOLD`].
#[cfg(unix)]
fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        return ExitCode::SUCCESS;
    }
    if let Some(starts) = std::env::var_os("KEEPSTER_STAND_IN_STARTS") {
        if let Err(error) = record_start(std::path::Path::new(&starts)) {
            eprintln!("keepster-stand-in-tray: the start could not be recorded: {error}");
            return ExitCode::FAILURE;
        }
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("keepster-stand-in-tray: no runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(hold(
        std::env::var_os("KEEPSTER_STAND_IN_IGNORE_TERM").is_some(),
    ))
}

/// Append this process's pid and a newline to `starts`, creating it if needed.
#[cfg(unix)]
fn record_start(starts: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(starts)?;
    writeln!(file, "{}", std::process::id())
}

/// Take the tray's lock as the tray does and hold it for [`HOLD`], leaving at once, successfully, when
/// another holds it. `SIGTERM` ends the process as it ends the tray — or, when `ignore_term`, is caught
/// and ignored: the handler is installed before the lock is taken, so a test that sees the lock held
/// knows the signal can no longer end it, and it stays installed for the life of the process.
#[cfg(unix)]
async fn hold(ignore_term: bool) -> ExitCode {
    use tokio::signal::unix::{SignalKind, signal};

    let _ignored = if ignore_term {
        match signal(SignalKind::terminate()) {
            Ok(ignored) => Some(ignored),
            Err(error) => {
                eprintln!("keepster-stand-in-tray: SIGTERM could not be caught: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    let Some(runtime_dir) = keepster_service::default_runtime_dir() else {
        eprintln!("keepster-stand-in-tray: this platform has no per-user runtime directory");
        return ExitCode::FAILURE;
    };
    if let Err(error) = std::fs::create_dir_all(&runtime_dir) {
        eprintln!("keepster-stand-in-tray: {}: {error}", runtime_dir.display());
        return ExitCode::FAILURE;
    }
    let _lock =
        match keepster_service::lock_pid_file(&runtime_dir.join(keepster_service::TRAY_PID_FILE)) {
            Ok(lock) => lock,
            Err(keepster_service::PidLockError::Held { .. }) => return ExitCode::SUCCESS,
            Err(keepster_service::PidLockError::Io(error)) => {
                eprintln!("keepster-stand-in-tray: the tray's lock could not be taken: {error}");
                return ExitCode::FAILURE;
            }
        };
    tokio::time::sleep(HOLD).await;

    ExitCode::SUCCESS
}

/// No Unix signals and no test that runs it: the tests needing a stand-in tray are Unix only, and this
/// exists so that `--all-features` builds everywhere.
#[cfg(not(unix))]
fn main() -> ExitCode {
    eprintln!("keepster-stand-in-tray: only Unix has a test that runs it");
    ExitCode::FAILURE
}
