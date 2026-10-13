//! Autostart on three platforms (ticket 61): a systemd user unit on Linux, a LaunchAgent on macOS, a
//! Scheduled Task registered at logon on Windows — each written, registered and removed by Keepster
//! itself, because only code that installs a unit can verify it (ticket 05).
//!
//! Nothing here needs administrator rights, and nothing starts a daemon except by asking the service
//! manager: a running daemon is always a supervised one. Nothing here deletes history either.

use std::io;
use std::path::{Path, PathBuf};

pub mod tray;

/// The exit code of a daemon refused because another one holds the pid file's lock, which the systemd
/// unit does not restart on, since starting it again would be refused again.
///
/// launchd's `KeepAlive` and Task Scheduler's restart cannot filter by exit code: a refused daemon there
/// is started again after their throttle interval, for as long as a daemon no manager started holds the
/// lock. `keepster service install` refuses beside one; the rest is ticket 82's to observe.
pub const REFUSED_EXIT: u8 = 5;

/// The systemd unit's name.
pub const SYSTEMD_UNIT: &str = "keepster.service";

/// The LaunchAgent's label.
pub const LAUNCH_AGENT: &str = "org.keepster.daemon";

/// The Scheduled Task's name.
pub const SCHEDULED_TASK: &str = "Keepster";

/// How long a stopping daemon is given to drain and flush before the service manager kills it, in
/// seconds. Being killed past it is safe (ticket 05).
const STOP_TIMEOUT_SECONDS: u32 = 10;

/// The systemd user unit that runs `exe daemon`.
pub fn systemd_unit(exe: &Path) -> String {
    format!(
        "# Written by `keepster service install`; `keepster service uninstall` removes it.\n\
         [Unit]\n\
         Description=Keepster, editor-independent file history\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} daemon\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         RestartPreventExitStatus={REFUSED_EXIT}\n\
         TimeoutStopSec={STOP_TIMEOUT_SECONDS}\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        systemd_quoted(exe)
    )
}

/// `path` as one word of an `ExecStart=` line: double-quoted, with what would end the quoting escaped,
/// and `%` doubled so systemd expands no specifier inside it.
fn systemd_quoted(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' | '\\' => {
                quoted.push('\\');
                quoted.push(character);
            }
            '%' => quoted.push_str("%%"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');

    quoted
}

/// The LaunchAgent that runs `exe daemon` at login and again whenever it dies other than cleanly.
pub fn launch_agent(exe: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <!-- Written by `keepster service install`; `keepster service uninstall` removes it. -->\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{LAUNCH_AGENT}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{}</string>\n\
         \t\t<string>daemon</string>\n\
         \t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<dict>\n\
         \t\t<key>SuccessfulExit</key>\n\
         \t\t<false/>\n\
         \t</dict>\n\
         \t<key>ProcessType</key>\n\
         \t<string>Background</string>\n\
         \t<key>ExitTimeOut</key>\n\
         \t<integer>{STOP_TIMEOUT_SECONDS}</integer>\n\
         </dict>\n\
         </plist>\n",
        xml_escaped(&exe.to_string_lossy())
    )
}

/// The Scheduled Task that runs `exe daemon` at `user`'s logon, as `user`, unelevated, restarted when it
/// fails. `user` is `DOMAIN\name`.
///
/// An interactive token rather than a stored password: an ordinary task running as the user would need
/// the password kept, which ticket 27 rejected.
pub fn scheduled_task(exe: &Path, user: &str) -> String {
    let user = xml_escaped(user);

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\
         <Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n\
         \x20 <RegistrationInfo>\n\
         \x20   <Description>Keepster, editor-independent file history. Written by `keepster service install`; `keepster service uninstall` removes it.</Description>\n\
         \x20 </RegistrationInfo>\n\
         \x20 <Triggers>\n\
         \x20   <LogonTrigger>\n\
         \x20     <Enabled>true</Enabled>\n\
         \x20     <UserId>{user}</UserId>\n\
         \x20   </LogonTrigger>\n\
         \x20 </Triggers>\n\
         \x20 <Principals>\n\
         \x20   <Principal id=\"Author\">\n\
         \x20     <UserId>{user}</UserId>\n\
         \x20     <LogonType>InteractiveToken</LogonType>\n\
         \x20     <RunLevel>LeastPrivilege</RunLevel>\n\
         \x20   </Principal>\n\
         \x20 </Principals>\n\
         \x20 <Settings>\n\
         \x20   <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n\
         \x20   <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n\
         \x20   <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n\
         \x20   <AllowHardTerminate>true</AllowHardTerminate>\n\
         \x20   <StartWhenAvailable>true</StartWhenAvailable>\n\
         \x20   <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>\n\
         \x20   <IdleSettings>\n\
         \x20     <StopOnIdleEnd>false</StopOnIdleEnd>\n\
         \x20     <RestartOnIdle>false</RestartOnIdle>\n\
         \x20   </IdleSettings>\n\
         \x20   <AllowStartOnDemand>true</AllowStartOnDemand>\n\
         \x20   <Enabled>true</Enabled>\n\
         \x20   <Hidden>false</Hidden>\n\
         \x20   <RunOnlyIfIdle>false</RunOnlyIfIdle>\n\
         \x20   <WakeToRun>false</WakeToRun>\n\
         \x20   <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\n\
         \x20   <Priority>7</Priority>\n\
         \x20   <RestartOnFailure>\n\
         \x20     <Interval>PT1M</Interval>\n\
         \x20     <Count>999</Count>\n\
         \x20   </RestartOnFailure>\n\
         \x20 </Settings>\n\
         \x20 <Actions Context=\"Author\">\n\
         \x20   <Exec>\n\
         \x20     <Command>{}</Command>\n\
         \x20     <Arguments>daemon</Arguments>\n\
         \x20   </Exec>\n\
         \x20 </Actions>\n\
         </Task>\n",
        xml_escaped(&exe.to_string_lossy())
    )
}

fn xml_escaped(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }

    escaped
}

/// Whether the daemon comes back at login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Autostart {
    /// The unit is installed and enabled.
    Installed,
    /// The unit is installed and switched off — on Linux, a unit nobody enabled or someone disabled.
    Disabled,
    /// No unit was ever installed, or it was uninstalled: a state of its own, never read as "stopped".
    NotInstalled,
    /// This platform, or this environment, cannot tell; why, for a person.
    Unknown(String),
}

/// Whether coverage survives the user's logout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Linger {
    /// It does: linger is on for this user (Linux).
    On,
    /// It does not, and it could: linger is off for this user (Linux).
    Off,
    /// It cannot: the platform ends a user's agents and logon tasks at logout (macOS, Windows), a gap
    /// accepted and shown rather than closed with an elevated install (ticket 05).
    LogoutGap,
    /// It cannot be told here.
    Unknown,
}

/// What `install` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// What was written and registered, for a person: the unit file, or the task's name.
    pub unit: String,
}

/// A step of installing or uninstalling that failed, and what the platform said.
#[derive(Debug, thiserror::Error)]
#[error("{step}: {detail}")]
pub struct ServiceError {
    /// What was being done.
    pub step: String,
    /// What failed, in the platform's words where it gave any.
    pub detail: String,
}

impl ServiceError {
    fn new(step: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            step: step.into(),
            detail: detail.into(),
        }
    }
}

/// Linger could not be switched on by the user alone; `command` is what an administrator runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LingerRefused {
    /// The exact command to run instead.
    pub command: String,
    /// What the platform said.
    pub detail: String,
}

/// Run `program` with `args`, failing with what it wrote to standard error when it does not succeed.
///
/// No shell: every argument is passed as it is.
fn run(step: &str, program: &str, args: &[&str]) -> Result<(), ServiceError> {
    let output = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| ServiceError::new(step, format!("`{program}` could not run: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let said = if said.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    } else {
        said
    };

    Err(ServiceError::new(
        step,
        format!(
            "`{program} {}` exited with {}{}",
            args.join(" "),
            output.status,
            if said.is_empty() {
                String::new()
            } else {
                format!(": {said}")
            }
        ),
    ))
}

fn write_file(step: &str, path: &Path, content: &[u8]) -> Result<(), ServiceError> {
    let failed = |error: io::Error| ServiceError::new(step, format!("{}: {error}", path.display()));
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(failed)?;
    }

    std::fs::write(path, content).map_err(failed)
}

fn remove_file(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

pub use platform::{
    autostart, enable_linger, install, restart, start, stop, uninstall, unit_location,
};

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    /// The user's configuration directory, where systemd's user units and the XDG autostart entries
    /// are both read from.
    pub(crate) fn config_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    }

    /// The directory systemd reads a user's own units from.
    fn unit_dir() -> Option<PathBuf> {
        config_dir().map(|config| config.join("systemd/user"))
    }

    /// Where the unit is, or would be.
    pub fn unit_location() -> Option<PathBuf> {
        unit_dir().map(|directory| directory.join(SYSTEMD_UNIT))
    }

    /// Read from the files systemd itself reads, so that `status` asks no process anything: the unit,
    /// and the link enabling it under `default.target`.
    pub fn autostart() -> Autostart {
        let Some(directory) = unit_dir() else {
            return Autostart::Unknown("no home directory to look for the unit in".to_owned());
        };
        if !directory.join(SYSTEMD_UNIT).exists() {
            return Autostart::NotInstalled;
        }
        let enabled = directory
            .join("default.target.wants")
            .join(SYSTEMD_UNIT)
            .symlink_metadata()
            .is_ok();

        if enabled {
            Autostart::Installed
        } else {
            Autostart::Disabled
        }
    }

    /// Whether linger is on for the invoking user, read from the file `logind` keeps for it.
    pub(crate) fn linger_state() -> Linger {
        match std::env::var_os("USER") {
            Some(user) if Path::new("/var/lib/systemd/linger").join(&user).exists() => Linger::On,
            Some(_) => Linger::Off,
            None => Linger::Unknown,
        }
    }

    /// Switch linger on for the invoking user. polkit lets a user do this for themselves without a
    /// prompt (`set-self-linger`); where a site's policy refuses, an administrator's command is the
    /// way, and it is printed rather than run.
    pub fn enable_linger() -> Result<(), LingerRefused> {
        run("enable linger", "loginctl", &["enable-linger"]).map_err(|error| {
            let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_owned());
            LingerRefused {
                command: format!("sudo loginctl enable-linger {user}"),
                detail: error.detail,
            }
        })
    }

    pub fn install(exe: &Path) -> Result<Installed, ServiceError> {
        let unit = unit_location().ok_or_else(|| {
            ServiceError::new("write the unit", "no home directory to write it in")
        })?;
        write_file("write the unit", &unit, systemd_unit(exe).as_bytes())?;

        // `restart` rather than `start`, so a reinstall after an upgrade runs the binary just written.
        let registered = run("reload systemd", "systemctl", &["--user", "daemon-reload"])
            .and_then(|()| {
                run(
                    "enable the unit",
                    "systemctl",
                    &["--user", "enable", SYSTEMD_UNIT],
                )
            })
            .and_then(|()| {
                run(
                    "start the unit",
                    "systemctl",
                    &["--user", "restart", SYSTEMD_UNIT],
                )
            });
        if let Err(error) = registered {
            // Never left half-installed: a unit on disk that systemd does not run would read as
            // installed.
            let _ = run(
                "disable the unit",
                "systemctl",
                &["--user", "disable", SYSTEMD_UNIT],
            );
            let _ = remove_file(&unit);
            let _ = run("reload systemd", "systemctl", &["--user", "daemon-reload"]);
            return Err(error);
        }

        Ok(Installed {
            unit: unit.display().to_string(),
        })
    }

    pub fn start() -> Result<(), ServiceError> {
        run(
            "start the unit",
            "systemctl",
            &["--user", "start", SYSTEMD_UNIT],
        )
    }

    /// Stop the daemon. systemd does not start a unit stopped this way again until asked.
    pub fn stop() -> Result<(), ServiceError> {
        run(
            "stop the unit",
            "systemctl",
            &["--user", "stop", SYSTEMD_UNIT],
        )
    }

    /// Stop the daemon and start it again, or start it when it was not running.
    pub fn restart() -> Result<(), ServiceError> {
        run(
            "restart the unit",
            "systemctl",
            &["--user", "restart", SYSTEMD_UNIT],
        )
    }

    /// Stop and disable the unit, then remove it. Linger is left as it is: other services of the user's
    /// may rely on it.
    pub fn uninstall() -> Result<Option<String>, ServiceError> {
        let Some(unit) = unit_location() else {
            return Err(ServiceError::new(
                "find the unit",
                "no home directory to look in",
            ));
        };
        if !unit.exists() {
            return Ok(None);
        }
        run(
            "stop the unit",
            "systemctl",
            &["--user", "disable", "--now", SYSTEMD_UNIT],
        )?;
        remove_file(&unit).map_err(|error| {
            ServiceError::new("remove the unit", format!("{}: {error}", unit.display()))
        })?;
        let _ = run("reload systemd", "systemctl", &["--user", "daemon-reload"]);

        Ok(Some(unit.display().to_string()))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    /// Where the LaunchAgent is, or would be.
    pub fn unit_location() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library/LaunchAgents")
                .join(format!("{LAUNCH_AGENT}.plist"))
        })
    }

    /// The user's GUI domain, which a LaunchAgent belongs to.
    pub(crate) fn domain() -> String {
        format!("gui/{}", rustix::process::getuid().as_raw())
    }

    /// The daemon's agent in that domain.
    fn service() -> String {
        format!("{}/{LAUNCH_AGENT}", domain())
    }

    /// Unload the agent `label`, ending its process. An agent that is not loaded has nothing to unload,
    /// so a failure is not one: what an uninstall needs is that the file goes, and a reload loads anew.
    pub(crate) fn unload_agent(label: &str) {
        let _ = run(
            "unload the agent",
            "launchctl",
            &["bootout", &format!("{}/{label}", domain())],
        );
    }

    /// Load the agent `label` from `plist`, replacing whatever an earlier install loaded, so the agent
    /// just written is the one running.
    ///
    /// launchd tears an agent down after `bootout` returns, and a `bootstrap` arriving meanwhile fails
    /// with an I/O error: it is asked again, a moment later, a few times.
    pub(crate) fn reload_agent(label: &str, plist: &str) -> Result<(), ServiceError> {
        unload_agent(label);
        let load = || {
            run(
                "load the agent",
                "launchctl",
                &["bootstrap", &domain(), plist],
            )
        };
        let mut loaded = load();
        for _ in 0..4 {
            if loaded.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            loaded = load();
        }

        loaded
    }

    /// launchd loads every agent in the user's `LaunchAgents` at login, so the file is the fact.
    pub fn autostart() -> Autostart {
        match unit_location() {
            Some(plist) if plist.exists() => Autostart::Installed,
            Some(_) => Autostart::NotInstalled,
            None => Autostart::Unknown("no home directory to look for the agent in".to_owned()),
        }
    }

    pub fn enable_linger() -> Result<(), LingerRefused> {
        Ok(())
    }

    pub fn install(exe: &Path) -> Result<Installed, ServiceError> {
        let plist = unit_location().ok_or_else(|| {
            ServiceError::new("write the agent", "no home directory to write it in")
        })?;
        write_file("write the agent", &plist, launch_agent(exe).as_bytes())?;
        let plist_text = plist.to_string_lossy().into_owned();

        if let Err(error) = reload_agent(LAUNCH_AGENT, &plist_text) {
            let _ = remove_file(&plist);
            return Err(error);
        }

        Ok(Installed { unit: plist_text })
    }

    pub fn start() -> Result<(), ServiceError> {
        run("start the agent", "launchctl", &["kickstart", &service()])
    }

    /// Stop the daemon. It exits cleanly on `SIGTERM`, which the agent's `KeepAlive` does not restart.
    pub fn stop() -> Result<(), ServiceError> {
        run(
            "stop the agent",
            "launchctl",
            &["kill", "SIGTERM", &service()],
        )
    }

    /// Stop the daemon and start it again, or start it when it was not running.
    pub fn restart() -> Result<(), ServiceError> {
        run(
            "restart the agent",
            "launchctl",
            &["kickstart", "-k", &service()],
        )
    }

    pub fn uninstall() -> Result<Option<String>, ServiceError> {
        let Some(plist) = unit_location() else {
            return Err(ServiceError::new(
                "find the agent",
                "no home directory to look in",
            ));
        };
        if !plist.exists() {
            return Ok(None);
        }
        unload_agent(LAUNCH_AGENT);
        remove_file(&plist).map_err(|error| {
            ServiceError::new("remove the agent", format!("{}: {error}", plist.display()))
        })?;

        Ok(Some(plist.display().to_string()))
    }

    pub(crate) fn linger_state() -> Linger {
        Linger::LogoutGap
    }
}

#[cfg(windows)]
mod platform {
    use super::*;

    /// The Scheduled Task has no file of the user's: it is named, not located.
    pub fn unit_location() -> Option<PathBuf> {
        Some(PathBuf::from(format!(r"Task Scheduler\{SCHEDULED_TASK}")))
    }

    /// `DOMAIN\name` of the invoking user, which the task runs as and starts at the logon of.
    fn user() -> Option<String> {
        let name = std::env::var("USERNAME").ok()?;
        Some(match std::env::var("USERDOMAIN") {
            Ok(domain) => format!(r"{domain}\{name}"),
            Err(_) => name,
        })
    }

    pub fn autostart() -> Autostart {
        match std::process::Command::new("schtasks")
            .args(["/Query", "/TN", SCHEDULED_TASK])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(status) if status.success() => Autostart::Installed,
            Ok(_) => Autostart::NotInstalled,
            Err(error) => Autostart::Unknown(format!("`schtasks` could not run: {error}")),
        }
    }

    pub fn enable_linger() -> Result<(), LingerRefused> {
        Ok(())
    }

    pub fn install(exe: &Path) -> Result<Installed, ServiceError> {
        let user = user().ok_or_else(|| {
            ServiceError::new(
                "name the user",
                "USERNAME is not set, so the logon to start at is unknown",
            )
        })?;
        // Task Scheduler reads its XML as UTF-16 with a byte order mark.
        let mut encoded = vec![0xFF, 0xFE];
        for unit in scheduled_task(exe, &user).encode_utf16() {
            encoded.extend_from_slice(&unit.to_le_bytes());
        }
        let definition =
            std::env::temp_dir().join(format!("keepster-task-{}.xml", std::process::id()));
        write_file("write the task", &definition, &encoded)?;
        let definition_text = definition.to_string_lossy().into_owned();

        let created = run(
            "register the task",
            "schtasks",
            &[
                "/Create",
                "/TN",
                SCHEDULED_TASK,
                "/XML",
                &definition_text,
                "/F",
            ],
        );
        let _ = remove_file(&definition);
        created?;
        if let Err(error) = run(
            "start the task",
            "schtasks",
            &["/Run", "/TN", SCHEDULED_TASK],
        ) {
            let _ = run(
                "remove the task",
                "schtasks",
                &["/Delete", "/TN", SCHEDULED_TASK, "/F"],
            );
            return Err(error);
        }

        Ok(Installed {
            unit: format!(r"Task Scheduler\{SCHEDULED_TASK}"),
        })
    }

    pub fn start() -> Result<(), ServiceError> {
        run(
            "start the task",
            "schtasks",
            &["/Run", "/TN", SCHEDULED_TASK],
        )
    }

    /// Stop the daemon. Ending a task ends its process outright, which the store survives as it
    /// survives any kill.
    pub fn stop() -> Result<(), ServiceError> {
        run("end the task", "schtasks", &["/End", "/TN", SCHEDULED_TASK])
    }

    /// Stop the daemon and start it again. Ending a task that is not running fails, and is not a
    /// reason to leave it stopped.
    pub fn restart() -> Result<(), ServiceError> {
        let _ = stop();
        start()
    }

    pub fn uninstall() -> Result<Option<String>, ServiceError> {
        if autostart() == Autostart::NotInstalled {
            return Ok(None);
        }
        // Ending a task ends its process outright, which the store survives as it survives any kill.
        let _ = run("end the task", "schtasks", &["/End", "/TN", SCHEDULED_TASK]);
        run(
            "remove the task",
            "schtasks",
            &["/Delete", "/TN", SCHEDULED_TASK, "/F"],
        )?;

        Ok(Some(format!(r"Task Scheduler\{SCHEDULED_TASK}")))
    }

    pub(crate) fn linger_state() -> Linger {
        Linger::LogoutGap
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::*;

    const UNSUPPORTED: &str = "this platform has no service manager Keepster can register with";

    pub fn unit_location() -> Option<PathBuf> {
        None
    }

    pub fn autostart() -> Autostart {
        Autostart::Unknown(UNSUPPORTED.to_owned())
    }

    pub fn enable_linger() -> Result<(), LingerRefused> {
        Ok(())
    }

    pub fn install(_exe: &Path) -> Result<Installed, ServiceError> {
        Err(ServiceError::new("install", UNSUPPORTED))
    }

    pub fn start() -> Result<(), ServiceError> {
        Err(ServiceError::new("start", UNSUPPORTED))
    }

    pub fn stop() -> Result<(), ServiceError> {
        Err(ServiceError::new("stop", UNSUPPORTED))
    }

    pub fn restart() -> Result<(), ServiceError> {
        Err(ServiceError::new("restart", UNSUPPORTED))
    }

    pub fn uninstall() -> Result<Option<String>, ServiceError> {
        Err(ServiceError::new("uninstall", UNSUPPORTED))
    }

    pub(crate) fn linger_state() -> Linger {
        Linger::Unknown
    }
}

/// Whether coverage survives the user's logout on this platform, for this user.
pub fn linger() -> Linger {
    platform::linger_state()
}
