//! The external `git` as pure decisions (ticket 52, ticket 11): which version is enough, where it is
//! looked for, and what a person is told to run when there is none.
//!
//! Capture never needs it — snapshots are written in-process — so everything here is about the Maintenance
//! Pass, and an answer of "none" costs housekeeping, never history.

use std::fmt;
use std::path::{Path, PathBuf};

/// The oldest `git` maintenance runs with: where `GIT_CONFIG_SYSTEM` and `GIT_CONFIG_GLOBAL`, which keep
/// every invocation hermetic, arrived (git 2.32.0's release notes). Nothing still supported is older.
pub const GIT_MINIMUM: GitVersion = GitVersion::new(2, 32, Some(0));

/// The page a person is pointed to for installing `git`, whatever their platform.
pub const GIT_DOWNLOADS: &str = "https://git-scm.com/downloads";

/// A `git` version as far as it can be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitVersion {
    major: u32,
    minor: u32,
    /// `None` when the third component is missing or not a number, which is unknown rather than wrong.
    patch: Option<u32>,
}

impl GitVersion {
    /// The version `major.minor.patch`.
    pub const fn new(major: u32, minor: u32, patch: Option<u32>) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// The version `git --version` printed, read leniently: after `git version `, the leading digits of
    /// the first three dot-separated components, and nothing else — a development build carries four or
    /// five components, Git for Windows appends `.windows.N`, and Apple a parenthesised tail whose own
    /// number holds a dot (ticket 11).
    pub fn parse(output: &str) -> Option<Self> {
        let token = output
            .trim()
            .strip_prefix("git version ")?
            .split_whitespace()
            .next()?;
        let mut parts = token.split('.');
        let leading = |part: Option<&str>| -> Option<u32> {
            let part = part?;
            let digits = part.len() - part.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            part[..digits].parse().ok()
        };

        Some(Self {
            major: leading(parts.next())?,
            minor: leading(parts.next())?,
            patch: parts
                .next()
                .filter(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
                .and_then(|part| part.parse().ok()),
        })
    }

    /// Whether this is at least `minimum`. An unknown third number counts as enough: it can only fall
    /// short in the minimum's own minor version, where every release since `.0` qualifies.
    pub fn meets(&self, minimum: GitVersion) -> bool {
        (self.major, self.minor, self.patch.unwrap_or(u32::MAX))
            >= (
                minimum.major,
                minimum.minor,
                minimum.patch.unwrap_or_default(),
            )
    }
}

impl fmt::Display for GitVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)?;
        if let Some(patch) = self.patch {
            write!(formatter, ".{patch}")?;
        }
        Ok(())
    }
}

/// Which family of platform `git` is looked for on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Linux, and every other Unix but macOS.
    Linux,
    /// macOS, where `/usr/bin/git` is a stub that raises an installer dialog.
    MacOs,
    /// Windows, where the registry says where Git for Windows is.
    Windows,
}

/// Where `git` is looked for when no `git.path` is configured, in order: stable absolute locations only,
/// never an inherited `PATH`, which a service does not have and a user-writable directory could hijack.
///
/// On NixOS and Guix the binary's own path changes with every upgrade, so their profile links are listed
/// instead. On macOS `/usr/bin/git` and `xcrun` are never among them, since running either without the
/// Command Line Tools raises a dialog a background daemon must never cause; `xcode` is what
/// `xcode-select -p` printed. Windows has none here: the registry holds Git for Windows' install path.
pub fn git_candidates(
    platform: Platform,
    home: Option<&Path>,
    xcode: Option<&Path>,
) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    match platform {
        Platform::Linux => {
            found.extend(
                ["/usr/bin", "/usr/local/bin", "/run/current-system/sw/bin"].map(PathBuf::from),
            );
            found.extend(home.map(|home| home.join(".nix-profile/bin")));
            found.push(PathBuf::from("/run/current-system/profile/bin"));
            found.extend(home.map(|home| home.join(".guix-profile/bin")));
            found.extend(
                [
                    "/home/linuxbrew/.linuxbrew/bin",
                    "/opt/local/bin",
                    "/snap/bin",
                ]
                .map(PathBuf::from),
            );
        }
        Platform::MacOs => {
            found.extend(
                [
                    "/opt/homebrew/bin",
                    "/usr/local/bin",
                    "/opt/local/bin",
                    "/Library/Developer/CommandLineTools/usr/bin",
                ]
                .map(PathBuf::from),
            );
            found.extend(xcode.map(|developer| developer.join("usr/bin")));
        }
        Platform::Windows => {}
    }

    found
        .into_iter()
        .map(|directory| directory.join("git"))
        .collect()
}

/// How `git` is installed where the daemon runs — which changes what a person is told, never what the
/// daemon does: it never runs a package manager itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFamily {
    /// Debian, Ubuntu and their kin.
    Apt,
    /// Fedora, RHEL and their kin.
    Dnf,
    /// Arch and its kin.
    Pacman,
    /// openSUSE and SUSE.
    Zypper,
    /// Alpine.
    Apk,
    /// macOS with Homebrew.
    Homebrew,
    /// macOS without Homebrew: the Command Line Tools.
    XcodeSelect,
    /// Windows.
    Winget,
    /// Anything else — Nix, Guix, an unrecognised distribution.
    Unknown,
}

impl PackageFamily {
    /// The family an `/etc/os-release` names, by its `ID` and then its `ID_LIKE`.
    pub fn from_os_release(text: &str) -> Self {
        let field = |key: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                .map(|value| value.trim().trim_matches('"').to_owned())
                .unwrap_or_default()
        };
        let names = format!("{} {}", field("ID"), field("ID_LIKE"));

        names
            .split_whitespace()
            .find_map(|name| match name {
                "debian" | "ubuntu" => Some(Self::Apt),
                "fedora" | "rhel" | "centos" | "rocky" | "almalinux" => Some(Self::Dnf),
                "arch" | "manjaro" => Some(Self::Pacman),
                "suse" | "opensuse" => Some(Self::Zypper),
                name if name.starts_with("opensuse") => Some(Self::Zypper),
                "alpine" => Some(Self::Apk),
                _ => None,
            })
            .unwrap_or(Self::Unknown)
    }

    /// The command that installs `git` here, for a person to run.
    pub fn install_command(self) -> Option<&'static str> {
        match self {
            Self::Apt => Some("sudo apt install git"),
            Self::Dnf => Some("sudo dnf install git"),
            Self::Pacman => Some("sudo pacman -S git"),
            Self::Zypper => Some("sudo zypper install git"),
            Self::Apk => Some("sudo apk add git"),
            Self::Homebrew => Some("brew install git"),
            Self::XcodeSelect => Some("xcode-select --install"),
            Self::Winget => Some("winget install --id Git.Git -e"),
            Self::Unknown => None,
        }
    }

    /// What a person should know besides the command.
    pub fn install_note(self) -> Option<&'static str> {
        match self {
            Self::Winget => {
                Some("winget prompts for elevation in practice, even for a per-user install")
            }
            Self::Unknown => Some(
                "install git as your system does, or set git.path in the configuration to a git of your own",
            ),
            _ => None,
        }
    }
}
