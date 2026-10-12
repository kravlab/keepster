//! The Registry on disk: one small file per Watched Root.
//!
//! This is the only state that is **not** derivable from git objects — which directory a history
//! belongs to — so it lives in plain files the daemon owns and replaces atomically, rather than in the
//! derived index, which may be deleted and rebuilt at any time (ticket 03).

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use keepster_core::{RootId, VolumePlace};
use serde::{Deserialize, Serialize};

/// The file that records one Root, beside that Root's History Store.
const RECORD_FILE: &str = "root.toml";

/// The record of a Root no longer watched, whose history decays down the ladder to its pinned last
/// state (ticket 53). A file of its own rather than a flag in the record, so that a build from before
/// it sees what it always saw there: a directory without a record, an orphaned store it leaves alone.
const UNWATCHED_FILE: &str = "unwatched.toml";

/// The container format this build writes and understands: the shape of a Root's directory and the
/// `refs/keepster/…` namespace.
///
/// One of the store's four version numbers (ADR 0010); the container one is the only one rewritten
/// automatically, because it is small, atomic and not history.
pub const CONTAINER_VERSION: u32 = 1;

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
    /// Where it sits on its volume, recorded at registration and compared before every capture
    /// (ADR 0011) — `None` where the volume could not be identified.
    pub volume: Option<VolumePlace>,
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
    /// The record becomes the record of a Root no longer watched: the directory and its History Store
    /// stay, which is what makes unwatching reversible (ticket 07), and the path stays with them, so
    /// that watching that directory again finds this history rather than starting another (ticket 53).
    pub fn remove(&self, id: RootId) -> Result<(), RegistryError> {
        let dir = self.root_dir(id);

        rename(&dir.join(RECORD_FILE), &dir.join(UNWATCHED_FILE))
    }

    /// Every Root no longer watched whose record this build can read, ordered by identity.
    pub fn unwatched(&self) -> Result<Vec<RootRecord>, RegistryError> {
        let mut found = Vec::new();
        for id in self.identities()? {
            let dir = self.root_dir(id);
            let unwatched = dir.join(UNWATCHED_FILE);
            if unwatched.is_file() && !dir.join(RECORD_FILE).is_file() {
                found.extend(read_record(&unwatched).ok());
            }
        }

        Ok(found)
    }

    /// Whether the Root is watched or, no longer watched, still has its history here.
    pub fn holds(&self, id: RootId) -> bool {
        let dir = self.root_dir(id);

        dir.join(RECORD_FILE).is_file() || dir.join(UNWATCHED_FILE).is_file()
    }

    /// Watch again a Root no longer watched, at the path it was unwatched at.
    pub fn rewatch(&self, id: RootId) -> Result<(), RegistryError> {
        let dir = self.root_dir(id);

        rename(&dir.join(UNWATCHED_FILE), &dir.join(RECORD_FILE))
    }

    /// Rewrite every record of an older container version in this build's, watched or not, saying how
    /// many were rewritten.
    ///
    /// The container is the one version rewritten automatically and silently (ADR 0010): a record is
    /// small, daemon-owned, replaced atomically, and not history. It is done once, before anything else
    /// can touch the Registry — a rewrite racing an unwatch could bring back the record the unwatch had
    /// just renamed away. A record this build cannot read is left for [`Registry::list`] to report.
    pub fn upgrade(&self) -> Result<u64, RegistryError> {
        let mut rewritten = 0;
        for id in self.identities()? {
            let dir = self.root_dir(id);
            for file in [RECORD_FILE, UNWATCHED_FILE] {
                let path = dir.join(file);
                if !version_of(&path).is_some_and(|version| version < CONTAINER_VERSION) {
                    continue;
                }
                if let Ok(record) = read_record(&path) {
                    write_record(&path, &record)?;
                    rewritten += 1;
                }
            }
        }

        Ok(rewritten)
    }

    /// Every identity a directory under the base is named by.
    fn identities(&self) -> Result<Vec<RootId>, RegistryError> {
        let listed = |source| RegistryError::Io {
            path: self.base.clone(),
            operation: "listed",
            source,
        };
        let mut ids = Vec::new();
        for directory in fs::read_dir(&self.base).map_err(listed)? {
            let directory = directory.map_err(listed)?;
            if let Some(id) = directory
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<RootId>().ok())
            {
                ids.push(id);
            }
        }
        ids.sort();

        Ok(ids)
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
    /// The volume's identity. Optional and additive, so a build from before it reads the record as it
    /// always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume: Option<String>,
    /// The Root's place within that volume, `/`-separated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    within_volume: Option<String>,
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
        volume: file.volume.map(|identity| VolumePlace {
            identity,
            within: file.within_volume.unwrap_or_default(),
        }),
    })
}

/// The container version a record says it was written in, when the file says at all.
fn version_of(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;

    toml::from_str::<RecordFile>(&text)
        .ok()
        .map(|file| file.version)
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
        volume: record.volume.as_ref().map(|place| place.identity.clone()),
        within_volume: record.volume.as_ref().map(|place| place.within.clone()),
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
        // though the file's contents reached the disk. That is a POSIX notion: Windows cannot open a
        // directory as a file at all ("Access is denied"), and NTFS journals the rename itself, so
        // there is no directory to flush there.
        #[cfg(unix)]
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

/// Move a record from one name to the other atomically, flushing the directory as [`write_record`]
/// does.
fn rename(from: &Path, to: &Path) -> Result<(), RegistryError> {
    let moved = || -> io::Result<()> {
        fs::rename(from, to)?;
        #[cfg(unix)]
        if let Some(parent) = to.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    };

    moved().map_err(|source| RegistryError::Io {
        path: from.to_path_buf(),
        operation: "moved",
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
