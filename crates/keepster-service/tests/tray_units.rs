//! The tray's autostart entries, as values (ticket 67): what each platform's session is told to start,
//! and how a graphical session is recognised, independent of the platform the test runs on. Registering
//! them with the real session is a change to the machine, which no test makes.

use std::ffi::OsString;
use std::path::Path;

use keepster_service::autostart::tray::{
    aqua_session, beside, desktop_entry, graphical_session_in, launch_agent, run_command,
};

#[test]
fn the_tray_is_the_binary_beside_the_cli() {
    assert_eq!(
        beside(Path::new("/opt/keepster/bin/keepster")),
        Path::new(&format!(
            "/opt/keepster/bin/keepster-tray{}",
            std::env::consts::EXE_SUFFIX
        ))
    );
}

#[test]
fn the_desktop_entry_starts_the_tray_at_a_graphical_login_without_a_terminal() {
    let entry = desktop_entry(Path::new("/opt/my tools/100%/keepster-tray"));

    assert!(entry.starts_with("[Desktop Entry]\n"), "{entry}");
    assert!(entry.contains("\nType=Application\n"), "{entry}");
    assert!(
        entry.contains("\nExec=\"/opt/my tools/100%%/keepster-tray\"\n"),
        "quoted, with `%` escaped from field codes: {entry}"
    );
    assert!(entry.contains("\nTerminal=false\n"), "{entry}");
    assert!(
        entry.contains("\nX-GNOME-Autostart-enabled=true\n"),
        "{entry}"
    );
}

#[test]
fn the_desktop_entry_escapes_what_its_quoting_and_its_string_rules_would_otherwise_read() {
    let entry = desktop_entry(Path::new("/home/a\"b\\c$d`e/keepster-tray"));

    assert!(
        entry.contains("\nExec=\"/home/a\\\\\"b\\\\\\\\c\\\\$d\\\\`e/keepster-tray\"\n"),
        "each reserved character escaped for the quoting, then each backslash for the string: {entry}"
    );
}

#[test]
fn the_tray_agent_runs_in_the_graphical_session_only_and_is_kept_alive_unless_it_quits() {
    let agent = launch_agent(Path::new("/Applications/<Keep>&ster/keepster-tray"));

    assert!(
        agent.contains("<key>Label</key>\n\t<string>org.keepster.tray</string>"),
        "{agent}"
    );
    assert!(
        agent.contains("<string>/Applications/&lt;Keep&gt;&amp;ster/keepster-tray</string>"),
        "the program, escaped: {agent}"
    );
    assert!(
        agent.contains("<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>"),
        "never loaded outside a login at the screen: {agent}"
    );
    assert!(agent.contains("<key>RunAtLoad</key>\n\t<true/>"), "{agent}");
    assert!(
        agent.contains(
            "<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>"
        ),
        "started again after a crash, not after Quit: {agent}"
    );
    assert!(
        agent.contains("<key>ProcessType</key>\n\t<string>Interactive</string>"),
        "{agent}"
    );
}

#[test]
fn the_run_value_is_the_tray_quoted() {
    assert_eq!(
        run_command(Path::new(r"C:\Program Files\Keepster\keepster-tray.exe")),
        r#""C:\Program Files\Keepster\keepster-tray.exe""#
    );
}

#[test]
fn a_linux_session_is_graphical_when_wayland_or_x_is_named() {
    let with = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    };

    assert!(graphical_session_in(with(&[(
        "WAYLAND_DISPLAY",
        "wayland-0"
    )])));
    assert!(graphical_session_in(with(&[("DISPLAY", ":0")])));
    assert!(!graphical_session_in(with(&[])));
    assert!(
        !graphical_session_in(with(&[("WAYLAND_DISPLAY", ""), ("DISPLAY", "")])),
        "an empty name names no display"
    );
}

#[test]
fn a_macos_session_is_graphical_when_launchd_manages_it_as_aqua() {
    assert!(aqua_session("Aqua\n"));
    assert!(!aqua_session("Background\n"));
    assert!(!aqua_session("StandardIO"));
    assert!(!aqua_session(""));
}
