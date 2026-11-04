//! The tray's autostart (ticket 67): it starts with the user's graphical session, where its icon and its
//! notifications can be seen — an XDG autostart entry on Linux, a LaunchAgent limited to the Aqua session
//! on macOS, a `Run` value on Windows — and is written by `keepster install` and removed by `keepster
//! uninstall` alongside the daemon's own unit.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod replace;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
use replace::replace_running_tray;
pub use replace::{END_PATIENCE, TrayEnded, TrayNotEnded, end_running};

/// The tray's binary, beside `keepster`.
pub const BINARY: &str = "keepster-tray";

/// The XDG autostart entry's file name.
pub const DESKTOP_FILE: &str = "keepster-tray.desktop";

/// The tray's LaunchAgent label.
pub const LAUNCH_AGENT: &str = "org.keepster.tray";

/// The name of the tray's value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
pub const RUN_VALUE: &str = "Keepster Tray";

/// The AppUserModelID the tray shows its toasts under on Windows, which the Start-menu shortcut carries.
pub const APP_ID: &str = "Keepster.Tray";

/// The Start-menu shortcut's file name on Windows (ticket 68).
pub const SHORTCUT: &str = "Keepster.lnk";

/// Where the Start-menu shortcut may be on Windows, as named by the environment `var` reads: the user's
/// own `Programs` folder, which `keepster install` writes, then every user's, which an administrator may
/// have written. A toast from an unpackaged program is shown only while such a shortcut carries its
/// AppUserModelID, so the tray looks in both.
pub fn shortcut_locations(var: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    ["APPDATA", "ProgramData"]
        .into_iter()
        .filter_map(|name| var(name).filter(|value| !value.is_empty()))
        .map(|base| {
            ["Microsoft", "Windows", "Start Menu", "Programs", SHORTCUT]
                .iter()
                .fold(PathBuf::from(base), |path, component| path.join(component))
        })
        .collect()
}

/// The tray installed beside the CLI at `cli`.
pub fn beside(cli: &Path) -> PathBuf {
    cli.with_file_name(format!("{BINARY}{}", std::env::consts::EXE_SUFFIX))
}

/// The XDG autostart entry that starts `tray` at a graphical login.
pub fn desktop_entry(tray: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         # Written by `keepster install`; `keepster uninstall` removes it.\n\
         Type=Application\n\
         Name=Keepster\n\
         Comment=Shows whether Keepster's file history is covering you, and tells you when it stops\n\
         Exec={}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n",
        exec_quoted(tray)
    )
}

/// `path` as the program of an `Exec=` key: double-quoted, with each character the quoting reserves
/// escaped by a backslash, then — the string rules apply first when the file is read — every backslash
/// doubled, and `%` doubled so it expands no field code.
fn exec_quoted(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(character);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');

    quoted
}

/// The LaunchAgent that starts `tray` in the user's Aqua session, and again whenever it dies other than
/// by quitting.
pub fn launch_agent(tray: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <!-- Written by `keepster install`; `keepster uninstall` removes it. -->\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{LAUNCH_AGENT}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{}</string>\n\
         \t</array>\n\
         \t<key>LimitLoadToSessionType</key>\n\
         \t<string>Aqua</string>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<dict>\n\
         \t\t<key>SuccessfulExit</key>\n\
         \t\t<false/>\n\
         \t</dict>\n\
         \t<key>ProcessType</key>\n\
         \t<string>Interactive</string>\n\
         </dict>\n\
         </plist>\n",
        super::xml_escaped(&tray.to_string_lossy())
    )
}

/// The command line of the tray's `Run` value.
pub fn run_command(tray: &Path) -> String {
    // A Windows path holds no `"`, so quoting is all a path with spaces needs.
    format!("\"{}\"", tray.to_string_lossy())
}

/// Whether the environment `var` reads names a Wayland or an X display, which is what a Linux graphical
/// session sets and a login over SSH does not.
pub fn graphical_session_in(var: impl Fn(&str) -> Option<OsString>) -> bool {
    ["WAYLAND_DISPLAY", "DISPLAY"]
        .iter()
        .any(|name| var(name).is_some_and(|value| !value.is_empty()))
}

/// Whether `launchctl managername` said `managername` for a session launchd manages as a login at the
/// screen.
pub fn aqua_session(managername: &str) -> bool {
    managername.trim() == "Aqua"
}

/// What installing the tray's autostart did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayInstalled {
    /// What was written, for a person: the entry's file, or the registry value.
    pub entry: String,
    /// Whether the tray was started now, and if not, why.
    pub start: TrayStart,
}

/// Whether installing the tray's autostart started the tray now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStart {
    /// The tray beside the new `keepster` was started, after any tray running before was ended.
    Started,
    /// There is no graphical session to show it in: it starts at the next graphical login instead.
    NextLogin,
    /// A tray already running was not ended — see [`TrayNotEnded`] — so none was started: the new one
    /// would have found the lock held and left at once. The entry is written, so the next login starts
    /// the new one.
    NotReplaced(TrayNotEnded),
}

impl From<TrayNotEnded> for TrayStart {
    /// A tray that was not ended is one the new tray could not replace.
    fn from(not_ended: TrayNotEnded) -> Self {
        Self::NotReplaced(not_ended)
    }
}

/// Start `tray` on its own, outliving the command that started it, with nothing of the terminal's.
#[cfg(any(target_os = "linux", windows))]
fn spawn_detached(tray: &Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};

    let mut command = Command::new(tray);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A group of its own, so the hang-up that ends the shell's jobs when its terminal closes does not
        // reach it.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    command.spawn().map(drop)
}

/// `tray`, or the refusal to install an autostart entry for a tray that is not there.
fn existing(tray: &Path) -> Result<(), super::ServiceError> {
    if tray.is_file() {
        return Ok(());
    }

    Err(super::ServiceError::new(
        "find the tray",
        format!("{} does not exist", tray.display()),
    ))
}

pub use platform::{entry_location, install, uninstall};

/// Linux: an XDG autostart entry.
#[cfg(target_os = "linux")]
mod platform {
    use super::super::{ServiceError, remove_file, write_file};
    use super::*;

    /// Where the autostart entry is, or would be.
    pub fn entry_location() -> Option<PathBuf> {
        super::super::platform::config_dir()
            .map(|config| config.join("autostart").join(DESKTOP_FILE))
    }

    /// Write the autostart entry, and start the tray now when there is a graphical session to show it
    /// in, ending the tray already running there first.
    pub fn install(tray: &Path) -> Result<TrayInstalled, ServiceError> {
        existing(tray)?;
        let entry = entry_location().ok_or_else(|| {
            ServiceError::new(
                "write the tray's autostart entry",
                "no home directory to write it in",
            )
        })?;
        write_file(
            "write the tray's autostart entry",
            &entry,
            desktop_entry(tray).as_bytes(),
        )?;
        let start = if graphical_session_in(|name| std::env::var_os(name)) {
            replace_running_tray(|| {
                spawn_detached(tray).map_err(|error| {
                    let _ = remove_file(&entry);
                    ServiceError::new("start the tray", format!("{}: {error}", tray.display()))
                })
            })?
        } else {
            TrayStart::NextLogin
        };

        Ok(TrayInstalled {
            entry: entry.display().to_string(),
            start,
        })
    }

    /// Remove the autostart entry. A tray running now goes on until it is quit or the session ends.
    pub fn uninstall() -> Result<Option<String>, ServiceError> {
        let Some(entry) = entry_location() else {
            return Err(ServiceError::new(
                "find the tray's autostart entry",
                "no home directory to look in",
            ));
        };
        if !entry.exists() {
            return Ok(None);
        }
        remove_file(&entry).map_err(|error| {
            ServiceError::new(
                "remove the tray's autostart entry",
                format!("{}: {error}", entry.display()),
            )
        })?;

        Ok(Some(entry.display().to_string()))
    }
}

/// macOS: a LaunchAgent limited to the graphical session.
#[cfg(target_os = "macos")]
mod platform {
    use super::super::platform::{load_agent, unload_agent};
    use super::super::{ServiceError, remove_file, write_file};
    use super::*;

    /// Where the tray's LaunchAgent is, or would be.
    pub fn entry_location() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library/LaunchAgents")
                .join(format!("{LAUNCH_AGENT}.plist"))
        })
    }

    /// Whether this command runs inside a login at the screen, by what launchd says it manages.
    fn in_aqua() -> bool {
        std::process::Command::new("launchctl")
            .arg("managername")
            .stdin(std::process::Stdio::null())
            .output()
            .is_ok_and(|output| {
                output.status.success() && aqua_session(&String::from_utf8_lossy(&output.stdout))
            })
    }

    /// Write the agent, and load it — which starts the tray — when there is an Aqua session to load it
    /// in, ending the tray already running there first. Without one, launchd loads it at the next login
    /// at the screen.
    ///
    /// The agent an earlier install loaded is unloaded before the lock is looked at: its tray is ended
    /// with it, and launchd, which restarts a tray that dies other than by quitting, cannot start the old
    /// one again while the new one waits for its lock. What then still holds the lock is a tray started
    /// outside launchd, which is ended by its pid.
    pub fn install(tray: &Path) -> Result<TrayInstalled, ServiceError> {
        existing(tray)?;
        let plist = entry_location().ok_or_else(|| {
            ServiceError::new("write the tray's agent", "no home directory to write it in")
        })?;
        write_file(
            "write the tray's agent",
            &plist,
            launch_agent(tray).as_bytes(),
        )?;
        let plist_text = plist.to_string_lossy().into_owned();
        let start = if in_aqua() {
            unload_agent(LAUNCH_AGENT);
            replace_running_tray(|| {
                load_agent(&plist_text).inspect_err(|_| {
                    let _ = remove_file(&plist);
                })
            })?
        } else {
            TrayStart::NextLogin
        };

        Ok(TrayInstalled {
            entry: plist_text,
            start,
        })
    }

    /// Unload the tray's agent, which quits the tray, and remove it.
    pub fn uninstall() -> Result<Option<String>, ServiceError> {
        let Some(plist) = entry_location() else {
            return Err(ServiceError::new(
                "find the tray's agent",
                "no home directory to look in",
            ));
        };
        if !plist.exists() {
            return Ok(None);
        }
        unload_agent(LAUNCH_AGENT);
        remove_file(&plist).map_err(|error| {
            ServiceError::new(
                "remove the tray's agent",
                format!("{}: {error}", plist.display()),
            )
        })?;

        Ok(Some(plist.display().to_string()))
    }
}

/// Windows: a value under the user's `Run` key, and the Start-menu shortcut whose AppUserModelID lets
/// the tray's toasts show.
#[cfg(windows)]
mod platform {
    use super::super::{ServiceError, remove_file};
    use super::*;

    /// The shortcut `keepster install` writes: the one in the user's own `Programs` folder.
    fn own_shortcut() -> Result<PathBuf, ServiceError> {
        shortcut_locations(|name| {
            (name == "APPDATA")
                .then(|| std::env::var_os(name))
                .flatten()
        })
        .into_iter()
        .next()
        .ok_or_else(|| ServiceError::new("find the Start menu", "APPDATA is not set"))
    }

    /// The value has no file of the user's: it is named, not located.
    pub fn entry_location() -> Option<PathBuf> {
        Some(PathBuf::from(format!(
            r"HKCU\{}\{RUN_VALUE}",
            crate::windows::RUN_KEY
        )))
    }

    /// Write the Start-menu shortcut and the `Run` value, then end the tray already running and start
    /// the new one now. The shortcut comes first, so that the tray started finds it and can show toasts
    /// from its first alarm.
    ///
    /// No graphical session is looked for: the `Run` key is read at an interactive logon, and a person
    /// running `keepster` on Windows is, short of an SSH login, in one.
    pub fn install(tray: &Path) -> Result<TrayInstalled, ServiceError> {
        existing(tray)?;
        let shortcut = own_shortcut()?;
        crate::windows::create_shortcut(&shortcut, tray).map_err(|error| {
            ServiceError::new(
                "write the Start-menu shortcut",
                format!("{}: {error}", shortcut.display()),
            )
        })?;
        let undo = || {
            let _ = crate::windows::delete_run_value(RUN_VALUE);
            let _ = remove_file(&shortcut);
        };
        if let Err(error) = crate::windows::set_run_value(RUN_VALUE, &run_command(tray)) {
            undo();
            return Err(ServiceError::new(
                "write the tray's Run value",
                error.to_string(),
            ));
        }
        // The `Run` value and the shortcut stay when a running tray is not ended: the next logon starts
        // the new one.
        let start = replace_running_tray(|| {
            spawn_detached(tray).map_err(|error| {
                undo();
                ServiceError::new("start the tray", format!("{}: {error}", tray.display()))
            })
        })?;

        Ok(TrayInstalled {
            entry: entries(&shortcut),
            start,
        })
    }

    /// Remove the `Run` value and the Start-menu shortcut. A tray running now goes on until it is quit
    /// or the session ends.
    pub fn uninstall() -> Result<Option<String>, ServiceError> {
        let removed = crate::windows::delete_run_value(RUN_VALUE)
            .map_err(|error| ServiceError::new("remove the tray's Run value", error.to_string()))?;
        let shortcut = own_shortcut()?;
        let had_shortcut = shortcut.exists();
        if had_shortcut {
            remove_file(&shortcut).map_err(|error| {
                ServiceError::new(
                    "remove the Start-menu shortcut",
                    format!("{}: {error}", shortcut.display()),
                )
            })?;
        }

        Ok((removed || had_shortcut).then(|| entries(&shortcut)))
    }

    /// What `keepster install` writes for the tray, for a person: the `Run` value and the shortcut.
    fn entries(shortcut: &Path) -> String {
        let run = entry_location()
            .map(|location| location.display().to_string())
            .unwrap_or_default();
        format!("{run} and {}", shortcut.display())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::super::ServiceError;
    use super::*;

    const UNSUPPORTED: &str = "this platform has no session Keepster can start a tray in";

    pub fn entry_location() -> Option<PathBuf> {
        None
    }

    pub fn install(tray: &Path) -> Result<TrayInstalled, ServiceError> {
        existing(tray)?;
        Err(ServiceError::new("install the tray", UNSUPPORTED))
    }

    pub fn uninstall() -> Result<Option<String>, ServiceError> {
        Err(ServiceError::new("uninstall the tray", UNSUPPORTED))
    }
}
