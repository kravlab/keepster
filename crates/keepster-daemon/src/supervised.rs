//! `keepster service`, and the CLI's way to a daemon that is not running (ticket 61).
//!
//! Part of the binary rather than of `keepster-cli`: registering a unit with the platform is
//! `keepster-service`'s, and the CLI stays a thin client over the API. Only the service manager ever
//! starts a daemon, so a running daemon is always a supervised one: when none answers, the platform is
//! asked, and no daemon is ever forked from here.

use std::path::Path;
use std::time::{Duration, Instant};

use keepster_api::Status;
use keepster_cli::{DaemonClient, ServiceCommand, ServiceInstallArgs, exit};
use keepster_service::autostart::{self, Autostart, tray};

/// How long a daemon the service manager was asked to start has to answer: its start recovers every
/// Root's index first, which on a large store is not instant.
const START_PATIENCE: Duration = Duration::from_secs(20);

/// What to tell a person whose autostart was never installed.
const INSTALL_HINT: &str = "run `keepster service install`";

/// The command a person would run to start the daemon by hand, for when asking for them fails.
fn start_command() -> String {
    if cfg!(target_os = "linux") {
        format!("systemctl --user start {}", autostart::SYSTEMD_UNIT)
    } else if cfg!(target_os = "macos") {
        format!(
            "launchctl kickstart gui/$(id -u)/{}",
            autostart::LAUNCH_AGENT
        )
    } else {
        format!("schtasks /Run /TN {}", autostart::SCHEDULED_TASK)
    }
}

/// Make sure a daemon answers at `control_path` before a command that needs one runs: ask the
/// service manager to start it when its unit is installed, and say what to do when it is not.
pub(crate) async fn reach(control_path: &Path) -> Result<(), u8> {
    if keepster_service::daemon_answers(control_path) {
        return Ok(());
    }
    if autostart::autostart() == Autostart::NotInstalled {
        eprintln!(
            "keepster: the daemon is not running, and its autostart was never installed: \
             {INSTALL_HINT}"
        );
        return Err(exit::DAEMON_UNREACHABLE);
    }

    eprintln!("keepster: the daemon is not running; asking the service manager to start it");
    if let Err(error) = blocking(autostart::start).await {
        eprintln!(
            "keepster: the service manager did not start it ({error}); `{}` is what was asked, and \
             `keepster service status` says more",
            start_command()
        );
        return Err(exit::DAEMON_UNREACHABLE);
    }
    if !answers_within(control_path, START_PATIENCE).await {
        eprintln!(
            "keepster: the daemon was started but did not answer within {}s; `keepster service \
             status` says more",
            START_PATIENCE.as_secs()
        );
        return Err(exit::DAEMON_UNREACHABLE);
    }

    Ok(())
}

/// Say why no daemon answers at `control_path`, without starting one: `status` reports what is true
/// now, and a daemon it had just started would report health that was not there a moment before.
pub(crate) fn unreachable(control_path: &Path) -> u8 {
    let why = match autostart::autostart() {
        Autostart::NotInstalled => format!("its autostart was never installed: {INSTALL_HINT}"),
        Autostart::Installed | Autostart::Disabled => format!(
            "its autostart is installed but not running; any other command asks the service manager \
             to start it, or run `{}`",
            start_command()
        ),
        Autostart::Unknown(why) => format!("whether its autostart is installed is unknown: {why}"),
    };
    eprintln!(
        "keepster: no daemon is listening on {}; {why}",
        control_path.display()
    );

    exit::DAEMON_UNREACHABLE
}

/// Run one `keepster service` command.
pub(crate) async fn service(command: &ServiceCommand, control_path: &Path) -> u8 {
    match command {
        ServiceCommand::Install(args) => install(args, control_path).await,
        ServiceCommand::Uninstall => uninstall().await,
        ServiceCommand::Status => status(control_path).await,
        ServiceCommand::Start => ask("started", autostart::start).await,
        ServiceCommand::Stop => ask("stopped", autostart::stop).await,
        ServiceCommand::Restart => ask("restarted", autostart::restart).await,
    }
}

/// Ask the service manager for `step`, and say whether it did it: `done` is what it did, said of the
/// daemon.
async fn ask(
    done: &str,
    step: impl FnOnce() -> Result<(), autostart::ServiceError> + Send + 'static,
) -> u8 {
    match blocking(step).await {
        Ok(()) => {
            println!("the service manager {done} the daemon");
            exit::SUCCESS
        }
        Err(error) => {
            eprintln!("keepster: {error}");
            exit::OTHER
        }
    }
}

async fn install(args: &ServiceInstallArgs, control_path: &Path) -> u8 {
    // A daemon no service manager started holds the pid file's lock, so the unit's own daemon would be
    // refused — and a check that "a daemon answers" would then pass for a supervised daemon that does
    // not exist. Only one with no unit installed can be such a daemon; a reinstall replaces its own.
    if autostart::autostart() == Autostart::NotInstalled {
        if let Some(pid) = running_pid(control_path).await {
            let pid = pid.map_or_else(String::new, |pid| format!(" (pid {pid})"));
            eprintln!(
                "keepster: a daemon no service manager started is running{pid}; stop it, then run \
                 `keepster service install` again, so that the daemon running is the supervised one"
            );
            return exit::STATE_CONFLICT;
        }
    }
    // Before the daemon is started, so a machine declared to have no web UI never binds the port.
    if args.no_web_ui {
        declare_no_web_ui();
    }
    // As the platform names it, not canonicalised: a package manager's link outlives the versioned
    // directory it points into, and Windows would answer with a verbatim `\\?\` path.
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("keepster: this program's own path could not be read: {error}");
            return exit::OTHER;
        }
    };

    let tray_exe = tray::beside(&exe);
    let installed = match blocking(move || autostart::install(&exe)).await {
        Ok(installed) => installed,
        Err(error) => {
            eprintln!("keepster: autostart was not installed: {error}");
            return exit::OTHER;
        }
    };
    println!("installed {}", installed.unit);
    // A tray that cannot be installed leaves the daemon installed: the insurance is the point, and the
    // tray only shows it. The exit code says that something asked for was not done.
    let mut code = exit::SUCCESS;
    if !args.no_tray {
        match blocking(move || tray::install(&tray_exe)).await {
            Ok(installed) => println!("{}", tray_installed(&installed)),
            Err(error) => {
                eprintln!("keepster: the tray's autostart was not installed: {error}");
                code = exit::OTHER;
            }
        }
    }

    if let Ok(Err(refused)) = tokio::task::spawn_blocking(autostart::enable_linger).await {
        println!(
            "linger could not be switched on ({}), so coverage ends at logout; an administrator can \
             run: {}",
            refused.detail, refused.command
        );
    }

    // Read back as `status` reads it — the unit in place and enabled — and the daemon answering. A
    // daemon no manager started was refused above, so the one answering is the unit's.
    if autostart::autostart() != Autostart::Installed {
        eprintln!(
            "keepster: the unit does not read as installed and enabled; `keepster service status` \
             says more"
        );
        return exit::OTHER;
    }
    if !answers_within(control_path, START_PATIENCE).await {
        eprintln!(
            "keepster: the unit is installed, but the daemon did not answer within {}s; `keepster \
             service status` says more",
            START_PATIENCE.as_secs()
        );
        return exit::OTHER;
    }
    println!("the daemon is running, and starts again at every login");
    if autostart::linger() == autostart::Linger::LogoutGap {
        println!("coverage ends at logout: this platform stops a user's agents when they log out");
    }

    code
}

/// What installing the tray's autostart did, said honestly of a machine with no graphical session now.
fn tray_installed(installed: &tray::TrayInstalled) -> String {
    if installed.started {
        format!(
            "installed {}; the tray is running, and starts again at every graphical login",
            installed.entry
        )
    } else {
        format!(
            "installed {}; there is no graphical session now, so the tray starts at the next \
             graphical login",
            installed.entry
        )
    }
}

/// Write the global configuration of a machine without a web UI, only if there is none (ticket 24).
fn declare_no_web_ui() {
    let Some(path) = keepster_service::default_config_path() else {
        eprintln!(
            "keepster: this platform has no per-user configuration directory, so `--no-web-ui` \
             wrote nothing"
        );
        return;
    };
    match keepster_service::declare_no_web_ui(&path) {
        Ok(keepster_service::NoWebUi::Created) => {
            println!(
                "wrote {}: the daemon has no web UI and no network surface",
                path.display()
            );
        }
        Ok(keepster_service::NoWebUi::Existing { add }) => println!(
            "{} exists and was left as it is; for no web UI, add `{add}` under its `[ui]` table, \
             then run `keepster service restart`",
            path.display()
        ),
        Err(error) => eprintln!(
            "keepster: {} could not be written: {error}; the web UI stays on",
            path.display()
        ),
    }
}

async fn uninstall() -> u8 {
    let daemon = match blocking(autostart::uninstall).await {
        Ok(Some(unit)) => {
            println!(
                "removed {unit}; the daemon is stopped. History, Roots and configuration are left as \
                 they are"
            );
            exit::SUCCESS
        }
        Ok(None) => {
            println!("autostart is not installed; nothing was removed");
            exit::SUCCESS
        }
        Err(error) => {
            eprintln!("keepster: autostart was not removed: {error}");
            exit::OTHER
        }
    };
    // The tray's goes whatever became of the daemon's: a tray started at login for no daemon shows
    // nothing but its absence.
    let tray = match blocking(tray::uninstall).await {
        Ok(Some(entry)) => {
            println!("removed {entry}; the tray no longer starts at login");
            exit::SUCCESS
        }
        Ok(None) => exit::SUCCESS,
        Err(error) => {
            eprintln!("keepster: the tray's autostart was not removed: {error}");
            exit::OTHER
        }
    };

    daemon.max(tray)
}

/// Run a platform step off the async runtime: each one starts a process and waits for it.
async fn blocking<T: Send + 'static>(
    step: impl FnOnce() -> Result<T, autostart::ServiceError> + Send + 'static,
) -> Result<T, autostart::ServiceError> {
    tokio::task::spawn_blocking(step)
        .await
        .unwrap_or_else(|error| {
            Err(autostart::ServiceError {
                step: "run a platform step".to_owned(),
                detail: error.to_string(),
            })
        })
}

/// What the platform says about the unit, whether a daemon answers, and whether coverage survives
/// logout. Exits `0` only when the daemon is running and comes back at login.
async fn status(control_path: &Path) -> u8 {
    let installed = tokio::task::spawn_blocking(autostart::autostart)
        .await
        .unwrap_or_else(|error| Autostart::Unknown(error.to_string()));
    let unit = autostart::unit_location()
        .map(|unit| format!(" ({})", unit.display()))
        .unwrap_or_default();
    let (shown, mut code) = match &installed {
        Autostart::Installed => (format!("installed{unit}"), exit::SUCCESS),
        Autostart::Disabled => (
            format!(
                "installed{unit} but disabled — the daemon will not come back after a reboot; {INSTALL_HINT}"
            ),
            exit::STATE_CONFLICT,
        ),
        Autostart::NotInstalled => (format!("not installed — {INSTALL_HINT}"), exit::NOT_FOUND),
        Autostart::Unknown(why) => (format!("unknown: {why}"), exit::OTHER),
    };
    println!("autostart  {shown}");

    let running = running_pid(control_path).await;
    match running {
        Some(Some(pid)) => println!("daemon     running (pid {pid})"),
        Some(None) => println!("daemon     running"),
        None => {
            println!("daemon     not running");
            if code == exit::SUCCESS {
                code = exit::DAEMON_UNREACHABLE;
            }
        }
    }

    let (row, reason) = match autostart::linger() {
        autostart::Linger::On => (keepster_api::Reported::Available, None),
        autostart::Linger::Off => (keepster_api::Reported::Unavailable, Some("disabled")),
        autostart::Linger::LogoutGap => (keepster_api::Reported::Unavailable, Some("logout_gap")),
        autostart::Linger::Unknown => (keepster_api::Reported::Unknown, None),
    };
    let linger = keepster_cli::render::linger(row, reason);
    println!("linger     {linger}");

    code
}

/// The pid of the daemon answering at `control_path` — `Some(None)` for one that answers without
/// saying — or `None` when none answers.
async fn running_pid(control_path: &Path) -> Option<Option<u32>> {
    if !keepster_service::daemon_answers(control_path) {
        return None;
    }
    let pid = DaemonClient::new(control_path)
        .get("/v1/status")
        .await
        .ok()
        .and_then(|response| serde_json::from_slice::<Status>(&response.body).ok())
        .map(|status| status.daemon.pid);

    Some(pid)
}

/// Whether a daemon answers at `path` within `patience`, asking every tenth of a second.
async fn answers_within(path: &Path, patience: Duration) -> bool {
    let deadline = Instant::now() + patience;
    loop {
        if keepster_service::daemon_answers(path) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use keepster_service::autostart::tray::TrayInstalled;

    use super::tray_installed;

    #[test]
    fn a_tray_started_now_is_said_to_be_running() {
        let said = tray_installed(&TrayInstalled {
            entry: "/home/u/.config/autostart/keepster-tray.desktop".to_owned(),
            started: true,
        });

        assert_eq!(
            said,
            "installed /home/u/.config/autostart/keepster-tray.desktop; the tray is running, and \
             starts again at every graphical login"
        );
    }

    #[test]
    fn without_a_graphical_session_the_install_says_when_the_tray_will_start() {
        let said = tray_installed(&TrayInstalled {
            entry: "/home/u/.config/autostart/keepster-tray.desktop".to_owned(),
            started: false,
        });

        assert!(said.contains("no graphical session now"), "{said}");
        assert!(said.contains("next graphical login"), "{said}");
    }
}
