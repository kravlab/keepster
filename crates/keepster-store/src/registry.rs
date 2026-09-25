//! The Registry on disk: one small file per Watched Root.
//!
//! This is the only state that is **not** derivable from git objects — which directory a history
//! belongs to — so it lives in plain files the daemon owns and replaces atomically, rather than in the
//! derived index, which may be deleted and rebuilt at any time (ticket 03).

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use keepster_core::RootId;
use serde::{Deserialize, Serialize};

/// The file that records one Root, beside that Root's History Store.
const RECORD_FILE: &str = "root.toml";

/// The container format this build writes and understands.
///
/// Ticket 26 governs the four version numbers of the store; this is the container one, the only one
/// that is rewritten automatically, because it is small, atomic and not history.
const CONTAINER_VERSION: u32 = 1;

/// One directory under the Registry's base: a Root's identity, and its record or why that record
/// cannot be read.
///
/// The two halves are separate because the identity survives what the record does not: a directory
/// named by a ULID is a Root even when the file inside it was written by a version this build cannot
/// open.
#[derive(Debug)]
pub struct RegistryEntry {
    /// The Root's identity, taken from the directory's name.
    pub id: RootId,
    /// The record, or the reason it is unreadable.
    pub record: Result<RootRecord, RegistryError>,
}

/// What the Registry knows about one Root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootRecord {
    /// The Root's identity, which outlives any path it has.
    pub id: RootId,
    /// Where the Root's directory is right now.
    pub path: PathBuf,
}

/// Why a Registry operation failed.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// The Registry could not be read or written.
    #[error("the registry at {path} could not be {operation}")]
    Io {
        /// The file or directory involved.
        path: PathBuf,
        /// What was being attempted, for a message a person can act on.
        operation: &'static str,
        /// The underlying failure.
        #[source]
        source: io::Error,
    },
    /// A record on disk is not something this build can read.
    #[error("the record at {path} is unreadable: {reason}")]
    Unreadable {
        /// The record involved.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// A record was written by a newer version of Keepster.
    ///
    /// Refused rather than guessed at: ticket 26 makes a newer container version mean the Root cannot
    /// be served at all by this build.
    #[error("the record at {path} was written by a newer version (container {found})")]
    TooNew {
        /// The record involved.
        path: PathBuf,
        /// The version found in it.
        found: u32,
    },
}

/// Every Root's own directory, under one base the daemon owns.
#[derive(Debug, Clone)]
pub struct Registry {
    base: PathBuf,
}

impl Registry {
    /// Open — and if necessary create — the Registry under `base`.
    ///
    /// The base holds one directory per Root, named by its identity, so enumerating Roots is a
    /// directory listing and no index has to be kept consistent with it.
    pub fn open(base: impl Into<PathBuf>) -> Result<Self, RegistryError> {
        let base = base.into();
        create_private_dir(&base)?;

        Ok(Self { base })
    }

    /// The directory that holds one Root's record and, from ticket 38, its History Store.
    pub fn root_dir(&self, id: RootId) -> PathBuf {
        self.base.join(id.to_string())
    }

    /// Every Root this Registry holds, ordered by identity — which, ULIDs being time-ordered, is
    /// registration order.
    ///
    /// A record this build cannot read is returned **as a failure of its own**, not as a failure of the
    /// listing: ticket 08 makes an unopenable Root a value rather than an error, because a report that
    /// collapses wholesale has nothing to say exactly when it matters most. The identity is always
    /// known, since it is the directory's name.
    pub fn list(&self) -> Result<Vec<RegistryEntry>, RegistryError> {
        let mut entries = Vec::new();

        let directories = fs::read_dir(&self.base).map_err(|source| RegistryError::Io {
            path: self.base.clone(),
            operation: "listed",
            source,
        })?;

        for directory in directories {
            let directory = directory.map_err(|source| RegistryError::Io {
                path: self.base.clone(),
                operation: "listed",
                source,
            })?;

            // A directory whose name is not an identity was not written by us, so it is not a Root and
            // saying nothing about it is not an omission.
            let Some(id) = directory
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<RootId>().ok())
            else {
                continue;
            };

            let record = directory.path().join(RECORD_FILE);
            if record.is_file() {
                entries.push(RegistryEntry {
                    id,
                    record: read_record(&record),
                });
            }
        }

        entries.sort_by_key(|entry| entry.id);

        Ok(entries)
    }

    /// Every Root whose record this build can read.
    ///
    /// For callers that act on Roots rather than report on them: registering, resolving and moving all
    /// need the path, which an unreadable record does not give.
    pub fn readable(&self) -> Result<Vec<RootRecord>, RegistryError> {
        Ok(self
            .list()?
            .into_iter()
            .filter_map(|entry| entry.record.ok())
            .collect())
    }

    /// Write a Root's record, creating its directory.
    ///
    /// Also the way a move is recorded: the record is replaced atomically, and the identity — which is
    /// the directory's name — never changes, so the history does not move with the path.
    pub fn insert(&self, record: &RootRecord) -> Result<(), RegistryError> {
        let dir = self.root_dir(record.id);
        create_private_dir(&dir)?;
        write_record(&dir.join(RECORD_FILE), record)
    }

    /// Whether this Root is registered.
    pub fn contains(&self, id: RootId) -> bool {
        self.root_dir(id).join(RECORD_FILE).is_file()
    }

    /// Deregister a Root, keeping everything it holds.
    ///
    /// Only the record goes: the directory and its History Store stay, which is what makes unwatching
    /// reversible (ticket 07). A directory without a record is precisely an orphaned store, the thing
    /// ticket 53 teaches Retention to decay.
    pub fn remove(&self, id: RootId) -> Result<(), RegistryError> {
        let record = self.root_dir(id).join(RECORD_FILE);

        fs::remove_file(&record).map_err(|source| RegistryError::Io {
            path: record,
            operation: "removed",
            source,
        })
    }

    /// Erase a Root and everything it holds.
    ///
    /// The one operation that destroys history, and therefore only ever reached through an explicit
    /// request that says so.
    pub fn purge(&self, id: RootId) -> Result<(), RegistryError> {
        let dir = self.root_dir(id);

        fs::remove_dir_all(&dir).map_err(|source| RegistryError::Io {
            path: dir,
            operation: "erased",
            source,
        })
    }
}

/// The on-disk shape, kept apart from [`RootRecord`] so that the wire of the file and the domain type
/// can move independently — and so that the version travels with the file rather than in code.
#[derive(Debug, Serialize, Deserialize)]
struct RecordFile {
    version: u32,
    id: String,
    path: PathBuf,
}

fn read_record(path: &Path) -> Result<RootRecord, RegistryError> {
    let text = fs::read_to_string(path).map_err(|source| RegistryError::Io {
        path: path.to_path_buf(),
        operation: "read",
        source,
    })?;

    let file: RecordFile = toml::from_str(&text).map_err(|error| RegistryError::Unreadable {
        path: path.to_path_buf(),
        reason: error.message().to_owned(),
    })?;

    if file.version > CONTAINER_VERSION {
        return Err(RegistryError::TooNew {
            path: path.to_path_buf(),
            found: file.version,
        });
    }

    let id = file.id.parse().map_err(|_| RegistryError::Unreadable {
        path: path.to_path_buf(),
        reason: format!("`{}` is not a Root identity", file.id),
    })?;

    Ok(RootRecord {
        id,
        path: file.path,
    })
}

/// Replace a record atomically: write a temporary file beside it, flush it to disk, rename over the
/// old one, then flush the directory.
///
/// A reader therefore sees either the previous record or the new one, never a half-written file — the
/// daemon can be killed at any point, and this is the one piece of state that cannot be rebuilt.
fn write_record(path: &Path, record: &RootRecord) -> Result<(), RegistryError> {
    let file = RecordFile {
        version: CONTAINER_VERSION,
        id: record.id.to_string(),
        path: record.path.clone(),
    };
    let text = toml::to_string_pretty(&file).map_err(|error| RegistryError::Unreadable {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;

    let temporary = path.with_extension("toml.new");
    let write = || -> io::Result<()> {
        let mut handle = File::create(&temporary)?;
        handle.write_all(text.as_bytes())?;
        handle.sync_all()?;
        drop(handle);

        fs::rename(&temporary, path)?;

        // The rename itself needs flushing, or a crash can leave the directory entry unwritten even
        // though the file's contents reached the disk.
        if let Some(parent) = path.parent() {
            File::open(parent)?.sync_all()?;
        }

        Ok(())
    };

    write().map_err(|source| RegistryError::Io {
        path: path.to_path_buf(),
        operation: "written",
        source,
    })
}

/// Create a directory only its owner can enter.
///
/// The store holds every version of every watched file, so its permissions are part of the product's
/// at-rest posture: the map leaves encryption out and relies on the operating system plus restrictive
/// permissions instead.
fn create_private_dir(path: &Path) -> Result<(), RegistryError> {
    let create = || -> io::Result<()> {
        fs::create_dir_all(path)?;

        #[cfg(unix)]
        {
            use std::fs::Permissions;
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(path, Permissions::from_mode(0o700))?;
        }

        Ok(())
    };

    create().map_err(|source| RegistryError::Io {
        path: path.to_path_buf(),
        operation: "created",
        source,
    })
}
