//! `keepster install`, `keepster uninstall` and `keepster service`, and the CLI's way to a daemon that is
//! not running (tickets 61 and 68), in a sandbox whose service manager cannot be reached: what is asserted
//! is what Keepster decides and says, never what a real systemd, launchd or Task Scheduler does —
//! registering a unit changes the machine a test runs on.
#![cfg(unix)]

#[cfg(target_os = "linux")]
use std::path::Path;

use crate::cli_lifecycle::Sandbox;

/// A command's output as text, whatever bytes it held.
pub(crate) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// What a person would have after `keepster install` on Linux, as files: the unit, and the link
/// enabling it.
#[cfg(target_os = "linux")]
fn installed_unit(sandbox: &Sandbox) {
    let units = sandbox.unit_dir();
    std::fs::create_dir_all(units.join("default.target.wants")).expect("the unit directories");
    let unit = units.join("keepster.service");
    std::fs::write(
        &unit,
        keepster_service::autostart::systemd_unit(Path::new("/usr/bin/keepster")),
    )
    .expect("the unit");
    std::os::unix::fs::symlink(&unit, units.join("default.target.wants/keepster.service"))
        .expect("enabled");
}

#[tokio::test]
async fn with_no_daemon_and_no_unit_a_command_points_at_the_install_and_starts_nothing() {
    let sandbox = Sandbox::new();

    let run = sandbox.run(&["roots"]).await;

    assert_eq!(run.status.code(), Some(3), "daemon unreachable");
    let stderr = text(&run.stderr);
    assert!(stderr.contains("keepster install"), "{stderr}");
    assert!(
        !sandbox.control_path().exists(),
        "no daemon was forked behind the service manager's back"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn with_a_unit_installed_a_command_asks_the_service_manager_and_never_forks_a_daemon() {
    let sandbox = Sandbox::new();
    installed_unit(&sandbox);

    let run = sandbox.run(&["roots"]).await;

    assert_eq!(run.status.code(), Some(3));
    let stderr = text(&run.stderr);
    assert!(stderr.contains("asking the service manager"), "{stderr}");
    assert!(
        stderr.contains("systemctl --user start keepster.service"),
        "{stderr}"
    );
    assert!(
        !sandbox.control_path().exists(),
        "the sandbox has no service manager to start one"
    );
}

#[tokio::test]
async fn status_never_starts_a_daemon_and_says_autostart_was_never_installed() {
    let sandbox = Sandbox::new();

    let run = sandbox.run(&["status"]).await;

    assert_eq!(run.status.code(), Some(3));
    let stderr = text(&run.stderr);
    assert!(stderr.contains("never installed"), "{stderr}");
    assert!(!stderr.contains("asking the service manager"), "{stderr}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn status_tells_a_stopped_service_from_one_never_installed() {
    let sandbox = Sandbox::new();
    installed_unit(&sandbox);

    let run = sandbox.run(&["status"]).await;

    assert_eq!(run.status.code(), Some(3));
    let stderr = text(&run.stderr);
    assert!(stderr.contains("installed but not running"), "{stderr}");
    assert!(!stderr.contains("never installed"), "{stderr}");
}

#[tokio::test]
async fn service_status_reports_a_unit_never_installed_as_not_found() {
    let sandbox = Sandbox::new();

    let run = sandbox.run(&["service", "status"]).await;

    assert_eq!(run.status.code(), Some(4), "{}", text(&run.stderr));
    let stdout = text(&run.stdout);
    assert!(stdout.contains("not installed"), "{stdout}");
    assert!(stdout.contains("keepster install"), "{stdout}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn service_status_tells_stopped_from_running() {
    let sandbox = Sandbox::new();
    installed_unit(&sandbox);

    let stopped = sandbox.run(&["service", "status"]).await;
    let daemon = sandbox.daemon().await;
    let running = sandbox.run(&["service", "status"]).await;

    assert_eq!(stopped.status.code(), Some(3), "{}", text(&stopped.stderr));
    assert!(
        text(&stopped.stdout).contains("not running"),
        "{}",
        text(&stopped.stdout)
    );
    assert_eq!(running.status.code(), Some(0), "{}", text(&running.stderr));
    let shown = text(&running.stdout);
    assert!(
        shown.contains(&format!("running (pid {})", daemon.id().expect("running"))),
        "{shown}"
    );
}

#[tokio::test]
async fn the_status_rows_say_whether_the_daemon_comes_back_and_survives_logout() {
    let sandbox = Sandbox::new();
    let _daemon = sandbox.daemon().await;

    let status = crate::cli_lifecycle::json(&sandbox.client(), "/v1/status").await;
    let shown = sandbox.run(&["status"]).await;

    let daemon = &status["daemon"];
    assert_eq!(daemon["autostart"], "unavailable", "{status}");
    assert_eq!(daemon["autostart_reason"], "not_installed", "{status}");
    assert_eq!(daemon["linger"], "unavailable", "{status}");
    let human = text(&shown.stdout);
    assert!(human.contains("keepster install"), "{human}");
    if cfg!(target_os = "macos") {
        assert_eq!(daemon["linger_reason"], "logout_gap", "{status}");
        assert!(human.contains("coverage ends at logout"), "{human}");
    } else {
        assert_eq!(daemon["linger_reason"], "disabled", "{status}");
        assert!(human.contains("loginctl enable-linger"), "{human}");
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_installed_and_enabled_unit_reads_as_autostart_available() {
    let sandbox = Sandbox::new();
    installed_unit(&sandbox);
    let _daemon = sandbox.daemon().await;

    let status = crate::cli_lifecycle::json(&sandbox.client(), "/v1/status").await;

    assert_eq!(status["daemon"]["autostart"], "available", "{status}");
    assert!(
        status["daemon"].get("autostart_reason").is_none(),
        "{status}"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_install_the_service_manager_refuses_leaves_no_unit_behind() {
    let sandbox = Sandbox::new();

    let run = sandbox.run(&["install"]).await;

    assert_eq!(run.status.code(), Some(1), "{}", text(&run.stderr));
    assert!(
        text(&run.stderr).contains("systemctl"),
        "{}",
        text(&run.stderr)
    );
    assert!(
        !sandbox.unit_dir().join("keepster.service").exists(),
        "a unit systemd does not run would read as installed"
    );
    assert!(
        !sandbox.tray_entry().exists(),
        "no tray is started at login for a daemon that is not"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn install_takes_a_choice_of_no_tray() {
    let sandbox = Sandbox::new();

    let run = sandbox.run(&["install", "--no-tray"]).await;

    assert_eq!(
        run.status.code(),
        Some(1),
        "refused by the service manager, not by the parser: {}",
        text(&run.stderr)
    );
    assert!(!sandbox.tray_entry().exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_service_verbs_ask_the_service_manager_and_say_what_it_answered() {
    let sandbox = Sandbox::new();

    for verb in ["start", "stop", "restart"] {
        let run = sandbox.run(&["service", verb]).await;

        assert_eq!(run.status.code(), Some(1), "{verb}: {}", text(&run.stderr));
        let stderr = text(&run.stderr);
        assert!(
            stderr.contains(&format!("{verb} the unit")) && stderr.contains("systemctl"),
            "{verb}: {stderr}"
        );
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn uninstall_removes_the_trays_autostart_entry_too() {
    let sandbox = Sandbox::new();
    let entry = sandbox.tray_entry();
    std::fs::create_dir_all(entry.parent().expect("a parent")).expect("its directory");
    std::fs::write(
        &entry,
        keepster_service::autostart::tray::desktop_entry(Path::new("/usr/bin/keepster-tray")),
    )
    .expect("the entry");

    let run = sandbox.run(&["uninstall"]).await;

    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    assert!(!entry.exists());
    assert!(
        text(&run.stdout).contains(&format!("removed {}", entry.display())),
        "{}",
        text(&run.stdout)
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn install_without_the_web_ui_creates_a_configuration_with_the_listener_off() {
    let sandbox = Sandbox::new();

    sandbox.run(&["install", "--no-web-ui"]).await;

    let written = std::fs::read_to_string(sandbox.config_path()).expect("created");
    assert!(written.contains("[ui]\nlistener = false\n"), "{written}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn install_without_the_web_ui_leaves_an_existing_configuration_and_names_the_line() {
    let sandbox = Sandbox::new();
    let mine = b"# mine\n[files]\nexclude = [\"*.log\"]\n";
    sandbox.write_config(mine);

    let run = sandbox.run(&["install", "--no-web-ui"]).await;

    assert_eq!(
        std::fs::read(sandbox.config_path()).expect("the file"),
        mine
    );
    let said = format!("{}{}", text(&run.stdout), text(&run.stderr));
    assert!(said.contains("listener = false"), "{said}");
    assert!(said.contains("[ui]"), "{said}");
    assert!(said.contains("`keepster service restart`"), "{said}");
}

#[tokio::test]
async fn uninstalling_what_was_never_installed_changes_nothing_and_says_so() {
    let sandbox = Sandbox::new();

    let run = sandbox.run(&["uninstall"]).await;

    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    assert!(
        text(&run.stdout).contains("not installed"),
        "{}",
        text(&run.stdout)
    );
}

#[tokio::test]
async fn install_refuses_beside_a_daemon_no_service_manager_started() {
    // The unit's daemon would be refused by the bare one's lock, and an install that then saw "a daemon
    // answers" would report a supervised daemon that does not exist.
    let sandbox = Sandbox::new();
    let bare = sandbox.daemon().await;

    let run = sandbox.run(&["install"]).await;

    assert_eq!(run.status.code(), Some(5), "{}", text(&run.stderr));
    let stderr = text(&run.stderr);
    assert!(
        stderr.contains(&format!("pid {}", bare.id().expect("running"))),
        "{stderr}"
    );
    #[cfg(target_os = "linux")]
    assert!(
        !sandbox.unit_dir().join("keepster.service").exists(),
        "nothing was registered"
    );
}

#[tokio::test]
async fn service_no_longer_installs_or_uninstalls() {
    let sandbox = Sandbox::new();

    for verb in ["install", "uninstall"] {
        let run = sandbox.run(&["service", verb]).await;

        assert_eq!(
            run.status.code(),
            Some(2),
            "`keepster {verb}` replaced it: {}",
            text(&run.stderr)
        );
    }
}

/// The runtime directory a daemon and a tray of this sandbox leave behind, with nothing holding it.
fn left_runtime_directory(sandbox: &Sandbox) -> std::path::PathBuf {
    let runtime = sandbox
        .control_path()
        .parent()
        .expect("the runtime directory")
        .to_path_buf();
    std::fs::create_dir_all(&runtime).expect("the runtime directory");
    for name in [
        "daemon.sock",
        "ui-token",
        keepster_service::PID_FILE,
        keepster_service::TRAY_PID_FILE,
    ] {
        std::fs::write(runtime.join(name), b"left").expect("a leftover");
    }
    runtime
}

#[tokio::test]
async fn uninstall_removes_the_runtime_directory() {
    let sandbox = Sandbox::new();
    let runtime = left_runtime_directory(&sandbox);

    let run = sandbox.run(&["uninstall"]).await;

    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    assert!(!runtime.exists(), "nothing of a daemon's start is left");
    assert!(
        text(&run.stdout).contains(&format!("removed {}", runtime.display())),
        "{}",
        text(&run.stdout)
    );
}

#[tokio::test]
async fn uninstall_leaves_the_lock_of_a_tray_still_running() {
    // A tray running now goes on until its session ends; unlinking its lock would let a second one start
    // beside it.
    let sandbox = Sandbox::new();
    let runtime = left_runtime_directory(&sandbox);
    let tray = keepster_service::lock_pid_file(&runtime.join(keepster_service::TRAY_PID_FILE))
        .expect("the tray's lock");

    let run = sandbox.run(&["uninstall"]).await;

    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    assert!(runtime.join(keepster_service::TRAY_PID_FILE).exists());
    for gone in ["daemon.sock", "ui-token", keepster_service::PID_FILE] {
        assert!(!runtime.join(gone).exists(), "{gone} is left");
    }
    drop(tray);
}

#[tokio::test]
async fn uninstall_leaves_the_runtime_directory_of_a_daemon_still_running() {
    // One no service manager started, which no unit removal stops: its socket and its lock are its own.
    let sandbox = Sandbox::new();
    let _bare = sandbox.daemon().await;

    let run = sandbox.run(&["uninstall"]).await;

    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    assert!(
        keepster_service::daemon_answers(&sandbox.control_path()),
        "the daemon still answers"
    );
}

/// `keepster install` beside a tray already running (issue 49): the tray left running is the one beside
/// the `keepster` installed. The installation is a copy of the binary under test with a copy of
/// `keepster-stand-in-tray` beside it as `keepster-tray`, and the service manager it asks — `systemctl`
/// and `loginctl` on Linux, `launchctl` on macOS — is a stand-in too, first on `PATH`, so that the
/// machine's own is never asked anything.
///
/// The stand-in tray is built only with the `test-stand-ins` feature, which the merge gate's
/// `--all-features` enables; without it these tests are left out.
#[cfg(all(
    feature = "test-stand-ins",
    any(target_os = "linux", target_os = "macos")
))]
mod tray_replacement {
    use std::ffi::OsString;
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;

    use keepster_service::{Pid, PidLockHolder, TRAY_PID_FILE, pid_lock_holder};

    use super::text;
    use crate::cli_lifecycle::{Sandbox, eventually};
    use crate::jobs::{install_script, wait_until_runnable};

    /// The stand-in for `keepster-tray`, which takes the tray's lock as the tray does.
    const STAND_IN_TRAY: &str = env!("CARGO_BIN_EXE_keepster-stand-in-tray");

    /// Put a copy of the executable `from` at `to`, by a rename so that nothing runs a half-written
    /// file, and wait until it can be run.
    fn install_copy(from: &Path, to: &Path) {
        let copied = to.with_extension("new");
        std::fs::copy(from, &copied).expect("a copy");
        std::fs::rename(&copied, to).expect("in place");
        wait_until_runnable(to);
    }

    /// `keepster` as installed into the sandbox, and the `PATH` it runs with.
    struct Installation {
        keepster: PathBuf,
        path: OsString,
    }

    impl Installation {
        /// A copy of the binary under test in the sandbox's `bin`, the stand-in tray beside it, and the
        /// platform's service manager stand-in first on `PATH`.
        fn new(sandbox: &Sandbox) -> Self {
            let bin = sandbox.scratch("bin");
            let keepster = bin.join("keepster");
            install_copy(Path::new(env!("CARGO_BIN_EXE_keepster")), &keepster);
            let tray = bin.join("keepster-tray");
            install_copy(Path::new(STAND_IN_TRAY), &tray);

            let stubs = sandbox.scratch("stubs");
            service_manager(sandbox, &stubs, &keepster, &tray);
            let mut path = OsString::from(stubs);
            if let Some(inherited) = std::env::var_os("PATH") {
                path.push(":");
                path.push(inherited);
            }

            Self { keepster, path }
        }

        /// Run the installed `keepster` with `args` in `sandbox`, in a graphical session, with every
        /// tray it starts recording its start in [`tray_starts`].
        async fn run(&self, sandbox: &Sandbox, args: &[&str]) -> std::process::Output {
            sandbox
                .command_of(&self.keepster, args)
                .env("PATH", &self.path)
                .env("DISPLAY", ":0-sandbox")
                .env_remove("WAYLAND_DISPLAY")
                .env("KEEPSTER_STAND_IN_STARTS", tray_starts(sandbox))
                .output()
                .await
                .unwrap_or_else(|error| panic!("running keepster {args:?}: {error}"))
        }
    }

    /// A `systemctl` and a `loginctl` in `stubs` that succeed at doing nothing: no unit is enabled, so
    /// the install ends without waiting for a daemon.
    #[cfg(target_os = "linux")]
    fn service_manager(_sandbox: &Sandbox, stubs: &Path, _keepster: &Path, _tray: &Path) {
        for program in ["systemctl", "loginctl"] {
            install_script(&stubs.join(program), "#!/bin/sh\nexit 0\n");
        }
    }

    /// A `launchctl` in `stubs` that says this is an Aqua session, records every call in
    /// [`launchctl_calls`], and does what launchd would when an agent is loaded: starts `keepster
    /// daemon` for the daemon's agent, and `tray` for the tray's, each in the background with nothing of
    /// the install's output. `bootout` and anything else succeed at doing nothing.
    #[cfg(target_os = "macos")]
    fn service_manager(sandbox: &Sandbox, stubs: &Path, keepster: &Path, tray: &Path) {
        install_script(
            &stubs.join("launchctl"),
            &format!(
                "#!/bin/sh\n\
                 [ \"$1\" = --version ] && exit 0\n\
                 echo \"$*\" >> '{calls}'\n\
                 case \"$1\" in\n\
                 managername) echo Aqua ;;\n\
                 bootstrap)\n\
                 case \"$3\" in\n\
                 *{daemon_agent}.plist) '{keepster}' daemon </dev/null >/dev/null 2>&1 & ;;\n\
                 *{tray_agent}.plist) '{tray}' </dev/null >/dev/null 2>&1 & ;;\n\
                 esac ;;\n\
                 esac\n\
                 exit 0\n",
                calls = launchctl_calls(sandbox).display(),
                daemon_agent = keepster_service::autostart::LAUNCH_AGENT,
                tray_agent = keepster_service::autostart::tray::LAUNCH_AGENT,
                keepster = keepster.display(),
                tray = tray.display(),
            ),
        );
    }

    /// Where the `launchctl` stand-in records its calls, one per line.
    #[cfg(target_os = "macos")]
    fn launchctl_calls(sandbox: &Sandbox) -> PathBuf {
        sandbox.scratch("records").join("launchctl")
    }

    /// The tray's lock in `sandbox`'s runtime directory.
    fn tray_lock(sandbox: &Sandbox) -> PathBuf {
        sandbox.control_path().with_file_name(TRAY_PID_FILE)
    }

    /// Where every stand-in tray of `sandbox` records its start.
    fn tray_starts(sandbox: &Sandbox) -> PathBuf {
        sandbox.scratch("records").join("tray-starts")
    }

    /// The pids of every stand-in tray started in `sandbox` so far, in the order they started.
    fn started(sandbox: &Sandbox) -> Vec<Pid> {
        std::fs::read_to_string(tray_starts(sandbox))
            .unwrap_or_default()
            .lines()
            .map(|line| {
                line.parse()
                    .ok()
                    .and_then(Pid::new)
                    .unwrap_or_else(|| panic!("a pid recorded: {line:?}"))
            })
            .collect()
    }

    /// Start a stand-in tray from an older install — one that ignores `SIGTERM` when `ignore_term` — and
    /// wait until it holds the tray's lock under its own pid. Killed when dropped.
    async fn old_tray(sandbox: &Sandbox, ignore_term: bool) -> (tokio::process::Child, Pid) {
        let old = sandbox.scratch("old").join("keepster-tray");
        let lock = tray_lock(sandbox);
        install_copy(Path::new(STAND_IN_TRAY), &old);
        let mut command = sandbox.command_of(&old, &[]);
        command
            .env("KEEPSTER_STAND_IN_STARTS", tray_starts(sandbox))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if ignore_term {
            command.env("KEEPSTER_STAND_IN_IGNORE_TERM", "1");
        }
        let child = command.spawn().expect("the old tray starts");
        let pid = Pid::new(child.id().expect("running")).expect("a child's pid");
        eventually("the old tray to hold the lock", || async {
            (pid_lock_holder(&lock) == PidLockHolder::Held { pid: Some(pid) }).then_some(())
        })
        .await;

        (child, pid)
    }

    /// The tray holding the lock other than `old`, once one does.
    async fn new_tray(lock: &Path, old: Option<Pid>) -> Detached {
        eventually("a new tray to hold the lock", || async {
            match pid_lock_holder(lock) {
                PidLockHolder::Held { pid: Some(pid) } if Some(pid) != old => Some(Detached(pid)),
                _ => None,
            }
        })
        .await
    }

    /// A tray — or, on macOS, a daemon — the install started on its own, which no test may leave running:
    /// killed when dropped.
    struct Detached(Pid);

    impl Drop for Detached {
        /// Kill it outright; it is not this test's child, so whoever adopted it reaps it.
        fn drop(&mut self) {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &self.0.to_string()])
                .status();
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn install_ends_the_running_tray_and_leaves_the_new_one_holding_the_lock() {
        let sandbox = Sandbox::new();
        let installation = Installation::new(&sandbox);
        let (mut old, old_pid) = old_tray(&sandbox, false).await;

        let run = installation.run(&sandbox, &["install"]).await;

        let ended = old.wait().await.expect("the old tray is reaped");
        assert_eq!(ended.signal(), Some(15), "asked to end: {ended}");
        let new = new_tray(&tray_lock(&sandbox), Some(old_pid)).await;
        assert_ne!(new.0, old_pid);
        assert_eq!(
            started(&sandbox),
            [old_pid, new.0],
            "one new tray, started once"
        );
        let stdout = text(&run.stdout);
        assert!(stdout.contains("the tray is running"), "{stdout}");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn with_no_tray_running_install_starts_one_as_before() {
        let sandbox = Sandbox::new();
        let installation = Installation::new(&sandbox);

        let run = installation.run(&sandbox, &["install"]).await;

        let new = new_tray(&tray_lock(&sandbox), None).await;
        assert_eq!(started(&sandbox), [new.0]);
        let stdout = text(&run.stdout);
        assert!(
            stdout.contains(&format!(
                "installed {}; the tray is running",
                sandbox.tray_entry().display()
            )),
            "{stdout}"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn install_with_the_no_tray_flag_neither_ends_nor_starts_a_tray() {
        let sandbox = Sandbox::new();
        let installation = Installation::new(&sandbox);
        let (mut old, old_pid) = old_tray(&sandbox, false).await;

        let run = installation.run(&sandbox, &["install", "--no-tray"]).await;

        assert!(
            old.try_wait().expect("its status").is_none(),
            "the old tray was ended: {}",
            text(&run.stderr)
        );
        assert_eq!(
            pid_lock_holder(&tray_lock(&sandbox)),
            PidLockHolder::Held { pid: Some(old_pid) }
        );
        assert_eq!(started(&sandbox), [old_pid], "no tray was started");
        assert!(!sandbox.tray_entry().exists());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_tray_that_will_not_end_is_named_and_never_said_to_be_running() {
        let sandbox = Sandbox::new();
        let installation = Installation::new(&sandbox);
        let (mut old, old_pid) = old_tray(&sandbox, true).await;

        let run = installation.run(&sandbox, &["install"]).await;

        assert_eq!(run.status.code(), Some(1), "{}", text(&run.stderr));
        let said = format!("{}{}", text(&run.stdout), text(&run.stderr));
        assert!(said.contains(&format!("(pid {old_pid})")), "{said}");
        assert!(said.contains("was not replaced"), "{said}");
        assert!(!said.contains("the tray is running"), "{said}");
        assert!(
            old.try_wait().expect("its status").is_none(),
            "it is still running"
        );
        assert_eq!(
            pid_lock_holder(&tray_lock(&sandbox)),
            PidLockHolder::Held { pid: Some(old_pid) },
            "no new tray took its place"
        );
        assert_eq!(started(&sandbox), [old_pid], "no new tray was started");
        old.kill().await.expect("killed");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn install_unloads_the_trays_agent_ends_the_running_tray_then_loads_the_new_one() {
        // The agent goes first, so that launchd cannot start the old tray again; a tray started outside
        // launchd, which this one is, is then ended by its pid, and only then is the new agent loaded.
        let sandbox = Sandbox::new();
        let installation = Installation::new(&sandbox);
        let (mut old, old_pid) = old_tray(&sandbox, false).await;

        let run = installation.run(&sandbox, &["install"]).await;

        // The daemon the stand-in `launchctl` started, killed however the test ends.
        let daemon_lock = sandbox
            .control_path()
            .with_file_name(keepster_service::PID_FILE);
        let _daemon = match pid_lock_holder(&daemon_lock) {
            PidLockHolder::Held { pid: Some(pid) } => Some(Detached(pid)),
            _ => None,
        };
        assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
        let ended = old.wait().await.expect("the old tray is reaped");
        assert_eq!(ended.signal(), Some(15), "asked to end: {ended}");
        let new = new_tray(&tray_lock(&sandbox), Some(old_pid)).await;
        assert_eq!(
            started(&sandbox),
            [old_pid, new.0],
            "the new tray started once, after the old one let go of the lock"
        );
        let stdout = text(&run.stdout);
        assert!(stdout.contains("the tray is running"), "{stdout}");
        let agent = keepster_service::autostart::tray::LAUNCH_AGENT;
        let calls =
            std::fs::read_to_string(launchctl_calls(&sandbox)).expect("launchctl was asked");
        let position = |verb: &str| {
            calls
                .lines()
                .position(|call| call.starts_with(verb) && call.contains(agent))
                .unwrap_or_else(|| panic!("no `{verb}` of {agent}: {calls}"))
        };
        assert!(position("bootout") < position("bootstrap"), "{calls}");
    }
}
