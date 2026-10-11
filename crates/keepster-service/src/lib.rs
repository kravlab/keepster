//! Platform integration: where the daemon's control channel lives, and — from ticket 61 — how
//! autostart is installed on each platform.
//!
//! The control-channel path lives here rather than in the daemon because the CLI needs it too, and
//! ticket 08 forbids the CLI to depend on the daemon. It is a platform fact, and this is the
//! platform crate.

use std::path::PathBuf;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{OwnerOnlyAccess, create_control_pipe, current_user_sid};

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
    #[cfg(target_os = "linux")]
    {
        // `XDG_RUNTIME_DIR` is the documented per-user, permission-protected runtime directory.
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join("keepster/daemon.sock"))
    }

    #[cfg(target_os = "macos")]
    {
        // Per-user and private on macOS, where there is no `XDG_RUNTIME_DIR`.
        std::env::var_os("TMPDIR").map(|dir| PathBuf::from(dir).join("keepster/daemon.sock"))
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
