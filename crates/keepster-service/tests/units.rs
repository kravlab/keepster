//! The autostart units Keepster writes, as values (ticket 61): what each platform's service manager is
//! told, independent of the platform the test runs on. Registering them with the real manager is a
//! change to the machine, which no test makes.

use std::path::Path;

use keepster_service::autostart::{REFUSED_EXIT, launch_agent, scheduled_task, systemd_unit};

#[test]
fn the_systemd_unit_runs_the_daemon_restarts_it_on_failure_and_starts_with_the_user() {
    let unit = systemd_unit(Path::new("/opt/my tools/100%/keepster"));

    assert!(
        unit.contains("ExecStart=\"/opt/my tools/100%%/keepster\" daemon\n"),
        "quoted, with `%` escaped from specifier expansion: {unit}"
    );
    assert!(unit.contains("Restart=on-failure\n"), "{unit}");
    assert!(
        unit.contains(&format!("RestartPreventExitStatus={REFUSED_EXIT}\n")),
        "a refused second daemon is not restarted: {unit}"
    );
    assert!(unit.contains("TimeoutStopSec=10\n"), "{unit}");
    assert!(unit.contains("WantedBy=default.target\n"), "{unit}");
}

#[test]
fn the_systemd_unit_escapes_what_its_quoting_would_otherwise_end() {
    let unit = systemd_unit(Path::new("/home/a\"b\\c/keepster"));

    assert!(
        unit.contains("ExecStart=\"/home/a\\\"b\\\\c/keepster\" daemon\n"),
        "{unit}"
    );
}

#[test]
fn the_launch_agent_runs_at_load_and_is_kept_alive_unless_it_exits_cleanly() {
    let agent = launch_agent(Path::new("/Users/me/bin/<keep>&ster"));

    assert!(agent.contains("<key>Label</key>"), "{agent}");
    assert!(
        agent.contains(
            "<string>/Users/me/bin/&lt;keep&gt;&amp;ster</string>\n\t\t<string>daemon</string>"
        ),
        "the program and its argument, escaped: {agent}"
    );
    assert!(agent.contains("<key>RunAtLoad</key>\n\t<true/>"), "{agent}");
    assert!(
        agent.contains(
            "<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>"
        ),
        "{agent}"
    );
    assert!(
        agent.contains("<key>ExitTimeOut</key>\n\t<integer>10</integer>"),
        "{agent}"
    );
}

#[test]
fn the_scheduled_task_starts_at_this_users_logon_unelevated_and_restarts_on_failure() {
    let task = scheduled_task(Path::new(r"C:\Users\Me & You\keepster.exe"), r"HOST\me");

    assert!(
        task.contains(
            "<LogonTrigger>\n      <Enabled>true</Enabled>\n      <UserId>HOST\\me</UserId>"
        ),
        "at this user's logon, not anyone's: {task}"
    );
    assert!(
        task.contains("<RunLevel>LeastPrivilege</RunLevel>"),
        "never elevated: {task}"
    );
    assert!(
        task.contains("<LogonType>InteractiveToken</LogonType>"),
        "no stored password: {task}"
    );
    assert!(task.contains("<RestartOnFailure>"), "{task}");
    assert!(
        task.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"),
        "never stopped for running long: {task}"
    );
    assert!(
        task.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"),
        "{task}"
    );
    assert!(
        task.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"),
        "{task}"
    );
    assert!(
        task.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"),
        "{task}"
    );
    assert!(
        task.contains(r"<Command>C:\Users\Me &amp; You\keepster.exe</Command>"),
        "{task}"
    );
    assert!(task.contains("<Arguments>daemon</Arguments>"), "{task}");
}
