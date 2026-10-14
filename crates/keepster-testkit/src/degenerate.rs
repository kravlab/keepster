//! The degenerate catalogue (ticket 69): small trees, one explicit case each, that ordinary fixtures
//! never hold — a name at the length limit, a name that is not UTF-8, symlinks, empty directories, a
//! file past the size cap.
//!
//! Each case is data: what it is, which decision it exercises, the entries it makes, and whether the
//! platform can hold it at all. A case the platform cannot hold is compiled out with its reason, so that
//! "impossible here" is decided when the code is built and never confused with "skipped". A case the
//! platform can hold but this machine cannot make — a capability it lacks — says so when made. Each comes
//! to one [`Outcome`], and the platform table (ticket 70, [`crate::platform`], `docs/reports.md`) is
//! generated from this metadata and those outcomes. The performance tripwire never measures these: they
//! are about correctness, and they are small.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use keepster_core::DEFAULT_MAX_SIZE;
use serde::{Deserialize, Serialize};

/// One degenerate case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Case {
    /// Its name, as a report keys it.
    pub name: &'static str,
    /// What it is, in a line a report can print.
    pub summary: &'static str,
    /// The decision whose edge it exercises.
    pub source: &'static str,
    /// What it makes, every parent before its children.
    pub entries: &'static [Entry],
    /// Whether this platform can hold it.
    pub here: Here,
    /// The capability a machine of this platform may lack to make it, which makes it unavailable there
    /// rather than failed; `None` when every machine that can hold it can make it.
    pub capability: Option<&'static str>,
}

/// One path a case makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// Where, relative to the directory the case is made in: `/`-joined bytes, since a name need not be
    /// UTF-8 (ticket 48).
    pub path: &'static [u8],
    /// What is made there.
    pub kind: Kind,
}

/// What an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A regular file of `size` zero bytes, its length set rather than written: a hole on Unix, so a
    /// file past the cap costs no disk there. NTFS allocates it, since a hole needs `FSCTL_SET_SPARSE`
    /// and so a dependency the catalogue does without (ticket 69).
    File {
        /// Its size in bytes.
        size: u64,
    },
    /// A directory.
    Directory,
    /// A symbolic link to `target`, relative to the link's own directory.
    Symlink {
        /// What the link names, `/`-joined bytes; perhaps nothing, for a dangling link.
        target: &'static [u8],
        /// Whether it is a link to a directory, which Windows makes differently from a link to a file.
        directory: bool,
    },
}

/// Whether the platform the code was built for can hold a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Here {
    /// It can; whether this machine can is found by making it.
    Possible,
    /// It cannot, for the reason given: decided at compile time, never at run time.
    Impossible(&'static str),
}

/// Why a case was not made.
#[derive(Debug, thiserror::Error)]
pub enum Unmade {
    /// The platform cannot hold it (see [`Here::Impossible`]).
    #[error("impossible on this platform: {0}")]
    Impossible(&'static str),
    /// The platform can, but this machine lacks the capability: a filesystem that refuses names that are
    /// not UTF-8, or a Windows account that may not create symbolic links. A report, not a failure.
    #[error("unavailable on this machine: {0}")]
    Unavailable(String),
    /// Making it failed: a mistake in the case or the test, or a machine short of something no case
    /// names as a capability, such as space.
    #[error("could not be made")]
    Failed(#[source] io::Error),
}

/// What a case came to on one machine: one of the three declared outcomes of ticket 31, so that
/// "impossible here" and "never run here" stay apart.
///
/// A platform table's row carries it as `"outcome"`, with the reason as `"reason"` where there is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "reason", rename_all = "snake_case")]
pub enum Outcome {
    /// The case was made and its every entry checked.
    Ran,
    /// The platform cannot hold it, for the reason given, decided when the code was built.
    Impossible(String),
    /// The platform can, but this machine lacks the capability, for the reason given.
    Unavailable(String),
}

/// An outcome as a report prints it: `ran`, or the outcome and its reason.
impl std::fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ran => write!(formatter, "ran"),
            Self::Impossible(reason) => write!(formatter, "impossible: {reason}"),
            Self::Unavailable(reason) => write!(formatter, "unavailable: {reason}"),
        }
    }
}

impl Unmade {
    /// What a case that was not made comes to on this machine: the outcome its row of the platform table
    /// carries (ticket 70), or, when it failed, the error, which is the test's to report and never a row.
    ///
    /// # Errors
    ///
    /// The error that kept a case from being made, for [`Unmade::Failed`].
    pub fn outcome(self) -> Result<Outcome, io::Error> {
        match self {
            Self::Impossible(reason) => Ok(Outcome::Impossible(reason.to_owned())),
            Self::Unavailable(reason) => Ok(Outcome::Unavailable(reason)),
            Self::Failed(error) => Err(error),
        }
    }
}

/// A name of 255 ASCII bytes: at ext4's and btrfs's limit of 255 bytes, and APFS's and NTFS's of 255
/// characters and UTF-16 units, which an ASCII name meets at the same length.
const LONG_NAME: [u8; 255] = [b'a'; 255];

/// Whether this platform can hold a name that is not UTF-8: where names are bytes.
#[cfg(unix)]
const NON_UTF8: Here = Here::Possible;
/// Whether this platform can hold a name that is not UTF-8: not where names are UTF-16.
#[cfg(not(unix))]
const NON_UTF8: Here = Here::Impossible(
    "names here are UTF-16 rather than bytes, so bytes that are not UTF-8 name no file",
);

/// `EILSEQ`, with which a filesystem holding only UTF-8 names refuses another; std gives it no
/// `ErrorKind` of its own. `None` where its number is not known here, so a refusal there fails rather
/// than passing as a capability.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const EILSEQ: Option<i32> = Some(84);
/// See the Linux `EILSEQ`.
#[cfg(target_os = "macos")]
const EILSEQ: Option<i32> = Some(92);
/// See the Linux `EILSEQ`.
#[cfg(not(any(
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
const EILSEQ: Option<i32> = None;

/// Every case, in the order ticket 69 lists them.
pub const CATALOGUE: &[Case] = &[
    Case {
        name: "long_name",
        summary: "a file whose name is 255 bytes long, the most a name may be",
        source: "ticket 69: a near-limit path",
        entries: &[Entry {
            path: &LONG_NAME,
            kind: Kind::File { size: 1 },
        }],
        here: Here::Possible,
        capability: None,
    },
    Case {
        name: "non_utf8_name",
        summary: "a file whose name is bytes that are not UTF-8",
        source: "ticket 48: paths are bytes",
        entries: &[Entry {
            path: b"caf\xe9.txt",
            kind: Kind::File { size: 1 },
        }],
        here: NON_UTF8,
        capability: Some("a filesystem that holds names that are not UTF-8"),
    },
    Case {
        name: "symlinks",
        summary: "a link to a file, a link to a directory, and a link to nothing",
        source: "ticket 49: symlinks are stored as links",
        entries: &[
            Entry {
                path: b"target.txt",
                kind: Kind::File { size: 1 },
            },
            Entry {
                path: b"folder",
                kind: Kind::Directory,
            },
            Entry {
                path: b"folder/inside.txt",
                kind: Kind::File { size: 1 },
            },
            Entry {
                path: b"to_file",
                kind: Kind::Symlink {
                    target: b"target.txt",
                    directory: false,
                },
            },
            Entry {
                path: b"to_folder",
                kind: Kind::Symlink {
                    target: b"folder",
                    directory: true,
                },
            },
            Entry {
                path: b"dangling",
                kind: Kind::Symlink {
                    target: b"missing.txt",
                    directory: false,
                },
            },
        ],
        here: Here::Possible,
        capability: Some(
            "creating symbolic links, which Windows allows only in Developer Mode or elevated",
        ),
    },
    Case {
        name: "empty_directories",
        summary: "an empty directory, and a directory holding only an empty one",
        source: "ticket 25: empty directories",
        entries: &[
            Entry {
                path: b"empty",
                kind: Kind::Directory,
            },
            Entry {
                path: b"hollow",
                kind: Kind::Directory,
            },
            Entry {
                path: b"hollow/empty",
                kind: Kind::Directory,
            },
        ],
        here: Here::Possible,
        capability: None,
    },
    Case {
        name: "past_the_cap",
        summary: "a file one byte past the default 100 MiB size cap",
        source: "ticket 10: the default size cap",
        entries: &[Entry {
            path: b"large.bin",
            kind: Kind::File {
                size: DEFAULT_MAX_SIZE + 1,
            },
        }],
        here: Here::Possible,
        capability: None,
    },
];

/// Make `case` in `directory`, which must not hold any of its entries yet: what is already there is not
/// part of the case.
///
/// # Errors
///
/// [`Unmade::Impossible`] for a case compiled out here; [`Unmade::Unavailable`] when this machine
/// refuses a name that is not UTF-8 or a symbolic link; [`Unmade::Failed`] for anything else, an entry
/// already there included.
pub fn make(case: &Case, directory: &Path) -> Result<(), Unmade> {
    if let Here::Impossible(reason) = case.here {
        return Err(Unmade::Impossible(reason));
    }
    for entry in case.entries {
        let path =
            directory.join(native(entry.path).ok_or_else(|| {
                Unmade::Failed(io::Error::other("a name this platform cannot hold"))
            })?);
        match entry.kind {
            Kind::File { size } => {
                let file = File::create_new(&path).map_err(|error| {
                    if std::str::from_utf8(entry.path).is_err()
                        && EILSEQ.is_some()
                        && error.raw_os_error() == EILSEQ
                    {
                        Unmade::Unavailable(format!(
                            "the filesystem refuses a name that is not UTF-8: {error}"
                        ))
                    } else {
                        Unmade::Failed(error)
                    }
                })?;
                file.set_len(size).map_err(Unmade::Failed)?;
            }
            Kind::Directory => fs::create_dir(&path).map_err(Unmade::Failed)?,
            Kind::Symlink { target, directory } => {
                let target = native(target).ok_or_else(|| {
                    Unmade::Failed(io::Error::other("a target this platform cannot name"))
                })?;
                symlink(&target, &path, directory)?;
            }
        }
    }
    Ok(())
}

/// `path`, `/`-joined bytes as the catalogue declares it, as a native relative path; `None` where
/// the platform's names are not bytes and these are not UTF-8.
pub fn native(path: &[u8]) -> Option<PathBuf> {
    path.split(|byte| *byte == b'/').map(component).collect()
}

/// One component's bytes as a native name.
#[cfg(unix)]
fn component(bytes: &[u8]) -> Option<std::ffi::OsString> {
    use std::os::unix::ffi::OsStrExt;

    Some(std::ffi::OsStr::from_bytes(bytes).to_os_string())
}

/// One component's bytes as a native name: only UTF-8 names one, since names here are UTF-16.
#[cfg(not(unix))]
fn component(bytes: &[u8]) -> Option<std::ffi::OsString> {
    std::str::from_utf8(bytes)
        .ok()
        .map(std::ffi::OsString::from)
}

/// A symbolic link at `link` to `target`; Unix makes a link to a directory as any other.
#[cfg(unix)]
fn symlink(target: &Path, link: &Path, _directory: bool) -> Result<(), Unmade> {
    std::os::unix::fs::symlink(target, link).map_err(Unmade::Failed)
}

/// A symbolic link at `link` to `target`, a link to a directory when `directory`, which Windows makes
/// differently. Windows lets only an elevated process or one in Developer Mode create one, and refuses
/// others with `ERROR_PRIVILEGE_NOT_HELD`: that refusal is this machine's capability, not a failure.
#[cfg(windows)]
fn symlink(target: &Path, link: &Path, directory: bool) -> Result<(), Unmade> {
    /// `ERROR_PRIVILEGE_NOT_HELD`: "A required privilege is not held by the client."
    const PRIVILEGE_NOT_HELD: i32 = 1314;

    let made = if directory {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    made.map_err(|error| {
        if error.raw_os_error() == Some(PRIVILEGE_NOT_HELD) {
            Unmade::Unavailable(format!(
                "this account may not create symbolic links (Developer Mode or elevation): {error}"
            ))
        } else {
            Unmade::Failed(error)
        }
    })
}
