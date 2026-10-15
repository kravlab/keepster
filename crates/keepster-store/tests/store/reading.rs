//! Reading history back: the timeline, a Snapshot's entries, one version's bytes — and the derived
//! index under them, whose two promises (rebuildable row for row from git alone, and only ever lagging
//! git) are physical facts about files on disk, so they are asserted against a real store here
//! (ticket 31, seam 2).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use keepster_core::{CapturePolicy, ChangeKind, RootId, RootPath, SnapshotId, Trigger};
use keepster_store::{
    CapturedSnapshot, Depth, EntryKind, HistoryStore, StoreError, TimelinePage, TimelineQuery,
};

/// 2026-09-30T14:37:05Z.
const MOMENT: u64 = 1_790_779_025;

fn at(epoch_seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(epoch_seconds)
}

/// A Root and its history, side by side in one temporary directory.
struct Fixture {
    directory: tempfile::TempDir,
    root: PathBuf,
    id: RootId,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path().join("project");
        fs::create_dir_all(root.join("src")).expect("a subdirectory");

        Self {
            id: RootId::generate(at(MOMENT)),
            directory,
            root,
        }
    }

    fn git(&self) -> PathBuf {
        self.directory.path().join("history.git")
    }

    fn index(&self) -> PathBuf {
        self.directory.path().join("index.sqlite")
    }

    fn store(&self) -> HistoryStore {
        HistoryStore::open(self.git(), self.index()).expect("a store")
    }

    fn write(&self, path: &str, content: &[u8]) {
        fs::write(self.root.join(path), content).expect("a file");
    }

    fn capture(&self, epoch_seconds: u64) -> CapturedSnapshot {
        let captured = self
            .store()
            .capture(
                self.id,
                &self.root,
                &CapturePolicy::default(),
                Trigger::Manual,
                at(epoch_seconds),
            )
            .expect("a Snapshot");
        assert_eq!(captured.index_lag, None, "the index kept up");
        captured
    }

    fn timeline(&self, query: TimelineQuery) -> TimelinePage {
        self.store().timeline(&query).expect("the timeline")
    }
}

fn everything() -> TimelineQuery {
    TimelineQuery {
        path: None,
        before: None,
        not_before: None,
        limit: 100,
    }
}

fn path(text: &str) -> RootPath {
    RootPath::parse(text).expect("a valid path")
}

/// Three Snapshots a minute apart: one that adds, one that modifies and adds, one that deletes.
fn three_snapshots(fixture: &Fixture) -> [CapturedSnapshot; 3] {
    fixture.write("a.txt", b"one");
    fixture.write("src/lib.rs", b"fn main() {}\n");
    let first = fixture.capture(MOMENT);

    fixture.write("a.txt", b"two");
    fixture.write("b.txt", b"new");
    let second = fixture.capture(MOMENT + 60);

    fs::remove_file(fixture.root.join("a.txt")).expect("a deletion");
    let third = fixture.capture(MOMENT + 120);

    [first, second, third]
}

#[test]
fn the_timeline_lists_snapshots_newest_first_with_what_each_changed() {
    let fixture = Fixture::new();
    let [first, second, third] = three_snapshots(&fixture);

    let page = fixture.timeline(everything());

    let ids: Vec<_> = page.snapshots.iter().map(|snapshot| snapshot.id).collect();
    assert_eq!(ids, [third.id, second.id, first.id], "newest first");
    assert!(!page.more, "and nothing beyond");

    let counts: Vec<_> = page
        .snapshots
        .iter()
        .map(|snapshot| snapshot.changed_paths)
        .collect();
    assert_eq!(
        counts,
        [1, 2, 2],
        "deleted one; modified one and added one; added two"
    );
    assert_eq!(
        [
            first.changed_paths,
            second.changed_paths,
            third.changed_paths
        ],
        [Ok(2), Ok(2), Ok(1)],
        "the capture's own answer agrees with the timeline"
    );

    for snapshot in &page.snapshots {
        assert_eq!(snapshot.trigger, "manual");
        assert_eq!(snapshot.omitted, 0);
        assert_eq!(snapshot.change, None, "no path was asked about");
        assert_eq!(snapshot.segment, "2026-09-30T14");
    }
}

#[test]
fn the_timeline_says_how_many_paths_each_snapshot_added_and_deleted() {
    // What a client needs to say "only deleted files" in words: the counts by kind, beside the total.
    let fixture = Fixture::new();
    let [first, second, third] = three_snapshots(&fixture);

    let page = fixture.timeline(everything());

    let counts: Vec<_> = page
        .snapshots
        .iter()
        .map(|snapshot| (snapshot.added_paths, snapshot.deleted_paths))
        .collect();
    assert_eq!(
        counts,
        [(0, 1), (1, 0), (2, 0)],
        "deleted one; added one beside a modification; added two"
    );
    assert_eq!(
        [first, second, third].map(|captured| (captured.added_paths, captured.deleted_paths)),
        [(Some(2), Some(0)), (Some(1), Some(0)), (Some(0), Some(1))],
        "the capture's own answer agrees with the timeline"
    );

    let filtered = fixture.timeline(TimelineQuery {
        path: Some(path("a.txt")),
        ..everything()
    });
    assert_eq!(
        filtered
            .snapshots
            .iter()
            .map(|snapshot| (snapshot.added_paths, snapshot.deleted_paths))
            .collect::<Vec<_>>(),
        [(0, 1), (1, 0), (2, 0)],
        "a filtered timeline counts the whole Snapshot, not the path asked about"
    );
}

#[test]
fn filtering_by_a_path_returns_only_the_snapshots_that_changed_it_and_how() {
    let fixture = Fixture::new();
    let [first, second, third] = three_snapshots(&fixture);

    let page = fixture.timeline(TimelineQuery {
        path: Some(path("a.txt")),
        ..everything()
    });

    let seen: Vec<_> = page
        .snapshots
        .iter()
        .map(|snapshot| (snapshot.id, snapshot.change))
        .collect();
    assert_eq!(
        seen,
        [
            (third.id, Some(ChangeKind::Deleted)),
            (second.id, Some(ChangeKind::Modified)),
            (first.id, Some(ChangeKind::Added)),
        ]
    );

    let untouched = fixture.timeline(TimelineQuery {
        path: Some(path("src/lib.rs")),
        ..everything()
    });
    assert_eq!(
        untouched
            .snapshots
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<Vec<_>>(),
        [first.id],
        "a file changed once appears once"
    );
}

#[cfg(unix)]
#[test]
fn a_permission_only_change_is_a_change_of_that_path() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    fixture.write("run.sh", b"#!/bin/sh\n");
    fixture.capture(MOMENT);
    fs::set_permissions(
        fixture.root.join("run.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("its mode");
    let chmodded = fixture.capture(MOMENT + 60);

    let page = fixture.timeline(TimelineQuery {
        path: Some(path("run.sh")),
        ..everything()
    });

    assert_eq!(page.snapshots[0].id, chmodded.id);
    assert_eq!(page.snapshots[0].change, Some(ChangeKind::Modified));
}

#[test]
fn pages_follow_one_another_by_cursor_without_gaps_or_repeats() {
    let fixture = Fixture::new();
    let mut taken = Vec::new();
    for minute in 0..5 {
        fixture.write("a.txt", format!("version {minute}").as_bytes());
        taken.push(fixture.capture(MOMENT + minute * 60).id);
    }
    taken.reverse();

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = fixture.timeline(TimelineQuery {
            before: cursor,
            limit: 2,
            ..everything()
        });
        seen.extend(page.snapshots.iter().map(|snapshot| snapshot.id));
        if !page.more {
            break;
        }
        cursor = page.snapshots.last().map(|snapshot| snapshot.id);
    }

    assert_eq!(seen, taken);
}

#[test]
fn a_cursor_naming_a_snapshot_that_no_longer_exists_still_marks_its_place() {
    // Retention thins Snapshots out from under a reader, so the cursor a client holds may name one
    // that is gone. The comparison is by value, so the boundary is still the right one (ticket 07).
    let fixture = Fixture::new();
    let [first, second, _] = three_snapshots(&fixture);
    let pruned = SnapshotId::generate(at(MOMENT + 90));

    let page = fixture.timeline(TimelineQuery {
        before: Some(pruned),
        ..everything()
    });

    let ids: Vec<_> = page.snapshots.iter().map(|snapshot| snapshot.id).collect();
    assert_eq!(ids, [second.id, first.id]);
}

#[test]
fn a_lower_bound_keeps_only_the_snapshots_at_or_after_it() {
    let fixture = Fixture::new();
    let [_, second, third] = three_snapshots(&fixture);

    let page = fixture.timeline(TimelineQuery {
        not_before: Some(SnapshotId::first_after(at(MOMENT + 30))),
        ..everything()
    });

    let ids: Vec<_> = page.snapshots.iter().map(|snapshot| snapshot.id).collect();
    assert_eq!(ids, [third.id, second.id]);
}

#[test]
fn snapshots_taken_in_one_millisecond_keep_the_order_they_were_taken_in() {
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    let mut taken: Vec<_> = (0..20).map(|_| fixture.capture(MOMENT).id).collect();
    taken.reverse();

    let page = fixture.timeline(everything());

    assert_eq!(
        page.snapshots
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<Vec<_>>(),
        taken
    );
}

#[test]
fn entries_list_a_directory_shallowly_by_default_and_in_full_on_request() {
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    fixture.write("src/lib.rs", b"fn main() {}\n");
    fixture.write("src/blob.bin", b"\x00\x01\x02");
    fs::create_dir_all(fixture.root.join("src/deep")).expect("a deeper directory");
    fixture.write("src/deep/x.txt", b"deep");
    let snapshot = fixture.capture(MOMENT).id;
    let store = fixture.store();

    let shallow = store
        .entries(snapshot, &RootPath::root(), Depth::One)
        .expect("the Root's children")
        .entries;
    let listed: Vec<_> = shallow
        .iter()
        .map(|entry| (entry.path.to_str().expect("a text path"), entry.kind))
        .collect();
    assert_eq!(
        listed,
        [("a.txt", EntryKind::File), ("src", EntryKind::Directory)]
    );

    let file = &shallow[0];
    assert_eq!(file.size, Some(3));
    assert_eq!(file.mode, Some(0o644));
    assert_eq!(file.binary, Some(false));
    let directory = &shallow[1];
    assert_eq!(
        (directory.size, directory.mode, directory.binary),
        (None, None, None)
    );

    let full = store
        .entries(snapshot, &path("src"), Depth::Infinity)
        .expect("everything under src")
        .entries;
    let listed: Vec<_> = full.iter().map(|entry| entry.path.to_lossy()).collect();
    assert_eq!(
        listed,
        ["src/blob.bin", "src/deep", "src/deep/x.txt", "src/lib.rs"]
    );
    assert_eq!(full[0].binary, Some(true), "a NUL byte makes a file binary");
}

#[test]
fn entries_of_a_path_that_is_not_a_directory_are_refused_by_name() {
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    let snapshot = fixture.capture(MOMENT).id;
    let store = fixture.store();

    assert!(matches!(
        store.entries(snapshot, &path("a.txt"), Depth::One),
        Err(StoreError::NotADirectory(_))
    ));
    assert!(matches!(
        store.entries(snapshot, &path("nope"), Depth::One),
        Err(StoreError::PathNotFound(_))
    ));
    assert!(matches!(
        store.entries(
            SnapshotId::generate(at(MOMENT)),
            &RootPath::root(),
            Depth::One
        ),
        Err(StoreError::SnapshotNotFound(_))
    ));
}

#[test]
fn a_version_reads_back_whole_or_by_range() {
    let fixture = Fixture::new();
    let body: Vec<u8> = (0..200_000u32).map(|n| (n % 251) as u8).collect();
    fixture.write("big.bin", &body);
    fixture.write("src/lib.rs", b"fn main() {}\n");
    let first = fixture.capture(MOMENT).id;
    fixture.write("big.bin", b"replaced");
    fixture.capture(MOMENT + 60);
    let store = fixture.store();

    let content = store.content(first, &path("big.bin")).expect("the version");
    assert_eq!(content.size(), body.len() as u64);

    let mut whole = Vec::new();
    content
        .read(0, content.size(), &mut |chunk| {
            whole.extend_from_slice(chunk);
            Ok(())
        })
        .expect("a whole read");
    assert_eq!(whole, body, "the old version, not the current one");

    let mut ranged = Vec::new();
    content
        .read(150_000, 10, &mut |chunk| {
            ranged.extend_from_slice(chunk);
            Ok(())
        })
        .expect("a ranged read");
    assert_eq!(ranged, &body[150_000..150_010]);

    let again = store.content(first, &path("big.bin")).expect("the version");
    assert_eq!(content.tag(), again.tag(), "the same bytes, the same tag");
    assert!(matches!(
        store.content(first, &path("src")),
        Err(StoreError::NotAFile(_))
    ));
}

/// Every row of every table in the index, in a stable order, without knowing the schema — so the
/// comparison covers a table added later without this test being taught about it.
fn dump(index: &Path) -> Vec<(String, Vec<Vec<String>>)> {
    let connection = rusqlite::Connection::open(index).expect("the index");
    let mut tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
        .expect("a query")
        .query_map([], |row| row.get(0))
        .expect("the tables")
        .collect::<Result<_, _>>()
        .expect("their names");
    tables.sort();

    tables
        .into_iter()
        .map(|table| {
            let mut statement = connection
                .prepare(&format!("SELECT * FROM \"{table}\""))
                .expect("a query");
            let columns = statement.column_count();
            let mut rows: Vec<Vec<String>> = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get_ref(column).map(|value| format!("{value:?}")))
                        .collect()
                })
                .expect("the rows")
                .collect::<Result<_, _>>()
                .expect("their values");
            rows.sort();
            (table, rows)
        })
        .collect()
}

/// Delete the index and its companions, as a person or a damaged disk might.
fn delete_index(index: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = index.as_os_str().to_os_string();
        name.push(suffix);
        let _ = fs::remove_file(PathBuf::from(name));
    }
}

/// History across two Segments with every kind of change, so a rebuild has something to get wrong.
fn varied_history(fixture: &Fixture) {
    three_snapshots(fixture);
    fixture.write("c.txt", b"next hour");
    fixture.capture(MOMENT + 3_600);
    fixture.write("src/lib.rs", b"fn main() { changed() }\n");
    fixture.capture(MOMENT + 3_660);
}

#[test]
fn deleting_the_index_and_rebuilding_from_git_reproduces_every_row() {
    let fixture = Fixture::new();
    varied_history(&fixture);
    let before = dump(&fixture.index());
    assert!(
        before.iter().any(|(_, rows)| rows.len() >= 5),
        "the history actually reached the index: {before:?}"
    );

    delete_index(&fixture.index());
    HistoryStore::recover(fixture.git(), fixture.index())
        .expect("a rebuild")
        .expect("an existing store");

    assert_eq!(dump(&fixture.index()), before, "row for row");
}

#[test]
fn an_index_that_is_not_a_database_is_rebuilt_rather_than_trusted() {
    let fixture = Fixture::new();
    varied_history(&fixture);
    let before = dump(&fixture.index());

    delete_index(&fixture.index());
    fs::write(fixture.index(), b"this is not SQLite").expect("a damaged index");
    HistoryStore::recover(fixture.git(), fixture.index())
        .expect("a rebuild")
        .expect("an existing store");

    assert_eq!(dump(&fixture.index()), before);
}

#[test]
fn a_write_killed_after_the_ref_update_leaves_the_index_lagging_and_the_watermark_repairs_it() {
    // The on-disk state of a daemon killed between the ref update and the index write is exactly "the
    // index as it was before the capture, with the capture's commit and ref in git" — because the
    // index write is the capture's last step. That state is produced here by keeping a copy of the
    // index from before the capture and putting it back after, which fabricates nothing in git.
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    let kept = fixture.capture(MOMENT);
    let saved = fixture.directory.path().join("saved.sqlite");
    fs::copy(fixture.index(), &saved).expect("a copy of the index");

    fixture.write("a.txt", b"two");
    let lost = fixture.capture(MOMENT + 60);
    let complete = dump(&fixture.index());
    delete_index(&fixture.index());
    fs::copy(&saved, fixture.index()).expect("the index as the kill left it");

    // Lagging, never ahead: every commit the index names exists, and the lost Snapshot is absent.
    let repository = git2::Repository::open_bare(fixture.git()).expect("the store");
    let connection = rusqlite::Connection::open(fixture.index()).expect("the index");
    let named: Vec<(String, String)> = connection
        .prepare("SELECT id, commit_id FROM snapshots")
        .expect("a query")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("the rows")
        .collect::<Result<_, _>>()
        .expect("their values");
    drop(connection);
    assert_eq!(named.len(), 1, "{named:?}");
    assert_eq!(named[0].0, kept.id.to_string());
    for (_, commit) in &named {
        repository
            .find_commit(git2::Oid::from_str(commit).expect("an oid"))
            .expect("the index names only commits that exist");
    }

    HistoryStore::recover(fixture.git(), fixture.index())
        .expect("a catch-up")
        .expect("an existing store");

    assert_eq!(
        dump(&fixture.index()),
        complete,
        "the watermark repaired exactly what was missing"
    );
    let ids: Vec<_> = fixture
        .timeline(everything())
        .snapshots
        .iter()
        .map(|snapshot| snapshot.id)
        .collect();
    assert_eq!(ids, [lost.id, kept.id]);
}

#[test]
fn a_clock_that_went_backwards_rebuilds_the_index_into_time_order() {
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    let later = fixture.capture(MOMENT + 3_600);
    fixture.write("a.txt", b"two");
    let earlier = fixture.capture(MOMENT);

    let ids: Vec<_> = fixture
        .timeline(everything())
        .snapshots
        .iter()
        .map(|snapshot| snapshot.id)
        .collect();
    assert_eq!(
        ids,
        [later.id, earlier.id],
        "ordered by moment, not by arrival"
    );

    let incremental = dump(&fixture.index());
    delete_index(&fixture.index());
    HistoryStore::recover(fixture.git(), fixture.index())
        .expect("a rebuild")
        .expect("an existing store");
    assert_eq!(dump(&fixture.index()), incremental);
}

#[cfg(unix)]
#[test]
fn an_incomplete_snapshot_says_so_in_the_timeline_after_a_rebuild_too() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    fixture.write("locked.txt", b"secret");
    fs::set_permissions(
        fixture.root.join("locked.txt"),
        fs::Permissions::from_mode(0o000),
    )
    .expect("its mode");
    fixture.capture(MOMENT);

    delete_index(&fixture.index());
    let page = fixture.timeline(everything());

    assert_eq!(
        page.snapshots[0].omitted, 1,
        "recorded with the Snapshot, not only in the index"
    );
}

#[test]
fn a_root_never_captured_has_no_store_to_read() {
    let fixture = Fixture::new();

    assert!(
        HistoryStore::open_existing(fixture.git(), fixture.index())
            .expect("no failure")
            .is_none()
    );
    assert!(!fixture.git().exists(), "reading created nothing");
}

#[test]
fn an_index_that_cannot_be_opened_costs_the_capture_nothing() {
    // The commit and the ref come first, so the Snapshot is durable whatever the index does; an index
    // that cannot even be opened must leave it lagging, not refuse the capture.
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    fs::create_dir_all(fixture.index()).expect("something that is not a database in the way");

    let captured = fixture
        .store()
        .capture(
            fixture.id,
            &fixture.root,
            &CapturePolicy::default(),
            Trigger::Manual,
            at(MOMENT),
        )
        .expect("the Snapshot is taken anyway");

    assert!(captured.index_lag.is_some(), "and the lag is reported");
    assert_eq!(captured.changed_paths, Ok(1));

    fs::remove_dir(fixture.index()).expect("the obstacle removed");
    let page = fixture.timeline(everything());
    assert_eq!(
        page.snapshots
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<Vec<_>>(),
        [captured.id],
        "and the next catch-up indexes it"
    );
}

#[test]
fn several_first_opens_of_a_missing_index_all_succeed() {
    // Two connections finding the same empty file must not both create the schema: the second would
    // fail on a table that now exists. Many readers at once, many times over, make the race likely.
    let fixture = Fixture::new();
    three_snapshots(&fixture);

    for _ in 0..20 {
        delete_index(&fixture.index());
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let (git, index) = (fixture.git(), fixture.index());
                std::thread::spawn(move || {
                    HistoryStore::open_existing(git, index)
                        .and_then(|store| store.expect("an existing store").timeline(&everything()))
                        .map(|page| page.snapshots.len())
                })
            })
            .collect();

        for reader in readers {
            match reader.join().expect("no panic") {
                Ok(seen) => assert_eq!(seen, 3, "every reader sees the whole history"),
                Err(error) => panic!("a first open failed: {error:?}"),
            }
        }
    }
}
