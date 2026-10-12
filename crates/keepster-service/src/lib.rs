//! Platform integration: where the daemon's control channel lives, and — from ticket 61 — how
//! autostart is installed on each platform.
//!
//! The control-channel path lives here rather than in the daemon because the CLI needs it too, and
//! ticket 08 forbids the CLI to depend on the daemon. It is a platform fact, and this is the
//! platform crate.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub mod autostart;
mod pid;
mod volume;
#[cfg(windows)]
mod windows;

pub use pid::{PID_FILE, PidLock, PidLockError, lock_pid_file};
pub use volume::{Volume, mount_points, volume_of};

#[cfg(windows)]
pub use windows::{
    OwnerOnlyAccess, create_control_pipe, current_user_sid, git_for_windows, hard_link_names,
};

/// Where the daemon listens for the CLI, and where the CLI looks for it.
///
/// One path per platform, needing no discovery file, protected by OS permissions rather than by a
/// secret (ticket 05). On Unix the socket sits in a per-user runtime directory; the path is kept
/// short deliberately, because `sun_path` holds 104 bytes on macOS and 108 on Linux. On Windows it is
/// a named pipe carrying the owner's SID in its name, so two users on one machine never collide.
///
/// Returns `None` when the platform's per-user runtime directory — or, on Windows, the owning SID —
/// cannot be determined, which the caller reports rather than guessing a world-readable location.
pub fn default_control_path() -> Option<PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        default_runtime_dir().map(|dir| dir.join("daemon.sock"))
    }

    #[cfg(windows)]
    {
        // The SID, not the user name: a display name can be shared or changed, a SID cannot.
        current_user_sid()
            .ok()
            .map(|sid| PathBuf::from(format!(r"\\.\pipe\keepster-{sid}")))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

/// The per-user runtime directory, private to its owner, which is where a secret that lives for one
/// daemon start belongs — the loopback listener's token (ticket 60). On Unix it holds the control socket
/// too.
///
/// Returns `None` when the platform's per-user runtime directory cannot be determined.
pub fn default_runtime_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        // `XDG_RUNTIME_DIR` is the documented per-user, permission-protected runtime directory.
        std::env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join("keepster"))
    }

    #[cfg(target_os = "macos")]
    {
        // Per-user and private on macOS, where there is no `XDG_RUNTIME_DIR`.
        std::env::var_os("TMPDIR").map(|dir| PathBuf::from(dir).join("keepster"))
    }

    #[cfg(windows)]
    {
        // A named pipe has no directory, so the token sits beside the store, under a per-user profile
        // directory whose inherited ACL admits its owner, SYSTEM and the administrators alone.
        std::env::var_os("LOCALAPPDATA").map(|local| PathBuf::from(local).join("Keepster\\run"))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

/// Where Keepster keeps its own data: one directory per Watched Root, holding that Root's Registry
/// record and its History Store.
///
/// This is the **global** store, which is the default; a Root may instead keep its History Store inside
/// its own directory, and ticket 42 makes that a configuration key — settable only from the global
/// configuration, because a file arriving in someone else's repository must not decide where your
/// history goes.
///
/// Returns `None` when the platform's per-user data directory cannot be determined.
pub fn default_store_base() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        // The XDG specification's directory for user-specific data files.
        if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
            return Some(PathBuf::from(data_home).join("keepster"));
        }

        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share/keepster"))
    }

    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library/Application Support/Keepster"))
    }

    #[cfg(windows)]
    {
        // Per-user and not roamed: the store holds every version of every watched file, which has no
        // business travelling to another machine.
        std::env::var_os("LOCALAPPDATA").map(|local| PathBuf::from(local).join("Keepster"))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

/// Where the user's global configuration lives: one TOML file, the user's own, which travels in nobody's
/// clone (ticket 04).
///
/// The platform's per-user configuration directory, so it sits where the user looks for configuration
/// and apart from the store, which is data. Returns `None` when that directory cannot be determined; the
/// daemon then runs on defaults and the user's own `.keepster.toml` files.
pub fn default_config_path() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        // The XDG specification's directory for user-specific configuration.
        if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
            return Some(PathBuf::from(config_home).join("keepster/config.toml"));
        }

        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config/keepster/config.toml"))
    }

    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home).join("Library/Application Support/Keepster/config.toml")
        })
    }

    #[cfg(windows)]
    {
        // Roamed, unlike the store: configuration is small and the user's to carry between machines.
        std::env::var_os("APPDATA")
            .map(|roaming| PathBuf::from(roaming).join("Keepster/config.toml"))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

/// The global configuration of an install that declines the web UI: no loopback listener, so no
/// network surface and no token (ticket 24).
pub const NO_WEB_UI: &str = "[ui]\nlistener = false\n";

/// What declaring a CLI-only install did to the global configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoWebUi {
    /// There was none, and one now switches the listener off.
    Created,
    /// There was one, which is left exactly as it is: rewriting a person's TOML would lose their
    /// comments and their order. `add` is the line for them to add, in its `[ui]` table — which the
    /// file may already have, so the table is not part of the line.
    Existing {
        /// The line to add under `[ui]`.
        add: &'static str,
    },
}

/// Declare at install time that this machine has no web UI: write the configuration that switches the
/// loopback listener off — only when this creates the file (ticket 24).
///
/// `keepster service install --no-web-ui` calls this (ticket 61). The file stays the single source of
/// truth: this is one way of producing it, and the daemon cannot tell how the key got there. A file
/// created at the same moment by someone else is theirs, and is left alone like any existing one.
pub fn declare_no_web_ui(config_path: &Path) -> io::Result<NoWebUi> {
    if let Some(directory) = config_path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(config_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Ok(NoWebUi::Existing {
                add: "listener = false",
            });
        }
        Err(error) => return Err(error),
    };
    let written = file
        .write_all(
            format!("# Written by `keepster service install --no-web-ui`.\n{NO_WEB_UI}").as_bytes(),
        )
        .and_then(|()| file.sync_all());
    if let Err(error) = written {
        // A file half-written by this call is not a person's; it goes rather than leave a typo behind.
        drop(file);
        let _ = std::fs::remove_file(config_path);
        return Err(error);
    }

    Ok(NoWebUi::Created)
}

/// Create `directory` and its parents, readable by their owner alone where the platform has modes.
pub(crate) fn create_private_dir(directory: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }

    builder.create(directory)
}

/// Whether a daemon answers on the control channel at `path` right now.
///
/// Connecting is the test, not the presence of a file: a socket a crash left behind is a file nothing
/// listens on.
pub fn daemon_answers(path: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::connect(path).is_ok()
    }

    #[cfg(windows)]
    {
        // Opening a pipe instance is a connection; the daemon has the next instance ready at once. Every
        // instance busy at this instant is a daemon that answers too, just not this microsecond.
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
        {
            Ok(_) => true,
            Err(error) => {
                error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32)
            }
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        false
    }
}
