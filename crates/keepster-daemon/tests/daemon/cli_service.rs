//! `keepster service` and the CLI's way to a daemon that is not running (ticket 61), in a sandbox whose
//! service manager cannot be reached: what is asserted is what Keepster decides and says, never what a
//! real systemd, launchd or Task Scheduler does — registering a unit changes the machine a test runs on.
#![cfg(unix)]

#[cfg(target_os = "linux")]
use std::path::Path;

use crate::cli_lifecycle::Sandbox;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// What a person would have after `keepster service install` on Linux, as files: the unit, and the link
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
    assert!(stderr.contains("keepster service install"), "{stderr}");
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
    assert!(stdout.contains("keepster service install"), "{stdout}");
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
    assert!(human.contains("keepster service install"), "{human}");
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

    let run = sandbox.run(&["service", "install"]).await;

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

    let run = sandbox.run(&["service", "install", "--no-tray"]).await;

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

    let run = sandbox.run(&["service", "uninstall"]).await;

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

    sandbox.run(&["service", "install", "--no-web-ui"]).await;

    let written = std::fs::read_to_string(sandbox.config_path()).expect("created");
    assert!(written.contains("[ui]\nlistener = false\n"), "{written}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn install_without_the_web_ui_leaves_an_existing_configuration_and_names_the_line() {
    let sandbox = Sandbox::new();
    let mine = b"# mine\n[files]\nexclude = [\"*.log\"]\n";
    sandbox.write_config(mine);

    let run = sandbox.run(&["service", "install", "--no-web-ui"]).await;

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

    let run = sandbox.run(&["service", "uninstall"]).await;

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

    let run = sandbox.run(&["service", "install"]).await;

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
