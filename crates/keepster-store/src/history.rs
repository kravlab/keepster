//! The History Store: a bare git repository per Watched Root, and the only place a git concept is named.
//!
//! Snapshots are stored as segmented chains — one ref per retention period, each Segment starting from a
//! parentless commit — so dropping a whole period is a ref deletion and thinning one rewrites only that
//! period (ADR 0001). Two configuration facts are part of that design rather than details: file modes
//! must not be probed from the filesystem, and reflogs must stay off or a deleted ref stays reachable
//! and Retention frees nothing.
//!
//! Beside the repository lives the derived index (ticket 03), which this type opens on first use and
//! writes only after the repository, so the ordering that keeps the index safe is a fact of one function
//! here rather than of whichever caller sequences two calls.

use std::cell::{RefCell, RefMut};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use keepster_core::{
    CapturePolicy, ChangeKind, ContentFormat, Header, IGNORE_FILE, Incomplete, Kind, ModeKind,
    ModeTable, Parsed, Record, RootId, RootPath, SIDECAR, SegmentName, Sidecar, SnapshotId,
    SpecialKind, Trigger, Verdict, Verify, WithheldReason, ignore_file_rules,
};

use crate::format;
use crate::index::{self, Index, Observation};

mod diff;
mod error_name;
mod forget;
mod maintenance;

pub use forget::Forgotten;
pub use maintenance::Maintained;
mod pins;
mod restore;
mod retention;

pub use diff::{
    Comparison, DIFF_MAX_SIZE, DiffLine, DiffTarget, FileChange, FileDiff, Hunk, LineKind,
};
pub use restore::{LinkNames, Preflight, RestoreScope, Written, destination_naming};
pub use retention::Retained;

/// What the trailer carrying a Snapshot's identity is called.
///
/// The identity lives in the message rather than being the commit hash, because thinning a Segment
/// recreates its commits and anything that referred to a Snapshot by hash would break (ADR 0001).
pub(crate) const SNAPSHOT_TRAILER: &str = "Keepster-Snapshot";

/// What the trailer carrying the Root's identity is called, so a store found on its own says which
/// Root it belongs to.
const ROOT_TRAILER: &str = "Keepster-Root";

/// What the trailer naming the Trigger that caused a Snapshot is called.
///
/// In the commit rather than only in the index, because the index must be rebuildable from git alone
/// and the timeline shows the Trigger (ticket 07).
pub(crate) const TRIGGER_TRAILER: &str = "Keepster-Trigger";

/// What the trailer counting the paths a Snapshot could not read is called.
///
/// The timeline's incomplete flag has to survive an index rebuild, so it is recorded with the Snapshot.
/// It is a count and not the paths: the per-path record, with its errno, is the sidecar's to hold
/// (ADR 0006, ticket 48), and a second home for the same fact would be one that can disagree.
pub(crate) const OMITTED_TRAILER: &str = "Keepster-Omitted";

/// What the trailer counting the paths Forget took from a Snapshot is called (ticket 81): how a
/// Snapshot Forget touched — one it may have left empty — is marked, in the Snapshot itself so that an
/// index rebuild keeps the mark.
pub(crate) const FORGOTTEN_TRAILER: &str = "Keepster-Forgotten";

/// What the trailer naming the content format a Snapshot was written in is called (ADR 0010): on every
/// commit, so a Segment thinned of its root and a Pin standing alone still say it.
pub(crate) const FORMAT_TRAILER: &str = "Keepster-Format";

/// Where a Segment's ref lives. One ref per period, and every name a leaf, because git's files backend
/// cannot hold both `refs/x` and `refs/x/y` (ADR 0001).
pub(crate) const SEGMENT_REFS: &str = "refs/keepster/segments";

/// Where a Pin's ref lives, one per pinned Snapshot and named by it, every name a leaf (ADR 0002).
pub(crate) const PIN_REFS: &str = "refs/keepster/pins";

/// Who the store records as the author of a Snapshot.
///
/// A Snapshot is taken by the daemon, not written by a person, so a fixed identity is more honest than
/// borrowing the user's git identity — which would also make the commit hash depend on their config.
const AUTHOR_NAME: &str = "Keepster";
const AUTHOR_EMAIL: &str = "keepster@localhost";

/// How much of a file git reads to decide whether it is binary, and so how much this store reads.
///
/// git's own rule — a NUL byte in the first 8000 bytes — so a file a person sees as binary in git is
/// binary here too.
const BINARY_PROBE: usize = 8000;

/// The largest packed blob a listing inflates to probe whether it is binary: libgit2 cannot stream a
/// packed object, so a larger one would cost its whole size for 8000 bytes, and is left unprobed.
const PACKED_PROBE_LIMIT: u64 = 1 << 20;

/// How much of a version is handed on at a time when it is read out.
const CHUNK: usize = 64 * 1024;

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
    /// The derived index could not be opened, read or written.
    #[error("the history index at {path} could not be {operation}")]
    Index {
        /// The index involved.
        path: PathBuf,
        /// What was being attempted.
        operation: &'static str,
        /// The underlying failure, carried opaquely for the same reason as a git one.
        #[source]
        source: rusqlite::Error,
    },
    /// The derived index cannot be kept where it is.
    #[error("the history index at {path} cannot be used: {reason}")]
    IndexUnusable {
        /// The index involved.
        path: PathBuf,
        /// Why, phrased for a person.
        reason: &'static str,
    },
    /// A damaged or outdated index could not be deleted so that it could be rebuilt.
    #[error("the history index at {path} could not be discarded for a rebuild")]
    Discard {
        /// The index involved.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: io::Error,
    },
    /// The Working Tree could not be read.
    #[error("{path} could not be read")]
    WorkingTree {
        /// The path that could not be read.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: io::Error,
    },
    /// The moment given for the Snapshot cannot be named or recorded.
    ///
    /// A clock far outside the representable range would otherwise put Snapshots in the wrong Segment
    /// for as long as it stayed wrong, which is worse than refusing.
    #[error("the moment of the Snapshot is outside the range that can be recorded")]
    UnnameableMoment(#[source] keepster_core::UnnameableMoment),
    /// No Snapshot with that identity is in this store.
    #[error("no Snapshot {0} is in this history")]
    SnapshotNotFound(SnapshotId),
    /// The Snapshot holds nothing at that path.
    #[error("the Snapshot holds nothing at `{0}`")]
    PathNotFound(RootPath),
    /// The path names a directory where a file was asked for.
    #[error("`{0}` is a directory in this Snapshot, not a file")]
    NotAFile(RootPath),
    /// The path names a file where a directory was asked for.
    #[error("`{0}` is a file in this Snapshot, not a directory")]
    NotADirectory(RootPath),
    /// The Snapshot holds no content at that path on purpose (ADR 0009).
    #[error("`{path}` is Withheld in this Snapshot ({}), so it holds no content for it", reason.as_str())]
    Withheld {
        /// The path asked for.
        path: RootPath,
        /// Why the Snapshot does not hold it.
        reason: WithheldReason,
    },
    /// Content read from the store could not be handed on, usually because the reader went away.
    #[error("the content could not be delivered")]
    Delivery(#[source] io::Error),
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
    /// The Trigger that caused it.
    pub trigger: Trigger,
    /// Paths that were meant to be captured and are not as they are now, and why.
    ///
    /// The Snapshot is taken anyway — one unreadable artefact must not freeze a Root's history
    /// (ticket 05) — and it says what it omitted, because a Snapshot that omits silently is the one
    /// thing this product forbids. Each path keeps its previous content where there was one, which
    /// ADR 0004 requires of a carried-forward entry; each is a record in the sidecar, and the commit
    /// counts them.
    pub incomplete: Vec<IncompletePath>,
    /// Paths present in the Working Tree whose content the Snapshot deliberately does not hold.
    ///
    /// Withholding is policy, not failure, so it does not make the Snapshot incomplete; and the path has
    /// no tree entry at all (ADR 0009). Each is a record in the sidecar.
    pub withheld: Vec<WithheldPath>,
    /// How many paths differ from the Snapshot before it, counted exactly as the index counts them —
    /// or why they could not be counted.
    ///
    /// Counting happens after the commit and the ref update, so a failure here is not the capture's:
    /// the Snapshot exists, and refusing the request would only invite a retry that takes a second one.
    pub changed_paths: Result<u64, String>,
    /// Why the derived index could not be brought up to date, if it could not.
    ///
    /// The Snapshot itself is durable either way — the commit and the ref came first — so a failing
    /// index fails nothing: it lags, and the next catch-up repairs it. This says so rather than hiding
    /// it, so the caller can report it.
    pub index_lag: Option<String>,
}

/// A path a Snapshot holds no content for, on purpose, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithheldPath {
    /// Where it is, relative to the Root.
    pub path: RootPath,
    /// Why its content was not held.
    pub reason: WithheldReason,
}

/// A path a Snapshot meant to hold and does not hold as it is now, and why — which makes the Snapshot
/// incomplete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompletePath {
    /// Where it is, relative to the Root.
    pub path: RootPath,
    /// Why it is not held as it is.
    pub why: Incomplete,
}

/// A Snapshot as the timeline lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSummary {
    /// The Snapshot's stable identity, which also carries the moment it was taken.
    pub id: SnapshotId,
    /// The Segment it belongs to.
    pub segment: String,
    /// The Trigger that caused it, as the commit recorded it — possibly one a newer build knows and
    /// this one does not, which is passed through rather than refused.
    pub trigger: String,
    /// How many paths it could not read; non-zero makes it incomplete.
    pub omitted: u64,
    /// How many paths differ from the Snapshot before it.
    pub changed_paths: u64,
    /// With a timeline filtered by a path, how that path changed in this Snapshot.
    pub change: Option<ChangeKind>,
    /// Whether it is pinned, and so survives Retention unconditionally (ADR 0002).
    pub pinned: bool,
    /// How many paths Forget took from it (ticket 81); a Snapshot left empty by Forget is one with
    /// none left.
    pub forgotten_paths: u64,
}

/// What a capture is for, beyond what it captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureRequest {
    /// The Trigger the Snapshot is recorded as.
    pub trigger: Trigger,
    /// The moment of the capture, which dates the Snapshot and starts the pass (ADR 0012).
    pub now: SystemTime,
    /// How a file is judged unchanged without being read.
    pub verify: Verify,
    /// Whether a tree identical to its baseline is still written as a Snapshot — which only a capture
    /// someone asked for is: a touched file or a quiet periodic floor is not a new version.
    pub record_unchanged: bool,
}

/// What one pass did.
#[derive(Debug)]
pub struct Pass {
    /// The Snapshot written, or `None` when nothing changed and none was asked for.
    pub snapshot: Option<CapturedSnapshot>,
    /// How many files were read and hashed — the rest were vouched for by the last pass's evidence.
    pub hashed: u64,
    /// How many files were compared.
    pub observed: u64,
    /// Whether every file's mtime fell on a whole second, which says the filesystem keeps no finer
    /// mtimes and the `stat` filter is weaker than it looks (ADR 0014). `status` reports it; hashing
    /// everything instead stays the user's choice.
    pub coarse_mtime: bool,
    /// Why the derived index — the timeline or this pass's evidence — could not be brought up to date.
    /// The Snapshot, if one was written, is durable regardless.
    pub index_lag: Option<String>,
    /// When no Snapshot was written because nothing changed, the one the Working Tree is identical to.
    pub unchanged_since: Option<SnapshotId>,
    /// How many paths the pass meant to capture and could not hold as they are — whether or not a
    /// Snapshot was written, since an unchanged tree can be an unchanged failure. Non-zero owes a retry
    /// (ADR 0004).
    pub incomplete: u64,
    /// Records of the previous Snapshot's sidecar this build does not know — a later build's facts —
    /// which this pass could not carry into its own: to be reported, never dropped in silence.
    pub unrecognized: Vec<String>,
    /// How many links the pass followed into their targets — which no watch covers, so this pass was
    /// their coverage (ticket 49).
    pub followed: u64,
}

/// Which part of a Root's chronology to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineQuery {
    /// Only the Snapshots that changed this path.
    pub path: Option<RootPath>,
    /// Only Snapshots whose identity sorts strictly before this one — a cursor, a moment's bound, or
    /// both reduced to the tighter of the two.
    pub before: Option<SnapshotId>,
    /// Only Snapshots whose identity sorts at or after this one.
    pub not_before: Option<SnapshotId>,
    /// At most this many.
    pub limit: usize,
}

/// One page of a Root's chronology, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelinePage {
    /// The Snapshots on this page.
    pub snapshots: Vec<SnapshotSummary>,
    /// Whether older Snapshots match beyond this page.
    pub more: bool,
    /// The Segments, and the Pins as `pins/` and their identity, in a content format this build cannot
    /// read — a newer one: their Snapshots are on no page, because nothing here can read them (ADR 0010).
    pub unreadable: Vec<String>,
}

/// How deep a listing of entries goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// The directory's own children.
    One,
    /// Everything beneath the directory.
    Infinity,
}

/// What kind of thing a Snapshot holds at a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link, held as a link.
    Symlink,
    /// Present when the Snapshot was taken, and deliberately not held: [`Entry::withheld`] says why.
    Withheld,
    /// Meant to be held, and not read, with no earlier version to keep: [`Entry::incomplete`] says why.
    Omitted,
}

/// What a listing of a Snapshot's entries found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// The entries, in path order.
    pub entries: Vec<Entry>,
    /// Records of the Snapshot's sidecar this build does not know, skipped — to be reported, since a
    /// later build wrote facts this one cannot show.
    pub unrecognized: Vec<String>,
}

/// One path a Snapshot holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Where it is, relative to the Root.
    pub path: RootPath,
    /// What it is.
    pub kind: EntryKind,
    /// Its size in bytes; absent for a directory.
    pub size: Option<u64>,
    /// A file's tree mode as permission bits, `0o644` or `0o755`: whether it is executable, on every
    /// platform.
    pub mode: Option<u32>,
    /// All twelve permission bits of a file or directory as the Snapshot holds them (ticket 50); absent
    /// where the capture observed none — on Windows, or before modes were captured — and for anything
    /// else.
    pub permissions: Option<u32>,
    /// Whether its content is binary by git's rule; absent for anything but a file, and for a packed
    /// file over a megabyte, which would have to be inflated whole to tell (ticket 52).
    pub binary: Option<bool>,
    /// Why the Snapshot does not hold it, for a Withheld path.
    pub withheld: Option<WithheldReason>,
    /// Why it is not held as it was when the Snapshot was taken: an `Omitted` path, or a file whose
    /// previous version was kept.
    pub incomplete: Option<Incomplete>,
}

impl Entry {
    /// An entry with its path and kind and nothing else known yet.
    fn bare(path: RootPath, kind: EntryKind) -> Self {
        Self {
            path,
            kind,
            size: None,
            mode: None,
            permissions: None,
            binary: None,
            withheld: None,
            incomplete: None,
        }
    }
}

/// One version's content, located and sized but not yet read.
pub struct Content<'store> {
    store: &'store HistoryStore,
    blob: git2::Oid,
    size: u64,
}

impl Content<'_> {
    /// How many bytes the version holds.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// An entity tag for this exact content.
    ///
    /// The content hash, so equal content has an equal tag across Snapshots and Roots and a client's
    /// cache stays valid for as long as the bytes do — the strongest validator there is, and free.
    pub fn tag(&self) -> String {
        self.blob.to_string()
    }

    /// Hand `length` bytes starting at `offset` to `sink`, a chunk at a time.
    ///
    /// The object is streamed where the object database can stream it, so a ranged read of a large
    /// file reads the bytes before the range and the range itself, never the remainder; a packed
    /// object cannot be streamed by libgit2 and is read whole, which bounds that case by the file and
    /// not by the response.
    pub fn read(
        &self,
        offset: u64,
        length: u64,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), StoreError> {
        let repository = &self.store.repository;
        let odb = repository.odb().map_err(self.store.failed("read"))?;

        match odb.reader(self.blob) {
            Ok((mut reader, _, _)) => {
                io::copy(&mut (&mut reader).take(offset), &mut io::sink())
                    .map_err(StoreError::Delivery)?;
                let mut remaining = reader.take(length);
                let mut buffer = vec![0; CHUNK];
                loop {
                    let read = remaining.read(&mut buffer).map_err(StoreError::Delivery)?;
                    if read == 0 {
                        return Ok(());
                    }
                    sink(&buffer[..read]).map_err(StoreError::Delivery)?;
                }
            }
            Err(_) => {
                let blob = repository
                    .find_blob(self.blob)
                    .map_err(self.store.failed("read"))?;
                let content = blob.content();
                let start = usize::try_from(offset)
                    .unwrap_or(usize::MAX)
                    .min(content.len());
                let end = start
                    .saturating_add(usize::try_from(length).unwrap_or(usize::MAX))
                    .min(content.len());
                for chunk in content[start..end].chunks(CHUNK) {
                    sink(chunk).map_err(StoreError::Delivery)?;
                }
                Ok(())
            }
        }
    }
}

/// One Watched Root's history, open for the length of one operation.
///
/// It is deliberately not held between operations: `git2`'s repository and index are neither `Send` nor
/// `Sync`, and ADR 0004 already decided that no index survives between Snapshots, so opening per
/// operation is both the cheap option and the one that cannot drift from what is on disk.
pub struct HistoryStore {
    repository: git2::Repository,
    path: PathBuf,
    /// Where the derived index lives, and whether its integrity is checked when it is first opened.
    index_path: PathBuf,
    verify_index: bool,
    /// The derived index, opened on first use. Lazily, because a capture must not depend on it: the
    /// commit and the ref come first, and an index that cannot even be opened only lags.
    index: RefCell<Option<Index>>,
}

impl HistoryStore {
    /// Open the store at `path` with its derived index at `index`, creating either if it is not there
    /// yet.
    pub fn open(path: impl AsRef<Path>, index: impl AsRef<Path>) -> Result<Self, StoreError> {
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

        Ok(Self {
            repository,
            path,
            index_path: index.as_ref().to_path_buf(),
            verify_index: false,
            index: RefCell::new(None),
        })
    }

    /// Open the store for reading, or `None` if no Snapshot was ever taken into it.
    ///
    /// Reading creates nothing: a Root that was registered and never captured has an empty history,
    /// which is an answer, not a reason to write a repository. Nor does it rewrite the store's
    /// configuration — that is the writer's assertion to make.
    pub fn open_existing(
        path: impl AsRef<Path>,
        index: impl AsRef<Path>,
    ) -> Result<Option<Self>, StoreError> {
        Self::open_existing_with(path.as_ref(), index.as_ref(), false)
    }

    /// Open the store as the daemon does at start: verify the index's integrity, discard and rebuild it
    /// if that fails, and catch it up with every Segment.
    ///
    /// This is ticket 03's recovery procedure, and also the whole of what a daemon killed between a
    /// ref update and an index write needs: the watermark shows how far the index got.
    pub fn recover(
        path: impl AsRef<Path>,
        index: impl AsRef<Path>,
    ) -> Result<Option<Self>, StoreError> {
        let Some(store) = Self::open_existing_with(path.as_ref(), index.as_ref(), true)? else {
            return Ok(None);
        };
        store.catch_up()?;

        Ok(Some(store))
    }

    fn open_existing_with(
        path: &Path,
        index: &Path,
        verify: bool,
    ) -> Result<Option<Self>, StoreError> {
        let repository = match git2::Repository::open_bare(path) {
            Ok(repository) => repository,
            Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(source) => {
                return Err(StoreError::Git {
                    path: path.to_path_buf(),
                    operation: "opened",
                    source,
                });
            }
        };

        Ok(Some(Self {
            repository,
            path: path.to_path_buf(),
            index_path: index.to_path_buf(),
            verify_index: verify,
            index: RefCell::new(None),
        }))
    }

    /// Where this store lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bring the derived index up to date with the Segment refs.
    pub fn catch_up(&self) -> Result<(), StoreError> {
        self.index()?.catch_up(&self.repository)
    }

    /// The derived index, opened — and, at a daemon's start, verified — on first use.
    fn index(&self) -> Result<RefMut<'_, Index>, StoreError> {
        let mut slot = self.index.borrow_mut();
        if slot.is_none() {
            *slot = Some(Index::open(&self.index_path, self.verify_index)?);
        }

        Ok(RefMut::map(slot, |slot| {
            slot.as_mut().expect("the index was opened just above")
        }))
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
    /// The commit and the ref update come first and the derived index after, in this one function, so
    /// the index can only ever lag git and never name a commit that does not exist (ticket 03).
    ///
    /// What enters the Snapshot is what `policy` admits: the user's Include Rules, the secret denylist
    /// and the size cap, applied during the walk so an excluded directory is never entered (ADR 0007).
    /// The records that name what a Snapshot deliberately does not hold arrive with ticket 48, and
    /// symlinks and special files with ticket 49; until then a path that is not a regular file is
    /// passed over.
    pub fn capture(
        &self,
        root_id: RootId,
        root_path: &Path,
        policy: &CapturePolicy,
        trigger: Trigger,
        now: SystemTime,
    ) -> Result<CapturedSnapshot, StoreError> {
        let pass = self.pass(
            root_id,
            root_path,
            policy,
            &CaptureRequest {
                trigger,
                now,
                verify: Verify::Stat,
                record_unchanged: true,
            },
        )?;

        Ok(pass
            .snapshot
            .expect("a Snapshot is always written when asked to record an unchanged tree"))
    }

    /// One pass over the Root: walk it, read what the evidence of the last pass cannot vouch for, and
    /// write a Snapshot unless the request says an unchanged tree needs none.
    ///
    /// Every capture is a pass — the watcher's, the periodic floor's, a Reconciliation's and one someone
    /// asked for alike — because the walk is what makes a capture correct whatever the watcher missed,
    /// and the `stat` filter is what makes the walk cheap (ADR 0014). The evidence is kept in the derived
    /// index; an index that cannot be read only means everything is read.
    pub fn pass(
        &self,
        root_id: RootId,
        root_path: &Path,
        policy: &CapturePolicy,
        request: &CaptureRequest,
    ) -> Result<Pass, StoreError> {
        let CaptureRequest {
            trigger,
            now,
            verify,
            record_unchanged,
        } = *request;
        let segment = self.segment_at(now)?;
        let reference = retention::segment_ref(&segment);
        let (parent, baseline) = self.parent_and_baseline(&segment, &reference)?;
        let baseline_tree = match &baseline {
            Some(commit) => Some(commit.tree().map_err(self.failed("read"))?),
            None => None,
        };

        // Evidence that cannot be read is no evidence: everything is read instead, which is the cost of
        // a lost index and never a wrong Snapshot. Verifying by hash reads everything regardless, but the
        // evidence is still loaded, so the rows this pass makes obsolete are still removed.
        let observations = self
            .index()
            .and_then(|index| index.observations())
            .unwrap_or_default();
        let vouching = match verify {
            Verify::Stat => &observations,
            Verify::Hash => &HashMap::new(),
        };
        let (tree_id, walked) = self.write_working_tree(
            baseline_tree.as_ref(),
            root_path,
            policy,
            vouching,
            now,
            trigger == Trigger::Reconcile,
        )?;
        let previous = baseline
            .as_ref()
            .and_then(|commit| commit.message().ok().and_then(index::trailers))
            .map(|trailers| trailers.id);
        // Counted by the very function the index uses, so the answer to this request and the timeline
        // can never disagree about what this Snapshot changed.
        let baseline_id = baseline_tree.as_ref().map(git2::Tree::id);
        let changes = index::changes_between(&self.repository, baseline_id, tree_id);
        // A tree written differently that changed no path — a header counted again by a
        // Reconciliation — is nothing to record: the next Snapshot that changes something carries it.
        // Modes observed for the first time are something: from then on a `chmod` can be told.
        let nothing_changed = match &baseline_tree {
            Some(baseline) => {
                changes.as_ref().is_ok_and(|changes| changes.is_empty())
                    && self.observes_modes(baseline)?
                        == self.observes_modes(&self.find(tree_id)?)?
            }
            None => false,
        };
        if !record_unchanged && nothing_changed {
            let index_lag = self.keep_observations(&walked, &observations);
            let mut pass = walked.pass(None, index_lag);
            pass.unchanged_since = previous;
            return Ok(pass);
        }
        let tree = self.find(tree_id)?;

        let id = SnapshotId::following(previous, now);
        let signature = self.signature(now)?;
        let message = format!(
            "Snapshot\n\n\
             {SNAPSHOT_TRAILER}: {id}\n\
             {ROOT_TRAILER}: {root_id}\n\
             {TRIGGER_TRAILER}: {trigger}\n\
             {OMITTED_TRAILER}: {}\n\
             {FORMAT_TRAILER}: {}\n",
            walked.incomplete.len(),
            ContentFormat::CURRENT,
        );
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

        let changed_paths = changes
            .map(|changes| u64::try_from(changes.len()).unwrap_or(u64::MAX))
            .map_err(|source| self.failed("read")(source).to_string());

        // Both, whatever the first does: a lagging timeline must not leave the evidence of a pass that
        // saw a file vanish describing it still.
        let lagging = self.catch_up().err().map(|error| error.to_string());
        let index_lag = self.keep_observations(&walked, &observations).or(lagging);
        let snapshot = CapturedSnapshot {
            id,
            segment,
            trigger,
            incomplete: walked.incomplete.clone(),
            withheld: walked.withheld.clone(),
            changed_paths,
            index_lag: index_lag.clone(),
        };

        Ok(walked.pass(Some(snapshot), index_lag))
    }

    /// The Segment a capture at `now` writes to: its hour's own, unless that holds another content
    /// format — then the hour's sibling in this build's, since no capture ever appends across formats
    /// (ADR 0010). Once the sibling exists, the hour's captures stay in it.
    fn segment_at(&self, now: SystemTime) -> Result<SegmentName, StoreError> {
        let period = SegmentName::hourly(now).map_err(StoreError::UnnameableMoment)?;
        let sibling = period.in_format(ContentFormat::CURRENT);
        if self
            .segment_tip(&retention::segment_ref(&sibling))?
            .is_some()
        {
            return Ok(sibling);
        }

        match self.segment_tip(&retention::segment_ref(&period))? {
            Some(tip)
                if format::of(&self.repository, tip.id()).map_err(self.failed("read"))?
                    != Some(ContentFormat::CURRENT) =>
            {
                Ok(sibling)
            }
            _ => Ok(period),
        }
    }

    /// The Snapshot a capture at `segment` would follow — its *parent*, the tip of that Segment — and the
    /// one whose tree it starts from — its *baseline*.
    ///
    /// Two different questions, and conflating them is what ADR 0004 warns about. The parent is the tip
    /// of this Segment, so a new Segment starts parentless (ADR 0001); the baseline is the newest Snapshot
    /// this build reads anywhere at or before it, because the previous Snapshot's tree must be readable
    /// at every capture, including the first of a new Segment — and because a period may hold a sibling
    /// in another format whose newest Snapshot is newer than this Segment's tip (ADR 0010).
    fn parent_and_baseline(
        &self,
        segment: &SegmentName,
        reference: &str,
    ) -> Result<(Option<git2::Commit<'_>>, Option<git2::Commit<'_>>), StoreError> {
        let parent = self.segment_tip(reference)?;
        let baseline = self.newest_snapshot_before(segment)?;

        Ok((parent, baseline))
    }

    /// The Working Tree as a tree in this store's object database, under `policy` — what a Snapshot
    /// taken now would hold — starting from the baseline's tree.
    ///
    /// Writing the tree writes the blobs it needs; when no Snapshot follows, they stay unreferenced until
    /// maintenance prunes them, and the next capture that needs them finds them already written.
    fn write_working_tree(
        &self,
        baseline_tree: Option<&git2::Tree<'_>>,
        root_path: &Path,
        policy: &CapturePolicy,
        vouching: &HashMap<String, Observation>,
        now: SystemTime,
        reconciling: bool,
    ) -> Result<(git2::Oid, Walked), StoreError> {
        let mut index = git2::Index::new().map_err(self.failed("prepared"))?;
        if let Some(tree) = baseline_tree {
            // Seeding from this store's own objects matters: cached subtree ids are copied without an
            // existence check, so a seed from elsewhere could yield a tree referencing objects this
            // store lacks (ADR 0004). Reading the tree also fills the TREE cache, which is what lets
            // `write_tree` rewrite only the changed paths' ancestry.
            index.read_tree(tree).map_err(self.failed("read"))?;
        }

        let mut walked =
            self.add_working_tree(&mut index, baseline_tree, root_path, policy, vouching, now)?;
        remove_vanished(&mut index, &walked)?;
        walked.unrecognized =
            self.write_sidecar(&mut index, baseline_tree, &walked, reconciling)?;
        let tree_id = index
            .write_tree_to(&self.repository)
            .map_err(self.failed("written"))?;

        Ok((tree_id, walked))
    }

    /// Put the walk's records in the sidecar at the tree root, or take the sidecar out when it would
    /// say nothing (ADR 0006), returning the baseline sidecar's records this build could not carry.
    ///
    /// The entry is left alone when the bytes did not change, for the TREE cache's sake (ADR 0004).
    ///
    /// The header names the Root's modal modes, which take the whole tree to count: a Reconciliation,
    /// which walks it anyway, counts them again, and so does a capture with no observed header to
    /// carry; every other capture carries the baseline's forward (ADR 0006). Each path then costs a
    /// record only where its mode is not its kind's default. On Unix the header also says the modes
    /// were observed, so every Snapshot taken there has a sidecar; elsewhere none are, and a Snapshot
    /// with no Withheld or incomplete path has none.
    ///
    /// The records come from this walk alone, so whatever the baseline's sidecar held that this build
    /// does not know — a later build's facts — is not written again, and is handed back to be reported
    /// rather than dropped in silence.
    fn write_sidecar(
        &self,
        index: &mut git2::Index,
        baseline: Option<&git2::Tree<'_>>,
        walked: &Walked,
        reconciling: bool,
    ) -> Result<Vec<String>, StoreError> {
        let recorded = baseline.and_then(|tree| tree.get_name(SIDECAR).map(|entry| entry.id()));
        let Parsed { sidecar, unknown } = match baseline {
            Some(tree) => self.sidecar(tree)?,
            None => Parsed {
                sidecar: Sidecar::with_records(Vec::new()),
                unknown: Vec::new(),
            },
        };

        let (header, modes) = if cfg!(unix) {
            let modes = tree_modes(index, walked, &sidecar.modes());
            // Counted again by a Reconciliation, and wherever there is no header worth carrying: the
            // first Snapshot, and the first after one that observed no modes.
            let header = if reconciling || !sidecar.header.observed {
                Header::modal(modes.iter().map(|(_, kind, mode)| (*kind, *mode)))
            } else {
                sidecar.header
            };
            (header, modes)
        } else {
            // Windows observes no modes, so the Snapshot holds none and says so (ticket 17).
            (Header::default(), Vec::new())
        };
        let mut records = walked.records();
        records.extend(
            modes
                .into_iter()
                .filter(|(_, kind, mode)| *mode != header.default_mode(*kind))
                .map(|(path, _, mode)| Record::Mode { path, mode }),
        );
        if records.is_empty() && header == Header::default() {
            if recorded.is_some() {
                index
                    .remove_path(Path::new(SIDECAR))
                    .map_err(self.failed("written"))?;
            }
            return Ok(unknown);
        }

        let mut written = Sidecar::with_records(records);
        written.header = header;
        let blob = self
            .repository
            .blob(&written.to_bytes())
            .map_err(self.failed("written"))?;
        if recorded != Some(blob) {
            index
                .add(&entry_for(
                    &WirePath(SIDECAR.to_owned()),
                    blob,
                    REGULAR_MODE,
                ))
                .map_err(self.failed("written"))?;
        }

        Ok(unknown)
    }

    /// One page of the Root's chronology, newest first.
    pub fn timeline(&self, query: &TimelineQuery) -> Result<TimelinePage, StoreError> {
        self.catch_up()?;

        let mut found = self.index()?.timeline(
            query.path.as_ref(),
            query.before,
            query.not_before,
            query.limit.saturating_add(1),
        )?;
        let more = found.len() > query.limit;
        found.truncate(query.limit);

        Ok(TimelinePage {
            snapshots: found.into_iter().map(SnapshotSummary::from).collect(),
            more,
            unreadable: self.index()?.unreadable()?,
        })
    }

    /// The paths a Snapshot holds under `directory`, in path order — and the paths it names without
    /// holding them: Withheld, or meant to be held and not read (ticket 48).
    ///
    /// The sidecar is the Snapshot's own record and never one of the Root's paths, so it is not listed;
    /// what it says is joined in instead. A directory that holds only Withheld paths has no tree in git,
    /// which holds no empty directories, and is listed all the same — or the paths beneath it could not
    /// be reached.
    pub fn entries(
        &self,
        snapshot: SnapshotId,
        directory: &RootPath,
        depth: Depth,
    ) -> Result<Listing, StoreError> {
        let root = self.snapshot_tree(snapshot)?;
        let Parsed { sidecar, unknown } = self.sidecar(&root)?;
        // A mode is a fact about a path the tree holds, read with it below, and never a path of its own.
        let records: Vec<&Record> = sidecar
            .records
            .iter()
            .filter(|record| !matches!(record, Record::Mode { .. }))
            .filter(|record| is_beneath(record.path(), directory))
            .collect();

        let tree = if directory.is_root() {
            Some(root)
        } else {
            match self.entry_at(&root, directory) {
                Ok(entry) if entry.kind() == Some(git2::ObjectType::Tree) => Some(
                    self.repository
                        .find_tree(entry.id())
                        .map_err(self.failed("read"))?,
                ),
                Ok(_) => return Err(StoreError::NotADirectory(directory.clone())),
                Err(StoreError::PathNotFound(_)) if !records.is_empty() => None,
                Err(StoreError::PathNotFound(path)) => return Err(nothing_at(&sidecar, path)),
                Err(error) => return Err(error),
            }
        };

        let mut entries = Vec::new();
        if let Some(tree) = tree {
            self.list(&tree, directory, depth, &mut entries)?;
        }
        // Only what the tree holds has a mode: a directory joined in below for the Withheld paths
        // beneath it was never held.
        let modes = sidecar.modes();
        for entry in &mut entries {
            let kind = match (entry.kind, entry.mode) {
                (EntryKind::File, Some(0o755)) => ModeKind::Executable,
                (EntryKind::File, _) => ModeKind::File,
                (EntryKind::Directory, _) => ModeKind::Directory,
                _ => continue,
            };
            entry.permissions = modes.mode_of(&entry.path, kind);
        }
        join_records(&mut entries, &records, directory, depth);

        // Byte order of the full path — not git's tree order, which sorts a directory as if its name
        // ended in `/` and so would interleave differently from what a client sorts by.
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        Ok(Listing {
            entries,
            unrecognized: unknown,
        })
    }

    /// What `tree`, the tree of `directory`, holds — the sidecar left out at the tree root.
    fn list(
        &self,
        tree: &git2::Tree<'_>,
        directory: &RootPath,
        depth: Depth,
        entries: &mut Vec<Entry>,
    ) -> Result<(), StoreError> {
        let is_sidecar =
            |prefix: &str, name: &str| directory.is_root() && prefix.is_empty() && name == SIDECAR;
        // One handle for the whole listing, rather than one per size and per probe.
        let odb = self.repository.odb().map_err(self.failed("read"))?;

        match depth {
            Depth::One => {
                for entry in tree.iter() {
                    let name = String::from_utf8_lossy(entry.name_bytes()).into_owned();
                    if is_sidecar("", &name) {
                        continue;
                    }
                    if let Some(entry) = self.describe(&odb, directory.child(&name), &entry)? {
                        entries.push(entry);
                    }
                }
            }
            Depth::Infinity => {
                let mut failure = None;
                tree.walk(git2::TreeWalkMode::PreOrder, |prefix, entry| {
                    let name = String::from_utf8_lossy(entry.name_bytes());
                    if is_sidecar(prefix, &name) {
                        return git2::TreeWalkResult::Skip;
                    }
                    let mut path = directory.clone();
                    for component in prefix.split('/').filter(|part| !part.is_empty()) {
                        path = path.child(component);
                    }
                    match self.describe(&odb, path.child(&name), entry) {
                        Ok(Some(entry)) => entries.push(entry),
                        Ok(None) => {}
                        Err(error) => {
                            failure = Some(error);
                            return git2::TreeWalkResult::Abort;
                        }
                    }
                    git2::TreeWalkResult::Ok
                })
                .map_err(|source| {
                    failure
                        .take()
                        .unwrap_or_else(|| self.failed("read")(source))
                })?;
                if let Some(error) = failure {
                    return Err(error);
                }
            }
        }

        Ok(())
    }

    /// The tree `id` in this store.
    fn find(&self, id: git2::Oid) -> Result<git2::Tree<'_>, StoreError> {
        self.repository.find_tree(id).map_err(self.failed("read"))
    }

    /// Whether the Snapshot whose tree is `root` observed its paths' modes.
    fn observes_modes(&self, root: &git2::Tree<'_>) -> Result<bool, StoreError> {
        Ok(self.sidecar(root)?.sidecar.header.observed)
    }

    /// What a Snapshot's sidecar says, or nothing when it has none.
    ///
    /// A sidecar in a grammar newer than this build is read as saying nothing this build understands,
    /// and its header is reported as the record that could not be read.
    fn sidecar(&self, root: &git2::Tree<'_>) -> Result<Parsed, StoreError> {
        let empty = || Sidecar::with_records(Vec::new());
        let Some(entry) = root.get_name(SIDECAR) else {
            return Ok(Parsed {
                sidecar: empty(),
                unknown: Vec::new(),
            });
        };
        let blob = self
            .repository
            .find_blob(entry.id())
            .map_err(self.failed("read"))?;

        Ok(Sidecar::parse(blob.content()).unwrap_or_else(|_| Parsed {
            sidecar: empty(),
            unknown: vec![
                String::from_utf8_lossy(
                    blob.content()
                        .split(|byte| *byte == b'\n')
                        .next()
                        .unwrap_or_default(),
                )
                .into_owned(),
            ],
        }))
    }

    /// One version of a file: what a Snapshot holds at `path`, located and sized.
    pub fn content(
        &self,
        snapshot: SnapshotId,
        path: &RootPath,
    ) -> Result<Content<'_>, StoreError> {
        if path.is_root() {
            return Err(StoreError::NotAFile(path.clone()));
        }

        let root = self.snapshot_tree(snapshot)?;
        if path.as_str() == SIDECAR {
            return Err(StoreError::PathNotFound(path.clone()));
        }
        let entry = match self.entry_at(&root, path) {
            Err(StoreError::PathNotFound(path)) => {
                return Err(nothing_at(&self.sidecar(&root)?.sidecar, path));
            }
            found => found?,
        };
        if entry.kind() != Some(git2::ObjectType::Blob) {
            return Err(StoreError::NotAFile(path.clone()));
        }

        let size = self.blob_size(entry.id())?;

        Ok(Content {
            store: self,
            blob: entry.id(),
            size,
        })
    }

    /// The root tree of a Snapshot, found through the index.
    fn snapshot_tree(&self, snapshot: SnapshotId) -> Result<git2::Tree<'_>, StoreError> {
        self.snapshot_commit(snapshot)?
            .tree()
            .map_err(self.failed("read"))
    }

    /// The commit the Snapshot is read from: its Pin's while it has one (ADR 0002).
    fn snapshot_commit(&self, snapshot: SnapshotId) -> Result<git2::Commit<'_>, StoreError> {
        self.catch_up()?;

        let indexed = self
            .index()?
            .snapshot(snapshot)?
            .ok_or(StoreError::SnapshotNotFound(snapshot))?;

        self.repository
            .find_commit(indexed.commit)
            .map_err(self.failed("read"))
    }

    fn entry_at(
        &self,
        root: &git2::Tree<'_>,
        path: &RootPath,
    ) -> Result<git2::TreeEntry<'static>, StoreError> {
        match root.get_path(Path::new(path.as_str())) {
            Ok(entry) => Ok(entry),
            Err(error) if error.code() == git2::ErrorCode::NotFound => {
                Err(StoreError::PathNotFound(path.clone()))
            }
            Err(source) => Err(self.failed("read")(source)),
        }
    }

    /// Describe one tree entry, or `None` for a kind this product never writes.
    fn describe(
        &self,
        odb: &git2::Odb<'_>,
        path: RootPath,
        entry: &git2::TreeEntry<'_>,
    ) -> Result<Option<Entry>, StoreError> {
        const DIRECTORY: i32 = 0o040_000;
        const EXECUTABLE: i32 = 0o100_755;
        const SYMLINK: i32 = 0o120_000;

        let mode = entry.filemode();
        let described = match mode {
            DIRECTORY => Entry::bare(path, EntryKind::Directory),
            SYMLINK => Entry {
                size: Some(self.size_in(odb, entry.id())?),
                ..Entry::bare(path, EntryKind::Symlink)
            },
            _ if entry.kind() == Some(git2::ObjectType::Blob) => {
                let size = self.size_in(odb, entry.id())?;
                Entry {
                    size: Some(size),
                    mode: Some(if mode == EXECUTABLE { 0o755 } else { 0o644 }),
                    binary: self.probe_binary(odb, entry.id(), size, PACKED_PROBE_LIMIT)?,
                    ..Entry::bare(path, EntryKind::File)
                }
            }
            // A submodule link: a Snapshot never holds one, so it is not described as anything.
            _ => return Ok(None),
        };

        Ok(Some(described))
    }

    /// A blob's size, read from its header rather than by inflating it.
    fn blob_size(&self, blob: git2::Oid) -> Result<u64, StoreError> {
        let odb = self.repository.odb().map_err(self.failed("read"))?;
        self.size_in(&odb, blob)
    }

    fn size_in(&self, odb: &git2::Odb<'_>, blob: git2::Oid) -> Result<u64, StoreError> {
        let (size, _) = odb.read_header(blob).map_err(self.failed("read"))?;

        Ok(u64::try_from(size).unwrap_or(u64::MAX))
    }

    /// Whether a blob is binary by git's rule, read whole if it must be — as a diff, which reads it
    /// whole anyway, may.
    fn is_binary(&self, blob: git2::Oid) -> Result<bool, StoreError> {
        let odb = self.repository.odb().map_err(self.failed("read"))?;

        Ok(self
            .probe_binary(&odb, blob, 0, u64::MAX)?
            .unwrap_or_default())
    }

    /// Whether a blob of `size` bytes is binary by git's rule, reading only the prefix that rule looks
    /// at where the object can be streamed — a loose one. A packed one cannot be (libgit2 inflates it
    /// whole), so it is probed only up to `limit` bytes and is otherwise not probed at all.
    fn probe_binary(
        &self,
        odb: &git2::Odb<'_>,
        blob: git2::Oid,
        size: u64,
        limit: u64,
    ) -> Result<Option<bool>, StoreError> {
        if let Ok((reader, _, _)) = odb.reader(blob) {
            let mut prefix = Vec::with_capacity(BINARY_PROBE);
            if reader
                .take(BINARY_PROBE as u64)
                .read_to_end(&mut prefix)
                .is_ok()
            {
                return Ok(Some(prefix.contains(&0)));
            }
        }
        if size > limit {
            return Ok(None);
        }

        let object = odb.read(blob).map_err(self.failed("read"))?;
        let content = object.data();

        Ok(Some(
            content[..content.len().min(BINARY_PROBE)].contains(&0),
        ))
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
        policy: &CapturePolicy,
        observations: &HashMap<String, Observation>,
        now: SystemTime,
    ) -> Result<Walked, StoreError> {
        let mut walked = Walked::default();
        let place =
            Place::root(fs::canonicalize(root_path).unwrap_or_else(|_| root_path.to_path_buf()));
        collect_files(root_path, &RootPath::root(), policy, &place, &mut walked)?;

        // Entries are applied in the index's own order, which is the byte order of the `/`-joined path —
        // not the component order `Path` compares by, where `sub/b.txt` would precede `sub.txt` while
        // the index expects the opposite. Ticket 12 measured 0.86 s in order against 22.74 s out of it.
        walked.paths.sort();

        let started = nanos_since_epoch(now);
        let mut sub_second = false;
        let mut paths = std::mem::take(&mut walked.paths);
        // Paths that are no longer regular files by the time they are read: deleted since the walk, or
        // replaced by a special file. Neither is the file the walk found, and neither is a failure.
        let mut gone = Vec::new();
        for path in &paths {
            let absolute = match walked.real.get(path) {
                Some(real) => real.clone(),
                None => root_path.join(path.as_filesystem_path()),
            };

            let metadata = match fs::symlink_metadata(&absolute) {
                Ok(metadata) => metadata,
                // A file already gone is simply deleted (ticket 10).
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    gone.push(path.clone());
                    continue;
                }
                // Unreadable: carry the baseline entry forward and say so, rather than losing the whole
                // Snapshot (ticket 05).
                Err(error) => {
                    walked.incomplete(path, Incomplete::Error(error_name::of(&error)));
                    continue;
                }
            };
            let recorded = baseline_entry(baseline, path);

            // The filter: the last pass read this very size and mtime and found the content the baseline
            // holds. Any doubt — no row, a different size or mtime, a baseline that disagrees — reads.
            let mtime = metadata.modified().ok().map(nanos_since_epoch);
            // Beyond a followed link the evidence is keyed by where the file really is, so a link
            // retargeted at a lookalike tree — same sizes, same mtimes — vouches for nothing.
            let evidence = match walked.real.get(path) {
                Some(real) => format!("{}\0{}", path.0, real.display()),
                None => path.0.clone(),
            };
            let vouched = match (observations.get(&evidence), mtime, recorded) {
                (Some(seen), Some(mtime), Some((oid, _)))
                    if seen.size == metadata.len() && seen.mtime == mtime && seen.oid == oid =>
                {
                    Some(oid)
                }
                _ => None,
            };
            // What was read is described by the `stat` taken while it was open, which is what the next
            // pass's filter must compare against.
            let (oid, metadata) = match vouched {
                Some(oid) => (oid, metadata),
                None => match self.read_settled(&absolute)? {
                    Settled::Held(oid, metadata) => {
                        walked.hashed += 1;
                        (oid, metadata)
                    }
                    // Whatever the baseline holds stays — it is already in the index — and a path it
                    // does not hold stays absent; either way the record names it.
                    Settled::Unstable => {
                        walked.incomplete(path, Incomplete::Unstable);
                        continue;
                    }
                    Settled::Failed(error) if error.kind() == io::ErrorKind::NotFound => {
                        gone.push(path.clone());
                        continue;
                    }
                    Settled::Failed(error) => {
                        walked.incomplete(path, Incomplete::Error(error_name::of(&error)));
                        continue;
                    }
                    Settled::Special(kind) => {
                        walked.withhold(path.to_root_path(), WithheldReason::SpecialFile(kind));
                        gone.push(path.clone());
                        continue;
                    }
                },
            };
            let mode = mode_for(&metadata);
            // Taken from the `stat` the filter already made, so a permission-only change costs nothing
            // to see: the sidecar's records differ, and with them the tree.
            if let Some(bits) = permissions(&metadata) {
                walked.modes.insert(path.0.clone(), bits);
            }
            let size = metadata.len();
            let mtime = metadata.modified().ok().map(nanos_since_epoch);
            sub_second |= mtime.is_some_and(|mtime| mtime % 1_000_000_000 != 0);

            if let Some(mtime) = mtime {
                walked
                    .observed
                    .push((evidence, Observation { size, mtime, oid }));
            }
            walked.seen += 1;

            // A permission-only change differs here, from the `stat` alone, with nothing read.
            if recorded == Some((oid, mode)) {
                continue;
            }

            index
                .add(&entry_for(path, oid, mode))
                .map_err(self.failed("written"))?;
        }
        // `gone` was collected in the order of `paths`, so both stay sorted.
        paths.retain(|path| gone.binary_search(path).is_err());
        walked.paths = paths;

        // A link's content is its target text, which costs nothing to read, so no evidence vouches for it.
        walked.links.sort();
        for (path, text) in &walked.links {
            let unchanged = git2::Oid::hash_object(git2::ObjectType::Blob, text)
                .is_ok_and(|oid| baseline_entry(baseline, path) == Some((oid, LINK_MODE)));
            if unchanged {
                continue;
            }
            let oid = self.repository.blob(text).map_err(self.failed("written"))?;
            index
                .add(&entry_for(path, oid, LINK_MODE))
                .map_err(self.failed("written"))?;
        }
        walked.coarse_mtime = walked.seen > 0 && !sub_second;

        // Git's racy rule: a file whose mtime is not older than the pass's start may change again within
        // the same tick and look identical, so it is not vouched for next time. The comparison is against
        // the daemon's clock, not the filesystem's — writing a probe into the user's tree to read the
        // filesystem's own time is not ours to do — so the start is moved back by the coarsest tick there
        // is: FAT's two seconds, which also covers a whole-second filesystem, a kernel stamping mtimes
        // from a coarse clock and a modest skew. What it costs is re-reading the files written in the
        // two seconds before a pass, once.
        walked
            .observed
            .retain(|(_, observation)| observation.mtime + COARSE_TICK_NANOS < started);

        Ok(walked)
    }

    /// Read one file into the object database, trying again while it changes under the read.
    ///
    /// A file being written is read again rather than captured half-written; one that never settles is
    /// `unstable`, and the caller carries its previous version forward (ticket 48).
    fn read_settled(&self, path: &Path) -> Result<Settled, StoreError> {
        for _ in 0..=READ_RETRIES {
            match self.read_once(path)? {
                Attempt::Settled(settled) => return Ok(settled),
                Attempt::Changed => {}
            }
        }

        Ok(Settled::Unstable)
    }

    /// One attempt at reading a file whole, streamed into the object database so a file is never held
    /// in memory.
    ///
    /// The file is opened without following a link and without blocking, so a link or a pipe that took
    /// its place since the walk can neither redirect the read nor stall the capture; it must still be a
    /// regular file once open. It counts as read only if the bytes are exactly as many as its `stat`
    /// said, and nothing about it — size, mtime, change time, identity — moved while it was read.
    fn read_once(&self, path: &Path) -> Result<Attempt, StoreError> {
        let failed = |error| Ok(Attempt::Settled(Settled::Failed(error)));
        let file = match open_unfollowed(path) {
            Ok(file) => file,
            Err(error) => return failed(error),
        };
        let before = match file.metadata() {
            Ok(metadata) => metadata,
            Err(error) => return failed(error),
        };
        if !before.is_file() {
            // A pipe or a device took its place since the walk: never read, as the walk would have it.
            return Ok(match special_kind(&before) {
                Some(kind) => Attempt::Settled(Settled::Special(kind)),
                None => Attempt::Changed,
            });
        }

        let mut writer = self
            .repository
            .blob_writer(None)
            .map_err(self.failed("written"))?;
        // One byte past the size, to see a file that grew without reading all it grew by.
        let mut source = (&file).take(before.len().saturating_add(1));
        let mut buffer = vec![0; CHUNK];
        let mut total: u64 = 0;
        loop {
            let read = match source.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return failed(error),
            };
            total += read as u64;
            writer.write_all(&buffer[..read]).map_err(|error| {
                self.failed("written")(git2::Error::from_str(&error.to_string()))
            })?;
        }

        let after = match file.metadata() {
            Ok(metadata) => metadata,
            Err(error) => return failed(error),
        };
        let still_there = fs::symlink_metadata(path)
            .is_ok_and(|there| Stamp::of(&there).identity == Stamp::of(&after).identity);
        if total != before.len() || Stamp::of(&before) != Stamp::of(&after) || !still_there {
            return Ok(Attempt::Changed);
        }

        let oid = writer.commit().map_err(self.failed("written"))?;
        Ok(Attempt::Settled(Settled::Held(oid, after)))
    }

    /// Replace the evidence the next pass compares against with what this one saw. A failure costs the
    /// next pass some reading, never this one's Snapshot, so it is reported rather than raised.
    fn keep_observations(
        &self,
        walked: &Walked,
        previous: &HashMap<String, Observation>,
    ) -> Option<String> {
        self.index()
            .and_then(|mut index| index.update_observations(previous, &walked.observed))
            .err()
            .map(|error| format!("the evidence for the next pass could not be kept: {error}"))
    }

    /// The newest Snapshot this build can read in any Segment at or before `segment`'s period.
    ///
    /// Segment names sort chronologically as text (ADR 0001), so "the Snapshot before this one" is in
    /// the greatest period that does not exceed the current one — which is how the baseline survives an
    /// hour boundary. A period may hold Segments of several formats (ADR 0010): the newest Snapshot
    /// among those this build reads is the one, and one only a newer build reads is passed over, so the
    /// baseline is never a tree this build would misread.
    fn newest_snapshot_before(
        &self,
        segment: &SegmentName,
    ) -> Result<Option<git2::Commit<'_>>, StoreError> {
        let references = self
            .repository
            .references_glob(&format!("{SEGMENT_REFS}/*"))
            .map_err(self.failed("read"))?;

        let mut periods: BTreeMap<SegmentName, Vec<git2::Commit<'_>>> = BTreeMap::new();
        for reference in references {
            let reference = reference.map_err(self.failed("read"))?;
            // A ref whose name is no Segment's, or which names no commit, was not written by us.
            let Some(name) = reference
                .name()
                .ok()
                .and_then(|name| name.rsplit('/').next())
                .and_then(SegmentName::parse)
            else {
                continue;
            };
            let tip = match reference.peel_to_commit() {
                Ok(tip) => tip,
                Err(error)
                    if matches!(
                        error.code(),
                        git2::ErrorCode::Peel | git2::ErrorCode::NotFound
                    ) =>
                {
                    continue;
                }
                Err(source) => return Err(self.failed("read")(source)),
            };
            if name.period() <= segment.period() {
                periods.entry(name.period()).or_default().push(tip);
            }
        }

        for tips in periods.into_values().rev() {
            let mut newest: Option<(SnapshotId, git2::Commit<'_>)> = None;
            for tip in tips {
                if !self.is_readable(tip.id())? {
                    continue;
                }
                let Some(id) = tip.message().ok().and_then(index::trailers).map(|t| t.id) else {
                    continue;
                };
                if newest.as_ref().is_none_or(|(best, _)| id > *best) {
                    newest = Some((id, tip));
                }
            }
            if let Some((_, tip)) = newest {
                return Ok(Some(tip));
            }
        }

        Ok(None)
    }

    /// Whether this build reads the Segment or Pin whose tip is `tip`: whether it is in a format from the
    /// first to this build's own. One in any other is unreadable — nothing reads it, nothing rewrites it,
    /// and it costs only its own history (ADR 0010).
    fn is_readable(&self, tip: git2::Oid) -> Result<bool, StoreError> {
        Ok(format::of(&self.repository, tip)
            .map_err(self.failed("read"))?
            .is_some_and(keepster_core::ContentFormat::is_readable))
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
        for (key, value) in crate::git::STORE_CONFIG {
            config.set_bool(key, value).map_err(failed("configured"))?;
        }

        Ok(())
    }
}

/// A path as the index and the tree hold it: relative to the Root and always `/`-separated.
///
/// A type of its own because the index's ordering is the byte order of exactly these strings, while the
/// platform's `Path` compares by component — which puts `sub/b.txt` before `sub.txt`, the opposite of
/// what the index expects, and would cost the out-of-order insertion ticket 12 measured at 22.74 s
/// against 0.86 s.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct WirePath(String);

impl WirePath {
    /// The same path as this platform spells it, for reaching the file itself.
    fn as_filesystem_path(&self) -> PathBuf {
        PathBuf::from(self.0.replace('/', std::path::MAIN_SEPARATOR_STR))
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    /// The same path as the domain names it.
    fn to_root_path(&self) -> RootPath {
        // Built from components rather than parsed: every one came from the filesystem as one name.
        self.0
            .split('/')
            .filter(|name| !name.is_empty())
            .fold(RootPath::root(), |path, name| path.child(name))
    }
}

/// What the walk found.
#[derive(Debug, Default)]
struct Walked {
    /// Every regular file, as the index names it.
    paths: Vec<WirePath>,
    /// Paths that were meant to be captured and are not held as they are now.
    incomplete: Vec<IncompletePath>,
    /// Paths that could not be looked at — a directory that could not be listed, an entry that could
    /// not be `stat`ed — so whatever the baseline holds at or beneath them is carried forward rather
    /// than read as a deletion.
    carried: Vec<WirePath>,
    /// Paths present but deliberately not held.
    withheld: Vec<WithheldPath>,
    /// Links stored as links, with their target text.
    links: Vec<(WirePath, Vec<u8>)>,
    /// Where a file reached through a followed link to it is read from, since the link is never opened.
    real: HashMap<WirePath, PathBuf>,
    /// The permission bits of every file read or vouched for and every directory entered, by the path
    /// the index names it by; empty where the platform keeps none (ticket 50).
    modes: HashMap<String, u32>,
    /// What the baseline's sidecar said that this build does not know, and so did not carry.
    unrecognized: Vec<String>,
    /// How many links the walk followed.
    followed: u64,
    /// What this pass observed of each file it read or vouched for, for the next pass to compare with.
    observed: Vec<(String, Observation)>,
    /// How many files were read and hashed.
    hashed: u64,
    /// How many files were compared at all.
    seen: u64,
    /// Whether every mtime observed fell on a whole second.
    coarse_mtime: bool,
}

impl Walked {
    fn pass(&self, snapshot: Option<CapturedSnapshot>, index_lag: Option<String>) -> Pass {
        Pass {
            snapshot,
            hashed: self.hashed,
            observed: self.seen,
            coarse_mtime: self.coarse_mtime,
            index_lag,
            unchanged_since: None,
            incomplete: u64::try_from(self.incomplete.len()).unwrap_or(u64::MAX),
            unrecognized: self.unrecognized.clone(),
            followed: self.followed,
        }
    }

    /// Name a path the Snapshot does not hold as it is now.
    fn incomplete(&mut self, path: &WirePath, why: Incomplete) {
        self.incomplete.push(IncompletePath {
            path: path.to_root_path(),
            why,
        });
    }

    /// Name a path that could not be looked at, and keep what the baseline holds there.
    fn could_not_look(&mut self, path: WirePath, error: &io::Error) {
        self.incomplete(&path, Incomplete::Error(error_name::of(error)));
        self.carried.push(path);
    }

    /// Act on the policy's verdict on `rooted`: `held` when its content is held, a Withheld record when
    /// it is withheld, and nothing when the rules leave it out.
    fn judge(
        &mut self,
        judged: Judged,
        rooted: &RootPath,
        held: impl FnOnce(&mut Self) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        match judged {
            Judged::Held => held(self),
            Judged::Withheld(reason) => {
                self.withhold(rooted.clone(), reason);
                Ok(())
            }
            Judged::Left => Ok(()),
        }
    }

    /// Name a directory that leads back to one already on the way to it, and keep what the baseline
    /// holds there, rather than walk it for ever.
    fn loops(&mut self, rooted: &RootPath) {
        let path = WirePath(rooted.as_str().to_owned());
        self.incomplete(&path, Incomplete::Error("eloop".to_owned()));
        self.carried.push(path);
    }

    /// Note the mode of a directory the walk enters at `rooted` — a followed link's target's, at the
    /// link's own path.
    fn entered(&mut self, rooted: &RootPath, metadata: &fs::Metadata) {
        if let Some(bits) = permissions(metadata) {
            self.modes.insert(rooted.as_str().to_owned(), bits);
        }
    }

    /// Hold back a path, with its reason.
    fn withhold(&mut self, path: RootPath, reason: WithheldReason) {
        self.withheld.push(WithheldPath { path, reason });
    }

    /// What the sidecar says of this walk.
    fn records(&self) -> Vec<Record> {
        self.withheld
            .iter()
            .map(|withheld| Record::Withheld {
                path: withheld.path.clone(),
                reason: withheld.reason,
            })
            .chain(self.incomplete.iter().map(|incomplete| Record::Incomplete {
                path: incomplete.path.clone(),
                why: incomplete.why.clone(),
            }))
            .collect()
    }

    /// Whether an index path from the baseline should survive this capture.
    ///
    /// A path that was walked survives because it is still there; a path at or beneath one that could
    /// not be looked at survives because its absence was never observed. The sidecar is written after
    /// this, from this walk's records. Everything else is gone, and a Snapshot that kept it would claim
    /// content the Working Tree no longer has.
    fn keeps(&self, path: &str) -> bool {
        // `paths` is sorted by the time the baseline is compared against it.
        path == SIDECAR
            || self
                .paths
                .binary_search_by(|walked| walked.as_str().cmp(path))
                .is_ok()
            || self
                .links
                .binary_search_by(|(walked, _)| walked.as_str().cmp(path))
                .is_ok()
            || self.carried.iter().any(|carried| {
                path == carried.as_str() || path.starts_with(&format!("{}/", carried.as_str()))
            })
    }
}

/// How many times a file that changes while it is read is read again before it is called `unstable`
/// (ticket 10).
const READ_RETRIES: usize = 3;

/// What reading a file came to.
enum Settled {
    /// Its content, and its `stat` as it was read.
    Held(git2::Oid, fs::Metadata),
    /// It changed under every attempt.
    Unstable,
    /// Reading it failed.
    Failed(io::Error),
    /// A special file is there now, which is Withheld rather than read.
    Special(SpecialKind),
}

/// What one attempt at reading a file came to.
enum Attempt {
    /// An answer.
    Settled(Settled),
    /// It changed while it was read.
    Changed,
}

/// What must not move while a file is read for its content to be the file's.
#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: Option<SystemTime>,
    /// The change time, which moves on any write even when a writer restores the mtime.
    changed: Option<(i64, i64)>,
    /// Which file it is, so a file renamed over it is not mistaken for it.
    identity: Option<(u64, u64)>,
}

impl Stamp {
    fn of(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            Self {
                size: metadata.len(),
                modified: metadata.modified().ok(),
                changed: Some((metadata.ctime(), metadata.ctime_nsec())),
                identity: Some((metadata.dev(), metadata.ino())),
            }
        }

        #[cfg(not(unix))]
        {
            Self {
                size: metadata.len(),
                modified: metadata.modified().ok(),
                changed: None,
                identity: None,
            }
        }
    }
}

/// Open a file to read it for a capture: never through a link, and never waiting on a pipe.
fn open_unfollowed(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        // And never as a terminal that would become the daemon's own.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY);
    }

    options.open(path)
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

/// The content and mode the baseline holds for this path, if it holds it.
///
/// What a pass compares against twice: the last pass's evidence vouches for a file only when it read
/// this very content, and an entry left exactly as the baseline has it is not inserted again — inserting
/// invalidates the path's ancestry in the TREE cache, and leaving it alone is what lets `write_tree`
/// reuse every still-valid subtree (ADR 0004).
fn baseline_entry(baseline: Option<&git2::Tree<'_>>, path: &WirePath) -> Option<(git2::Oid, u32)> {
    let entry = baseline?.get_path(&path.as_filesystem_path()).ok()?;

    Some((entry.id(), u32::try_from(entry.filemode()).ok()?))
}

/// Whether `path` is strictly beneath `directory`.
fn is_beneath(path: &RootPath, directory: &RootPath) -> bool {
    if directory.is_root() {
        return !path.is_root();
    }

    path.as_str()
        .strip_prefix(directory.as_str())
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Why `sidecar` says its Snapshot holds no content at `path`, if it says so — of the path itself, or of a
/// directory above it that was Withheld whole and never entered.
fn withheld_at(sidecar: &Sidecar, path: &RootPath) -> Option<WithheldReason> {
    sidecar.records.iter().find_map(|record| match record {
        Record::Withheld {
            path: withheld,
            reason,
        } if withheld == path || is_beneath(path, withheld) => Some(*reason),
        _ => None,
    })
}

/// What to answer for a `path` the Snapshot's tree has no entry for: Withheld, if `sidecar` says so,
/// and otherwise not found.
fn nothing_at(sidecar: &Sidecar, path: RootPath) -> StoreError {
    match withheld_at(sidecar, &path) {
        Some(reason) => StoreError::Withheld { path, reason },
        None => StoreError::PathNotFound(path),
    }
}

/// Join the sidecar's `records` beneath `directory` into a listing of it: a file kept from an earlier
/// version is marked, a path with no tree entry is added, and so is every directory above one that git
/// holds no tree for.
fn join_records(entries: &mut Vec<Entry>, records: &[&Record], directory: &RootPath, depth: Depth) {
    // Indexed by path, since one unreadable directory can name thousands of paths (ADR 0009).
    let mut at: HashMap<RootPath, usize> = entries
        .iter()
        .enumerate()
        .map(|(position, entry)| (entry.path.clone(), position))
        .collect();
    fn add(entries: &mut Vec<Entry>, at: &mut HashMap<RootPath, usize>, entry: Entry) {
        at.insert(entry.path.clone(), entries.len());
        entries.push(entry);
    }

    for record in records {
        let path = record.path();
        let leaf = depth == Depth::Infinity || path.parent() == *directory;

        let mut above: Vec<RootPath> = path
            .ancestors()
            .filter(|ancestor| is_beneath(ancestor, directory))
            .collect();
        if depth == Depth::One {
            above.truncate(1);
        }
        for ancestor in above {
            if !at.contains_key(&ancestor) {
                add(
                    entries,
                    &mut at,
                    Entry::bare(ancestor, EntryKind::Directory),
                );
            }
        }
        if !leaf {
            continue;
        }

        match (record, at.get(path).copied()) {
            (Record::Incomplete { why, .. }, Some(kept)) => {
                entries[kept].incomplete = Some(why.clone());
            }
            (Record::Incomplete { why, .. }, None) => add(
                entries,
                &mut at,
                Entry {
                    incomplete: Some(why.clone()),
                    ..Entry::bare(path.clone(), EntryKind::Omitted)
                },
            ),
            (Record::Withheld { reason, .. }, None) => add(
                entries,
                &mut at,
                Entry {
                    withheld: Some(*reason),
                    ..Entry::bare(path.clone(), EntryKind::Withheld)
                },
            ),
            // A tree entry where the sidecar says there is none was not written by this build's
            // rule (ADR 0009); the entry is what can be read, so it stands.
            (Record::Withheld { .. }, Some(_)) => {}
            // Never passed in: a mode is read with the tree entry it belongs to.
            (Record::Mode { .. }, _) => {}
        }
    }
}

/// The coarsest mtime tick a filesystem this product meets keeps: FAT and exFAT record two seconds.
const COARSE_TICK_NANOS: i128 = 2_000_000_000;

/// A moment as nanoseconds since the epoch, before it as a negative count.
fn nanos_since_epoch(moment: SystemTime) -> i128 {
    match moment.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => i128::try_from(after.as_nanos()).unwrap_or(i128::MAX),
        Err(before) => -i128::try_from(before.duration().as_nanos()).unwrap_or(i128::MAX),
    }
}

/// Collect every file the policy admits under `root`, as paths relative to it.
///
/// The policy is applied *during* the walk rather than after it, which is what makes "an excluded
/// directory is never entered" true (ADR 0007): a pruned directory is not listed, so it costs nothing to
/// walk and nothing beneath it can come back. A reserved name — the directory's own `.git` — is passed
/// over the same way, whatever any rule says. With ignore files honoured, each directory's `.gitignore`
/// joins the policy, beneath the user's own rules, for that directory and everything under it.
///
/// A directory that cannot be listed is recorded rather than fatal: one unreadable artefact must not
/// freeze a Root's history (ticket 05), and whatever the baseline holds beneath it is carried forward,
/// because its absence was never observed. Only the Root itself being unreadable is an error, since then
/// there is nothing to capture at all.
fn collect_files(
    root: &Path,
    here: &RootPath,
    policy: &CapturePolicy,
    place: &Place,
    walked: &mut Walked,
) -> Result<(), StoreError> {
    let relative = WirePath(here.as_str().to_owned());
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

            walked.could_not_look(relative, &source);

            return Ok(());
        }
    };

    let extended;
    let policy = if policy.honours_ignore_files() {
        match fs::read_to_string(directory.join(IGNORE_FILE)) {
            Ok(text) => {
                let source = here.child(IGNORE_FILE);
                extended =
                    policy.with_ignore_rules(ignore_file_rules(here, &text, source.as_str()));
                &extended
            }
            // No ignore file, or one that cannot be read: it belongs to the directory's own tooling, and
            // its absence changes nothing.
            Err(_) => policy,
        }
    } else {
        policy
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            // The listing broke off: what it did not reach was never observed to be gone.
            Err(source) => {
                if relative.as_str().is_empty() {
                    return Err(StoreError::WorkingTree {
                        path: directory,
                        source,
                    });
                }
                walked.could_not_look(relative, &source);
                return Ok(());
            }
        };

        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            // A name that is not UTF-8 needs the byte-exact representation of ticket 74; until then it
            // is named, as nearly as text can name it, with the error that says why it cannot be held.
            let lossy = here.child(&entry.file_name().to_string_lossy());
            walked.incomplete.push(IncompletePath {
                path: lossy,
                why: Incomplete::Error(error_name::UNREPRESENTABLE.to_owned()),
            });
            continue;
        };

        // Built from the walk's own components rather than parsed: a name the filesystem handed over is
        // one component, whatever characters it holds.
        let rooted = here.child(name);
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                walked.could_not_look(WirePath(rooted.as_str().to_owned()), &error);
                continue;
            }
        };

        // Present, but without content: no tree entry at all (ADR 0009), and a record in the sidecar.
        // A reserved or fenced directory is one record, and is never entered.
        if metadata.file_type().is_symlink() {
            walk_link(root, &rooted, &directory.join(name), policy, place, walked)?;
        } else if metadata.is_dir() {
            let real = place.real.join(name);
            match Judged::from(policy.directory(&rooted)) {
                // A fence is named by its real path where a followed link reaches it.
                Judged::Held => match policy.fenced_at(&real) {
                    Some(reason) => walked.withhold(rooted, reason),
                    // A directory already on the way here — a bind mount of an ancestor — would never
                    // end: what the baseline holds there is kept, and the loop named.
                    None if place.revisits(&metadata) => walked.loops(&rooted),
                    None => {
                        walked.entered(&rooted, &metadata);
                        collect_files(
                            root,
                            &rooted,
                            policy,
                            &place.within(real, &metadata),
                            walked,
                        )?;
                    }
                },
                Judged::Withheld(reason) => walked.withhold(rooted, reason),
                Judged::Left => {}
            }
        } else if metadata.is_file() {
            match Judged::from(policy.file(&rooted, metadata.len())) {
                Judged::Held => {
                    let wire = WirePath(rooted.as_str().to_owned());
                    // Beyond a followed link, the evidence of the last pass is keyed by where the file
                    // really is, so a link retargeted at a lookalike tree vouches for nothing.
                    if place.is_followed() {
                        walked.real.insert(wire.clone(), place.real.join(name));
                    }
                    walked.paths.push(wire);
                }
                Judged::Withheld(reason) => walked.withhold(rooted, reason),
                Judged::Left => {}
            }
        } else if let Some(kind) = special_kind(&metadata) {
            // Never opened: a pipe without a writer would block the capture, and a device has no content
            // that is the user's. What the rules exclude stays excluded.
            match Judged::from(policy.file(&rooted, 0)) {
                Judged::Held => walked.withhold(rooted, WithheldReason::SpecialFile(kind)),
                Judged::Withheld(reason) => walked.withhold(rooted, reason),
                Judged::Left => {}
            }
        }
    }

    Ok(())
}

/// Where the directory a walk is in really is, for deciding where a link it meets may lead.
struct Place {
    /// The Root's own real path, where a cycle is measured from.
    root: PathBuf,
    /// Its real path: the Root's, joined with names, or a followed link's target, joined with names.
    real: PathBuf,
    /// The targets of the links followed to get here, by their real paths.
    followed: Vec<PathBuf>,
    /// Every directory on the way here by its identity, `(dev, ino)`, which no path comparison can
    /// replace: a bind mount, or a chain of links, reaches one directory by several paths. Empty
    /// where the platform gives no identity.
    seen: Vec<(u64, u64)>,
}

impl Place {
    /// The Root itself, at its real path.
    fn root(real: PathBuf) -> Self {
        let seen = fs::metadata(&real)
            .ok()
            .and_then(|metadata| identity(&metadata))
            .into_iter()
            .collect();
        Self {
            root: real.clone(),
            real,
            followed: Vec::new(),
            seen,
        }
    }

    /// The directory `real`, `metadata` its own, beneath this one, reached by name.
    fn within(&self, real: PathBuf, metadata: &fs::Metadata) -> Self {
        let mut seen = self.seen.clone();
        seen.extend(identity(metadata));
        Self {
            root: self.root.clone(),
            real,
            followed: self.followed.clone(),
            seen,
        }
    }

    /// The target `real`, `metadata` its own, of a link followed from here.
    fn following(&self, real: PathBuf, metadata: &fs::Metadata) -> Self {
        let mut next = self.within(real.clone(), metadata);
        next.followed.push(real);
        next
    }

    /// Whether the directory `metadata` describes is already on the way here.
    fn revisits(&self, metadata: &fs::Metadata) -> bool {
        identity(metadata).is_some_and(|identity| self.seen.contains(&identity))
    }

    /// Whether a followed link led here.
    fn is_followed(&self) -> bool {
        !self.followed.is_empty()
    }

    /// Everything a link followed from here must not lead back to: the Root, the targets followed on
    /// the way here, and this directory itself.
    fn walk(&self) -> Vec<PathBuf> {
        let mut walk = Vec::with_capacity(self.followed.len() + 2);
        walk.push(self.root.clone());
        walk.extend(self.followed.iter().cloned());
        walk.push(self.real.clone());
        walk
    }
}

/// What a walk does with the link at `rooted` — `absolute` on disk — met in `place` (ticket 49).
///
/// Stored as a link, its target text the blob, unless the follow list names it: then its target is
/// captured at the link's own path, under the rules for that logical path — unless following it would
/// close a cycle or lead into another Root or a History Store, when it is stored as a link after all. A
/// followed link whose target cannot be reached is not read as everything beneath it deleted: the
/// baseline is carried forward and the link named with its error, as an unmount must be.
fn walk_link(
    root: &Path,
    rooted: &RootPath,
    absolute: &Path,
    policy: &CapturePolicy,
    place: &Place,
    walked: &mut Walked,
) -> Result<(), StoreError> {
    let wire = || WirePath(rooted.as_str().to_owned());
    let target = fs::metadata(absolute);
    // A target that cannot be reached has no kind to read, and an unmounted directory must still be
    // known by a pattern naming it as one, or its subtree would read as deleted.
    let follows = match &target {
        Ok(metadata) if metadata.is_dir() => policy.follows(rooted, Kind::Directory),
        Ok(_) => policy.follows(rooted, Kind::File),
        Err(_) => policy.follows(rooted, Kind::Directory) || policy.follows(rooted, Kind::File),
    };

    if follows {
        let followed = target.and_then(|metadata| Ok((metadata, fs::canonicalize(absolute)?)));
        let (metadata, real) = match followed {
            Ok(followed) => followed,
            Err(error) => {
                walked.judge(Judged::from(policy.file(rooted, 0)), rooted, |walked| {
                    walked.could_not_look(wire(), &error);
                    Ok(())
                })?;
                return Ok(());
            }
        };

        let stopped = policy.stop_following(&real, &place.walk()).is_some()
            || (metadata.is_dir() && place.revisits(&metadata));
        if !stopped {
            if metadata.is_dir() {
                walked.judge(Judged::from(policy.directory(rooted)), rooted, |walked| {
                    walked.followed += 1;
                    walked.entered(rooted, &metadata);
                    let inside = place.following(real, &metadata);
                    collect_files(root, rooted, policy, &inside, walked)
                })?;
            } else if metadata.is_file() {
                // Read by its real path: the link itself is never opened, since reads never follow one.
                walked.judge(
                    Judged::from(policy.file(rooted, metadata.len())),
                    rooted,
                    |walked| {
                        walked.followed += 1;
                        walked.real.insert(wire(), real);
                        walked.paths.push(wire());
                        Ok(())
                    },
                )?;
            } else if let Some(special) = special_kind(&metadata) {
                walked.judge(Judged::from(policy.file(rooted, 0)), rooted, |walked| {
                    walked.withhold(rooted.clone(), WithheldReason::SpecialFile(special));
                    Ok(())
                })?;
            }
            return Ok(());
        }
    }

    let text = fs::read_link(absolute).map(|text| link_text(&text));
    let size = text
        .as_ref()
        .map_or(0, |text| u64::try_from(text.len()).unwrap_or(u64::MAX));
    walked.judge(Judged::from(policy.file(rooted, size)), rooted, |walked| {
        match text {
            Ok(text) => walked.links.push((wire(), text)),
            Err(error) => walked.could_not_look(wire(), &error),
        }
        Ok(())
    })
}

/// A directory's identity, `(dev, ino)`, where the platform gives one.
fn identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        Some((metadata.dev(), metadata.ino()))
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

/// A link's target as git holds it: its bytes on Unix, `/`-separated text elsewhere.
fn link_text(target: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        target.as_os_str().as_bytes().to_vec()
    }

    #[cfg(not(unix))]
    {
        target.to_string_lossy().replace('\\', "/").into_bytes()
    }
}

/// What a walk does with a path, by the policy's verdict on it.
enum Judged {
    /// Its content is held: a file is read, a directory entered.
    Held,
    /// It is recorded as there, without its content, and why.
    Withheld(WithheldReason),
    /// The rules leave it out, which is the user's choice and no record of the Snapshot's.
    Left,
}

impl From<Verdict<'_>> for Judged {
    fn from(verdict: Verdict<'_>) -> Self {
        match verdict {
            Verdict::Captured => Self::Held,
            Verdict::TooLarge { .. } => Self::Withheld(WithheldReason::TooLarge),
            Verdict::Reserved => Self::Withheld(WithheldReason::ReservedName),
            Verdict::Fenced(reason) => Self::Withheld(reason),
            Verdict::Excluded(_) | Verdict::Pruned { .. } | Verdict::Secret(_) => Self::Left,
        }
    }
}

/// What kind of special file this is, if it is one.
fn special_kind(metadata: &fs::Metadata) -> Option<SpecialKind> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        let kind = metadata.file_type();
        if kind.is_fifo() {
            Some(SpecialKind::Fifo)
        } else if kind.is_socket() {
            Some(SpecialKind::Socket)
        } else if kind.is_char_device() {
            Some(SpecialKind::CharDevice)
        } else if kind.is_block_device() {
            Some(SpecialKind::BlockDevice)
        } else {
            None
        }
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

/// The mode a sidecar, or any regular file that is not executable, is held with.
const REGULAR_MODE: u32 = 0o100_644;

/// The mode a link is held with, its blob the target text.
const LINK_MODE: u32 = 0o120_000;

/// The tree mode for a file.
///
/// ADR 0001 requires a normalised mode: one of git's own values, chosen by us. On Unix the executable
/// bit comes from `stat`; elsewhere there is nothing to read it from, and ticket 49 is where a Windows
/// Snapshot learns to carry the bit forward from the Snapshot before it.
fn mode_for(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        const EXECUTABLE: u32 = 0o100_755;

        if metadata.permissions().mode() & 0o111 == 0 {
            REGULAR_MODE
        } else {
            EXECUTABLE
        }
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        REGULAR_MODE
    }
}

/// All twelve permission bits, where the platform keeps them: on Unix, and nowhere else (ticket 50).
fn permissions(metadata: &fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        Some(metadata.permissions().mode() & 0o7777)
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

/// What a tree entry's mode makes it, for the mode it has by default — `None` for a link, whose
/// permission bits mean nothing, and for anything else a Snapshot keeps no mode for.
fn mode_kind(filemode: i32) -> Option<ModeKind> {
    match filemode {
        0o040_000 => Some(ModeKind::Directory),
        0o100_755 => Some(ModeKind::Executable),
        0o100_644 => Some(ModeKind::File),
        _ => None,
    }
}

/// Every file and directory the tree being written holds, with its kind and mode: as this walk found
/// it, or — for a path it carried forward — as the baseline held it, if the baseline observed one.
fn tree_modes(
    index: &git2::Index,
    walked: &Walked,
    baseline: &ModeTable,
) -> Vec<(RootPath, ModeKind, u32)> {
    let mut held: BTreeMap<String, ModeKind> = BTreeMap::new();
    for entry in index.iter() {
        let Ok(path) = String::from_utf8(entry.path) else {
            continue;
        };
        if path == SIDECAR {
            continue;
        }
        // Every directory above a held path is a tree of its own.
        for (end, _) in path.match_indices('/') {
            held.insert(path[..end].to_owned(), ModeKind::Directory);
        }
        if let Some(kind) = i32::try_from(entry.mode).ok().and_then(mode_kind) {
            held.insert(path, kind);
        }
    }

    held.into_iter()
        .filter_map(|(path, kind)| {
            let rooted = WirePath(path).to_root_path();
            let mode = walked
                .modes
                .get(rooted.as_str())
                .copied()
                .or_else(|| baseline.mode_of(&rooted, kind))?;
            Some((rooted, kind, mode))
        })
        .collect()
}

/// A path both of two trees hold whose mode differs between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModeChange {
    /// Where.
    pub path: RootPath,
    /// What it is in the newer tree.
    pub kind: ModeKind,
    /// The mode in the older tree.
    pub old: u32,
    /// The mode in the newer tree.
    pub new: u32,
}

/// Every path held by both `before` and `after`, directories included, whose mode — as each tree's
/// sidecar says — differs between them, in path order.
///
/// Only the paths with a record on either side can differ while the headers agree; a header that
/// changed can change any path's mode, and then every path of `after` is compared.
pub(crate) fn mode_changes(
    before: &git2::Tree<'_>,
    was: &ModeTable,
    after: &git2::Tree<'_>,
    is: &ModeTable,
) -> Vec<ModeChange> {
    let kind_in = |tree: &git2::Tree<'_>, path: &RootPath| {
        tree.get_path(Path::new(path.as_str()))
            .ok()
            .and_then(|entry| mode_kind(entry.filemode()))
    };
    // A side that observed no modes has none to compare.
    if !was.header().observed || !is.header().observed {
        return Vec::new();
    }
    let compare = |path: RootPath| -> Option<ModeChange> {
        let (kind_before, kind) = (kind_in(before, &path)?, kind_in(after, &path)?);
        let (old, new) = (was.mode_of(&path, kind_before)?, is.mode_of(&path, kind)?);
        (old != new).then_some(ModeChange {
            path,
            kind,
            old,
            new,
        })
    };

    let mut changes: Vec<ModeChange> = if was.header() == is.header() {
        let candidates: BTreeSet<RootPath> = was
            .records()
            .chain(is.records())
            .map(|(path, _)| path.clone())
            .collect();
        candidates.into_iter().filter_map(compare).collect()
    } else {
        let mut paths = Vec::new();
        let _ = after.walk(git2::TreeWalkMode::PreOrder, |prefix, entry| {
            let name = String::from_utf8_lossy(entry.name_bytes());
            if prefix.is_empty() && name == SIDECAR {
                return git2::TreeWalkResult::Skip;
            }
            paths.push(WirePath(format!("{prefix}{name}")).to_root_path());
            git2::TreeWalkResult::Ok
        });
        paths.into_iter().filter_map(compare).collect()
    };
    changes.sort_by(|a, b| a.path.cmp(&b.path));

    changes
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

impl From<index::IndexedSnapshot> for SnapshotSummary {
    fn from(snapshot: index::IndexedSnapshot) -> Self {
        Self {
            id: snapshot.id,
            segment: snapshot.segment,
            trigger: snapshot.trigger,
            omitted: snapshot.omitted,
            changed_paths: snapshot.changed,
            change: snapshot.change,
            pinned: snapshot.pinned,
            forgotten_paths: snapshot.forgotten,
        }
    }
}
