//! The machine a report was made on (ticket 70), on every platform the suite runs on.
//!
//! A report gates nothing, and it cannot lie only because every outcome it records carries where it was
//! observed (ticket 32): "ran on an ubuntu24 runner on ext4" does not pretend to describe a user's
//! laptop. [`environment`](crate::environment) describes the tripwire's one reference machine in detail
//! and only on Linux; this names any machine in a line, from what each platform tells: its version
//! command or file, the filesystem a directory is on as `keepster-service` reads it, and the image
//! variables a hosted CI runner sets. The parsers are pure, so they are tested on every platform; only [`detect`] asks the
//! machine.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// What [`detect`] found. `None` wherever the machine would not say, which a report prints as such
/// rather than guessing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// The operating system, as `std::env::consts::OS` names it: `linux`, `macos`, `windows`.
    pub os: String,
    /// The architecture, as `std::env::consts::ARCH` names it: `x86_64`, `aarch64`.
    pub arch: String,
    /// The operating system's version: the kernel release on Linux, the product version on macOS, and
    /// on Windows the version `ver` prints, its build number included: `10.0.20348.2700`.
    pub version: Option<String>,
    /// The type of the filesystem the report's directory is on: `ext4`, `apfs`, `NTFS`.
    pub filesystem: Option<String>,
    /// The hosted CI runner's image and its version, when the machine is one.
    pub runner: Option<String>,
}

/// The machine in a line, as a report prints it beside what was observed there:
/// `linux x86_64, version 6.8.0, filesystem ext4, runner ubuntu24 20251006.1`.
impl std::fmt::Display for Host {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let unknown =
            |value: &Option<String>| value.clone().unwrap_or_else(|| "unknown".to_owned());
        write!(
            formatter,
            "{} {}, version {}, filesystem {}, ",
            self.os,
            self.arch,
            unknown(&self.version),
            unknown(&self.filesystem)
        )?;
        match &self.runner {
            Some(runner) => write!(formatter, "runner {runner}"),
            None => write!(formatter, "not a hosted runner"),
        }
    }
}

/// The machine this runs on, with the filesystem `directory` is on.
pub fn detect(directory: &Path) -> Host {
    Host {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        version: version(),
        filesystem: keepster_service::volume_of(directory)
            .ok()
            .map(|volume| volume.filesystem)
            .filter(|filesystem| !filesystem.is_empty()),
        runner: runner(
            std::env::var("ImageOS").ok().as_deref(),
            std::env::var("ImageVersion").ok().as_deref(),
        ),
    }
}

/// The text a version file or command holds when it holds nothing but the version, without its line
/// break: Linux's `/proc/sys/kernel/osrelease`, macOS's `sw_vers -productVersion`.
pub fn version_line(text: &str) -> Option<String> {
    let line = text.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

/// The version `ver` prints on Windows, `Microsoft Windows [Version 10.0.20348.2700]`: the word inside
/// the brackets that starts with a digit, since the word beside it is translated with the system and a
/// translation may put it on either side.
pub fn windows_version(ver: &str) -> Option<String> {
    let (_, opened) = ver.split_once('[')?;
    let (inside, _) = opened.split_once(']')?;

    inside
        .split_whitespace()
        .find(|word| word.starts_with(|first: char| first.is_ascii_digit()))
        .map(str::to_owned)
}

/// A hosted runner's name from the image variables GitHub's runners set, `ImageOS` and `ImageVersion`;
/// `None` without an image, since a version alone names nothing.
pub fn runner(image: Option<&str>, version: Option<&str>) -> Option<String> {
    let image = image.map(str::trim).filter(|image| !image.is_empty())?;

    Some(
        match version.map(str::trim).filter(|version| !version.is_empty()) {
            Some(version) => format!("{image} {version}"),
            None => image.to_owned(),
        },
    )
}

/// This machine's operating system version, as the platform tells it.
#[cfg(target_os = "linux")]
fn version() -> Option<String> {
    std::fs::read_to_string(crate::environment::OSRELEASE)
        .ok()
        .as_deref()
        .and_then(version_line)
}

/// This machine's operating system version, as the platform tells it.
#[cfg(target_os = "macos")]
fn version() -> Option<String> {
    printed("sw_vers", &["-productVersion"])
        .as_deref()
        .and_then(version_line)
}

/// This machine's operating system version, as the platform tells it.
#[cfg(windows)]
fn version() -> Option<String> {
    printed("cmd", &["/C", "ver"])
        .as_deref()
        .and_then(windows_version)
}

/// This machine's operating system version: unknown on a platform the suite does not run on.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn version() -> Option<String> {
    None
}

/// What `program` prints, when it runs and succeeds.
#[cfg(any(target_os = "macos", windows))]
fn printed(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;

    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}
