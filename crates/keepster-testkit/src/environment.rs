//! The machine a tripwire run measures on: its processor, memory, kernel, and the filesystem the fixture
//! is written to (ticket 30).
//!
//! Every report prints it beside the numbers, and none of it is committed: the thresholds file names no
//! machine. [`describe`] and the parsers under it are pure functions over what was read, so they are
//! tested on every platform; only [`detect`] reads `/proc`, and only on Linux, the one platform the
//! tripwire has a reference on. Mounts are read through `keepster_service::mountinfo`, the same reading
//! the daemon's volume lookup uses. A fixture in memory is refused rather than measured: tmpfs and ramfs
//! never touch a disk, so their numbers would say nothing about a capture that does.

use std::path::{Path, PathBuf};

#[cfg(not(windows))]
use keepster_core::{Platform, git_candidates};
use keepster_service::mountinfo::{PATH as MOUNTINFO, mount_of, parse};
use keepster_store::{Git, GitUnavailable};

/// What [`detect`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    /// The processor.
    pub cpu: Cpu,
    /// How many logical processors this process may run on: fewer than [`Cpu::threads`] under an
    /// affinity mask or a cgroup quota.
    pub available_threads: usize,
    /// The memory the kernel can use, in bytes: what is installed less what firmware and the kernel
    /// reserve, and the host's figure inside a container.
    pub memory_bytes: u64,
    /// The kernel's release string.
    pub kernel: String,
    /// The filesystem the fixture directory is on.
    pub filesystem: Filesystem,
}

/// A processor, as `/proc/cpuinfo` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cpu {
    /// Its model name; `None` on architectures whose `cpuinfo` names none.
    pub model: Option<String>,
    /// How many logical processors the machine has.
    pub threads: usize,
}

/// A mounted filesystem, as `/proc/self/mountinfo` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filesystem {
    /// Its type: `ext4`, `btrfs`, `tmpfs`.
    pub kind: String,
    /// What is mounted: the device, or a name standing in for one.
    pub source: String,
    /// Where it is mounted.
    pub mount_point: PathBuf,
}

impl Filesystem {
    /// Whether it lives in memory, where a measurement would say nothing about a disk.
    pub fn in_memory(&self) -> bool {
        matches!(self.kind.as_str(), "tmpfs" | "ramfs")
    }
}

/// Why the environment could not be described, or may not be measured in.
#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    /// Not Linux: the tripwire's one reference machine runs Linux (ticket 30).
    #[error("the tripwire runs on Linux only, the platform its reference machine runs")]
    NotLinux,
    /// A file describing the machine, or the fixture directory itself, could not be read.
    #[error("{path} cannot be read")]
    Unreadable {
        /// Which.
        path: PathBuf,
        /// What reading it said.
        source: std::io::Error,
    },
    /// The system would not say how many processors this process may run on.
    #[error("the processors this process may run on cannot be counted")]
    Uncounted {
        /// What asking said.
        source: std::io::Error,
    },
    /// A file describing the machine does not say what it should.
    #[error("{path} does not give {what}")]
    Malformed {
        /// Which.
        path: PathBuf,
        /// What it was read for.
        what: &'static str,
    },
    /// No mount the mount table lists holds the fixture directory.
    #[error("no mount in {MOUNTINFO} holds the fixture directory {directory}")]
    NoMount {
        /// The fixture directory.
        directory: PathBuf,
    },
    /// The fixture directory is on a filesystem in memory.
    #[error(
        "the fixture directory is on {kind} at {mount_point}, in memory, where a measurement says nothing \
         about a disk"
    )]
    InMemory {
        /// The filesystem's type.
        kind: String,
        /// Where it is mounted.
        mount_point: PathBuf,
    },
}

/// Where [`detect`] reads the processor.
const CPUINFO: &str = "/proc/cpuinfo";
/// Where [`detect`] reads the memory.
const MEMINFO: &str = "/proc/meminfo";
/// Where [`detect`] reads the kernel release.
const OSRELEASE: &str = "/proc/sys/kernel/osrelease";

/// What [`detect`] reads, for [`describe`] to make an [`Environment`] of.
#[derive(Debug, Clone, Copy)]
pub struct Readings<'a> {
    /// The fixture directory: absolute, with no symlink in it.
    pub directory: &'a Path,
    /// The device its `stat` reports.
    pub device: u64,
    /// How many logical processors this process may run on.
    pub available_threads: usize,
    /// The bytes of `/proc/self/mountinfo`.
    pub mountinfo: &'a [u8],
    /// The text of `/proc/cpuinfo`.
    pub cpuinfo: &'a str,
    /// The text of `/proc/meminfo`.
    pub meminfo: &'a str,
    /// The text of `/proc/sys/kernel/osrelease`.
    pub osrelease: &'a str,
}

/// The environment a run writing its fixture beneath `directory` measures in.
///
/// # Errors
///
/// [`EnvironmentError::NotLinux`] off Linux; [`EnvironmentError::InMemory`] when `directory` is on tmpfs
/// or ramfs; otherwise when `directory` or a `/proc` file cannot be read or does not parse.
pub fn detect(directory: &Path) -> Result<Environment, EnvironmentError> {
    #[cfg(target_os = "linux")]
    {
        linux::detect(directory)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = directory;
        Err(EnvironmentError::NotLinux)
    }
}

/// The machine's own `git`, found as the daemon finds it: the platform's stable places, never `PATH`
/// (ticket 11). The tripwire maintains its stores with it, so store growth is measured after the
/// Maintenance Pass a user's daemon would run. Unlike the daemon it takes no configured path and does
/// not look for macOS's stub, which only changes what a missing `git` is reported as.
///
/// # Errors
///
/// When no `git` is at any of those places, or the one found does not run or is too old.
pub fn git() -> Result<Git, GitUnavailable> {
    #[cfg(windows)]
    let candidates = keepster_service::git_for_windows();
    #[cfg(not(windows))]
    let candidates = git_candidates(
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        },
        std::env::home_dir().as_deref(),
        None,
    );

    Git::locate(None, &candidates, None)
}

/// The environment `readings` describe.
///
/// # Errors
///
/// [`EnvironmentError::NoMount`] when no mount holds the fixture directory;
/// [`EnvironmentError::InMemory`] when the one that does is tmpfs or ramfs;
/// [`EnvironmentError::Malformed`], naming the file, when one does not say what it is read for.
///
/// # Panics
///
/// When the fixture directory does not start from the root: a mount point holds only paths that do, so
/// one that does not would be held by none, a mistake in the caller rather than a fact about the machine.
/// Starting from the root, not being absolute, is what is asked: mountinfo's paths are Unix paths, which
/// Windows, where these functions are tested too, calls rooted but not absolute.
pub fn describe(readings: &Readings) -> Result<Environment, EnvironmentError> {
    let directory = readings.directory;
    assert!(
        directory.has_root(),
        "the fixture directory {} must start from the root",
        directory.display()
    );
    let malformed = |path: &str, what| EnvironmentError::Malformed {
        path: PathBuf::from(path),
        what,
    };

    let mounts = parse(readings.mountinfo);
    let mount =
        mount_of(&mounts, directory, readings.device).ok_or_else(|| EnvironmentError::NoMount {
            directory: directory.to_owned(),
        })?;
    let filesystem = Filesystem {
        kind: mount.filesystem.clone(),
        source: mount.source.clone(),
        mount_point: mount.mount_point.clone(),
    };
    if filesystem.in_memory() {
        return Err(EnvironmentError::InMemory {
            kind: filesystem.kind,
            mount_point: filesystem.mount_point,
        });
    }
    let cpu = cpu(readings.cpuinfo);
    if cpu.threads == 0 {
        return Err(malformed(CPUINFO, "a processor"));
    }
    let memory_bytes =
        memory(readings.meminfo).ok_or_else(|| malformed(MEMINFO, "the memory total"))?;
    let kernel =
        kernel(readings.osrelease).ok_or_else(|| malformed(OSRELEASE, "the kernel release"))?;

    Ok(Environment {
        cpu,
        available_threads: readings.available_threads,
        memory_bytes,
        kernel,
        filesystem,
    })
}

/// [`detect`] on Linux: the reading, around [`describe`].
#[cfg(target_os = "linux")]
mod linux {
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    use super::{
        CPUINFO, Environment, EnvironmentError, MEMINFO, MOUNTINFO, OSRELEASE, Readings, describe,
    };

    /// [`super::detect`], here where everything it needs is in `/proc`.
    pub(super) fn detect(directory: &Path) -> Result<Environment, EnvironmentError> {
        let unreadable = |source| EnvironmentError::Unreadable {
            path: directory.to_owned(),
            source,
        };
        let real = directory.canonicalize().map_err(unreadable)?;
        let device = real.metadata().map_err(unreadable)?.dev();
        let available_threads = std::thread::available_parallelism()
            .map_err(|source| EnvironmentError::Uncounted { source })?
            .get();
        let mountinfo = read(MOUNTINFO)?;
        let (cpuinfo, meminfo, osrelease) = (text(CPUINFO)?, text(MEMINFO)?, text(OSRELEASE)?);

        describe(&Readings {
            directory: &real,
            device,
            available_threads,
            mountinfo: &mountinfo,
            cpuinfo: &cpuinfo,
            meminfo: &meminfo,
            osrelease: &osrelease,
        })
    }

    /// The bytes of `path`.
    fn read(path: &str) -> Result<Vec<u8>, EnvironmentError> {
        std::fs::read(path).map_err(|source| EnvironmentError::Unreadable {
            path: PathBuf::from(path),
            source,
        })
    }

    /// The text of `path`, read lossily: the files read as text are only ever shown.
    fn text(path: &str) -> Result<String, EnvironmentError> {
        read(path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// The processor `/proc/cpuinfo`'s text describes: the first `model name` and how many `processor`
/// entries there are.
pub fn cpu(cpuinfo: &str) -> Cpu {
    let mut model = None;
    let mut threads = 0;
    for line in cpuinfo.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "processor" => threads += 1,
            "model name" if model.is_none() => model = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    Cpu { model, threads }
}

/// The memory `/proc/meminfo`'s text says the kernel can use, in bytes: its `MemTotal`, which the kernel
/// gives in kibibytes and labels `kB`.
pub fn memory(meminfo: &str) -> Option<u64> {
    let total = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?;
    let kibibytes = total.trim().strip_suffix(" kB")?.trim();
    kibibytes.parse::<u64>().ok()?.checked_mul(1024)
}

/// The kernel release `/proc/sys/kernel/osrelease`'s text names, without its line break.
pub fn kernel(osrelease: &str) -> Option<String> {
    let release = osrelease.trim();
    (!release.is_empty()).then(|| release.to_owned())
}
