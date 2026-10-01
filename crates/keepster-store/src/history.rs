//! The History Store: a bare git repository per Watched Root, and the only place a git concept is named.
//!
//! Snapshots are stored as segmented chains — one ref per retention period, each Segment starting from a
//! parentless commit — so dropping a whole period is a ref deletion and thinning one rewrites only that
//! period (ADR 0001). Two configuration facts are part of that design rather than details: file modes
//! must not be probed from the filesystem, and reflogs must stay off or a deleted ref stays reachable
//! and Retention frees nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use keepster_core::{RootId, SegmentName, SnapshotId};

/// What the trailer carrying a Snapshot's identity is called.
///
/// The identity lives in the message rather than being the commit hash, because thinning a Segment
/// recreates its commits and anything that referred to a Snapshot by hash would break (ADR 0001).
const SNAPSHOT_TRAILER: &str = "Keepster-Snapshot";

/// What the trailer carrying the Root's identity is called, so a store found on its own says which
/// Root it belongs to.
const ROOT_TRAILER: &str = "Keepster-Root";

/// Where a Segment's ref lives. One ref per period, and every name a leaf, because git's files backend
/// cannot hold both `refs/x` and `refs/x/y` (ADR 0001).
const SEGMENT_REFS: &str = "refs/keepster/segments";

/// Who the store records as the author of a Snapshot.
///
/// A Snapshot is taken by the daemon, not written by a person, so a fixed identity is more honest than
/// borrowing the user's git identity — which would also make the commit hash depend on their config.
const AUTHOR_NAME: &str = "Keepster";
const AUTHOR_EMAIL: &str = "keepster@localhost";

/// Why a History Store operation failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The repository itself could not be opened, created or written.
    #[error("the history store at {path} could not be {operation}")]
    Git {
        /// The store involved.
        path: PathBuf,
        /// What was being attempted, phrased for a message a person can act on.
        operation: &'static str,
        /// The underlying failure, carried opaquely: no caller outside this crate matches on a git
        /// error, which is what keeps the storage engine replaceable in one place (ticket 08).
        #[source]
        source: git2::Error,
    },
    /// The Working Tree could not be read.
    #[error("{path} could not be read")]
    WorkingTree {
        /// The path that could not be read.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },
    /// The moment given for the Snapshot cannot be named or recorded.
    ///
    /// A clock far outside the representable range would otherwise put Snapshots in the wrong Segment
    /// for as long as it stayed wrong, which is worse than refusing.
    #[error("the moment of the Snapshot is outside the range that can be recorded")]
    UnnameableMoment(#[source] keepster_core::UnnameableMoment),
}

/// What a capture produced.
///
/// Domain values only: a git object id appears in no public signature of this crate, which is what
/// makes the storage engine replaceable in one place rather than absorbed by an interface (ticket 08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedSnapshot {
    /// The Snapshot's stable identity.
    pub id: SnapshotId,
    /// The Segment it was written into.
    pub segment: SegmentName,
    /// Paths that were meant to be captured and could not be read.
    ///
    /// The Snapshot is taken anyway — one unreadable artefact must not freeze a Root's history
    /// (ticket 05) — and it says what it omitted, because a Snapshot that omits silently is the one
    /// thing this product forbids. Each path keeps its previous content, which ADR 0004 requires of a
    /// carried-forward entry, and ticket 48 is where the omission becomes part of the Snapshot itself
    /// rather than only of this answer.
    pub unreadable: Vec<PathBuf>,
}

/// One Watched Root's history, open for the length of one operation.
///
/// It is deliberately not held between operations: `git2`'s repository and index are neither `Send` nor
/// `Sync`, and ADR 0004 already decided that no index survives between Snapshots, so opening per
/// operation is both the cheap option and the one that cannot drift from what is on disk.
pub struct HistoryStore {
    repository: git2::Repository,
    path: PathBuf,
}

impl HistoryStore {
    /// Open the store at `path`, creating it with its invariants if it is not there yet.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();

        let repository = match git2::Repository::open_bare(&path) {
            Ok(repository) => {
                // ADR 0001 calls both settings invariants to assert, not steps to perform once: if
                // anything ever turned reflogs on, Retention would free nothing and the ladder would
                // become decoration, silently. Re-applying them is idempotent and costs one write.
                Self::apply_invariants(&repository, &path)?;
                repository
            }
            Err(error) if error.code() == git2::ErrorCode::NotFound => Self::create(&path)?,
            Err(source) => {
                return Err(StoreError::Git {
                    path,
                    operation: "opened",
                    source,
                });
            }
        };

        Ok(Self { repository, path })
    }

    /// Where this store lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Capture the Root's current contents as a Snapshot.
    ///
    /// The baseline for the tree is read back out of the previous Snapshot's tree at every capture, and
    /// no index survives between Snapshots (ADR 0004): a surviving map would be state that disagrees
    /// with reality silently, which is the loss this product exists to prevent. `now` is a parameter, so
    /// a month of history can be produced by this very path in a test (ADR 0012).
    ///
    /// The walk lives here, beside the object writes, because streaming a file into the object database
    /// is a git operation on a filesystem path — reading every file into memory a layer above would
    /// bound a Snapshot by available memory.
    ///
    /// This build captures every regular file it can reach. Include Rules arrive with ticket 41, the
    /// records that name what a Snapshot deliberately does not hold with ticket 48, and symlinks and
    /// special files with ticket 49; until then a path that is not a regular file is passed over.
    pub fn capture(
        &self,
        root_id: RootId,
        root_path: &Path,
        now: SystemTime,
    ) -> Result<CapturedSnapshot, StoreError> {
        let segment = SegmentName::hourly(now).map_err(StoreError::UnnameableMoment)?;
        let reference = format!("{SEGMENT_REFS}/{segment}");

        // Two different questions, and conflating them is what ADR 0004 warns about. The *parent* is
        // the tip of this Segment, so a new Segment starts parentless (ADR 0001); the *baseline* is the
        // newest Snapshot anywhere at or before it, because the previous Snapshot's tree must be
        // readable at every capture, including the first of a new Segment.
        let parent = self.segment_tip(&reference)?;
        let baseline = match &parent {
            Some(parent) => Some(parent.clone()),
            None => self.newest_snapshot_before(&segment)?,
        };

        let mut index = git2::Index::new().map_err(self.failed("prepared"))?;
        let baseline_tree = match &baseline {
            Some(commit) => Some(commit.tree().map_err(self.failed("read"))?),
            None => None,
        };
        if let Some(tree) = &baseline_tree {
            // Seeding from this store's own objects matters: cached subtree ids are copied without an
            // existence check, so a seed from elsewhere could yield a tree referencing objects this
            // store lacks (ADR 0004). Reading the tree also fills the TREE cache, which is what lets
            // `write_tree` rewrite only the changed paths' ancestry.
            index.read_tree(tree).map_err(self.failed("read"))?;
        }

        let walked = self.add_working_tree(&mut index, baseline_tree.as_ref(), root_path)?;
        remove_vanished(&mut index, &walked)?;

        let tree_id = index
            .write_tree_to(&self.repository)
            .map_err(self.failed("written"))?;
        let tree = self
            .repository
            .find_tree(tree_id)
            .map_err(self.failed("read"))?;

        let id = SnapshotId::generate(now);
        let signature = self.signature(now)?;
        let message = format!("Snapshot\n\n{SNAPSHOT_TRAILER}: {id}\n{ROOT_TRAILER}: {root_id}\n");
        let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();

        self.repository
            .commit(
                Some(&reference),
                &signature,
                &signature,
                &message,
                &tree,
                &parents,
            )
            .map_err(self.failed("written"))?;

        Ok(CapturedSnapshot {
            id,
            segment,
            unreadable: walked.unreadable,
        })
    }

    /// Walk the Root, writing blobs and touching only the index entries that changed.
    ///
    /// An entry whose content and mode already match the baseline is left alone deliberately: inserting
    /// into the index invalidates the ancestry of that path in the TREE cache, so touching every path
    /// would throw the cache away and make `write_tree` rebuild the whole tree — the cost ADR 0004
    /// exists to avoid.
    fn add_working_tree(
        &self,
        index: &mut git2::Index,
        baseline: Option<&git2::Tree<'_>>,
        root_path: &Path,
    ) -> Result<Walked, StoreError> {
        let mut walked = Walked::default();
        collect_files(root_path, &WirePath(String::new()), &mut walked)?;

        // Entries are applied in the index's own order, which is the byte order of the `/`-joined path —
        // not the component order `Path` compares by, where `sub/b.txt` would precede `sub.txt` while
        // the index expects the opposite. Ticket 12 measured 0.86 s in order against 22.74 s out of it.
        walked.paths.sort();

        for path in &walked.paths {
            let absolute = root_path.join(path.as_filesystem_path());

            let metadata = match fs::symlink_metadata(&absolute) {
                Ok(metadata) => metadata,
                // Gone between the walk and the read, or unreadable: carry the baseline entry forward
                // and say so, rather than losing the whole Snapshot (ticket 05).
                Err(_) => {
                    walked.unreadable.push(absolute);
                    continue;
                }
            };

            let oid = match self.repository.blob_path(&absolute) {
                Ok(oid) => oid,
                Err(_) => {
                    walked.unreadable.push(absolute);
                    continue;
                }
            };
            let mode = mode_for(&metadata);

            if unchanged(baseline, path, oid, mode) {
                continue;
            }

            index
                .add(&entry_for(path, oid, mode))
                .map_err(self.failed("written"))?;
        }

        Ok(walked)
    }

    /// The newest Snapshot in any Segment at or before `segment`.
    ///
    /// Segment names sort chronologically as text (ADR 0001), so "the Snapshot before this one" is the
    /// tip of the greatest ref name that does not exceed the current Segment — which is how the baseline
    /// survives an hour boundary.
    fn newest_snapshot_before(
        &self,
        segment: &SegmentName,
    ) -> Result<Option<git2::Commit<'_>>, StoreError> {
        let references = self
            .repository
            .references_glob(&format!("{SEGMENT_REFS}/*"))
            .map_err(self.failed("read"))?;

        let mut newest: Option<String> = None;
        for reference in references {
            let reference = reference.map_err(self.failed("read"))?;
            // A ref name that is not UTF-8 was not written by us, since every Segment name is a
            // calendar hour, so it names no Segment and is passed over.
            let Ok(name) = reference.name().map(ToOwned::to_owned) else {
                continue;
            };
            let Some(period) = name.rsplit('/').next() else {
                continue;
            };

            if period <= segment.as_str()
                && newest.as_deref().is_none_or(|best| name.as_str() > best)
            {
                newest = Some(name);
            }
        }

        match newest {
            Some(name) => self.segment_tip(&name),
            None => Ok(None),
        }
    }

    /// The newest Snapshot in a Segment, or nothing if the Segment does not exist yet.
    ///
    /// A Segment is created lazily by the first Snapshot falling in its period, so the ref count tracks
    /// active periods rather than elapsed ones (ticket 06).
    fn segment_tip(&self, reference: &str) -> Result<Option<git2::Commit<'_>>, StoreError> {
        match self.repository.find_reference(reference) {
            Ok(found) => Ok(Some(found.peel_to_commit().map_err(self.failed("read"))?)),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(source) => Err(StoreError::Git {
                path: self.path.clone(),
                operation: "read",
                source,
            }),
        }
    }

    /// The identity and moment a Snapshot's commit records.
    fn signature(&self, now: SystemTime) -> Result<git2::Signature<'static>, StoreError> {
        let seconds = now
            .duration_since(UNIX_EPOCH)
            .map_err(|_| StoreError::UnnameableMoment(keepster_core::UnnameableMoment))?
            .as_secs();
        let seconds = i64::try_from(seconds)
            .map_err(|_| StoreError::UnnameableMoment(keepster_core::UnnameableMoment))?;

        // Offset zero: a Snapshot is recorded in UTC, so the same moment reads the same way wherever
        // the history is later opened.
        git2::Signature::new(AUTHOR_NAME, AUTHOR_EMAIL, &git2::Time::new(seconds, 0))
            .map_err(self.failed("written"))
    }

    fn failed(&self, operation: &'static str) -> impl Fn(git2::Error) -> StoreError + '_ {
        move |source| StoreError::Git {
            path: self.path.clone(),
            operation,
            source,
        }
    }

    fn create(path: &Path) -> Result<git2::Repository, StoreError> {
        let mut options = git2::RepositoryInitOptions::new();
        options.bare(true).no_reinit(true);

        let repository =
            git2::Repository::init_opts(path, &options).map_err(|source| StoreError::Git {
                path: path.to_path_buf(),
                operation: "created",
                source,
            })?;
        Self::apply_invariants(&repository, path)?;

        Ok(repository)
    }

    /// Put the store's two configuration invariants in force.
    ///
    /// Reflogs off, because a deleted ref would otherwise stay reachable and Retention would free
    /// nothing; file modes unprobed, because git sets that true itself on Linux and macOS and a
    /// Snapshot's tree entries must carry a mode we set rather than a raw `st_mode` (ADR 0001).
    fn apply_invariants(repository: &git2::Repository, path: &Path) -> Result<(), StoreError> {
        let failed = |operation: &'static str| {
            move |source| StoreError::Git {
                path: path.to_path_buf(),
                operation,
                source,
            }
        };

        let mut config = repository.config().map_err(failed("configured"))?;
        config
            .set_bool("core.logAllRefUpdates", false)
            .map_err(failed("configured"))?;
        config
            .set_bool("core.filemode", false)
            .map_err(failed("configured"))?;

        Ok(())
    }
}

/// A path as the index and the tree hold it: relative to the Root and always `/`-separated.
///
/// A type of its own because the index's ordering is the byte order of exactly these strings, while the
/// platform's `Path` compares by component — which puts `sub/b.txt` before `sub.txt`, the opposite of
/// what the index expects, and would cost the out-of-order insertion ticket 12 measured at 22.74 s
/// against 0.86 s.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct WirePath(String);

impl WirePath {
    /// Extend a wire path with one more component.
    fn child(&self, name: &str) -> Self {
        if self.0.is_empty() {
            Self(name.to_owned())
        } else {
            Self(format!("{}/{name}", self.0))
        }
    }

    /// The same path as this platform spells it, for reaching the file itself.
    fn as_filesystem_path(&self) -> PathBuf {
        PathBuf::from(self.0.replace('/', std::path::MAIN_SEPARATOR_STR))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// What the walk found.
#[derive(Debug, Default)]
struct Walked {
    /// Every regular file, as the index names it.
    paths: Vec<WirePath>,
    /// Paths that were meant to be captured and could not be read.
    unreadable: Vec<PathBuf>,
    /// Directories whose contents could not be listed, so whatever the baseline holds beneath them must
    /// be carried forward rather than read as a deletion.
    opaque: Vec<WirePath>,
}

impl Walked {
    /// Whether an index path from the baseline should survive this capture.
    ///
    /// A path that was walked survives because it is still there; a path beneath a directory that could
    /// not be listed survives because its absence was never observed. Everything else is gone, and a
    /// Snapshot that kept it would claim content the Working Tree no longer has.
    fn keeps(&self, path: &str) -> bool {
        self.paths.iter().any(|walked| walked.as_str() == path)
            || self
                .opaque
                .iter()
                .any(|prefix| path.starts_with(&format!("{}/", prefix.as_str())))
    }
}

/// Drop every baseline entry whose path is no longer in the Working Tree.
///
/// Without this a deleted file would be carried forward for ever, and the Snapshot would claim content
/// that is gone — the silent disagreement with reality ADR 0004 rejects, in the one direction the
/// product exists to record.
fn remove_vanished(index: &mut git2::Index, walked: &Walked) -> Result<(), StoreError> {
    let vanished: Vec<PathBuf> = index
        .iter()
        .filter_map(|entry| String::from_utf8(entry.path).ok())
        .filter(|path| !walked.keeps(path))
        .map(|path| WirePath(path).as_filesystem_path())
        .collect();

    for path in vanished {
        index.remove_path(&path).map_err(|source| StoreError::Git {
            path: path.clone(),
            operation: "written",
            source,
        })?;
    }

    Ok(())
}

/// Whether the baseline already holds this path with this content and mode.
///
/// Inserting into the index invalidates that path's ancestry in the TREE cache, so leaving an unchanged
/// entry alone is what lets `write_tree` reuse every still-valid subtree (ADR 0004). Until ticket 44
/// brings the stat cache, the file is still hashed to find this out; what is saved is the tree rewrite,
/// which is the part measured in seconds.
fn unchanged(
    baseline: Option<&git2::Tree<'_>>,
    path: &WirePath,
    oid: git2::Oid,
    mode: u32,
) -> bool {
    let Some(tree) = baseline else {
        return false;
    };

    match tree.get_path(&path.as_filesystem_path()) {
        Ok(entry) => entry.id() == oid && entry.filemode() == mode as i32,
        Err(_) => false,
    }
}

/// The names a path component may not have, whatever the user's rules say.
///
/// `.git` and its platform aliases cannot be stored under their own names at all: libgit2's owner-less
/// index refuses such a path on every platform. The map also rules out any interaction with a watched
/// directory's own git, so the directory is not entered either — every git command would otherwise churn
/// its index and objects into history. Ticket 48 adds the record that says a path was deliberately not
/// held; this build simply does not hold it.
fn is_reserved(name: &str) -> bool {
    let folded = name.to_ascii_lowercase();
    let trimmed = folded.trim_end_matches(['.', ' ']);

    trimmed == ".git" || trimmed == "git~1"
}

/// Collect every regular file under `root`, as paths relative to it.
///
/// A directory that cannot be listed is recorded rather than fatal: one unreadable artefact must not
/// freeze a Root's history (ticket 05), and whatever the baseline holds beneath it is carried forward,
/// because its absence was never observed. Only the Root itself being unreadable is an error, since then
/// there is nothing to capture at all.
fn collect_files(root: &Path, relative: &WirePath, walked: &mut Walked) -> Result<(), StoreError> {
    let directory = if relative.as_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative.as_filesystem_path())
    };

    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(source) => {
            if relative.as_str().is_empty() {
                return Err(StoreError::WorkingTree {
                    path: directory,
                    source,
                });
            }

            walked.unreadable.push(directory);
            walked.opaque.push(relative.clone());

            return Ok(());
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                walked.unreadable.push(directory.clone());
                continue;
            }
        };

        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            // A name that is not UTF-8 needs the byte-exact representation ticket 48 brings; until then
            // it cannot be recorded honestly, so it is reported rather than passed over in silence.
            walked.unreadable.push(directory.join(entry.file_name()));
            continue;
        };

        if is_reserved(name) {
            continue;
        }

        let path = relative.child(name);
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => {
                walked.unreadable.push(directory.join(name));
                continue;
            }
        };

        if metadata.is_dir() {
            collect_files(root, &path, walked)?;
        } else if metadata.is_file() {
            walked.paths.push(path);
        }
        // Anything else — a symlink, a socket, a device — waits for tickets 48 and 49, which decide how
        // a Snapshot says that it holds no content for a path.
    }

    Ok(())
}

/// The tree mode for a file.
///
/// ADR 0001 requires a normalised mode: one of git's own values, chosen by us. On Unix the executable
/// bit comes from `stat`; elsewhere there is nothing to read it from, and ticket 49 is where a Windows
/// Snapshot learns to carry the bit forward from the Snapshot before it.
fn mode_for(metadata: &fs::Metadata) -> u32 {
    const REGULAR: u32 = 0o100_644;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        const EXECUTABLE: u32 = 0o100_755;

        if metadata.permissions().mode() & 0o111 == 0 {
            REGULAR
        } else {
            EXECUTABLE
        }
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        REGULAR
    }
}

/// One index entry, with a mode this code sets rather than one git probed.
fn entry_for(path: &WirePath, oid: git2::Oid, mode: u32) -> git2::IndexEntry {
    git2::IndexEntry {
        // This index is written straight to a tree and never to disk, so the fields that exist for
        // git's own staleness checks carry no information here and are left at zero rather than filled
        // with a value that would be read as meaningful (ticket 12).
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id: oid,
        flags: 0,
        flags_extended: 0,
        path: path.as_str().as_bytes().to_vec(),
    }
}
