//! Cloud placeholders are never hydrated (ticket 75): a file whose data is only in the cloud is never
//! opened, and the version a Snapshot already holds of it is carried forward while it is provably that
//! version.
//!
//! No test machine has OneDrive or iCloud Drive, so which files are placeholders is a stand-in's answer,
//! as which volume a path is on is `FakeVolumes`'s. What the stand-in calls a placeholder is still an
//! ordinary file whose bytes differ from the version held, so a capture that read it would be caught
//! holding the wrong bytes.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use keepster_core::{CapturePolicy, RootId, RuleSet, SIDECAR, Trigger, Verify, WithheldReason};
use keepster_store::{CaptureRequest, CapturedSnapshot, CloudFiles, HistoryStore, Pass};

/// 2026-09-30T14:37:05Z.
const MOMENT: u64 = 1_790_779_025;

/// When the fixture's files were last written: long before any capture, so the evidence a capture
/// keeps of them is never too recent to vouch (ADR 0014).
const WRITTEN: u64 = MOMENT - 3_600;

/// The moment `seconds` after the Unix epoch.
fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// `WRITTEN` as a `p` record spells it.
fn written_nanos() -> String {
    format!("{WRITTEN}000000000")
}

/// Which files are placeholders, by their path relative to the Root, and from which look on: a file
/// can be dehydrated between the walk's `stat` and the read that follows it.
#[derive(Debug, Default)]
struct FakeCloud {
    /// Each placeholder, with how many looks at it still see it as local.
    placeholders: Mutex<HashMap<PathBuf, usize>>,
}

impl FakeCloud {
    /// Free `path`'s space from now on: every look sees a placeholder.
    fn dehydrate(&self, path: &str) {
        self.dehydrate_after(path, 0);
    }

    /// Free `path`'s space after `looks` more looks have seen it local.
    fn dehydrate_after(&self, path: &str, looks: usize) {
        self.placeholders
            .lock()
            .expect("the stand-in")
            .insert(PathBuf::from(path), looks);
    }

    /// Download `path` again: every look sees it local.
    fn hydrate(&self, path: &str) {
        self.placeholders
            .lock()
            .expect("the stand-in")
            .remove(Path::new(path));
    }
}

impl CloudFiles for FakeCloud {
    fn is_placeholder(&self, path: &Path, _metadata: &fs::Metadata) -> bool {
        let mut placeholders = self.placeholders.lock().expect("the stand-in");
        let Some(local) = placeholders
            .iter_mut()
            .find_map(|(placeholder, local)| path.ends_with(placeholder).then_some(local))
        else {
            return false;
        };
        if *local == 0 {
            return true;
        }
        *local -= 1;
        false
    }
}

/// A Root in a temporary directory, its store, and the stand-in that says which of its files are
/// placeholders.
struct Fixture {
    directory: tempfile::TempDir,
    root: PathBuf,
    store: HistoryStore,
    cloud: Arc<FakeCloud>,
    id: RootId,
}

impl Fixture {
    /// An empty Root with a store beside it, where no file is a placeholder yet.
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path().join("project");
        fs::create_dir_all(&root).expect("the Root");
        let cloud = Arc::new(FakeCloud::default());
        let store = Self::open(directory.path(), &cloud);

        Self {
            directory,
            root,
            store,
            cloud,
            id: RootId::generate(at(MOMENT)),
        }
    }

    /// The store in `directory`, asking `cloud` which files are placeholders.
    fn open(directory: &Path, cloud: &Arc<FakeCloud>) -> HistoryStore {
        HistoryStore::open(
            directory.join("history.git"),
            directory.join("index.sqlite"),
        )
        .expect("a store")
        .with_cloud_files(cloud.clone())
    }

    /// The store opened again with its derived index deleted, as a user may delete it (ticket 03).
    fn lose_the_index(&mut self) {
        // The store it replaces closes its index; the new one opens one only when first asked.
        self.store = Self::open(self.directory.path(), &self.cloud);
        keepster_store::discard_index(&self.directory.path().join("index.sqlite"))
            .expect("the index is discarded");
    }

    /// Write `content` at `path`, dated `WRITTEN`.
    fn write(&self, path: &str, content: &[u8]) {
        self.write_dated(path, content, WRITTEN);
    }

    /// Write `content` at `path`, dated `seconds` after the Unix epoch.
    fn write_dated(&self, path: &str, content: &[u8], seconds: u64) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("its directory");
        fs::write(&path, content).expect("a file");
        fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(at(seconds)))
            .expect("its mtime");
    }

    /// A manual Snapshot at `seconds`, which is taken whether or not anything changed.
    fn capture(&self, seconds: u64) -> CapturedSnapshot {
        self.store
            .capture(
                self.id,
                &self.root,
                &CapturePolicy::default(),
                Trigger::Manual,
                at(seconds),
            )
            .expect("a Snapshot")
    }

    /// A watcher's pass at `seconds` under the default rules.
    fn pass(&self, seconds: u64, verify: Verify, record_unchanged: bool) -> Pass {
        self.pass_under(&CapturePolicy::default(), seconds, verify, record_unchanged)
    }

    /// A watcher's pass at `seconds` under `policy`; with `record_unchanged` false, as the daemon's
    /// triggers ask, a pass that changes nothing records nothing.
    fn pass_under(
        &self,
        policy: &CapturePolicy,
        seconds: u64,
        verify: Verify,
        record_unchanged: bool,
    ) -> Pass {
        self.store
            .pass(
                self.id,
                &self.root,
                policy,
                &CaptureRequest {
                    trigger: Trigger::Watcher,
                    now: at(seconds),
                    verify,
                    record_unchanged,
                    hold_emptied: false,
                },
            )
            .expect("a pass")
    }

    /// The tree of the newest Snapshot, read through git as a test may (ticket 08).
    fn newest(&self) -> (git2::Repository, git2::Oid) {
        let repository = git2::Repository::open_bare(self.store.path()).expect("the store");
        let mut newest = None;
        for reference in repository
            .references_glob("refs/keepster/segments/*")
            .expect("the Segments")
        {
            let commit = reference
                .expect("a Segment")
                .peel_to_commit()
                .expect("its tip");
            if newest
                .as_ref()
                .is_none_or(|(time, _): &(i64, git2::Oid)| commit.time().seconds() >= *time)
            {
                newest = Some((commit.time().seconds(), commit.id()));
            }
        }
        let (_, commit) = newest.expect("a Snapshot");
        (repository, commit)
    }

    /// What the newest Snapshot holds at `path`, if anything.
    fn held(&self, path: &str) -> Option<Vec<u8>> {
        let (repository, commit) = self.newest();
        let tree = repository
            .find_commit(commit)
            .expect("the commit")
            .tree()
            .expect("its tree");
        let entry = tree.get_path(Path::new(path)).ok()?;
        Some(
            repository
                .find_blob(entry.id())
                .expect("a blob")
                .content()
                .to_vec(),
        )
    }

    /// The newest Snapshot's sidecar records, its header left out.
    fn records(&self) -> Vec<String> {
        self.held(SIDECAR)
            .map(|bytes| {
                String::from_utf8(bytes)
                    .expect("the sidecar is text")
                    .lines()
                    .skip(1)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Each path a Snapshot holds no content for, with why.
fn withheld(captured: &CapturedSnapshot) -> Vec<(String, WithheldReason)> {
    captured
        .withheld
        .iter()
        .map(|withheld| (withheld.path.to_lossy().into_owned(), withheld.reason))
        .collect()
}

#[test]
fn a_placeholder_that_was_never_local_is_withheld_and_never_read() {
    let fixture = Fixture::new();
    fixture.write("cloud/remote.txt", b"only in the cloud");
    fixture.write("local.txt", b"here");
    fixture.cloud.dehydrate("cloud/remote.txt");

    let captured = fixture.capture(MOMENT);

    assert_eq!(fixture.held("cloud/remote.txt"), None);
    assert_eq!(fixture.held("local.txt").as_deref(), Some(&b"here"[..]));
    assert_eq!(
        withheld(&captured),
        [(
            "cloud/remote.txt".to_owned(),
            WithheldReason::CloudPlaceholder
        )]
    );
    assert!(
        captured.incomplete.is_empty(),
        "a Withheld path is not a failure"
    );
    assert_eq!(
        fixture.records(),
        ["w cloud_placeholder cloud/remote.txt"],
        "no mtime for a version that is not held"
    );
}

#[test]
fn a_file_dehydrated_after_its_capture_carries_that_version_forward_unread() {
    let fixture = Fixture::new();
    fixture.write("report.txt", b"the local version");
    fixture.capture(MOMENT);

    // Same size and mtime, other bytes: a capture that read it would hold these.
    fixture.write("report.txt", b"THE CLOUD BYTES!!");
    fixture.cloud.dehydrate("report.txt");
    // Verifying by hash reads every file it may, and a placeholder is still not one of them.
    let pass = fixture.pass(MOMENT + 60, Verify::Hash, true);

    assert_eq!(
        fixture.held("report.txt").as_deref(),
        Some(&b"the local version"[..])
    );
    assert_eq!(
        fixture.records(),
        [format!("p {} report.txt", written_nanos())]
    );
    let captured = pass.snapshot.expect("a Snapshot was asked for");
    assert!(captured.withheld.is_empty() && captured.incomplete.is_empty());
    assert_eq!(pass.hashed, 0, "nothing was read");
}

#[test]
fn a_placeholder_whose_size_or_mtime_moved_is_withheld_as_changed_remotely() {
    let fixture = Fixture::new();
    fixture.write("grown.txt", b"short");
    fixture.write("touched.txt", b"same length");
    fixture.capture(MOMENT);

    fixture.write("grown.txt", b"edited elsewhere, longer");
    fixture.write_dated("touched.txt", b"SAME LENGTH", WRITTEN + 5);
    fixture.cloud.dehydrate("grown.txt");
    fixture.cloud.dehydrate("touched.txt");
    let captured = fixture.capture(MOMENT + 60);

    assert_eq!(fixture.held("grown.txt"), None);
    assert_eq!(fixture.held("touched.txt"), None);
    assert_eq!(
        withheld(&captured),
        [
            ("grown.txt".to_owned(), WithheldReason::CloudPlaceholder),
            ("touched.txt".to_owned(), WithheldReason::CloudPlaceholder),
        ]
    );
}

#[test]
fn the_p_record_carries_a_placeholder_forward_once_the_index_is_gone() {
    let mut fixture = Fixture::new();
    fixture.write("report.txt", b"the local version");
    fixture.capture(MOMENT);
    fixture.write("report.txt", b"THE CLOUD BYTES!!");
    fixture.cloud.dehydrate("report.txt");
    fixture.capture(MOMENT + 60);

    fixture.lose_the_index();
    let captured = fixture.capture(MOMENT + 120);

    assert_eq!(
        fixture.held("report.txt").as_deref(),
        Some(&b"the local version"[..])
    );
    assert!(captured.withheld.is_empty());
    assert_eq!(
        fixture.records(),
        [format!("p {} report.txt", written_nanos())]
    );
}

#[test]
fn the_first_time_a_file_is_freed_mints_one_snapshot_and_no_cycle_after_it_does() {
    let fixture = Fixture::new();
    fixture.write("report.txt", b"the local version");
    fixture.capture(MOMENT);

    // As the daemon's triggers capture: a pass that changes nothing records nothing.
    fixture.write("report.txt", b"THE CLOUD BYTES!!");
    fixture.cloud.dehydrate("report.txt");
    let freed = fixture.pass(MOMENT + 60, Verify::Stat, false);
    let recorded = fixture.records();
    fixture.write("report.txt", b"the local version");
    fixture.cloud.hydrate("report.txt");
    let hydrated = fixture.pass(MOMENT + 120, Verify::Stat, false);
    fixture.write("report.txt", b"THE CLOUD BYTES!!");
    fixture.cloud.dehydrate("report.txt");
    let freed_again = fixture.pass(MOMENT + 180, Verify::Stat, false);

    let first = freed
        .snapshot
        .expect("a p record seen for the first time is something to record");
    assert_eq!(first.changed_paths, Ok(0), "no path changed");
    assert_eq!(recorded, [format!("p {} report.txt", written_nanos())]);
    assert!(hydrated.snapshot.is_none(), "the p record stays");
    assert!(freed_again.snapshot.is_none(), "and nothing else moved");
}

#[test]
fn a_file_freed_while_its_root_is_quiet_is_carried_forward_once_the_index_is_gone() {
    let mut fixture = Fixture::new();
    fixture.write("report.txt", b"the local version");
    fixture.capture(MOMENT);
    fixture.write("report.txt", b"THE CLOUD BYTES!!");
    fixture.cloud.dehydrate("report.txt");
    fixture.pass(MOMENT + 60, Verify::Stat, false);
    fixture.write("report.txt", b"the local version");
    fixture.cloud.hydrate("report.txt");
    fixture.pass(MOMENT + 120, Verify::Stat, false);

    fixture.write("report.txt", b"THE CLOUD BYTES!!");
    fixture.cloud.dehydrate("report.txt");
    fixture.lose_the_index();
    let pass = fixture.pass(MOMENT + 180, Verify::Stat, false);

    assert!(
        pass.snapshot.is_none(),
        "the version held is carried forward, so nothing changed: {:?}",
        pass.snapshot.map(|snapshot| withheld(&snapshot))
    );
    assert_eq!(
        fixture.held("report.txt").as_deref(),
        Some(&b"the local version"[..])
    );
}

#[test]
fn the_p_record_stays_while_the_file_is_held_and_moves_only_with_the_file() {
    let fixture = Fixture::new();
    fixture.write("report.txt", b"the local version");
    fixture.capture(MOMENT);
    fixture.write("report.txt", b"THE CLOUD BYTES!!");
    fixture.cloud.dehydrate("report.txt");
    fixture.capture(MOMENT + 60);

    fixture.write("report.txt", b"the local version");
    fixture.cloud.hydrate("report.txt");
    fixture.capture(MOMENT + 120);
    let hydrated = fixture.records();
    fixture.write_dated("report.txt", b"edited here, now", WRITTEN + 30);
    fixture.capture(MOMENT + 180);
    let edited = fixture.records();
    fs::remove_file(fixture.root.join("report.txt")).expect("deleted");
    fixture.capture(MOMENT + 240);

    assert_eq!(hydrated, [format!("p {} report.txt", written_nanos())]);
    assert_eq!(edited, [format!("p {}000000000 report.txt", WRITTEN + 30)]);
    assert_eq!(
        fixture.held("report.txt").as_deref(),
        None,
        "and goes with the file"
    );
    assert!(fixture.records().is_empty());
}

#[test]
fn a_file_dehydrated_between_the_walk_and_its_read_is_not_read() {
    let fixture = Fixture::new();
    fixture.write("report.txt", b"only in the cloud");
    // The walk's look sees it local; the look at the file once open sees the placeholder.
    fixture.cloud.dehydrate_after("report.txt", 1);

    let captured = fixture.capture(MOMENT);

    assert_eq!(fixture.held("report.txt"), None);
    assert_eq!(
        withheld(&captured),
        [("report.txt".to_owned(), WithheldReason::CloudPlaceholder)]
    );
}

#[test]
fn the_rules_decide_before_a_placeholder_is_considered() {
    let fixture = Fixture::new();
    fixture.write("big.iso", b"over a cap of four bytes");
    fixture.cloud.dehydrate("big.iso");
    let capped = CapturePolicy::new(RuleSet::default(), RuleSet::default(), 4, Vec::new());

    let pass = fixture.pass_under(&capped, MOMENT, Verify::Stat, true);

    let captured = pass.snapshot.expect("a Snapshot was asked for");
    assert_eq!(
        withheld(&captured),
        [("big.iso".to_owned(), WithheldReason::TooLarge)]
    );
}

#[test]
fn a_directory_whose_ignore_file_is_a_placeholder_keeps_what_it_held_and_names_the_file() {
    let fixture = Fixture::new();
    let honouring = CapturePolicy::default().honouring_ignore_files(true);
    fixture.write("notes/.gitignore", b"*.log\n");
    fixture.write("notes/kept.txt", b"the local version");
    fixture.write("notes/debug.log", b"left out by its rules");
    fixture.write("top.txt", b"outside it");
    fixture.pass_under(&honouring, MOMENT, Verify::Stat, true);

    // Its rules cannot be known without downloading it, so nothing beneath it can be judged.
    fixture.cloud.dehydrate("notes/.gitignore");
    fixture.write("notes/kept.txt", b"edited since, and never read");
    fixture.write("top.txt", b"edited since, outside it");
    let pass = fixture.pass_under(&honouring, MOMENT + 60, Verify::Stat, true);

    let captured = pass.snapshot.expect("a Snapshot was asked for");
    assert_eq!(
        incomplete(&captured),
        [(
            "notes/.gitignore".to_owned(),
            "cloud_placeholder".to_owned()
        )]
    );
    assert_eq!(
        fixture.held("notes/kept.txt").as_deref(),
        Some(&b"the local version"[..]),
        "what the directory held is carried forward"
    );
    assert_eq!(
        fixture.held("notes/.gitignore").as_deref(),
        Some(&b"*.log\n"[..])
    );
    assert_eq!(fixture.held("notes/debug.log"), None, "and nothing more");
    assert_eq!(
        fixture.held("top.txt").as_deref(),
        Some(&b"edited since, outside it"[..])
    );
}

#[test]
fn a_root_whose_ignore_file_is_a_placeholder_keeps_everything_it_held() {
    let fixture = Fixture::new();
    let honouring = CapturePolicy::default().honouring_ignore_files(true);
    fixture.write(".gitignore", b"*.log\n");
    fixture.write("kept.txt", b"the local version");
    fixture.pass_under(&honouring, MOMENT, Verify::Stat, true);

    fixture.cloud.dehydrate(".gitignore");
    fixture.write("kept.txt", b"edited since, and never read");
    fixture.write("debug.log", b"left out by its rules");
    let pass = fixture.pass_under(&honouring, MOMENT + 60, Verify::Stat, true);

    let captured = pass.snapshot.expect("a Snapshot was asked for");
    assert_eq!(
        incomplete(&captured),
        [(".gitignore".to_owned(), "cloud_placeholder".to_owned())]
    );
    assert_eq!(
        fixture.held("kept.txt").as_deref(),
        Some(&b"the local version"[..])
    );
    assert_eq!(fixture.held("debug.log"), None);
}

#[test]
fn a_carried_directory_keeps_the_p_records_beneath_it() {
    let mut fixture = Fixture::new();
    let honouring = CapturePolicy::default().honouring_ignore_files(true);
    fixture.write("notes/.gitignore", b"*.log\n");
    fixture.write("notes/report.txt", b"the local version");
    fixture.pass_under(&honouring, MOMENT, Verify::Stat, true);
    fixture.write("notes/report.txt", b"THE CLOUD BYTES!!");
    fixture.cloud.dehydrate("notes/report.txt");
    fixture.pass_under(&honouring, MOMENT + 60, Verify::Stat, true);

    // The directory is carried forward, and with it what its records said.
    fixture.cloud.dehydrate("notes/.gitignore");
    fixture.pass_under(&honouring, MOMENT + 120, Verify::Stat, true);
    let carried = fixture.records();
    fixture.cloud.hydrate("notes/.gitignore");
    fixture.lose_the_index();
    let pass = fixture.pass_under(&honouring, MOMENT + 180, Verify::Stat, true);

    assert!(
        carried.contains(&format!("p {} notes/report.txt", written_nanos())),
        "{carried:?}"
    );
    let captured = pass.snapshot.expect("a Snapshot was asked for");
    assert!(captured.withheld.is_empty(), "{:?}", withheld(&captured));
    assert_eq!(
        fixture.held("notes/report.txt").as_deref(),
        Some(&b"the local version"[..]),
        "the p record outlived the carried pass and the lost index"
    );
}

#[test]
fn a_carried_directory_keeps_the_withheld_paths_beneath_it() {
    let fixture = Fixture::new();
    let capped = CapturePolicy::new(RuleSet::default(), RuleSet::default(), 16, Vec::new())
        .honouring_ignore_files(true);
    fixture.write("notes/.gitignore", b"*.log\n");
    fixture.write("notes/disk.iso", b"far over the cap of sixteen bytes");
    fixture.pass_under(&capped, MOMENT, Verify::Stat, true);

    fixture.cloud.dehydrate("notes/.gitignore");
    let pass = fixture.pass_under(&capped, MOMENT + 60, Verify::Stat, true);

    let captured = pass.snapshot.expect("a Snapshot was asked for");
    assert_eq!(
        withheld(&captured),
        [("notes/disk.iso".to_owned(), WithheldReason::TooLarge)],
        "a carried directory still says what it holds without content"
    );
    assert!(
        fixture
            .records()
            .contains(&"w too_large notes/disk.iso".to_owned()),
        "{:?}",
        fixture.records()
    );
}

/// Each path a Snapshot names as not held as it is, with its reason's wire name.
fn incomplete(captured: &CapturedSnapshot) -> Vec<(String, String)> {
    captured
        .incomplete
        .iter()
        .map(|incomplete| {
            (
                incomplete.path.to_lossy().into_owned(),
                incomplete.why.as_str().to_owned(),
            )
        })
        .collect()
}
