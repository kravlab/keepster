//! Replacing a running tray (issue 49): the tray holding the tray's lock is ended before the one beside
//! the new `keepster` is started, which would otherwise find the lock held and leave at once. `keepster
//! install` does this, and `keepster update` will too (ADR 0015).

use std::path::Path;
use std::time::{Duration, Instant};

use crate::{Pid, PidLockHolder};

/// How long a running tray asked to end has to release its lock before `keepster install` gives up on
/// replacing it: the tray leaves at once when asked, so this is room for a loaded machine, not for work.
pub const END_PATIENCE: Duration = Duration::from_secs(5);

/// How often the lock is looked at while waiting for it.
const POLL: Duration = Duration::from_millis(50);

/// What [`end_running`] found and did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEnded {
    /// No tray held the lock: there was nothing to end.
    NoneRunning,
    /// The tray holding the lock was asked to end — or was found gone already — and its lock was
    /// released in time.
    Ended {
        /// The pid it had written into its lock.
        pid: Pid,
    },
}

/// A tray holds the lock and was not ended: it would not end in time, could not be asked to, or wrote no
/// pid to ask by. A tray started now would find the lock held and leave at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayNotEnded {
    /// The pid written into the lock, or `None` when there was none to read.
    pub pid: Option<Pid>,
}

/// End the tray holding the tray's lock at `lock`, and wait for the lock to be released: the tray
/// started next must find it free, or it leaves at once and the old one goes on running from wherever
/// the previous install put it. `patience` bounds the whole of it, the waits below included.
///
/// The process ended is the one whose pid the lock's holder wrote. A tray writes its pid just after it
/// takes the lock, so a holder that wrote none yet is waited for; one that still has not when patience
/// runs out is not ended, since a guess could end a stranger. A lock released before any pid was read is
/// [`TrayEnded::NoneRunning`]: nothing was asked to end.
///
/// A holder already gone when asked — exiting on its own meanwhile — is not a refusal: its lock follows
/// it, and is waited for as an ended tray's is.
pub fn end_running(lock: &Path, patience: Duration) -> Result<TrayEnded, TrayNotEnded> {
    let deadline = Instant::now() + patience;
    let pid = loop {
        match crate::pid_lock_holder(lock) {
            PidLockHolder::Free => return Ok(TrayEnded::NoneRunning),
            PidLockHolder::Held { pid: Some(pid) } => break pid,
            PidLockHolder::Held { pid: None } if Instant::now() >= deadline => {
                return Err(TrayNotEnded { pid: None });
            }
            PidLockHolder::Held { pid: None } => std::thread::sleep(POLL),
        }
    };
    let not_ended = TrayNotEnded { pid: Some(pid) };
    match crate::end_process(pid) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(not_ended),
    }

    // The lock, not the process, is what the next tray needs: it is released when the process is gone,
    // even while nobody has reaped it yet.
    loop {
        if crate::pid_lock_holder(lock) == PidLockHolder::Free {
            return Ok(TrayEnded::Ended { pid });
        }
        if Instant::now() >= deadline {
            return Err(not_ended);
        }
        std::thread::sleep(POLL);
    }
}

/// End the tray running now, if any, in this user's runtime directory, waiting [`END_PATIENCE`].
/// Without a runtime directory there is no lock to look at, and nothing is ended.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn end_running_tray() -> Result<(), TrayNotEnded> {
    let Some(runtime) = crate::default_runtime_dir() else {
        return Ok(());
    };

    end_running(&runtime.join(crate::TRAY_PID_FILE), END_PATIENCE).map(drop)
}

/// End the tray running now, then `start` the new one — each platform's own way of starting it — and
/// say which happened. A running tray that is not ended is not followed by a start: the new tray would
/// find the lock held and leave at once, so the old one is reported instead, as not replaced.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub(super) fn replace_running_tray(
    start: impl FnOnce() -> Result<(), super::super::ServiceError>,
) -> Result<super::TrayStart, super::super::ServiceError> {
    if let Err(not_ended) = end_running_tray() {
        return Ok(not_ended.into());
    }
    start()?;

    Ok(super::TrayStart::Started)
}
