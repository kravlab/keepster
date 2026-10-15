//! Forget against a real store (ticket 81, ticket 06): every version of a matching path leaves every
//! Segment and every Pin — the sidecar's records too — while every Snapshot keeps its identity, a
//! Snapshot left empty is kept and marked, a Maintenance Pass leaves no blob of it on disk, and a file
//! still holding what was removed is named.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use keepster_core::{
    CapturePolicy, Level, RootId, RootPath, Rule, RuleOrigin, RuleSet, SnapshotId, Trigger,
};
use keepster_store::{Depth, Git, HistoryStore, SnapshotSummary, StoreError, TimelineQuery};

/// 2026-09-30T14:00:00Z.
const NOW: u64 = 1_790_776_800;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

fn path(text: &str) -> RootPath {
    RootPath::parse(text).expect("a path")
}

fn glob(text: &str) -> Rule {
    Rule::parse(
        text,
        RootPath::root(),
        RuleOrigin {
            level: Level::Forget,
            source: "keepster forget".to_owned(),
            line: None,
        },
    )
    .expect("a pattern")
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
        fs::create_dir_all(root.join("secrets")).expect("the Root");
        let store = HistoryStore::open(
            directory.path().join("history.git"),
            directory.path().join("index.sqlite"),
        )
        .expect("a store");

        Self {
            directory,
            root,
            store,
            id: RootId::generate(at(NOW)),
        }
    }

    fn write(&self, name: &str, content: &str) {
        fs::write(self.root.join(name), content).expect("a file");
    }

    /// A Snapshot at `moment`, under a cap of 64 bytes, so a bigger file is Withheld.
    fn capture(&self, moment: u64) -> SnapshotId {
        let policy = CapturePolicy::new(RuleSet::default(), RuleSet::default(), 64, Vec::new());
        self.store
            .capture(self.id, &self.root, &policy, Trigger::Manual, at(moment))
            .expect("a Snapshot")
            .id
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

    fn holds(&self, snapshot: SnapshotId) -> BTreeSet<String> {
        self.store
            .entries(snapshot, &RootPath::root(), Depth::Infinity)
            .expect("a listing")
            .entries
            .iter()
            .map(|entry| entry.path.to_lossy().into_owned())
            .collect()
    }

    fn repository(&self) -> git2::Repository {
        git2::Repository::open_bare(self.directory.path().join("history.git")).expect("the store")
    }
}

#[test]
fn every_version_of_a_forgotten_path_leaves_every_segment_and_pin() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", "kept");
    fixture.write("secret.txt", "hunter2");
    let first = fixture.capture(NOW - 2 * 3600);
    fixture.write("secret.txt", "hunter3");
    let second = fixture.capture(NOW - 3600);
    fs::remove_file(fixture.root.join("secret.txt")).expect("gone");
    fixture.write("notes.txt", "still kept");
    let third = fixture.capture(NOW - 60);
    fixture.store.pin(first).expect("pinned");
    let secret = fixture
        .store
        .content(first, &path("secret.txt"))
        .expect("there to forget");
    let secret_blob = git2::Oid::from_str(&secret.tag()).ok();
    let before: Vec<SnapshotId> = fixture.timeline().iter().map(|s| s.id).collect();

    let mut progress = Vec::new();
    let forgotten = fixture
        .store
        .forget(
            &glob("secret.txt"),
            Some(&fixture.root),
            &mut |done, total| {
                progress.push((done, total));
            },
        )
        .expect("forgotten");

    assert_eq!(
        forgotten.snapshots_touched, 2,
        "the first and the second — the Pin of the first is the same Snapshot"
    );
    assert_eq!(forgotten.paths, vec![path("secret.txt")]);
    assert_eq!(forgotten.blobs_removed, 2, "both versions");
    assert!(forgotten.still_on_disk.is_empty(), "the file is gone");
    assert_eq!(
        progress.last().map(|(done, total)| done == total),
        Some(true),
        "{progress:?}"
    );

    let after = fixture.timeline();
    assert_eq!(
        after.iter().map(|s| s.id).collect::<Vec<_>>(),
        before,
        "every Snapshot keeps its identity"
    );
    for snapshot in [first, second, third] {
        assert!(
            matches!(
                fixture.store.content(snapshot, &path("secret.txt")),
                Err(StoreError::PathNotFound(_))
            ),
            "{snapshot}"
        );
        assert!(fixture.holds(snapshot).contains("notes.txt"), "{snapshot}");
    }
    let forgotten_in: Vec<(SnapshotId, u64)> = after
        .iter()
        .map(|snapshot| (snapshot.id, snapshot.forgotten_paths))
        .collect();
    assert_eq!(
        forgotten_in,
        vec![(third, 0), (second, 1), (first, 1)],
        "each Snapshot that held it is marked"
    );
    assert!(
        after.iter().any(|s| s.id == first && s.pinned),
        "still pinned"
    );

    fixture.store.maintain(&git()).expect("maintained");
    if let Some(blob) = secret_blob {
        assert!(
            fixture.repository().find_blob(blob).is_err(),
            "no version of it is left on disk"
        );
    }
}

#[test]
fn a_snapshot_left_empty_is_kept_and_marked_and_records_go_with_their_paths() {
    let fixture = Fixture::new();
    fixture.write("secrets/key.txt", "s3cr3t");
    fixture.write("secrets/big.bin", &"x".repeat(1000));
    let only = fixture.capture(NOW - 60);
    assert!(
        fixture.holds(only).contains("secrets/big.bin"),
        "Withheld, named"
    );

    let forgotten = fixture
        .store
        .forget(&glob("secrets/"), Some(&fixture.root), &mut |_, _| {})
        .expect("forgotten");

    assert_eq!(forgotten.snapshots_touched, 1);
    assert_eq!(
        forgotten.paths,
        vec![path("secrets/big.bin"), path("secrets/key.txt")],
        "the Withheld path's record goes too"
    );
    let timeline = fixture.timeline();
    assert_eq!(timeline.len(), 1, "the Snapshot is kept");
    assert_eq!(timeline[0].id, only);
    assert_eq!(timeline[0].forgotten_paths, 2, "and marked");
    assert!(fixture.holds(only).is_empty(), "holding nothing now");
}

#[test]
fn a_live_file_still_holding_what_was_removed_is_named() {
    let fixture = Fixture::new();
    fixture.write("secret.txt", "hunter2");
    fixture.write("rotated.txt", "old");
    fixture.capture(NOW - 60);
    fixture.write("rotated.txt", "new");

    let forgotten = fixture
        .store
        .forget(&glob("*.txt"), Some(&fixture.root), &mut |_, _| {})
        .expect("forgotten");

    assert_eq!(
        forgotten.still_on_disk,
        vec![path("secret.txt")],
        "the next Snapshot would capture it again; the rotated one would not"
    );
}

#[test]
fn the_paths_a_glob_would_forget_can_be_listed_beforehand() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "1");
    fixture.capture(NOW - 120);
    fs::remove_file(fixture.root.join("a.txt")).expect("gone");
    fixture.write("b.txt", "2");
    fixture.capture(NOW - 60);

    assert_eq!(
        fixture.store.forgettable(&glob("*.txt")).expect("listed"),
        BTreeSet::from([path("a.txt"), path("b.txt")])
    );
}

#[test]
fn looking_at_what_would_be_forgotten_writes_nothing() {
    let fixture = Fixture::new();
    fixture.write("secret.txt", "hunter2");
    fixture.capture(NOW - 60);
    let objects = || {
        let repository = fixture.repository();
        let odb = repository.odb().expect("the objects");
        let mut count = 0;
        odb.foreach(|_| {
            count += 1;
            true
        })
        .expect("every object");
        count
    };
    let before = objects();

    fixture
        .store
        .forgettable(&glob("secret.txt"))
        .expect("listed");

    assert_eq!(objects(), before, "it may run without the store's lock");
}

#[cfg(unix)]
#[test]
fn a_pattern_for_directories_leaves_a_file_of_that_name_alone() {
    use std::os::unix::fs::PermissionsExt;

    // A file with a mode of its own among files with the usual one, so the sidecar records it by its
    // path rather than in its header.
    let fixture = Fixture::new();
    fixture.write("a.txt", "usual");
    fixture.write("b.txt", "usual");
    fixture.write("notes.txt", "kept");
    fs::set_permissions(
        fixture.root.join("notes.txt"),
        fs::Permissions::from_mode(0o600),
    )
    .expect("its mode");
    fixture.capture(NOW - 60);
    let tips = || {
        fixture
            .repository()
            .references()
            .expect("the refs")
            .filter_map(Result::ok)
            .filter_map(|reference| reference.target())
            .collect::<Vec<_>>()
    };
    let before = tips();

    let forgotten = fixture
        .store
        .forget(&glob("notes.txt/"), Some(&fixture.root), &mut |_, _| {})
        .expect("nothing to forget");

    assert!(forgotten.paths.is_empty(), "{forgotten:?}");
    assert_eq!(tips(), before, "nothing rewritten: the mode record stays");
}
