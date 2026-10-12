//! The `git` the daemon maintains stores with (ticket 52, ticket 11): looked for at the start and at each
//! housekeeping look — never under another lock, and never longer than a probe's timeout — checked again
//! right before its first real pass, and reported in `status` with the command that installs one when
//! there is none.
//!
//! One daemon runs one `git`, so whether maintenance can run is the daemon's state, never a Root's.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use keepster_api::{GitStatus, InstallHint, Reported};
use keepster_core::{GIT_DOWNLOADS, GIT_MINIMUM, PackageFamily, Platform};
use keepster_store::{Git, GitUnavailable};

use crate::DaemonState;

/// How often a `git` that is not usable is looked for again — one may have been installed or upgraded
/// since — when `git.path` has not changed. A failed one is never looked for again this way: what it
/// said stays in `status` until `git.path` changes or the daemon restarts.
const LOOK_AGAIN: Duration = Duration::from_secs(60);

/// What the daemon knows of its `git`.
#[derive(Debug, Clone)]
pub(crate) struct Found {
    /// Which finding this is: a probe started on an older one never writes over a newer one.
    generation: u64,
    /// The `git.path` it was found under, so a change to it finds it again.
    configured: Option<PathBuf>,
    /// When it was looked for.
    looked: Instant,
    /// The `git`, or why there is none.
    git: Result<Git, GitUnavailable>,
    /// Whether a real pass has run it since it was found: the version is checked again before the first,
    /// and until one has succeeded a failure is the `git`'s, not one store's.
    proven: bool,
}

/// Look for the `git` again if `git.path` changed, or — at most once a minute — if none is usable for a
/// reason other than a failed pass. Runs on the housekeeping loop's thread, holding no lock while it
/// looks.
pub(crate) fn refresh(state: &DaemonState) {
    let configured = state.config().git_path();
    let previous = state.git().clone();
    let stale = match &previous {
        None => true,
        Some(found) => {
            found.configured != configured
                || (matches!(
                    found.git,
                    Err(GitUnavailable::Missing { .. }
                        | GitUnavailable::MacosStub
                        | GitUnavailable::TooOld { .. }
                        | GitUnavailable::NotExecutable { .. })
                ) && found.looked.elapsed() >= LOOK_AGAIN)
        }
    };
    if !stale {
        return;
    }

    let git = locate(configured.as_deref());
    match (&git, previous.as_ref().map(|found| &found.git)) {
        (Err(unavailable), Some(Err(before))) if before == unavailable => {}
        (Err(unavailable), _) => tracing::warn!(%unavailable, "store maintenance is unavailable"),
        (Ok(git), _) => {
            tracing::info!(git = %git.path().display(), version = %git.version(), "store maintenance runs with this git");
        }
    }
    let mut current = state.git();
    let generation = current.as_ref().map_or(0, |found| found.generation + 1);
    *current = Some(Found {
        generation,
        configured,
        looked: Instant::now(),
        git,
        proven: false,
    });
}

/// The `git` found, if one is usable — without running it.
pub(crate) fn current(state: &DaemonState) -> Option<(u64, Git)> {
    let found = state.git();
    let found = found.as_ref()?;

    found
        .git
        .as_ref()
        .ok()
        .map(|git| (found.generation, git.clone()))
}

/// The `git` of `generation`, checked again if no pass has run it yet: a version printed when it was
/// found says nothing of the binary that is there now (ticket 11). Called right before a real pass; a
/// `git` that fails the check is unavailable from then on, and the error says why.
pub(crate) fn proven(
    state: &DaemonState,
    generation: u64,
    git: Git,
) -> Result<Git, GitUnavailable> {
    let checked = {
        let found = state.git();
        found
            .as_ref()
            .is_some_and(|found| found.generation == generation && found.proven)
    };
    if checked {
        return Ok(git);
    }

    Git::probe(git.path()).inspect_err(|unavailable| {
        failed(state, generation, unavailable.clone());
    })
}

/// A pass with the `git` of `generation` succeeded: it is proven.
pub(crate) fn succeeded(state: &DaemonState, generation: u64) {
    if let Some(found) = state
        .git()
        .as_mut()
        .filter(|found| found.generation == generation)
    {
        found.proven = true;
    }
}

/// A pass with the `git` of `generation` failed. Until a pass has succeeded with it, that is the
/// `git`'s failure, and maintenance is unavailable for every Root with what it said in `status`; after
/// one has, it is the store's alone, which the caller reports and retries the next day. Says which.
pub(crate) fn failed(state: &DaemonState, generation: u64, unavailable: GitUnavailable) -> bool {
    let mut found = state.git();
    let Some(found) = found
        .as_mut()
        .filter(|found| found.generation == generation)
    else {
        return false;
    };
    if found.proven {
        return false;
    }
    found.git = Err(unavailable);

    true
}

/// What `status` says of maintenance and its `git`.
pub(crate) fn report(state: &DaemonState) -> (Reported, GitStatus) {
    let found = state.git().clone();
    let mut status = GitStatus {
        path: None,
        version: None,
        minimum: GIT_MINIMUM.to_string(),
        reason: None,
        detail: None,
        install: None,
    };
    let Some(found) = found else {
        return (Reported::Unknown, status);
    };

    match found.git {
        Ok(git) => {
            status.path = Some(git.path().display().to_string());
            status.version = Some(git.version().to_string());
            (Reported::Available, status)
        }
        Err(unavailable) => {
            status.path = unavailable.path().map(|path| path.display().to_string());
            status.version = unavailable.version().map(|version| version.to_string());
            status.reason = Some(unavailable.reason().to_owned());
            status.detail = unavailable.detail().map(ToOwned::to_owned);
            let family = package_family();
            status.install = Some(InstallHint {
                command: family.install_command().map(ToOwned::to_owned),
                note: family.install_note().map(ToOwned::to_owned),
                link: GIT_DOWNLOADS.to_owned(),
            });
            (Reported::Unavailable, status)
        }
    }
}

/// Find the `git` to run: `configured`, authoritatively, or the first that is there of the platform's
/// stable places — never by the `PATH` the daemon happened to inherit.
fn locate(configured: Option<&Path>) -> Result<Git, GitUnavailable> {
    #[cfg(windows)]
    let candidates = keepster_service::git_for_windows();
    #[cfg(not(windows))]
    let candidates = keepster_core::git_candidates(
        platform(),
        std::env::home_dir().as_deref(),
        xcode().as_deref(),
    );
    // macOS's `/usr/bin/git` is a stub that raises an installer dialog; it is only ever `stat`ed.
    let stub = (platform() == Platform::MacOs).then(|| Path::new("/usr/bin/git"));

    Git::locate(configured, &candidates, stub)
}

/// The platform this daemon runs on, as the search for `git` tells them apart.
fn platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}

/// Where Xcode is, as `xcode-select -p` says — an OS tool, not a Command Line Tools trampoline, so safe
/// to run where the tools are missing (ticket 11).
#[cfg(not(windows))]
fn xcode() -> Option<PathBuf> {
    if platform() != Platform::MacOs {
        return None;
    }
    let output = std::process::Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .env_clear()
        .output()
        .ok()?;
    let printed = String::from_utf8(output.stdout).ok()?;

    output
        .status
        .success()
        .then(|| PathBuf::from(printed.trim()))
        .filter(|path| path.is_absolute())
}

/// How `git` is installed here, which changes only what a person is told.
fn package_family() -> PackageFamily {
    match platform() {
        Platform::Windows => PackageFamily::Winget,
        Platform::MacOs => {
            let brew = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"];
            if brew.iter().any(|path| Path::new(path).is_file()) {
                PackageFamily::Homebrew
            } else {
                PackageFamily::XcodeSelect
            }
        }
        Platform::Linux => std::fs::read_to_string("/etc/os-release")
            .map(|text| PackageFamily::from_os_release(&text))
            .unwrap_or(PackageFamily::Unknown),
    }
}
