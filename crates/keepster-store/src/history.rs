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
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use keepster_core::{
    CapturePolicy, ChangeKind, IGNORE_FILE, RootId, RootPath, SegmentName, SnapshotId, Trigger,
    Verdict, Verify, ignore_file_rules,
};

use crate::index::{self, Index, Observation};

mod diff;
mod restore;

pub use diff::{
    Comparison, DIFF_MAX_SIZE, DiffLine, DiffTarget, FileChange, FileDiff, Hunk, LineKind,
};
pub use restore::{Preflight, RestoreScope, Written, destination_naming};

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

/// Where a Segment's ref lives. One ref per period, and every name a leaf, because git's files backend
/// cannot hold both `refs/x` and `refs/x/y` (ADR 0001).
pub(crate) const SEGMENT_REFS: &str = "refs/keepster/segments";

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
    /// Paths that were meant to be captured and could not be read.
    ///
    /// The Snapshot is taken anyway — one unreadable artefact must not freeze a Root's history
    /// (ticket 05) — and it says what it omitted, because a Snapshot that omits silently is the one
    /// thing this product forbids. Each path keeps its previous content, which ADR 0004 requires of a
    /// carried-forward entry; the commit records how many there were, and ticket 48 is where each one
    /// becomes a record of the Snapshot itself.
    pub unreadable: Vec<PathBuf>,
    /// Files present in the Working Tree whose content the Snapshot deliberately does not hold.
    ///
    /// Withholding is policy, not failure, so it does not make the Snapshot incomplete; and the path has
    /// no tree entry at all (ADR 0009). Ticket 48 makes this a record of the Snapshot itself, in the
    /// sidecar; until then the capture's own answer is where it is named.
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

/// Why a Snapshot holds no content for a path that is there (ticket 10). Ticket 48 adds the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithheldReason {
    /// Over its size cap.
    TooLarge,
}

impl WithheldReason {
    /// The reason as the wire spells it (ticket 10).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
        }
    }
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
    /// Its permission bits, as far as this build records them; absent where nothing is recorded.
    ///
    /// The executable bit is all a tree mode carries, so a file reads as `0o644` or `0o755` until the
    /// sidecar of ticket 50 holds all twelve bits.
    pub mode: Option<u32>,
    /// Whether its content is binary by git's rule; absent for anything but a file.
    pub binary: Option<bool>,
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
        let segment = SegmentName::hourly(now).map_err(StoreError::UnnameableMoment)?;
        let reference = format!("{SEGMENT_REFS}/{segment}");
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
        let (tree_id, walked) =
            self.write_working_tree(baseline_tree.as_ref(), root_path, policy, vouching, now)?;
        let previous = baseline
            .as_ref()
            .and_then(|commit| commit.message().ok().and_then(index::trailers))
            .map(|trailers| trailers.id);
        if !record_unchanged && baseline_tree.as_ref().map(git2::Tree::id) == Some(tree_id) {
            let index_lag = self.keep_observations(&walked, &observations);
            let mut pass = walked.pass(None, index_lag);
            pass.unchanged_since = previous;
            return Ok(pass);
        }
        let tree = self
            .repository
            .find_tree(tree_id)
            .map_err(self.failed("read"))?;

        let id = SnapshotId::following(previous, now);
        let signature = self.signature(now)?;
        let message = format!(
            "Snapshot\n\n\
             {SNAPSHOT_TRAILER}: {id}\n\
             {ROOT_TRAILER}: {root_id}\n\
             {TRIGGER_TRAILER}: {trigger}\n\
             {OMITTED_TRAILER}: {}\n",
            walked.unreadable.len()
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

        // Counted by the very function the index uses, so the answer to this request and the timeline
        // can never disagree about what this Snapshot changed.
        let changed_paths = index::changes_between(
            &self.repository,
            baseline_tree.as_ref().map(git2::Tree::id),
            tree_id,
        )
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
            unreadable: walked.unreadable.clone(),
            withheld: walked.withheld.clone(),
            changed_paths,
            index_lag: index_lag.clone(),
        };

        Ok(walked.pass(Some(snapshot), index_lag))
    }

    /// The Snapshot a capture at `segment` would follow — its *parent*, the tip of that Segment — and the
    /// one whose tree it starts from — its *baseline*.
    ///
    /// Two different questions, and conflating them is what ADR 0004 warns about. The parent is the tip
    /// of this Segment, so a new Segment starts parentless (ADR 0001); the baseline is the newest Snapshot
    /// anywhere at or before it, because the previous Snapshot's tree must be readable at every capture,
    /// including the first of a new Segment.
    fn parent_and_baseline(
        &self,
        segment: &SegmentName,
        reference: &str,
    ) -> Result<(Option<git2::Commit<'_>>, Option<git2::Commit<'_>>), StoreError> {
        let parent = self.segment_tip(reference)?;
        let baseline = match &parent {
            Some(parent) => Some(parent.clone()),
            None => self.newest_snapshot_before(segment)?,
        };

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
    ) -> Result<(git2::Oid, Walked), StoreError> {
        let mut index = git2::Index::new().map_err(self.failed("prepared"))?;
        if let Some(tree) = baseline_tree {
            // Seeding from this store's own objects matters: cached subtree ids are copied without an
            // existence check, so a seed from elsewhere could yield a tree referencing objects this
            // store lacks (ADR 0004). Reading the tree also fills the TREE cache, which is what lets
            // `write_tree` rewrite only the changed paths' ancestry.
            index.read_tree(tree).map_err(self.failed("read"))?;
        }

        let walked =
            self.add_working_tree(&mut index, baseline_tree, root_path, policy, vouching, now)?;
        remove_vanished(&mut index, &walked)?;
        let tree_id = index
            .write_tree_to(&self.repository)
            .map_err(self.failed("written"))?;

        Ok((tree_id, walked))
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
            snapshots: found
                .into_iter()
                .map(|snapshot| SnapshotSummary {
                    id: snapshot.id,
                    segment: snapshot.segment,
                    trigger: snapshot.trigger,
                    omitted: snapshot.omitted,
                    changed_paths: snapshot.changed,
                    change: snapshot.change,
                })
                .collect(),
            more,
        })
    }

    /// The paths a Snapshot holds under `directory`, in path order.
    pub fn entries(
        &self,
        snapshot: SnapshotId,
        directory: &RootPath,
        depth: Depth,
    ) -> Result<Vec<Entry>, StoreError> {
        let root = self.snapshot_tree(snapshot)?;
        let tree = if directory.is_root() {
            root
        } else {
            let entry = self.entry_at(&root, directory)?;
            if entry.kind() != Some(git2::ObjectType::Tree) {
                return Err(StoreError::NotADirectory(directory.clone()));
            }
            self.repository
                .find_tree(entry.id())
                .map_err(self.failed("read"))?
        };

        let mut entries = Vec::new();
        match depth {
            Depth::One => {
                for entry in tree.iter() {
                    let name = String::from_utf8_lossy(entry.name_bytes()).into_owned();
                    if let Some(entry) = self.describe(directory.child(&name), &entry)? {
                        entries.push(entry);
                    }
                }
            }
            Depth::Infinity => {
                let mut failure = None;
                tree.walk(git2::TreeWalkMode::PreOrder, |prefix, entry| {
                    let name = String::from_utf8_lossy(entry.name_bytes());
                    let mut path = directory.clone();
                    for component in prefix.split('/').filter(|part| !part.is_empty()) {
                        path = path.child(component);
                    }
                    match self.describe(path.child(&name), entry) {
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

        // Byte order of the full path — not git's tree order, which sorts a directory as if its name
        // ended in `/` and so would interleave differently from what a client sorts by.
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        Ok(entries)
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
        let entry = self.entry_at(&root, path)?;
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
        self.catch_up()?;

        let indexed = self
            .index()?
            .snapshot(snapshot)?
            .ok_or(StoreError::SnapshotNotFound(snapshot))?;

        self.repository
            .find_commit(indexed.commit)
            .and_then(|commit| commit.tree())
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
        path: RootPath,
        entry: &git2::TreeEntry<'_>,
    ) -> Result<Option<Entry>, StoreError> {
        const DIRECTORY: i32 = 0o040_000;
        const EXECUTABLE: i32 = 0o100_755;
        const SYMLINK: i32 = 0o120_000;

        let mode = entry.filemode();
        let described = match mode {
            DIRECTORY => Entry {
                path,
                kind: EntryKind::Directory,
                size: None,
                mode: None,
                binary: None,
            },
            SYMLINK => Entry {
                path,
                kind: EntryKind::Symlink,
                size: Some(self.blob_size(entry.id())?),
                mode: None,
                binary: None,
            },
            _ if entry.kind() == Some(git2::ObjectType::Blob) => Entry {
                path,
                kind: EntryKind::File,
                size: Some(self.blob_size(entry.id())?),
                mode: Some(if mode == EXECUTABLE { 0o755 } else { 0o644 }),
                binary: Some(self.is_binary(entry.id())?),
            },
            // A submodule link: a Snapshot never holds one, so it is not described as anything.
            _ => return Ok(None),
        };

        Ok(Some(described))
    }

    /// A blob's size, read from its header rather than by inflating it.
    fn blob_size(&self, blob: git2::Oid) -> Result<u64, StoreError> {
        let odb = self.repository.odb().map_err(self.failed("read"))?;
        let (size, _) = odb.read_header(blob).map_err(self.failed("read"))?;

        Ok(u64::try_from(size).unwrap_or(u64::MAX))
    }

    /// Whether a blob is binary by git's rule, reading only the prefix that rule looks at where the
    /// object can be streamed.
    fn is_binary(&self, blob: git2::Oid) -> Result<bool, StoreError> {
        let odb = self.repository.odb().map_err(self.failed("read"))?;

        if let Ok((reader, _, _)) = odb.reader(blob) {
            let mut prefix = Vec::with_capacity(BINARY_PROBE);
            if reader
                .take(BINARY_PROBE as u64)
                .read_to_end(&mut prefix)
                .is_ok()
            {
                return Ok(prefix.contains(&0));
            }
        }

        let blob = self
            .repository
            .find_blob(blob)
            .map_err(self.failed("read"))?;
        let content = blob.content();

        Ok(content[..content.len().min(BINARY_PROBE)].contains(&0))
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
        collect_files(root_path, &RootPath::root(), policy, &mut walked)?;

        // Entries are applied in the index's own order, which is the byte order of the `/`-joined path —
        // not the component order `Path` compares by, where `sub/b.txt` would precede `sub.txt` while
        // the index expects the opposite. Ticket 12 measured 0.86 s in order against 22.74 s out of it.
        walked.paths.sort();

        let started = nanos_since_epoch(now);
        let mut sub_second = false;
        let paths = std::mem::take(&mut walked.paths);
        for path in &paths {
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
            let mode = mode_for(&metadata);
            let size = metadata.len();
            let mtime = metadata.modified().ok().map(nanos_since_epoch);
            sub_second |= mtime.is_some_and(|mtime| mtime % 1_000_000_000 != 0);
            let recorded = baseline_entry(baseline, path);

            // The filter: the last pass read this very size and mtime and found the content the baseline
            // holds. Any doubt — no row, a different size or mtime, a baseline that disagrees — reads.
            let vouched = match (observations.get(&path.0), mtime, recorded) {
                (Some(seen), Some(mtime), Some((oid, _)))
                    if seen.size == size && seen.mtime == mtime && seen.oid == oid =>
                {
                    Some(oid)
                }
                _ => None,
            };
            let oid = match vouched {
                Some(oid) => oid,
                None => match self.repository.blob_path(&absolute) {
                    Ok(oid) => {
                        walked.hashed += 1;
                        oid
                    }
                    Err(_) => {
                        walked.unreadable.push(absolute);
                        continue;
                    }
                },
            };

            if let Some(mtime) = mtime {
                walked
                    .observed
                    .push((path.0.clone(), Observation { size, mtime, oid }));
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
        walked.paths = paths;
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
    /// Files present but deliberately not held.
    withheld: Vec<WithheldPath>,
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
        }
    }

    /// Whether an index path from the baseline should survive this capture.
    ///
    /// A path that was walked survives because it is still there; a path beneath a directory that could
    /// not be listed survives because its absence was never observed. Everything else is gone, and a
    /// Snapshot that kept it would claim content the Working Tree no longer has.
    fn keeps(&self, path: &str) -> bool {
        // `paths` is sorted by the time the baseline is compared against it.
        self.paths
            .binary_search_by(|walked| walked.as_str().cmp(path))
            .is_ok()
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

            walked.unreadable.push(directory);
            walked.opaque.push(relative);

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

        // Built from the walk's own components rather than parsed: a name the filesystem handed over is
        // one component, whatever characters it holds.
        let rooted = here.child(name);
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => {
                walked.unreadable.push(directory.join(name));
                continue;
            }
        };

        if metadata.is_dir() {
            if policy.directory(&rooted) == Verdict::Captured {
                collect_files(root, &rooted, policy, walked)?;
            }
        } else if metadata.is_file() {
            match policy.file(&rooted, metadata.len()) {
                Verdict::Captured => walked.paths.push(WirePath(rooted.as_str().to_owned())),
                // Present, but without its content: no tree entry at all (ADR 0009), and the capture
                // names it. Ticket 48 makes the record part of the Snapshot itself.
                Verdict::TooLarge { .. } => walked.withheld.push(WithheldPath {
                    path: rooted,
                    reason: WithheldReason::TooLarge,
                }),
                Verdict::Reserved
                | Verdict::Excluded(_)
                | Verdict::Pruned { .. }
                | Verdict::Secret(_) => {}
            }
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
