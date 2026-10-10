//! The derived index: which Snapshots exist, in what order, and which paths each one changed.
//!
//! Everything in this database is a **derived cache** (ticket 03). It may be deleted at any time and is
//! rebuilt from git objects alone, so nothing here is ever authoritative and nothing here is ever
//! migrated: a schema this build does not recognise, a file that is not a database and a failed
//! integrity check all mean the same thing — the index is absent — and the answer is to discard it and
//! rebuild (ADR 0010).
//!
//! Two invariants make that safe. The commit and the ref update always precede the index write, so the
//! index can only *lag* git and never name a commit that does not exist. And a per-Segment watermark —
//! the tip each Segment was indexed up to — turns "the daemon was killed" into a catch-up over exactly
//! the code path a full rebuild takes, so the rare path is the common one and cannot rot.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use keepster_core::{ChangeKind, RootPath, SnapshotId};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::history::StoreError;

/// The index schema's version, kept in SQLite's own `user_version`.
///
/// Every mismatch is a rebuild rather than a migration (ADR 0010), which is why changing the schema is
/// free: bump this and the next start rebuilds.
const SCHEMA_VERSION: i64 = 1;

/// The schema. `STRICT` so a value of the wrong type is an error rather than a silent coercion; no
/// row ids, because every table already has a natural key.
const SCHEMA: &str = "
    CREATE TABLE snapshots (
        id         TEXT    NOT NULL PRIMARY KEY,
        segment    TEXT    NOT NULL,
        commit_id  TEXT    NOT NULL,
        trigger    TEXT    NOT NULL,
        omitted    INTEGER NOT NULL,
        changed    INTEGER NOT NULL
    ) STRICT, WITHOUT ROWID;

    CREATE TABLE changes (
        path     TEXT NOT NULL,
        snapshot TEXT NOT NULL REFERENCES snapshots (id) ON DELETE CASCADE,
        kind     TEXT NOT NULL,
        PRIMARY KEY (path, snapshot)
    ) STRICT, WITHOUT ROWID;

    CREATE INDEX changes_by_snapshot ON changes (snapshot);

    CREATE TABLE watermarks (
        segment TEXT NOT NULL PRIMARY KEY,
        tip     TEXT NOT NULL
    ) STRICT, WITHOUT ROWID;
";

/// How long a connection waits for another one's write before giving up.
///
/// Reads stay unlocked by the store (ticket 06), so a read that finds the index lagging and a capture
/// that is bringing it up to date can meet here; SQLite serialises them, and this is how long the
/// second one is prepared to wait.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// One Snapshot as the index holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedSnapshot {
    pub(crate) id: SnapshotId,
    pub(crate) segment: String,
    pub(crate) commit: git2::Oid,
    pub(crate) trigger: String,
    pub(crate) omitted: u64,
    pub(crate) changed: u64,
    /// How the path a timeline was filtered by changed in this Snapshot, when it was filtered.
    pub(crate) change: Option<ChangeKind>,
}

/// One Snapshot found in git and not yet in the index.
struct Unindexed {
    id: SnapshotId,
    segment: String,
    commit: git2::Oid,
    tree: git2::Oid,
    trigger: String,
    omitted: u64,
}

/// What a commit's trailers say about the Snapshot it holds.
pub(crate) struct Trailers {
    pub(crate) id: SnapshotId,
    pub(crate) trigger: String,
    pub(crate) omitted: u64,
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
        let mode: String = connection
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
            .map_err(failed("opened"))?;
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
        let tables: i64 = connection
            .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
            .map_err(failed("read"))?;

        match (version, tables) {
            (SCHEMA_VERSION, _) => {}
            (0, 0) => {
                connection
                    .execute_batch(&format!(
                        "BEGIN; {SCHEMA} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
                    ))
                    .map_err(failed("created"))?;
            }
            _ => return Ok(None),
        }

        Ok(Some(Self {
            connection,
            path: path.to_path_buf(),
        }))
    }

    /// Bring the index up to date with the Segment refs, rebuilding it where it cannot simply catch up.
    ///
    /// A Segment whose tip still equals its watermark costs nothing; one whose watermark is an ancestor
    /// of its tip costs only the new commits. Anything else — a Segment that vanished, or one whose
    /// commits were recreated so the watermark is no longer in its chain, or a Snapshot that sorts
    /// before one already indexed — invalidates the predecessors the change rows were computed against,
    /// and the whole index is rebuilt by the same loop with every watermark treated as absent.
    pub(crate) fn catch_up(&mut self, repository: &git2::Repository) -> Result<(), StoreError> {
        let tips = segment_tips(repository).map_err(git_failed_at(repository, "read"))?;

        // A read path calls this on every request, so the common answer — nothing to do — must not
        // take the write lock.
        if self.watermarks()? == tips {
            return Ok(());
        }

        let path = self.path.clone();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(index_failed(&path, "written"))?;

        let mut rebuild = false;
        loop {
            // Read inside the write lock: a capture or another read may have caught up meanwhile.
            let marks = read_watermarks(&transaction).map_err(index_failed(&path, "read"))?;
            if marks == tips {
                break;
            }

            let invalidated = marks.iter().any(|(segment, mark)| match tips.get(segment) {
                None => true,
                Some(tip) => {
                    tip != mark && !repository.graph_descendant_of(*tip, *mark).unwrap_or(false)
                }
            });
            rebuild |= invalidated;
            if rebuild {
                transaction
                    .execute_batch(
                        "DELETE FROM changes; DELETE FROM snapshots; DELETE FROM watermarks;",
                    )
                    .map_err(index_failed(&path, "written"))?;
            }
            let marks = if rebuild { BTreeMap::new() } else { marks };

            let mut found =
                unindexed(repository, &tips, &marks).map_err(git_failed_at(repository, "read"))?;
            found.sort_by_key(|snapshot| snapshot.id);

            let newest = newest_indexed(&transaction).map_err(index_failed(&path, "read"))?;
            // A Snapshot that sorts before one already indexed changes the predecessor of the ones
            // after it, so their rows would be wrong. Only a clock that went backwards produces this,
            // and a rebuild — which starts from an empty index, so cannot meet this again — is the
            // honest answer to it.
            if let (Some((newest_id, _)), Some(first)) = (&newest, found.first()) {
                if first.id <= *newest_id {
                    rebuild = true;
                    continue;
                }
            }

            let mut previous = match newest {
                Some((_, commit)) => Some(
                    repository
                        .find_commit(commit)
                        .and_then(|commit| commit.tree())
                        .map(|tree| tree.id())
                        .map_err(git_failed_at(repository, "read"))?,
                ),
                None => None,
            };

            for snapshot in &found {
                let changes = changes_between(repository, previous, snapshot.tree)
                    .map_err(git_failed_at(repository, "read"))?;
                insert(&transaction, snapshot, &changes).map_err(index_failed(&path, "written"))?;
                previous = Some(snapshot.tree);
            }

            transaction
                .execute("DELETE FROM watermarks", [])
                .map_err(index_failed(&path, "written"))?;
            for (segment, tip) in &tips {
                transaction
                    .execute(
                        "INSERT INTO watermarks (segment, tip) VALUES (?1, ?2)",
                        params![segment, tip.to_string()],
                    )
                    .map_err(index_failed(&path, "written"))?;
            }
            break;
        }

        transaction.commit().map_err(index_failed(&path, "written"))
    }

    /// One Snapshot, if the index knows it.
    pub(crate) fn snapshot(&self, id: SnapshotId) -> Result<Option<IndexedSnapshot>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, segment, commit_id, trigger, omitted, changed, NULL
                 FROM snapshots WHERE id = ?1",
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
                    .prepare(
                        "SELECT id, segment, commit_id, trigger, omitted, changed, NULL
                         FROM snapshots
                         WHERE id < ?1 AND id >= ?2
                         ORDER BY id DESC LIMIT ?3",
                    )
                    .map_err(self.failed("read"))?;
                statement
                    .query_map(params![upper, lower, limit], indexed_snapshot)
                    .and_then(Iterator::collect::<Result<Vec<_>, _>>)
            }
            Some(path) => {
                let mut statement = self
                    .connection
                    .prepare(
                        "SELECT s.id, s.segment, s.commit_id, s.trigger, s.omitted, s.changed, c.kind
                         FROM changes c JOIN snapshots s ON s.id = c.snapshot
                         WHERE c.path = ?1 AND s.id < ?2 AND s.id >= ?3
                         ORDER BY s.id DESC LIMIT ?4",
                    )
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
    for (key, value) in found.iter() {
        match key {
            crate::history::SNAPSHOT_TRAILER => id = value.parse().ok(),
            crate::history::TRIGGER_TRAILER => trigger = Some(value.to_owned()),
            crate::history::OMITTED_TRAILER => omitted = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    Some(Trailers {
        id: id?,
        // A Snapshot from before Triggers were recorded was taken on demand: nothing else existed.
        trigger: trigger.unwrap_or_else(|| keepster_core::Trigger::Manual.as_str().to_owned()),
        omitted,
    })
}

/// How every path differs between two trees, in path order.
///
/// `None` as the earlier tree is the first Snapshot a Root ever had, against which everything was
/// added. The diff recurses, so a directory appears only through the files beneath it.
pub(crate) fn changes_between(
    repository: &git2::Repository,
    before: Option<git2::Oid>,
    after: git2::Oid,
) -> Result<Vec<(String, ChangeKind)>, git2::Error> {
    if before == Some(after) {
        return Ok(Vec::new());
    }

    let before = before.map(|id| repository.find_tree(id)).transpose()?;
    let after = repository.find_tree(after)?;
    let mut options = git2::DiffOptions::new();
    options.include_typechange(true);
    let diff = repository.diff_tree_to_tree(before.as_ref(), Some(&after), Some(&mut options))?;

    let mut changes: Vec<(String, ChangeKind)> = diff
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

            Some((path, kind))
        })
        .collect();
    changes.sort();

    Ok(changes)
}

/// Every Segment ref and the commit at its tip.
fn segment_tips(repository: &git2::Repository) -> Result<BTreeMap<String, git2::Oid>, git2::Error> {
    let mut tips = BTreeMap::new();

    for reference in repository.references_glob(&format!("{}/*", crate::history::SEGMENT_REFS))? {
        let reference = reference?;
        // A ref name that is not UTF-8 was not written by us, so it names no Segment.
        let Ok(name) = reference.name() else {
            continue;
        };
        let Some(segment) = name.rsplit('/').next().map(ToOwned::to_owned) else {
            continue;
        };
        tips.insert(segment, reference.peel_to_commit()?.id());
    }

    Ok(tips)
}

/// Every Snapshot reachable from a Segment's tip and not from its watermark.
fn unindexed(
    repository: &git2::Repository,
    tips: &BTreeMap<String, git2::Oid>,
    marks: &BTreeMap<String, git2::Oid>,
) -> Result<Vec<Unindexed>, git2::Error> {
    let mut found = Vec::new();

    for (segment, tip) in tips {
        let mark = marks.get(segment);
        if mark == Some(tip) {
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

            found.push(Unindexed {
                id: trailers.id,
                segment: segment.clone(),
                commit: commit.id(),
                tree: commit.tree_id(),
                trigger: trailers.trigger,
                omitted: trailers.omitted,
            });
        }
    }

    Ok(found)
}

fn insert(
    transaction: &rusqlite::Transaction<'_>,
    snapshot: &Unindexed,
    changes: &[(String, ChangeKind)],
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO snapshots (id, segment, commit_id, trigger, omitted, changed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            snapshot.id.to_string(),
            snapshot.segment,
            snapshot.commit.to_string(),
            snapshot.trigger,
            i64::try_from(snapshot.omitted).unwrap_or(i64::MAX),
            i64::try_from(changes.len()).unwrap_or(i64::MAX),
        ],
    )?;

    let mut statement =
        transaction.prepare("INSERT INTO changes (path, snapshot, kind) VALUES (?1, ?2, ?3)")?;
    for (path, kind) in changes {
        statement.execute(params![path, snapshot.id.to_string(), kind.as_str()])?;
    }

    Ok(())
}

fn newest_indexed(connection: &Connection) -> rusqlite::Result<Option<(SnapshotId, git2::Oid)>> {
    connection
        .query_row(
            "SELECT id, commit_id FROM snapshots ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((parse_id(row, 0)?, parse_oid(row, 1)?)),
        )
        .optional()
}

fn read_watermarks(connection: &Connection) -> rusqlite::Result<BTreeMap<String, git2::Oid>> {
    let mut statement = connection.prepare("SELECT segment, tip FROM watermarks")?;
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
        trigger: row.get(3)?,
        omitted: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
        changed: u64::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
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
pub(crate) fn discard(path: &Path) -> Result<(), StoreError> {
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
