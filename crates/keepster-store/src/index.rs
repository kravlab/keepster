//! The derived index: which Snapshots exist, in what order, and which paths each one changed.
//!
//! Everything in this database is a **derived cache** (ticket 03). It may be deleted at any time and is
//! rebuilt from git objects alone, so nothing here is ever authoritative and nothing here is ever
//! migrated: a schema this build does not recognise, a file that is not a database and a failed
//! integrity check all mean the same thing — the index is absent — and the answer is to discard it and
//! rebuild (ADR 0010).
//!
//! Two invariants make that safe. The commit and the ref update always precede the index write, so the
//! index can only *lag* git and never name a commit that does not exist. And a per-source watermark —
//! the tip each Segment, and each Pin, was indexed up to — turns "the daemon was killed" into a catch-up
//! over exactly the code path a full rebuild takes, so the rare path is the common one and cannot rot.
//!
//! A Snapshot can have two copies: the commit in its Segment, and the parentless one its Pin re-created
//! (ADR 0002). Every copy is indexed, and the Snapshot is read from its Pin while it has one — so it is
//! there for as long as either copy is, and Retention never has to know which.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use keepster_core::{ChangeKind, Record, RootPath, SIDECAR, Sidecar, SnapshotId};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

use crate::history::{StoreError, mode_changes};

/// The index schema's version, kept in SQLite's own `user_version`.
///
/// Every mismatch is a rebuild rather than a migration (ADR 0010), which is why changing the schema is
/// free: bump this and the next start rebuilds.
pub const INDEX_SCHEMA_VERSION: u32 = 6;

/// The same, as SQLite reads it.
const SCHEMA_VERSION: i64 = INDEX_SCHEMA_VERSION as i64;

/// The schema. `STRICT` so a value of the wrong type is an error rather than a silent coercion; no
/// row ids, because every table already has a natural key.
const SCHEMA: &str = "
    CREATE TABLE copies (
        source     TEXT    NOT NULL,
        id         TEXT    NOT NULL,
        commit_id  TEXT    NOT NULL,
        trigger    TEXT    NOT NULL,
        omitted    INTEGER NOT NULL,
        forgotten  INTEGER NOT NULL,
        PRIMARY KEY (source, id)
    ) STRICT, WITHOUT ROWID;

    CREATE INDEX copies_by_id ON copies (id);

    CREATE UNIQUE INDEX one_chained_copy ON copies (id) WHERE source NOT LIKE 'pins/%';

    CREATE TABLE snapshots (
        id         TEXT    NOT NULL PRIMARY KEY,
        segment    TEXT    NOT NULL,
        commit_id  TEXT    NOT NULL,
        trigger    TEXT    NOT NULL,
        omitted    INTEGER NOT NULL,
        changed    INTEGER NOT NULL,
        pinned     INTEGER NOT NULL,
        forgotten  INTEGER NOT NULL
    ) STRICT, WITHOUT ROWID;

    CREATE TABLE changes (
        path     TEXT NOT NULL,
        snapshot TEXT NOT NULL REFERENCES snapshots (id) ON DELETE CASCADE,
        kind     TEXT NOT NULL,
        blob     TEXT,
        PRIMARY KEY (path, snapshot)
    ) STRICT, WITHOUT ROWID;

    CREATE INDEX changes_by_snapshot ON changes (snapshot);

    CREATE TABLE watermarks (
        source  TEXT NOT NULL PRIMARY KEY,
        tip     TEXT NOT NULL
    ) STRICT, WITHOUT ROWID;

    CREATE TABLE unreadable (
        source  TEXT NOT NULL PRIMARY KEY
    ) STRICT, WITHOUT ROWID;

    CREATE TABLE observed (
        path  TEXT    NOT NULL PRIMARY KEY,
        size  INTEGER NOT NULL,
        mtime TEXT    NOT NULL,
        oid   TEXT    NOT NULL
    ) STRICT, WITHOUT ROWID;
";

/// The two columns every read of a Snapshot row selects after `forgotten`: how many of its changes are
/// additions and how many deletions, counted from `changes` at read time so the schema needs no column
/// for them. Each query names its `snapshots` row `s`.
macro_rules! kind_counts {
    () => {
        ", (SELECT count(*) FROM changes k WHERE k.snapshot = s.id AND k.kind = 'added'),
           (SELECT count(*) FROM changes k WHERE k.snapshot = s.id AND k.kind = 'deleted')"
    };
}

/// What the last pass saw of one file, and the content it read for it: the evidence a pass compares
/// against before deciding whether to read the file again (ADR 0014).
///
/// Derived from observation rather than from git, so a rebuild from the Segments cannot restore it —
/// losing it costs one pass that reads everything, never a wrong Snapshot, because the object id is the
/// verdict and this only decides whether to compute one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Observation {
    pub(crate) size: u64,
    /// Nanoseconds since the epoch, as the filesystem reported them.
    pub(crate) mtime: i128,
    pub(crate) oid: git2::Oid,
}

/// How long a connection waits for another one's write before giving up.
///
/// Reads stay unlocked by the store (ticket 06), so a read that finds the index lagging and a capture
/// that is bringing it up to date can meet here; SQLite serialises them, and this is how long the
/// second one is prepared to wait.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the index reads a Pin from: a source of its own, beside the Segments, named so that no Segment
/// name — a calendar hour — can be mistaken for one.
const PIN_SOURCE: &str = "pins/";

/// One Snapshot as the index holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedSnapshot {
    pub(crate) id: SnapshotId,
    /// The Segment it was taken into — still its name when only its Pin is left.
    pub(crate) segment: String,
    /// The copy it is read from: its Pin's while it has one.
    pub(crate) commit: git2::Oid,
    pub(crate) pinned: bool,
    /// How many paths Forget took from it.
    pub(crate) forgotten: u64,
    pub(crate) trigger: String,
    pub(crate) omitted: u64,
    pub(crate) changed: u64,
    /// How many of the changed paths it added.
    pub(crate) added: u64,
    /// How many of the changed paths it deleted.
    pub(crate) deleted: u64,
    /// How the path a timeline was filtered by changed in this Snapshot, when it was filtered.
    pub(crate) change: Option<ChangeKind>,
}

/// How one path changed in one Snapshot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PathChange {
    /// The path, Root-relative.
    pub(crate) path: String,
    /// How.
    pub(crate) kind: ChangeKind,
    /// The content it holds from this Snapshot on: none for a path deleted, and none for one the
    /// Snapshot names without holding — Withheld, which has no tree entry (ADR 0009).
    pub(crate) blob: Option<git2::Oid>,
}

/// Everything the history says of one path: how many Snapshots changed it, the first and the last, and
/// whether its last change left it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathHistory {
    pub(crate) path: String,
    pub(crate) changes: u64,
    pub(crate) first: SnapshotId,
    pub(crate) last: SnapshotId,
    pub(crate) exists: bool,
}

/// One change as the index holds it, with the Snapshot it was in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedChange {
    pub(crate) path: String,
    pub(crate) snapshot: SnapshotId,
    pub(crate) kind: ChangeKind,
    pub(crate) blob: Option<git2::Oid>,
}

/// One copy of a Snapshot found in git and not yet in the index.
struct Unindexed {
    id: SnapshotId,
    /// The Segment it was found in, or `pins/` and its identity.
    source: String,
    commit: git2::Oid,
    trigger: String,
    omitted: u64,
    forgotten: u64,
}

/// What a commit's trailers say about the Snapshot it holds.
pub(crate) struct Trailers {
    pub(crate) id: SnapshotId,
    pub(crate) trigger: String,
    pub(crate) omitted: u64,
    pub(crate) forgotten: u64,
}

/// The open index of one History Store.
pub(crate) struct Index {
    connection: Connection,
    path: PathBuf,
}

impl Index {
    /// Open the index at `path`, discarding and recreating it if it is not one this build can use.
    ///
    /// With `verify`, SQLite's own integrity check runs as well. It is linear in the size of the file,
    /// so it is the daemon's start that pays for it, not every request.
    pub(crate) fn open(path: &Path, verify: bool) -> Result<Self, StoreError> {
        match Self::open_as_is(path, verify) {
            Ok(Some(index)) => Ok(index),
            // Foreign, from another schema or damaged: all the same thing, and none of them is worth
            // more than a rebuild (ticket 03, Phase 0). Anything else — a busy lock, a full disk — is
            // a failure of this attempt and not of the file, and discarding a database another
            // connection holds is itself a corruption vector, so it is reported instead.
            Ok(None) => Self::rebuilt(path),
            Err(StoreError::Index { ref source, .. }) if is_damage(source) => Self::rebuilt(path),
            Err(error) => Err(error),
        }
    }

    /// Discard the index at `path` and create an empty one with this build's schema in its place.
    fn rebuilt(path: &Path) -> Result<Self, StoreError> {
        discard(path)?;

        Self::open_as_is(path, false)?.ok_or_else(|| StoreError::IndexUnusable {
            path: path.to_path_buf(),
            reason: "a freshly created index does not hold the schema just written to it",
        })
    }

    /// Open the file as it is, creating the schema in an empty one, or `None` if it holds something
    /// else.
    fn open_as_is(path: &Path, verify: bool) -> Result<Option<Self>, StoreError> {
        let failed = |operation| index_failed(path, operation);

        let connection = Connection::open(path).map_err(failed("opened"))?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(failed("opened"))?;
        // WAL so a reader never blocks the writer and the writer never blocks a reader; `NORMAL` is
        // safe from corruption in WAL mode and gives up only durability, which a cache can afford
        // because the Snapshots themselves are already durable git objects (ticket 03).
        let mode = write_ahead(&connection).map_err(failed("opened"))?;
        if !mode.eq_ignore_ascii_case("wal") {
            // WAL is refused on a filesystem that cannot share memory between processes — a network
            // mount above all, where ticket 03 forbids the index anyway. Not damage: a rebuild would
            // meet the same refusal.
            return Err(StoreError::IndexUnusable {
                path: path.to_path_buf(),
                reason: "its filesystem does not support write-ahead logging",
            });
        }
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(failed("opened"))?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(failed("opened"))?;

        if verify {
            let verdict: String = connection
                .query_row("PRAGMA quick_check", [], |row| row.get(0))
                .map_err(failed("checked"))?;
            if verdict != "ok" {
                return Ok(None);
            }
        }

        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(failed("read"))?;

        // An unversioned file is either empty or being given its schema by another connection right
        // now; only a look under the write lock can tell which, so the decision is made there. Two
        // separate reads here could see the version from before another connection's commit and the
        // tables from after it, and call a healthy index foreign.
        match version {
            SCHEMA_VERSION => {}
            0 => {
                if !create_schema(&connection).map_err(failed("created"))? {
                    return Ok(None);
                }
            }
            _ => return Ok(None),
        }

        Ok(Some(Self {
            connection,
            path: path.to_path_buf(),
        }))
    }

    /// Everything the last pass observed, by path.
    pub(crate) fn observations(
        &self,
    ) -> Result<std::collections::HashMap<String, Observation>, StoreError> {
        let failed = index_failed(&self.path, "read");
        let mut statement = self
            .connection
            .prepare("SELECT path, size, mtime, oid FROM observed")
            .map_err(&failed)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(&failed)?;

        let mut observations = std::collections::HashMap::new();
        for row in rows {
            let (path, size, mtime, oid) = row.map_err(&failed)?;
            // A row this build cannot read back is evidence it cannot use, which only costs a read.
            let (Ok(size), Ok(mtime), Ok(oid)) = (
                u64::try_from(size),
                mtime.parse::<i128>(),
                git2::Oid::from_str(&oid),
            ) else {
                continue;
            };
            observations.insert(path, Observation { size, mtime, oid });
        }

        Ok(observations)
    }

    /// Make what is observed be what one pass saw, writing only the difference from `previous` — what
    /// the pass started from — so a pass over an unchanged tree writes nothing, and a path that vanished
    /// loses its row in the same pass that noticed.
    pub(crate) fn update_observations(
        &mut self,
        previous: &std::collections::HashMap<String, Observation>,
        observed: &[(String, Observation)],
    ) -> Result<(), StoreError> {
        let seen: std::collections::HashSet<&str> =
            observed.iter().map(|(path, _)| path.as_str()).collect();
        let vanished: Vec<&String> = previous
            .keys()
            .filter(|path| !seen.contains(path.as_str()))
            .collect();
        let changed: Vec<&(String, Observation)> = observed
            .iter()
            .filter(|(path, observation)| previous.get(path) != Some(observation))
            .collect();
        if vanished.is_empty() && changed.is_empty() {
            return Ok(());
        }

        let path = self.path.clone();
        let failed = index_failed(&path, "written");
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(&failed)?;
        {
            let mut delete = transaction
                .prepare("DELETE FROM observed WHERE path = ?1")
                .map_err(&failed)?;
            for path in vanished {
                delete.execute(params![path]).map_err(&failed)?;
            }
            let mut upsert = transaction
                .prepare(
                    "INSERT INTO observed (path, size, mtime, oid) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (path) DO UPDATE SET
                         size = excluded.size, mtime = excluded.mtime, oid = excluded.oid",
                )
                .map_err(&failed)?;
            for (path, observation) in changed {
                // An mtime is kept as text: SQLite's integers stop at 64 bits, and nanoseconds since the
                // epoch need more on a filesystem that dates files far from now.
                upsert
                    .execute(params![
                        path,
                        i64::try_from(observation.size).unwrap_or(i64::MAX),
                        observation.mtime.to_string(),
                        observation.oid.to_string(),
                    ])
                    .map_err(&failed)?;
            }
        }
        transaction.commit().map_err(&failed)
    }

    /// Bring the index up to date with the Segment and Pin refs, re-reading only what changed.
    ///
    /// A Segment whose tip still equals its watermark costs nothing; one whose watermark is an ancestor
    /// of its tip costs only the new commits. A Segment that vanished, or whose commits were recreated
    /// so the watermark is no longer in its chain — what Retention does to every Segment it thins —
    /// loses its rows and is read again from its root (ticket 51). Each Snapshot's changes are counted
    /// against the Snapshot before it in identity order, across Segments, so besides every row written
    /// here the one after each row written or removed is counted again: it has a new predecessor. That
    /// also places a Snapshot sorting before ones already indexed — a clock that went backwards —
    /// without rebuilding anything else. A Pin is a source of one commit, so pinning and unpinning are
    /// caught up the same way: a Snapshot goes only once neither copy is left.
    pub(crate) fn catch_up(&mut self, repository: &git2::Repository) -> Result<(), StoreError> {
        // A read path calls this on every request, so the common answer — nothing to do — must not
        // take the write lock.
        let glance = source_tips(repository).map_err(git_failed_at(repository, "read"))?;
        if self.watermarks()? == glance {
            return Ok(());
        }

        let path = self.path.clone();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(index_failed(&path, "written"))?;

        // The tips are read again *under* the write lock, never reused from the glance above. A ref
        // only ever moves before its index write, and every writer reads its tips under this lock, so
        // tips read here are at least as new as any watermark already committed. Tips read before the
        // lock could be older than the watermarks a capture wrote meanwhile — which would look like a
        // rewritten Segment and read it again back to the older state.
        let tips = source_tips(repository).map_err(git_failed_at(repository, "read"))?;
        // Read inside the write lock: a capture or another read may have caught up meanwhile.
        let marks = read_watermarks(&transaction).map_err(index_failed(&path, "read"))?;
        if marks == tips {
            return transaction.commit().map_err(index_failed(&path, "written"));
        }

        let mut from = marks.clone();
        let mut touched: BTreeSet<SnapshotId> = BTreeSet::new();
        for (source, mark) in &marks {
            let rewritten = match tips.get(source) {
                None => true,
                Some(tip) => {
                    tip != mark && !repository.graph_descendant_of(*tip, *mark).unwrap_or(false)
                }
            };
            if rewritten {
                touched.extend(
                    remove_source(&transaction, source).map_err(index_failed(&path, "written"))?,
                );
                from.remove(source);
            }
        }

        let (found, unreadable) =
            unindexed(repository, &tips, &from).map_err(git_failed_at(repository, "read"))?;
        // Whether a source is readable is learned again whenever it moved, so a source rewritten,
        // dropped or read anew loses what was known of it first.
        for source in marks.keys().chain(tips.keys()) {
            if marks.get(source) != tips.get(source) {
                transaction
                    .execute("DELETE FROM unreadable WHERE source = ?1", [source])
                    .map_err(index_failed(&path, "written"))?;
            }
        }
        for source in &unreadable {
            transaction
                .execute("INSERT INTO unreadable (source) VALUES (?1)", [source])
                .map_err(index_failed(&path, "written"))?;
        }
        for copy in &found {
            insert(&transaction, copy).map_err(index_failed(&path, "written"))?;
            touched.insert(copy.id);
        }
        let mut recount: BTreeSet<SnapshotId> = BTreeSet::new();
        for id in &touched {
            if resolve(&transaction, *id).map_err(index_failed(&path, "written"))? {
                recount.insert(*id);
            }
        }
        for id in &touched {
            if let Some(next) =
                successor(&transaction, *id).map_err(index_failed(&path, "written"))?
            {
                recount.insert(next);
            }
        }
        for id in recount {
            recount_changes(repository, &transaction, &path, id)?;
        }

        transaction
            .execute("DELETE FROM watermarks", [])
            .map_err(index_failed(&path, "written"))?;
        for (source, tip) in &tips {
            transaction
                .execute(
                    "INSERT INTO watermarks (source, tip) VALUES (?1, ?2)",
                    params![source, tip.to_string()],
                )
                .map_err(index_failed(&path, "written"))?;
        }

        transaction.commit().map_err(index_failed(&path, "written"))
    }

    /// One Snapshot, if the index knows it.
    pub(crate) fn snapshot(&self, id: SnapshotId) -> Result<Option<IndexedSnapshot>, StoreError> {
        self.connection
            .query_row(
                concat!(
                    "SELECT id, segment, commit_id, trigger, omitted, changed, NULL, pinned, forgotten",
                    kind_counts!(),
                    " FROM snapshots s WHERE id = ?1"
                ),
                [id.to_string()],
                indexed_snapshot,
            )
            .optional()
            .map_err(self.failed("read"))
    }

    /// The newest Snapshot strictly older than `before` and no older than `not_before`, newest first,
    /// at most `limit` of them — filtered, with `path`, to the Snapshots that changed it.
    ///
    /// Bounds are identities rather than moments: a ULID orders by its millisecond first, so a
    /// cursor, a moment and a pruned Snapshot's identity are all just a place in one order, and a
    /// cursor naming a Snapshot that no longer exists still marks the right boundary (ticket 07).
    pub(crate) fn timeline(
        &self,
        path: Option<&RootPath>,
        before: Option<SnapshotId>,
        not_before: Option<SnapshotId>,
        limit: usize,
    ) -> Result<Vec<IndexedSnapshot>, StoreError> {
        // Crockford base32 has no character above `Z`, so `~` bounds every identity from above and the
        // empty string from below; the query then has one shape whatever the caller leaves out.
        let upper = before.map_or_else(|| "~".to_owned(), |id| id.to_string());
        let lower = not_before.map_or_else(String::new, |id| id.to_string());
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);

        let rows = match path {
            None => {
                let mut statement = self
                    .connection
                    .prepare(concat!(
                        "SELECT id, segment, commit_id, trigger, omitted, changed, NULL, pinned,
                                forgotten",
                        kind_counts!(),
                        " FROM snapshots s
                         WHERE id < ?1 AND id >= ?2
                         ORDER BY id DESC LIMIT ?3"
                    ))
                    .map_err(self.failed("read"))?;
                statement
                    .query_map(params![upper, lower, limit], indexed_snapshot)
                    .and_then(Iterator::collect::<Result<Vec<_>, _>>)
            }
            Some(path) => {
                let mut statement = self
                    .connection
                    .prepare(concat!(
                        "SELECT s.id, s.segment, s.commit_id, s.trigger, s.omitted, s.changed, c.kind,
                                s.pinned, s.forgotten",
                        kind_counts!(),
                        " FROM changes c JOIN snapshots s ON s.id = c.snapshot
                         WHERE c.path = ?1 AND s.id < ?2 AND s.id >= ?3
                         ORDER BY s.id DESC LIMIT ?4"
                    ))
                    .map_err(self.failed("read"))?;
                statement
                    .query_map(
                        params![path.as_str(), upper, lower, limit],
                        indexed_snapshot,
                    )
                    .and_then(Iterator::collect::<Result<Vec<_>, _>>)
            }
        };

        rows.map_err(self.failed("read"))
    }

    /// Every pinned Snapshot, newest first.
    pub(crate) fn pinned(&self) -> Result<Vec<IndexedSnapshot>, StoreError> {
        let mut statement = self
            .connection
            .prepare(concat!(
                "SELECT id, segment, commit_id, trigger, omitted, changed, NULL, pinned, forgotten",
                kind_counts!(),
                " FROM snapshots s WHERE pinned = 1 ORDER BY id DESC"
            ))
            .map_err(self.failed("read"))?;

        statement
            .query_map([], indexed_snapshot)
            .and_then(Iterator::collect::<Result<Vec<_>, _>>)
            .map_err(self.failed("read"))
    }

    /// Every Segment and Pin in a content format this build cannot read, by the source the index names
    /// it: unreadable, and reported wherever its Snapshots would have been listed (ADR 0010).
    pub(crate) fn unreadable(&self) -> Result<Vec<String>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT source FROM unreadable ORDER BY source")
            .map_err(self.failed("read"))?;

        statement
            .query_map([], |row| row.get(0))
            .and_then(Iterator::collect::<Result<Vec<_>, _>>)
            .map_err(self.failed("read"))
    }

    /// The history of every path `keep` keeps, in path order, at most `limit` of them — and whether there
    /// were more.
    ///
    /// The paths are read once and filtered here, where case folds as Unicode folds it; each one kept
    /// costs one lookup on the path's own rows.
    pub(crate) fn path_histories(
        &self,
        keep: &dyn Fn(&str) -> bool,
        limit: usize,
    ) -> Result<(Vec<PathHistory>, bool), StoreError> {
        let read = || -> rusqlite::Result<(Vec<PathHistory>, bool)> {
            let mut paths = self
                .connection
                .prepare("SELECT DISTINCT path FROM changes ORDER BY path")?;
            let kept: Vec<String> = paths
                .query_map([], |row| row.get::<_, String>(0))?
                .filter(|path| path.as_ref().map_or(true, |path| keep(path)))
                .take(limit.saturating_add(1))
                .collect::<rusqlite::Result<_>>()?;
            let more = kept.len() > limit;

            let mut history = self.connection.prepare(
                "SELECT COUNT(*), MIN(snapshot), MAX(snapshot),
                        (SELECT kind FROM changes AS latest WHERE latest.path = ?1
                         ORDER BY latest.snapshot DESC LIMIT 1)
                 FROM changes WHERE path = ?1",
            )?;
            let mut found = Vec::with_capacity(kept.len().min(limit));
            for path in kept.into_iter().take(limit) {
                let entry = history.query_row([&path], |row| {
                    let kind: String = row.get(3)?;
                    Ok(PathHistory {
                        changes: u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                        first: parse_id(row, 1)?,
                        last: parse_id(row, 2)?,
                        exists: ChangeKind::parse(&kind) != Some(ChangeKind::Deleted),
                        path: path.clone(),
                    })
                })?;
                found.push(entry);
            }

            Ok((found, more))
        };

        read().map_err(self.failed("read"))
    }

    /// Every change to a path at or beneath `under` — every path when it is the Root — in path order and,
    /// for each path, in the order of its Snapshots.
    pub(crate) fn changes_under(&self, under: &RootPath) -> Result<Vec<IndexedChange>, StoreError> {
        let read = || -> rusqlite::Result<Vec<IndexedChange>> {
            let row = |row: &rusqlite::Row<'_>| {
                let kind: String = row.get(2)?;
                let blob: Option<String> = row.get(3)?;
                Ok(IndexedChange {
                    path: row.get(0)?,
                    snapshot: parse_id(row, 1)?,
                    kind: ChangeKind::parse(&kind).unwrap_or(ChangeKind::Modified),
                    blob: blob.and_then(|blob| git2::Oid::from_str(&blob).ok()),
                })
            };
            if under.is_root() {
                let mut statement = self.connection.prepare(
                    "SELECT path, snapshot, kind, blob FROM changes ORDER BY path, snapshot",
                )?;
                return statement.query_map([], row)?.collect();
            }
            // The path itself, and every path beneath it: those sort between `path/` and `path0`, `0`
            // being the character after `/`.
            let mut statement = self.connection.prepare(
                "SELECT path, snapshot, kind, blob FROM changes
                 WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')
                 ORDER BY path, snapshot",
            )?;
            statement.query_map([under.as_str()], row)?.collect()
        };

        read().map_err(self.failed("read"))
    }

    /// Every Snapshot's identity, oldest first.
    pub(crate) fn snapshot_ids(&self) -> Result<Vec<SnapshotId>, StoreError> {
        let read = || -> rusqlite::Result<Vec<SnapshotId>> {
            let mut statement = self
                .connection
                .prepare("SELECT id FROM snapshots ORDER BY id")?;
            statement.query_map([], |row| parse_id(row, 0))?.collect()
        };

        read().map_err(self.failed("read"))
    }

    fn watermarks(&self) -> Result<BTreeMap<String, git2::Oid>, StoreError> {
        read_watermarks(&self.connection).map_err(self.failed("read"))
    }

    fn failed<'a>(
        &'a self,
        operation: &'static str,
    ) -> impl Fn(rusqlite::Error) -> StoreError + 'a {
        index_failed(&self.path, operation)
    }
}

/// What a commit's message says about the Snapshot it holds, or `None` for a commit this product did
/// not write.
pub(crate) fn trailers(message: &str) -> Option<Trailers> {
    let found = git2::message_trailers_strs(message).ok()?;

    let mut id = None;
    let mut trigger = None;
    let mut omitted = 0;
    let mut forgotten = 0;
    for (key, value) in found.iter() {
        match key {
            crate::history::SNAPSHOT_TRAILER => id = value.parse().ok(),
            crate::history::TRIGGER_TRAILER => trigger = Some(value.to_owned()),
            crate::history::OMITTED_TRAILER => omitted = value.parse().unwrap_or(0),
            crate::history::FORGOTTEN_TRAILER => forgotten = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    Some(Trailers {
        id: id?,
        // A Snapshot from before Triggers were recorded was taken on demand: nothing else existed.
        trigger: trigger.unwrap_or_else(|| keepster_core::Trigger::Manual.as_str().to_owned()),
        omitted,
        forgotten,
    })
}

/// How every path differs between two trees, in path order.
///
/// `None` as the earlier tree is the first Snapshot a Root ever had, against which everything was
/// added. The diff recurses, so a directory appears only through the files beneath it — or by its own
/// mode, when that is all that changed there (ticket 50).
///
/// The sidecar is not a path of the Root, so it never appears itself; what it says does. A path that
/// became Withheld with nothing else changing — a new pipe, a file grown past its cap — or whose record
/// of being incomplete came or went is a change of that path, or a Snapshot written for it alone would
/// look empty (ticket 48).
pub(crate) fn changes_between(
    repository: &git2::Repository,
    before: Option<git2::Oid>,
    after: git2::Oid,
) -> Result<Vec<PathChange>, git2::Error> {
    if before == Some(after) {
        return Ok(Vec::new());
    }

    let before = before.map(|id| repository.find_tree(id)).transpose()?;
    let after = repository.find_tree(after)?;
    let mut options = git2::DiffOptions::new();
    options.include_typechange(true);
    let diff = repository.diff_tree_to_tree(before.as_ref(), Some(&after), Some(&mut options))?;

    let mut sidecar_changed = false;
    let mut changes: Vec<PathChange> = diff
        .deltas()
        .filter_map(|delta| {
            let kind = match delta.status() {
                git2::Delta::Added => ChangeKind::Added,
                git2::Delta::Deleted => ChangeKind::Deleted,
                git2::Delta::Modified | git2::Delta::Typechange => ChangeKind::Modified,
                _ => return None,
            };
            let file = match kind {
                ChangeKind::Deleted => delta.old_file(),
                _ => delta.new_file(),
            };
            let path = String::from_utf8_lossy(file.path_bytes()?).into_owned();
            if path == SIDECAR {
                sidecar_changed = true;
                return None;
            }
            let blob = (kind != ChangeKind::Deleted).then(|| file.id());

            Some(PathChange { path, kind, blob })
        })
        .collect();
    if sidecar_changed {
        let changed: BTreeSet<String> = changes.iter().map(|change| change.path.clone()).collect();
        changes.extend(
            record_changes(repository, before.as_ref(), &after)
                .into_iter()
                .filter(|(path, _)| !changed.contains(path))
                .map(|(path, kind)| {
                    // What the path holds, if its record changed with its content in place — a mode, an
                    // incomplete read keeping the version before; a Withheld path holds nothing.
                    let blob = (kind != ChangeKind::Deleted)
                        .then(|| after.get_path(Path::new(&path)).ok())
                        .flatten()
                        .filter(|entry| entry.kind() == Some(git2::ObjectType::Blob))
                        .map(|entry| entry.id());
                    PathChange { path, kind, blob }
                }),
        );
    }
    changes.sort();

    Ok(changes)
}

/// The paths whose sidecar records differ between two trees, and how: a path that became Withheld was
/// added to what the Snapshot names, one that stopped being Withheld went from it, and one whose
/// incomplete record came, went or changed was modified — as was one whose mode changed, with nothing
/// else (ticket 50).
fn record_changes(
    repository: &git2::Repository,
    before: Option<&git2::Tree<'_>>,
    after: &git2::Tree<'_>,
) -> Vec<(String, ChangeKind)> {
    let sidecar = |tree: Option<&git2::Tree<'_>>| -> Sidecar {
        tree.and_then(|tree| tree.get_name(SIDECAR))
            .and_then(|entry| repository.find_blob(entry.id()).ok())
            .and_then(|blob| Sidecar::parse(blob.content()).ok())
            .map(|parsed| parsed.sidecar)
            .unwrap_or_default()
    };
    // What each path is recorded as, a mode aside: a mode is compared as the mode it makes, which a
    // changed header can change with no record changing at all.
    let facts = |sidecar: &Sidecar| -> BTreeMap<String, Vec<Record>> {
        let mut facts: BTreeMap<String, Vec<Record>> = BTreeMap::new();
        for record in &sidecar.records {
            if !matches!(record, Record::Mode { .. }) {
                facts
                    .entry(record.path().as_str().to_owned())
                    .or_default()
                    .push(record.clone());
            }
        }
        facts
    };
    let (was_sidecar, is_sidecar) = (sidecar(before), sidecar(Some(after)));
    let (was, is) = (facts(&was_sidecar), facts(&is_sidecar));
    let withheld = |records: &[Record]| {
        records
            .iter()
            .any(|record| matches!(record, Record::Withheld { .. }))
    };

    let mut changes = BTreeMap::new();
    for (path, records) in &is {
        match was.get(path) {
            None if withheld(records) => {
                changes.insert(path.clone(), ChangeKind::Added);
            }
            Some(previous) if previous == records => {}
            _ => {
                changes.insert(path.clone(), ChangeKind::Modified);
            }
        }
    }
    for (path, records) in &was {
        if !is.contains_key(path) {
            let kind = if withheld(records) {
                ChangeKind::Deleted
            } else {
                ChangeKind::Modified
            };
            changes.insert(path.clone(), kind);
        }
    }
    if let Some(before) = before {
        for change in mode_changes(before, &was_sidecar.modes(), after, &is_sidecar.modes()) {
            changes
                .entry(change.path.as_str().to_owned())
                .or_insert(ChangeKind::Modified);
        }
    }

    changes.into_iter().collect()
}

/// Every Segment ref and every Pin ref, by the source the index names it, and the commit at its tip.
pub(crate) fn source_tips(
    repository: &git2::Repository,
) -> Result<BTreeMap<String, git2::Oid>, git2::Error> {
    let mut tips = BTreeMap::new();

    for (refs, prefix) in [
        (crate::history::SEGMENT_REFS, ""),
        (crate::history::PIN_REFS, PIN_SOURCE),
    ] {
        for reference in repository.references_glob(&format!("{refs}/*"))? {
            let reference = reference?;
            // A ref name that is not UTF-8 was not written by us, so it names no Segment or Pin.
            let Ok(name) = reference.name() else {
                continue;
            };
            let Some(leaf) = name.rsplit('/').next() else {
                continue;
            };
            let tip = if prefix.is_empty() {
                reference.peel_to_commit()?.id()
            } else {
                // A Pin is a leaf naming one commit (ADR 0002): a ref nested deeper, or one that names
                // something else, was not written by us — and must not cost every read of the store.
                match reference.peel_to_commit() {
                    Ok(commit) if name.len() == refs.len() + 1 + leaf.len() => commit.id(),
                    _ => continue,
                }
            };
            tips.insert(format!("{prefix}{leaf}"), tip);
        }
    }

    Ok(tips)
}

/// Every Snapshot reachable from a Segment's tip and not from its watermark — and every source moved
/// since that this build cannot read.
fn unindexed(
    repository: &git2::Repository,
    tips: &BTreeMap<String, git2::Oid>,
    marks: &BTreeMap<String, git2::Oid>,
) -> Result<(Vec<Unindexed>, Vec<String>), git2::Error> {
    let mut found = Vec::new();
    let mut unreadable = Vec::new();

    for (source, tip) in tips {
        let mark = marks.get(source);
        if mark == Some(tip) {
            continue;
        }
        // A Segment or Pin in a format newer than this build is opaque: indexing it would put
        // Snapshots in the timeline that nothing here can read (ADR 0010).
        let readable = crate::format::of(repository, *tip)?
            .is_some_and(keepster_core::ContentFormat::is_readable);
        if !readable {
            unreadable.push(source.clone());
            continue;
        }

        let mut walk = repository.revwalk()?;
        walk.push(*tip)?;
        if let Some(mark) = mark {
            walk.hide(*mark)?;
        }

        for commit in walk {
            let commit = repository.find_commit(commit?)?;
            // A commit without our trailer is not a Snapshot, and inventing an identity for it would
            // put something in the timeline no capture took.
            let Some(trailers) = commit.message().ok().and_then(trailers) else {
                continue;
            };
            // A Pin is one commit, and only of the Snapshot its ref names: anything else under
            // `pins/` was not written by us.
            let foreign_pin = source.strip_prefix(PIN_SOURCE).is_some_and(|pinned| {
                pinned != trailers.id.to_string() || commit.parent_count() != 0
            });
            if foreign_pin {
                continue;
            }

            found.push(Unindexed {
                id: trailers.id,
                source: source.clone(),
                commit: commit.id(),
                trigger: trailers.trigger,
                omitted: trailers.omitted,
                forgotten: trailers.forgotten,
            });
        }
    }

    Ok((found, unreadable))
}

/// Index one copy of a Snapshot. A source read again lost its copies first, so an identity already in
/// another Segment is two chained commits claiming one Snapshot, and refused; a Pin is a copy of its own.
fn insert(transaction: &rusqlite::Transaction<'_>, copy: &Unindexed) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO copies (source, id, commit_id, trigger, omitted, forgotten)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            copy.source,
            copy.id.to_string(),
            copy.commit.to_string(),
            copy.trigger,
            i64::try_from(copy.omitted).unwrap_or(i64::MAX),
            i64::try_from(copy.forgotten).unwrap_or(i64::MAX),
        ],
    )?;

    Ok(())
}

/// Drop every copy `source` held, returning the identities they were of.
fn remove_source(
    transaction: &rusqlite::Transaction<'_>,
    source: &str,
) -> rusqlite::Result<Vec<SnapshotId>> {
    let mut statement = transaction.prepare("SELECT id FROM copies WHERE source = ?1")?;
    let ids = statement
        .query_map([source], |row| parse_id(row, 0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    transaction.execute("DELETE FROM copies WHERE source = ?1", [source])?;

    Ok(ids)
}

/// Make the Snapshot `id` what its copies say: read from its Pin while it has one, from its Segment
/// otherwise, and gone once neither is left. Says whether it is there to be counted.
fn resolve(transaction: &rusqlite::Transaction<'_>, id: SnapshotId) -> rusqlite::Result<bool> {
    let mut statement = transaction.prepare(
        "SELECT source, commit_id, trigger, omitted, forgotten FROM copies WHERE id = ?1
         ORDER BY source LIKE 'pins/%' DESC",
    )?;
    let copies = statement
        .query_map([id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let Some((source, commit, trigger, omitted, forgotten)) = copies.first() else {
        transaction.execute("DELETE FROM snapshots WHERE id = ?1", [id.to_string()])?;
        return Ok(false);
    };

    let pinned = source.starts_with(PIN_SOURCE);
    let segment = match copies
        .iter()
        .find(|(source, ..)| !source.starts_with(PIN_SOURCE))
    {
        Some((chained, ..)) => chained.clone(),
        // Only the Pin is left: the Segment it was taken into is still the one its moment names, every
        // Segment this store writes being an hour (ADR 0001).
        None => keepster_core::SegmentName::hourly(id.moment())
            .map(|name| name.as_str().to_owned())
            .unwrap_or_default(),
    };
    transaction.execute(
        "INSERT INTO snapshots (id, segment, commit_id, trigger, omitted, changed, pinned, forgotten)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7)
         ON CONFLICT (id) DO UPDATE SET
             segment = excluded.segment, commit_id = excluded.commit_id,
             trigger = excluded.trigger, omitted = excluded.omitted, pinned = excluded.pinned,
             forgotten = excluded.forgotten",
        params![id.to_string(), segment, commit, trigger, omitted, pinned, forgotten],
    )?;

    Ok(true)
}

/// The first indexed Snapshot after `id`.
fn successor(connection: &Connection, id: SnapshotId) -> rusqlite::Result<Option<SnapshotId>> {
    connection
        .query_row(
            "SELECT id FROM snapshots WHERE id > ?1 ORDER BY id LIMIT 1",
            [id.to_string()],
            |row| parse_id(row, 0),
        )
        .optional()
}

/// Count again what the Snapshot `id` changed, against the indexed Snapshot before it — or against
/// nothing, when it is the first.
fn recount_changes(
    repository: &git2::Repository,
    transaction: &rusqlite::Transaction<'_>,
    path: &Path,
    id: SnapshotId,
) -> Result<(), StoreError> {
    let tree_of = |commit: git2::Oid| {
        repository
            .find_commit(commit)
            .map(|commit| commit.tree_id())
            .map_err(git_failed_at(repository, "read"))
    };
    let commit_of = |query: &str| {
        transaction
            .query_row(query, [id.to_string()], |row| parse_oid(row, 0))
            .optional()
            .map_err(index_failed(path, "read"))
    };

    let Some(commit) = commit_of("SELECT commit_id FROM snapshots WHERE id = ?1")? else {
        return Ok(());
    };
    let previous =
        commit_of("SELECT commit_id FROM snapshots WHERE id < ?1 ORDER BY id DESC LIMIT 1")?
            .map(tree_of)
            .transpose()?;
    let changes = changes_between(repository, previous, tree_of(commit)?)
        .map_err(git_failed_at(repository, "read"))?;

    let written = || -> rusqlite::Result<()> {
        transaction.execute("DELETE FROM changes WHERE snapshot = ?1", [id.to_string()])?;
        transaction.execute(
            "UPDATE snapshots SET changed = ?2 WHERE id = ?1",
            params![
                id.to_string(),
                i64::try_from(changes.len()).unwrap_or(i64::MAX)
            ],
        )?;
        let mut statement = transaction
            .prepare("INSERT INTO changes (path, snapshot, kind, blob) VALUES (?1, ?2, ?3, ?4)")?;
        for change in &changes {
            statement.execute(params![
                change.path,
                id.to_string(),
                change.kind.as_str(),
                change.blob.map(|blob| blob.to_string())
            ])?;
        }
        Ok(())
    };

    written().map_err(index_failed(path, "written"))
}

/// The watermarks of the index at `path` as they stand, or `None` when there is no index there.
///
/// For a report, so it changes nothing: the file is opened read-only, never created, and an index this
/// build would rebuild — another schema, a damaged file — is reported as unusable instead of being
/// discarded, which would destroy the very evidence the report is for.
pub(crate) fn watermarks_at(
    path: &Path,
) -> Result<Option<BTreeMap<String, git2::Oid>>, StoreError> {
    if !path.exists() {
        return Ok(None);
    }
    let failed = |operation| index_failed(path, operation);
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(failed("opened"))?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(failed("opened"))?;
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(failed("read"))?;
    if version != SCHEMA_VERSION {
        return Err(StoreError::IndexUnusable {
            path: path.to_path_buf(),
            reason: "it holds another schema than this build's, so the daemon rebuilds it",
        });
    }

    read_watermarks(&connection)
        .map(Some)
        .map_err(failed("read"))
}

fn read_watermarks(connection: &Connection) -> rusqlite::Result<BTreeMap<String, git2::Oid>> {
    let mut statement = connection.prepare("SELECT source, tip FROM watermarks")?;
    statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, parse_oid(row, 1)?)))?
        .collect()
}

fn indexed_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndexedSnapshot> {
    let change: Option<String> = row.get(6)?;

    Ok(IndexedSnapshot {
        id: parse_id(row, 0)?,
        segment: row.get(1)?,
        commit: parse_oid(row, 2)?,
        pinned: row.get(7)?,
        forgotten: u64::try_from(row.get::<_, i64>(8)?).unwrap_or(0),
        trigger: row.get(3)?,
        omitted: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
        changed: u64::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
        added: u64::try_from(row.get::<_, i64>(9)?).unwrap_or(0),
        deleted: u64::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
        change: change.as_deref().and_then(ChangeKind::parse),
    })
}

fn parse_id(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<SnapshotId> {
    let text: String = row.get(column)?;
    text.parse().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn parse_oid(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<git2::Oid> {
    let text: String = row.get(column)?;
    git2::Oid::from_str(&text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

/// Put the database in WAL mode, returning the mode it is in afterwards.
///
/// The mode is persistent, so an existing index is only asked — which takes no lock. Switching a new
/// file needs an exclusive lock, and SQLite does not always run the busy handler for that upgrade: two
/// first opens of one fresh file can see `SQLITE_BUSY` at once, however long the busy timeout. The
/// switch is therefore retried briefly, for about as long as the busy timeout would have waited.
fn write_ahead(connection: &Connection) -> rusqlite::Result<String> {
    const ATTEMPTS: u32 = 100;
    const PAUSE: Duration = Duration::from_millis(50);

    let mode: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
        return Ok(mode);
    }

    let mut attempt = 0;
    loop {
        match connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0)) {
            Err(error)
                if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy)
                    && attempt < ATTEMPTS =>
            {
                attempt += 1;
                std::thread::sleep(PAUSE);
            }
            answer => return answer,
        }
    }
}

/// Create the schema in an empty database, unless another connection has just done so.
///
/// Two first opens of a fresh file can both find it empty; the check is therefore repeated under the
/// write lock, so the second sees the first's schema instead of failing on a table that exists.
/// Returns whether the file now holds this build's schema.
fn create_schema(connection: &Connection) -> rusqlite::Result<bool> {
    connection.execute_batch("BEGIN IMMEDIATE")?;

    let created = (|| {
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let tables: i64 =
            connection.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;

        match (version, tables) {
            (SCHEMA_VERSION, _) => Ok(true),
            (0, 0) => {
                connection
                    .execute_batch(&format!("{SCHEMA} PRAGMA user_version = {SCHEMA_VERSION};"))?;
                Ok(true)
            }
            _ => Ok(false),
        }
    })();

    match created {
        Ok(created) => {
            connection.execute_batch("COMMIT")?;
            Ok(created)
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// Whether a failure says the file is not a usable database, rather than that this attempt failed.
fn is_damage(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt)
    )
}

/// Delete an index and its two companion files together.
///
/// Deleting some of the set but not the others is itself a corruption vector (ticket 03, Phase 1), so
/// the three are always removed as one, and a file that is already gone is not an error.
pub fn discard(path: &Path) -> Result<(), StoreError> {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);

        match fs::remove_file(PathBuf::from(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(StoreError::Discard {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
    }

    Ok(())
}

fn index_failed<'a>(
    path: &'a Path,
    operation: &'static str,
) -> impl Fn(rusqlite::Error) -> StoreError + 'a {
    move |source| StoreError::Index {
        path: path.to_path_buf(),
        operation,
        source,
    }
}

fn git_failed_at<'a>(
    repository: &'a git2::Repository,
    operation: &'static str,
) -> impl Fn(git2::Error) -> StoreError + 'a {
    move |source| StoreError::Git {
        path: repository.path().to_path_buf(),
        operation,
        source,
    }
}
