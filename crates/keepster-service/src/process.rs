//! Ending another process by its pid (issue 49): how `keepster install`, and later `keepster update`,
//! end a tray running from an older install so that the one beside the new `keepster` can take its lock.

use std::io;

use crate::Pid;

/// Ask the process `pid` to end: `SIGTERM` on Unix, so it may leave as it does at a logout; process
/// termination on Windows, which has no such request for a process without a window to close.
///
/// Returns once the request is made, not once the process is gone: the caller waits for what it needs
/// gone, such as a lock. A [`Pid`] is never 0, and on Unix a pid beyond `i32::MAX` is refused too, so
/// that no call reaches a whole process group rather than one process.
///
/// A process that is already gone fails with [`io::ErrorKind::NotFound`] on every platform, so that a
/// caller waiting for it to go can tell that from a refusal, such as one owned by another user.
pub fn end_process(pid: Pid) -> io::Result<()> {
    #[cfg(unix)]
    {
        use rustix::process::{Pid as RawPid, Signal, kill_process};

        // Above `i32::MAX` it would read as negative, which `kill` takes for a process group.
        let raw = i32::try_from(pid.get())
            .ok()
            .and_then(RawPid::from_raw)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{pid} is beyond the range of a pid"),
                )
            })?;
        kill_process(raw, Signal::TERM).map_err(|error| {
            if error == rustix::io::Errno::SRCH {
                io::Error::new(io::ErrorKind::NotFound, format!("no process has pid {pid}"))
            } else {
                io::Error::from(error)
            }
        })
    }

    #[cfg(windows)]
    {
        crate::windows::terminate_process(pid.get())
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this platform has no way Keepster knows to end a process",
        ))
    }
}
