//! A capture that would delete every path, held where nothing proves the tree is the one registered
//! (ticket 56, ADR 0011): an unmounted mount point is an ordinary empty directory, and a month of
//! captures of it would age the last Snapshot holding real content off the ladder.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use keepster_core::{CapturePolicy, RootId, Trigger, Verify};
use keepster_store::{CaptureRequest, HistoryStore, Pass, TimelineQuery};

/// 2026-09-30T14:00:00Z.
const NOW: u64 = 1_790_776_800;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    store: HistoryStore,
    id: RootId,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path().join("project");
        fs::create_dir_all(root.join("src")).expect("the Root");
        let store = HistoryStore::open(
            directory.path().join("history.git"),
            directory.path().join("index.sqlite"),
        )
        .expect("a store");

        Self {
            root,
            store,
            id: RootId::generate(at(NOW)),
            _directory: directory,
        }
    }

    fn pass(&self, moment: u64, hold_emptied: bool) -> Pass {
        self.store
            .pass(
                self.id,
                &self.root,
                &CapturePolicy::default(),
                &CaptureRequest {
                    trigger: Trigger::Periodic,
                    now: at(moment),
                    verify: Verify::Stat,
                    record_unchanged: false,
                    hold_emptied,
                },
            )
            .expect("a pass")
    }

    fn snapshots(&self) -> usize {
        self.store
            .timeline(&TimelineQuery {
                path: None,
                before: None,
                not_before: None,
                limit: 100,
            })
            .expect("a timeline")
            .snapshots
            .len()
    }

    /// Everything inside the Root gone, the Root itself left in place — never deleted and made again,
    /// which Windows can refuse while the deletion is still pending.
    fn empty_the_root(&self) {
        for entry in fs::read_dir(&self.root).expect("the Root").flatten() {
            let path = entry.path();
            if path.is_dir() {
                fs::remove_dir_all(&path).expect("emptied");
            } else {
                fs::remove_file(&path).expect("emptied");
            }
        }
    }
}

#[test]
fn a_capture_that_would_delete_every_path_is_held_and_writes_nothing() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("notes.txt"), "kept").expect("a file");
    fs::write(fixture.root.join("src/main.rs"), "fn main() {}").expect("a file");
    fixture.pass(NOW, true);
    fixture.empty_the_root();

    let held = fixture.pass(NOW + 60, true);

    assert!(held.held, "held");
    assert!(held.snapshot.is_none(), "nothing written");
    assert_eq!(
        fixture.snapshots(),
        1,
        "the last Snapshot with content stays the newest"
    );
}

#[test]
fn unguarded_the_same_deletion_is_recorded_as_the_insured_event_it_may_be() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("notes.txt"), "kept").expect("a file");
    fixture.pass(NOW, false);
    fixture.empty_the_root();

    let pass = fixture.pass(NOW + 60, false);

    assert!(!pass.held);
    assert!(
        pass.snapshot.is_some(),
        "a mass deletion is what this tool insures"
    );
}

#[test]
fn a_deletion_that_leaves_anything_at_all_is_never_held() {
    // Not a threshold on how much vanished: only total disappearance is in question.
    let fixture = Fixture::new();
    fs::write(fixture.root.join("notes.txt"), "kept").expect("a file");
    fs::write(fixture.root.join("src/main.rs"), "fn main() {}").expect("a file");
    fs::write(fixture.root.join("src/lib.rs"), "").expect("a file");
    fixture.pass(NOW, true);
    fs::remove_file(fixture.root.join("notes.txt")).expect("gone");
    fs::remove_file(fixture.root.join("src/main.rs")).expect("gone");

    let pass = fixture.pass(NOW + 60, true);

    assert!(!pass.held);
    assert!(pass.snapshot.is_some());
}

#[test]
fn an_empty_root_with_no_history_has_nothing_to_delete_and_is_not_held() {
    let fixture = Fixture::new();
    fixture.empty_the_root();

    let pass = fixture.pass(NOW, true);

    assert!(!pass.held);
}
