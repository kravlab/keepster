//! Every word the tray shows a person, in one place, so that nothing in the model or an adapter spells
//! out text of its own.

/// The tray's name, as the platform shows it beside the icon and above a notification.
pub const APP_NAME: &str = "Keepster";

/// The menu's item that opens the web UI.
pub const OPEN_HISTORY: &str = "Open history";
/// The menu's item that mutes notifications, while they are not muted.
pub const MUTE_FOR_AN_HOUR: &str = "Mute for 1 hour";
/// The menu's item that lifts the mute, while one is in force.
pub const UNMUTE: &str = "Unmute";
/// The menu's item that restarts the daemon, while it answers.
pub const RESTART_DAEMON: &str = "Restart daemon";
/// The menu's item that starts the daemon, while it does not answer.
pub const START_DAEMON: &str = "Start daemon";
/// The menu's item that ends the tray.
pub const QUIT_TRAY: &str = "Quit tray";

/// The status line while every Root is covered.
pub fn covering(roots: usize) -> String {
    match roots {
        0 => "Keepster is watching no roots".to_owned(),
        1 => "Keepster is covering 1 root".to_owned(),
        many => format!("Keepster is covering {many} roots"),
    }
}

/// The status line while alarms are in force.
pub fn alarms(count: usize) -> String {
    match count {
        1 => "Keepster: 1 alarm".to_owned(),
        many => format!("Keepster: {many} alarms"),
    }
}

/// What the status line says besides, while notifications are muted.
pub fn muted(status: &str) -> String {
    format!("{status}; notifications muted")
}

/// The status line while the daemon does not answer.
pub const DAEMON_NOT_RUNNING: &str = "Keepster's daemon is not running";

/// `(title, body)` of a Root that nothing can be captured from.
pub fn root_unavailable(name: &str, reason: Option<&str>) -> (String, String) {
    (
        format!("{name} is not being captured"),
        format!(
            "Keepster cannot capture it{}; `keepster status` says why.",
            reason_of(reason)
        ),
    )
}

/// `(title, body)` of a Root watched, but not as promised.
pub fn root_degraded(name: &str, reason: Option<&str>) -> (String, String) {
    (
        format!("{name} is not covered as promised"),
        format!(
            "Keepster still watches it, but not as promised{}; `keepster status` says why.",
            reason_of(reason)
        ),
    )
}

/// `(title, body)` of a job that failed.
pub fn job_failed(name: &str, kind: &str, job: &str) -> (String, String) {
    (
        format!("A {kind} of {name} failed"),
        format!("`keepster job {job}` says what went wrong."),
    )
}

/// `(title, body)` of an alarm of a kind this build does not know.
pub fn unknown_alarm(name: &str, kind: &str) -> (String, String) {
    (
        format!("{name}: {kind}"),
        "`keepster status` says more.".to_owned(),
    )
}

/// `(title, body)` of a Root covered again.
pub fn root_recovered(name: &str) -> (String, String) {
    (
        format!("{name} is covered again"),
        "Keepster captures it as promised.".to_owned(),
    )
}

/// `(title, body)` of a job that succeeded after one that failed.
pub fn job_recovered(name: &str, kind: &str) -> (String, String) {
    (
        format!("A {kind} of {name} succeeded"),
        "The failure before it no longer stands.".to_owned(),
    )
}

/// `(title, body)` of a daemon that died.
pub fn daemon_died() -> (String, String) {
    (
        "Keepster's daemon stopped unexpectedly".to_owned(),
        "Nothing is captured until it runs again.".to_owned(),
    )
}

/// `(title, body)` of a daemon back after it died.
pub fn daemon_back() -> (String, String) {
    (
        "Keepster's daemon is running again".to_owned(),
        "Capture has resumed.".to_owned(),
    )
}

/// ` (reason)`, or nothing without one.
fn reason_of(reason: Option<&str>) -> String {
    reason.map_or_else(String::new, |reason| format!(" ({reason})"))
}
