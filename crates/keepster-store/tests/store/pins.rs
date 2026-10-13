//! Pins against a real store (ticket 53, ADR 0002): a pinned Snapshot is re-created as a parentless
//! commit sharing its tree, so it costs one commit object, survives Retention, a Maintenance Pass and a
//! rebuilt index, and is the copy the index reads the Snapshot from.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use keepster_core::{CapturePolicy, Ladder, Newest, RootId, RootPath, SnapshotId, Trigger};
use keepster_store::{Git, HistoryStore, SnapshotSummary, StoreError, TimelineQuery};

/// 2026-09-30T14:00:00Z.
const NOW: u64 = 1_790_776_800;
const DAY: u64 = 24 * 3600;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// The machine's own `git`, found as the daemon finds it.
fn git() -> Git {
    #[cfg(windows)]
    let candidates = keepster_service::git_for_windows();
    #[cfg(not(windows))]
    let candidates = keepster_core::git_candidates(
        if cfg!(target_os = "macos") {
            keepster_core::Platform::MacOs
        } else {
            keepster_core::Platform::Linux
        },
        std::env::home_dir().as_deref(),
        None,
    );

    Git::locate(None, &candidates, None).expect("a git on the machine running the tests")
}

struct Fixture {
    directory: tempfile::TempDir,
    root: PathBuf,
    store: HistoryStore,
    id: RootId,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path().join("project");
        fs::create_dir_all(&root).expect("the Root");
        let store = Self::open(&directory);

        Self {
            directory,
            root,
            store,
            id: RootId::generate(at(NOW)),
        }
    }

    fn open(directory: &tempfile::TempDir) -> HistoryStore {
        HistoryStore::open(
            directory.path().join("history.git"),
            directory.path().join("index.sqlite"),
        )
        .expect("a store")
    }

    /// A Snapshot at `moment` with `notes.txt` holding `content`.
    fn capture(&self, moment: u64, content: &str) -> SnapshotId {
        fs::write(self.root.join("notes.txt"), content).expect("a change");
        self.store
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

    fn repository(&self) -> git2::Repository {
        git2::Repository::open_bare(self.directory.path().join("history.git")).expect("the store")
    }

    fn commits(&self) -> usize {
        let repository = self.repository();
        let odb = repository.odb().expect("the objects");
        let mut commits = 0;
        odb.foreach(|oid| {
            if odb
                .read_header(*oid)
                .is_ok_and(|(_, kind)| kind == git2::ObjectType::Commit)
            {
                commits += 1;
            }
            true
        })
        .expect("every object");
        commits
    }

    fn timeline(&self) -> Vec<SnapshotSummary> {
        self.store
            .timeline(&TimelineQuery {
                path: None,
                before: None,
                not_before: None,
                limit: 1000,
            })
            .expect("a timeline")
            .snapshots
    }

    fn read(&self, snapshot: SnapshotId) -> String {
        let content = self
            .store
            .content(snapshot, &RootPath::parse("notes.txt").expect("a path"))
            .expect("the version");
        let mut bytes = Vec::new();
        content
            .read(0, content.size(), &mut |chunk| {
                bytes.extend_from_slice(chunk);
                Ok(())
            })
            .expect("read");
        String::from_utf8(bytes).expect("text")
    }
}

#[test]
fn a_pin_is_one_parentless_commit_sharing_the_snapshots_tree() {
    let fixture = Fixture::new();
    fixture.capture(NOW - 120, "one");
    let pinned = fixture.capture(NOW - 60, "two");
    fixture.capture(NOW, "three");
    let before = fixture.commits();

    assert!(fixture.store.pin(pinned).expect("pinned"), "newly pinned");

    assert_eq!(fixture.commits(), before + 1, "exactly one commit object");
    let repository = fixture.repository();
    let pin = repository
        .find_reference(&format!("refs/keepster/pins/{pinned}"))
        .and_then(|reference| reference.peel_to_commit())
        .expect("the pin ref");
    assert_eq!(pin.parent_count(), 0, "parentless, so it keeps no ancestor");
    let original = repository
        .find_reference(&format!(
            "refs/keepster/segments/{}",
            keepster_core::SegmentName::hourly(at(NOW - 60)).expect("a name")
        ))
        .and_then(|reference| reference.peel_to_commit())
        .expect("the Snapshot, the tip of the hour before NOW's");
    assert_eq!(pin.tree_id(), original.tree_id(), "the tree is shared");
    assert_eq!(pin.message(), original.message(), "it is the same Snapshot");
}

#[test]
fn pinning_and_unpinning_are_idempotent_and_pins_are_listed() {
    let fixture = Fixture::new();
    let first = fixture.capture(NOW - 60, "one");
    let second = fixture.capture(NOW, "two");

    assert!(fixture.store.pin(second).expect("pinned"));
    assert!(!fixture.store.pin(second).expect("pinned again"), "already");
    assert!(fixture.store.pin(first).expect("pinned"));
    let commits = fixture.commits();
    assert!(!fixture.store.pin(first).expect("pinned again"));
    assert_eq!(fixture.commits(), commits, "a second pin writes nothing");

    let pins: Vec<SnapshotId> = fixture
        .store
        .pins()
        .expect("the pins")
        .iter()
        .map(|pin| pin.id)
        .collect();
    assert_eq!(pins, vec![second, first], "newest first, as the timeline");
    assert_eq!(
        fixture
            .store
            .pins()
            .expect("the pins")
            .iter()
            .map(|pin| (pin.added_paths, pin.deleted_paths))
            .collect::<Vec<_>>(),
        vec![(0, 0), (1, 0)],
        "counted by kind, as the timeline counts them"
    );

    assert!(fixture.store.unpin(second).expect("unpinned"));
    assert!(
        !fixture.store.unpin(second).expect("unpinned again"),
        "already"
    );
    assert_eq!(
        fixture
            .store
            .pins()
            .expect("the pins")
            .iter()
            .map(|pin| pin.id)
            .collect::<Vec<_>>(),
        vec![first]
    );
    assert!(
        fixture
            .timeline()
            .iter()
            .any(|snapshot| snapshot.id == second),
        "unpinning removes the pin, never the Snapshot"
    );

    let unknown = SnapshotId::following(None, at(NOW - 3 * DAY));
    assert!(matches!(
        fixture.store.pin(unknown),
        Err(StoreError::SnapshotNotFound(id)) if id == unknown
    ));
}

#[test]
fn a_pinned_snapshot_outlives_retention_a_maintenance_pass_and_a_rebuilt_index() {
    let fixture = Fixture::new();
    let before = fixture.capture(NOW - 41 * DAY, "older");
    let pinned = fixture.capture(NOW - 40 * DAY, "kept by its pin");
    let after = fixture.capture(NOW - 39 * DAY, "newer");
    let newest = fixture.capture(NOW - 60, "today");
    fixture.store.pin(pinned).expect("pinned");

    let retained = fixture
        .store
        .retain(&Ladder::default(), at(NOW), Newest::Survives)
        .expect("a Retention pass");
    assert_eq!(
        retained.removed_snapshots, 3,
        "Retention is pin-unaware: it removes the Segment copy too"
    );
    fixture.store.maintain(&git()).expect("a Maintenance Pass");

    assert_eq!(fixture.read(pinned), "kept by its pin");
    let timeline = fixture.timeline();
    assert_eq!(
        timeline
            .iter()
            .map(|snapshot| (snapshot.id, snapshot.pinned))
            .collect::<Vec<_>>(),
        vec![(newest, false), (pinned, true)],
        "{before} and {after} are gone"
    );
    let only_pin = &timeline[1];
    assert_eq!(
        only_pin.segment,
        keepster_core::SegmentName::hourly(at(NOW - 40 * DAY))
            .expect("a name")
            .as_str(),
        "still the Segment it was taken into"
    );
    assert_eq!(only_pin.changed_paths, 1, "counted from nothing: one file");
    assert_eq!(
        timeline[0].changed_paths, 1,
        "counted against the pinned one"
    );

    // The index is a cache: deleted, it is rebuilt from refs alone, and the Pin is one of them.
    let Fixture {
        directory, store, ..
    } = fixture;
    drop(store);
    fs::remove_file(directory.path().join("index.sqlite")).expect("the index");
    let fixture = Fixture {
        store: Fixture::open(&directory),
        root: directory.path().join("project"),
        directory,
        id: RootId::generate(at(NOW)),
    };
    assert_eq!(
        fixture
            .timeline()
            .iter()
            .map(|snapshot| (snapshot.id, snapshot.pinned, snapshot.changed_paths))
            .collect::<Vec<_>>(),
        vec![(newest, false, 1), (pinned, true, 1)]
    );
    assert_eq!(fixture.read(pinned), "kept by its pin");

    // Unpinned, nothing holds it any more.
    assert!(fixture.store.unpin(pinned).expect("unpinned"));
    assert_eq!(
        fixture
            .timeline()
            .iter()
            .map(|snapshot| (snapshot.id, snapshot.changed_paths))
            .collect::<Vec<_>>(),
        vec![(newest, 1)],
        "the next one is counted from nothing now"
    );
}

#[cfg(unix)]
#[test]
fn a_pin_carries_the_incomplete_marking_of_its_snapshot() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    fixture.capture(NOW - 120, "readable");
    let locked = fixture.root.join("locked.txt");
    fs::write(&locked, "unreadable").expect("a file");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("its mode");
    let incomplete = fixture
        .store
        .capture(
            fixture.id,
            &fixture.root,
            &CapturePolicy::default(),
            Trigger::Manual,
            at(NOW - 60),
        )
        .expect("a Snapshot");
    assert_eq!(incomplete.incomplete.len(), 1, "the Snapshot is incomplete");
    fixture.store.pin(incomplete.id).expect("pinned");
    // Its Segment copy gone, the Pin alone still says so.
    fixture
        .store
        .retain(&Ladder::default(), at(NOW + 40 * DAY), Newest::Survives)
        .expect("a Retention pass");

    let pins = fixture.store.pins().expect("the pins");
    assert_eq!(
        pins.iter()
            .map(|pin| (pin.id, pin.omitted))
            .collect::<Vec<_>>(),
        vec![(incomplete.id, 1)]
    );
}

#[test]
fn a_ref_under_a_pins_name_that_is_not_its_pin_is_replaced_and_strays_are_passed_over() {
    let fixture = Fixture::new();
    let first = fixture.capture(NOW - 41 * DAY, "first");
    let second = fixture.capture(NOW - 41 * DAY + 60, "second");
    let newest = fixture.capture(NOW - 60, "today");
    let repository = fixture.repository();
    // By hand: `second`'s name on its chained commit, which has a parent — not a Pin.
    let chained = repository
        .find_reference(&format!(
            "refs/keepster/segments/{}",
            keepster_core::SegmentName::hourly(at(NOW - 41 * DAY)).expect("a name")
        ))
        .and_then(|reference| reference.peel_to_commit())
        .expect("the Segment");
    repository
        .reference(
            &format!("refs/keepster/pins/{second}"),
            chained.id(),
            false,
            "by hand",
        )
        .expect("a ref");
    // Strays: one naming a blob, one nested below the leaf level.
    let blob = repository.blob(b"not a commit").expect("a blob");
    repository
        .reference(
            &format!("refs/keepster/pins/{first}"),
            blob,
            false,
            "by hand",
        )
        .expect("a ref");
    repository
        .reference(
            &format!("refs/keepster/pins/nested/{first}"),
            chained.id(),
            false,
            "by hand",
        )
        .expect("a ref");

    assert!(
        fixture.timeline().iter().all(|snapshot| !snapshot.pinned),
        "none of them is a Pin, and none of them stops the store being read"
    );
    assert!(fixture.store.pin(second).expect("pinned"), "replaced");
    fixture
        .store
        .retain(&Ladder::default(), at(NOW), Newest::Survives)
        .expect("a Retention pass");

    assert_eq!(
        fixture
            .timeline()
            .iter()
            .map(|snapshot| (snapshot.id, snapshot.pinned))
            .collect::<Vec<_>>(),
        vec![(newest, false), (second, true)]
    );
    assert_eq!(fixture.read(second), "second");
}
