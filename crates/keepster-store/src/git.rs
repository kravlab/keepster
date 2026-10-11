//! The external `git`, for maintenance only (ticket 52, ticket 11): found without an inherited `PATH`,
//! its version checked, and every invocation hermetic.
//!
//! Capture writes through the in-process library and never needs it. Pruning what Retention made
//! unreachable does — neither libgit2 nor gitoxide collects garbage (ADR 0001) — so its absence costs
//! space, never history.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use keepster_core::{GIT_MINIMUM, GitVersion};

/// Configuration the store's correctness rests on: written into each store when it is created, and
/// passed again on every invocation, where the command line outranks every configuration file — so a
/// person running `git gc` in the store by hand gets the same behaviour, and an edit to the store's
/// config changes nothing ours does (ticket 11).
pub(crate) const STORE_CONFIG: [(&str, bool); 2] = [
    // Ref deletion frees objects only if no reflog still reaches them (ADR 0001).
    ("core.logAllRefUpdates", false),
    // Trees hold modes we chose, never the filesystem's (ADR 0001).
    ("core.filemode", false),
];

/// Environment variables passed through to `git`, by name; everything else is withheld, so nothing the
/// daemon was started with — `GIT_DIR`, `GIT_WORK_TREE`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`, a pager —
/// reaches it. Windows needs its system directories and temporary directory to start a process at all.
const PASSED_THROUGH: [&str; 5] = ["SYSTEMROOT", "WINDIR", "TEMP", "TMP", "TMPDIR"];

/// How long `git --version` may take before the binary is taken for one that cannot be run: a `git.path`
/// on a hung mount must not hang the daemon.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long one Maintenance Pass may take before it is stopped. A store of a few thousand commits packs
/// in seconds (ticket 06), so an hour is a `git` that will not finish.
const PASS_TIMEOUT: Duration = Duration::from_secs(3600);

/// A `git` the daemon may run: where it is, and what version it said it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Git {
    path: PathBuf,
    version: GitVersion,
}

/// Why there is no `git` maintenance can run with. Each is reported, never fatal: capture goes on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitUnavailable {
    /// None was found where one is looked for.
    #[error("no git was found")]
    Missing {
        /// Every place looked in.
        looked: Vec<PathBuf>,
    },
    /// Only macOS's `/usr/bin/git` is there, a stub that raises an installer dialog when run — which it
    /// never is.
    #[error("only the macOS stub of git is installed, without the Command Line Tools")]
    MacosStub,
    /// One is there and could not be run, or did not say what version it is.
    #[error("{path} could not be run as git: {error}")]
    NotExecutable {
        /// Which.
        path: PathBuf,
        /// What running it said.
        error: String,
    },
    /// One is there, older than [`GIT_MINIMUM`].
    #[error("{path} is git {version}, older than {GIT_MINIMUM}")]
    TooOld {
        /// Which.
        path: PathBuf,
        /// What it said it was.
        version: GitVersion,
    },
    /// A Maintenance Pass ran it and it failed.
    #[error("{path} failed: {stderr}")]
    Failed {
        /// Which.
        path: PathBuf,
        /// What version it said it was.
        version: GitVersion,
        /// What it wrote to its error stream, or how it ended.
        stderr: String,
    },
}

impl GitUnavailable {
    /// The reason as the wire spells it: an open set, as error codes are (ADR 0005).
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Missing { .. } => "missing",
            Self::MacosStub => "macos_stub",
            Self::NotExecutable { .. } => "not_executable",
            Self::TooOld { .. } => "too_old",
            Self::Failed { .. } => "failed",
        }
    }

    /// The `git` it is about, if one was found.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::NotExecutable { path, .. }
            | Self::TooOld { path, .. }
            | Self::Failed { path, .. } => Some(path),
            Self::Missing { .. } | Self::MacosStub => None,
        }
    }

    /// The version the `git` it is about said it was, if it said.
    pub fn version(&self) -> Option<GitVersion> {
        match self {
            Self::TooOld { version, .. } | Self::Failed { version, .. } => Some(*version),
            Self::Missing { .. } | Self::MacosStub | Self::NotExecutable { .. } => None,
        }
    }

    /// What running it said, word for word, when it was run and went wrong.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Failed { stderr, .. } => Some(stderr),
            Self::NotExecutable { error, .. } => Some(error),
            Self::Missing { .. } | Self::MacosStub | Self::TooOld { .. } => None,
        }
    }
}

impl Git {
    /// The `git` to maintain stores with: `configured` when it is set — authoritatively, so one that does
    /// not work is reported rather than replaced by another — and otherwise the first of `candidates`
    /// that is a file, found by `stat` alone so that no candidate is run to test it.
    ///
    /// `stub` is a place where only a stub that must never be run may sit: when it is there and nothing
    /// else is, that is what is said.
    pub fn locate(
        configured: Option<&Path>,
        candidates: &[PathBuf],
        stub: Option<&Path>,
    ) -> Result<Self, GitUnavailable> {
        if let Some(path) = configured {
            return Self::probe(path);
        }
        if let Some(found) = candidates
            .iter()
            .find(|candidate| std::fs::metadata(candidate).is_ok_and(|metadata| metadata.is_file()))
        {
            return Self::probe(found);
        }
        if stub.is_some_and(Path::exists) {
            return Err(GitUnavailable::MacosStub);
        }

        Err(GitUnavailable::Missing {
            looked: candidates.to_vec(),
        })
    }

    /// The `git` at `path`, if it runs and is new enough: `--version` is read leniently, and an unknown
    /// patch number is enough (ticket 11).
    pub fn probe(path: &Path) -> Result<Self, GitUnavailable> {
        let not_executable = |error: String| GitUnavailable::NotExecutable {
            path: path.to_path_buf(),
            error,
        };
        let mut command = hermetic(path);
        command.arg("--version");
        let output =
            run(command, PROBE_TIMEOUT).map_err(|error| not_executable(error.to_string()))?;
        if !output.status.success() {
            return Err(not_executable(stderr_of(&output.stderr, &output.status)));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let version = GitVersion::parse(&text)
            .ok_or_else(|| not_executable(format!("it printed `{}`", text.trim())))?;
        if !version.meets(GIT_MINIMUM) {
            return Err(GitUnavailable::TooOld {
                path: path.to_path_buf(),
                version,
            });
        }

        Ok(Self {
            path: path.to_path_buf(),
            version,
        })
    }

    /// Where it is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What version it said it was.
    pub fn version(&self) -> GitVersion {
        self.version
    }

    /// Run it in the store at `git_dir` with `arguments`, hermetically: the store named explicitly, the
    /// system configuration off and the global one an empty file, the store's correctness keys on the
    /// command line, and nothing of the daemon's environment but what Windows needs to start a process.
    pub(crate) fn run_in(&self, git_dir: &Path, arguments: &[&str]) -> Result<(), GitUnavailable> {
        let failed = |stderr: String| GitUnavailable::Failed {
            path: self.path.clone(),
            version: self.version,
            stderr,
        };
        let empty = empty_config(git_dir).map_err(|error| {
            failed(format!(
                "the empty configuration it is pointed at could not be written: {error}"
            ))
        })?;

        // The store's path as the platform holds it, never through a lossy display.
        let mut store = OsString::from("--git-dir=");
        store.push(git_dir);
        let mut command = hermetic(&self.path);
        command.env("GIT_CONFIG_GLOBAL", &empty).arg(store);
        for (key, value) in STORE_CONFIG {
            command.arg("-c").arg(format!("{key}={value}"));
        }
        command.args(arguments);
        let output = run(command, PASS_TIMEOUT).map_err(|error| failed(error.to_string()))?;
        if !output.status.success() {
            return Err(failed(stderr_of(&output.stderr, &output.status)));
        }

        Ok(())
    }
}

/// A command running `git` at `path` with no shell, no inherited environment and no system
/// configuration.
fn hermetic(path: &Path) -> Command {
    let mut command = Command::new(path);
    command.env_clear();
    for name in PASSED_THROUGH {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .stdin(Stdio::null());

    command
}

/// Run `command` to its end, or stop it after `timeout` — reading its output meanwhile, so a full pipe
/// never stalls it.
fn run(mut command: Command, timeout: Duration) -> io::Result<Output> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let drain = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>),
    );

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "it did not finish within {}s and was stopped",
                    timeout.as_secs()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// The empty file the global configuration is pointed at, beside the store's own: an empty file rather
/// than the null device, whose meaning on Windows git does not document (ticket 11).
fn empty_config(git_dir: &Path) -> io::Result<PathBuf> {
    let path = git_dir.join("keepster-empty.gitconfig");
    if std::fs::metadata(&path).map_or(true, |metadata| metadata.len() != 0) {
        std::fs::write(&path, b"")?;
    }

    Ok(path)
}

/// What a failed run wrote to its error stream, or how it ended when it wrote nothing.
fn stderr_of(stderr: &[u8], status: &std::process::ExitStatus) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_owned();
    if text.is_empty() {
        format!("it exited with {status}")
    } else {
        text
    }
}
