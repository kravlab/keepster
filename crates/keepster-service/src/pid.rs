//! The single-instance authority: an OS lock on `daemon.pid`, held for the daemon's whole life
//! (ticket 05). The pid written inside is for a person to read; the lock, not the content, decides.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// The pid file's name in the runtime directory.
pub const PID_FILE: &str = "daemon.pid";

/// The lock on the pid file, held until this is dropped — or until the process dies, which the
/// operating system turns into a release, so a crash never leaves a lock nobody holds.
#[derive(Debug)]
pub struct PidLock {
    _file: File,
}

/// Why the lock was not taken.
#[derive(Debug)]
pub enum PidLockError {
    /// Another process holds it: a daemon is running, and this is its pid, if it had written one yet.
    Held {
        /// What the live daemon wrote into the file.
        pid: Option<u32>,
    },
    /// The file could not be opened, locked or written.
    Io(io::Error),
}

impl From<io::Error> for PidLockError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Take the lock on the pid file at `path`, without waiting, and write this process's pid into it.
pub fn lock_pid_file(path: &Path) -> Result<PidLock, PidLockError> {
    if let Some(directory) = path.parent() {
        crate::create_private_dir(directory)?;
    }
    let mut file = open_locked(path)?;
    // Only once the lock is ours: a refused second daemon must not have truncated the live one's pid.
    file.set_len(0)?;
    write!(file, "{}", std::process::id())?;
    file.sync_all()?;

    Ok(PidLock { _file: file })
}

/// The pid a live daemon wrote, for naming it.
fn written_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(unix)]
fn open_locked(path: &Path) -> Result<File, PidLockError> {
    use std::os::unix::fs::OpenOptionsExt;

    use rustix::fs::{FlockOperation, flock};
    use rustix::io::Errno;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(file),
        Err(error) if error == Errno::WOULDBLOCK => Err(PidLockError::Held {
            pid: written_pid(path),
        }),
        Err(error) => Err(PidLockError::Io(error.into())),
    }
}

/// On Windows the lock is the file's share mode: opened for writing and shared for reading alone, no
/// other process can open it for writing while this handle lives — and anyone may still read the pid.
#[cfg(windows)]
fn open_locked(path: &Path) -> Result<File, PidLockError> {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(FILE_SHARE_READ)
        .open(path)
    {
        Ok(file) => Ok(file),
        // Another handle holds the file without sharing it for writing.
        Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) => {
            Err(PidLockError::Held {
                pid: written_pid(path),
            })
        }
        Err(error) => Err(PidLockError::Io(error)),
    }
}
