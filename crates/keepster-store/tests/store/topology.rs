//! What a store holds, as a diagnostics bundle reports it (ticket 63): every Segment and Pin with its
//! Snapshots and format, how far the derived index got, and the space both take — read from refs,
//! commit messages and the index alone, without bringing the index up to date first.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use keepster_core::{CapturePolicy, ContentFormat, RootId, SegmentName, SnapshotId, Trigger};
use keepster_store::HistoryStore;

/// 2026-09-30T14:00:00Z.
const NOW: u64 = 1_790_776_800;
const HOUR: u64 = 3600;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

struct Fixture {
    directory: tempfile::TempDir,
    root: PathBuf,
    id: RootId,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path().join("project");
        fs::create_dir_all(&root).expect("the Root");

        Self {
            directory,
            root,
            id: RootId::generate(at(NOW)),
        }
    }

    fn history(&self) -> PathBuf {
        self.directory.path().join("history.git")
    }

    fn index(&self) -> PathBuf {
        self.directory.path().join("index.sqlite")
    }

    fn open(&self) -> HistoryStore {
        HistoryStore::open(self.history(), self.index()).expect("a store")
    }

    /// A Snapshot at `moment` with `notes.txt` holding `content`.
    fn capture(&self, store: &HistoryStore, moment: u64, content: &str) -> SnapshotId {
        fs::write(self.root.join("notes.txt"), content).expect("a change");
        store
            .capture(
                self.id,
                &self.root,
                &CapturePolicy::default(),
                Trigger::Manual,
                at(moment),
            )
            .expect("a Snapshot")
            .id
    }
}

fn segment(moment: u64) -> String {
    SegmentName::hourly(at(moment))
        .expect("a nameable moment")
        .as_str()
        .to_owned()
}

#[test]
fn every_segment_and_pin_is_named_with_its_snapshots_and_format() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let first = fixture.capture(&store, NOW - 2 * HOUR, "one");
    let second = fixture.capture(&store, NOW - 2 * HOUR + 60, "two");
    let third = fixture.capture(&store, NOW, "three");
    store.pin(second).expect("pinned");

    let topology = store.topology().expect("the topology");

    let segments: Vec<_> = topology
        .segments
        .iter()
        .map(|shape| {
            (
                shape.name.clone(),
                shape.snapshots,
                shape.oldest,
                shape.newest,
                shape.format,
            )
        })
        .collect();
    let current = Some(ContentFormat::CURRENT.number());
    assert_eq!(
        segments,
        vec![
            (
                segment(NOW - 2 * HOUR),
                2,
                Some(first),
                Some(second),
                current
            ),
            (segment(NOW), 1, Some(third), Some(third), current),
        ],
        "in name order, which is chronological"
    );
    assert!(
        topology.segments.iter().all(|shape| shape.foreign == 0),
        "every commit here holds a Snapshot"
    );
    assert_eq!(
        topology
            .pins
            .iter()
            .map(|pin| (pin.snapshot.clone(), pin.format))
            .collect::<Vec<_>>(),
        vec![(second.to_string(), current)]
    );
    assert!(topology.history_bytes > 0, "the repository takes space");
    assert!(topology.index_bytes > 0, "so does the index");
}

#[test]
fn the_watermarks_show_how_far_the_index_got_without_bringing_it_up_to_date() {
    let fixture = Fixture::new();
    {
        let store = fixture.open();
        fixture.capture(&store, NOW - 2 * HOUR, "one");
        fixture.capture(&store, NOW, "two");
    }
    // The store is closed before its index goes, as a daemon killed between a ref update and an index
    // write leaves it — and as Windows needs to delete an open file.
    remove_index(&fixture.index());
    let store = HistoryStore::open_existing(fixture.history(), fixture.index())
        .expect("opened")
        .expect("a store with history");

    let lagging = store.topology().expect("the topology");

    let marks = lagging.watermarks.expect("the watermarks");
    assert_eq!(
        marks
            .iter()
            .map(|mark| (mark.source.clone(), mark.indexed.clone()))
            .collect::<Vec<_>>(),
        vec![(segment(NOW - 2 * HOUR), None), (segment(NOW), None)],
        "every source is named, and none is indexed yet"
    );
    assert!(marks.iter().all(|mark| mark.tip.is_some()));
    assert_eq!(lagging.index_bytes, 0);
    assert!(
        !fixture.index().exists(),
        "reading the watermarks neither creates nor rebuilds the index"
    );

    store.catch_up().expect("caught up");
    let caught_up = store.topology().expect("the topology");

    let marks = caught_up.watermarks.expect("the watermarks");
    assert_eq!(marks.len(), 2);
    assert!(
        marks
            .iter()
            .all(|mark| mark.indexed.is_some() && mark.indexed == mark.tip),
        "a caught-up index is indexed to every tip: {marks:?}"
    );
}

fn remove_index(index: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut path = index.as_os_str().to_owned();
        path.push(suffix);
        let _ = fs::remove_file(PathBuf::from(path));
    }
}
