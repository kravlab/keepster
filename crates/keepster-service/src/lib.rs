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
