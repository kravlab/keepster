//! The single-instance authority: an OS lock on `daemon.pid`, held for the daemon's whole life
//! (ticket 05), and on `tray.pid` for the tray's (ticket 67). The pid written inside is for a person to
//! read; the lock, not the content, decides.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::num::NonZeroU32;
use std::path::Path;

/// The daemon's pid file's name in the runtime directory.
pub const PID_FILE: &str = "daemon.pid";

/// The tray's pid file's name in the runtime directory: its lock keeps one tray per user, so a second
/// one, started by hand, leaves at once. `keepster install` ends the running one first (issue 49), so
/// that the tray it starts is not the one to leave.
pub const TRAY_PID_FILE: &str = "tray.pid";

/// A process id, as a pid file holds one (issue 49). Never 0, which names no single process: on Unix
/// it would reach a whole process group, so no pid read from a file can stand for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pid(NonZeroU32);

impl Pid {
    /// `pid` as a process id, or `None` for 0.
    pub fn new(pid: u32) -> Option<Self> {
        NonZeroU32::new(pid).map(Self)
    }

    /// The number, as the platform's calls take it.
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Display for Pid {
    /// The bare number, as a person reads it after "pid".
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

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
        pid: Option<Pid>,
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

/// What clearing the runtime directory did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeDirCleared {
    /// It was removed whole.
    Removed,
    /// There was none.
    Absent,
    /// A daemon still holds its lock — one no service manager started, or one it did not stop — so the
    /// socket, the token and the lock it is serving from were left as they are.
    DaemonRunning,
    /// Everything but the lock of a tray still running was removed: unlinking a held lock would let a
    /// second tray start beside the first.
    TrayRunning,
}

/// Remove the runtime directory at `dir` (ticket 68), leaving what a process still running holds.
///
/// Everything in it lives for one start of a process — the socket, the browser's token, the pid files
/// — so nothing of the user's is lost; a lock is "held" when taking it is refused, as
/// [`lock_pid_file`] decides.
pub fn clear_runtime_dir(dir: &Path) -> io::Result<RuntimeDirCleared> {
    if !dir.exists() {
        return Ok(RuntimeDirCleared::Absent);
    }
    if pid_lock_holder(&dir.join(PID_FILE)) != PidLockHolder::Free {
        return Ok(RuntimeDirCleared::DaemonRunning);
    }
    if pid_lock_holder(&dir.join(TRAY_PID_FILE)) == PidLockHolder::Free {
        std::fs::remove_dir_all(dir)?;
        return Ok(RuntimeDirCleared::Removed);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name() == TRAY_PID_FILE {
            continue;
        }
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }

    Ok(RuntimeDirCleared::TrayRunning)
}

/// Who holds the lock on a pid file, as [`pid_lock_holder`] found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidLockHolder {
    /// Nobody: there is no file, or its lock could be taken — whatever pid a process long gone wrote
    /// into it.
    Free,
    /// A live process holds it, or the file could not even be opened to tell.
    Held {
        /// What the holder wrote into the file — `None` before it has written one, or when the file
        /// could not be opened, so that no process is ever named on a guess.
        pid: Option<Pid>,
    },
}

/// Who holds the lock on the pid file at `path` (issue 49): what `keepster install` asks before it
/// ends a running tray, and `keepster uninstall` before it clears the runtime directory.
///
/// The lock is taken and dropped at once to tell, so the answer holds for that moment only. A missing
/// file is not created. A file that cannot even be opened counts as held, by nobody named, so that what
/// cannot be judged is kept rather than cleared or ended.
pub fn pid_lock_holder(path: &Path) -> PidLockHolder {
    if !path.exists() {
        return PidLockHolder::Free;
    }
    match open_locked(path) {
        Ok(_) => PidLockHolder::Free,
        Err(PidLockError::Held { pid }) => PidLockHolder::Held { pid },
        Err(PidLockError::Io(_)) => PidLockHolder::Held { pid: None },
    }
}

/// The pid a live daemon wrote, for naming it: `None` for none, or for anything that is no pid, 0
/// among them.
fn written_pid(path: &Path) -> Option<Pid> {
    Pid::new(std::fs::read_to_string(path).ok()?.trim().parse().ok()?)
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
