//! What a Snapshot does not hold, and says so (ticket 48): Withheld paths with their reasons, and files
//! it meant to hold and could not, each a record in the sidecar at the tree root (ADR 0006, ADR 0009).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[cfg(unix)]
use keepster_core::Incomplete;
use keepster_core::{
    CapturePolicy, RootId, RootPath, RuleSet, SIDECAR, Trigger, Verify, WithheldReason,
};
use keepster_store::{CaptureRequest, CapturedSnapshot, HistoryStore, Pass};
#[cfg(unix)]
use keepster_store::{Depth, DiffTarget, Entry, EntryKind, FileChange, IncompletePath, StoreError};

/// 2026-09-30T14:37:05Z.
const MOMENT: u64 = 1_790_779_025;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

fn rp(path: &str) -> RootPath {
    RootPath::parse(path).expect("a Root-relative path")
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
        fs::create_dir_all(&root).expect("the Root");
        let store = HistoryStore::open(
            directory.path().join("history.git"),
            directory.path().join("index.sqlite"),
        )
        .expect("a store");

        Self {
            _directory: directory,
            root,
            store,
            id: RootId::generate(at(MOMENT)),
        }
    }

    fn write(&self, path: &str, content: &[u8]) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("its directory");
        fs::write(path, content).expect("a file");
    }

    fn capture(&self, policy: &CapturePolicy, seconds: u64) -> CapturedSnapshot {
        self.store
            .capture(self.id, &self.root, policy, Trigger::Manual, at(seconds))
            .expect("a Snapshot")
    }

    fn pass(&self, policy: &CapturePolicy, seconds: u64) -> Pass {
        self.store
            .pass(
                self.id,
                &self.root,
                policy,
                &CaptureRequest {
                    trigger: Trigger::Watcher,
                    now: at(seconds),
                    verify: Verify::Stat,
                    record_unchanged: false,
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

    /// The newest Snapshot's sidecar, as text.
    fn sidecar(&self) -> Option<String> {
        self.held(SIDECAR)
            .map(|bytes| String::from_utf8(bytes).expect("the sidecar is text"))
    }

    #[cfg(unix)]
    fn message(&self) -> String {
        let (repository, commit) = self.newest();
        repository
            .find_commit(commit)
            .expect("the commit")
            .message()
            .expect("a message")
            .to_owned()
    }
}

/// The sidecar's first line under the umask the tests run with: on Unix it says the modes were observed,
/// so every Snapshot taken there has one (ticket 50).
#[cfg(unix)]
const HEADER: &str = "keepster-meta 1 file=0644 dir=0755 modes\n";
#[cfg(not(unix))]
const HEADER: &str = "keepster-meta 1 file=0644 dir=0755\n";

#[test]
fn a_snapshot_with_nothing_to_say_says_only_its_header() {
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");

    let captured = fixture.capture(&CapturePolicy::default(), MOMENT);

    let expected = cfg!(unix).then_some(HEADER);
    assert_eq!(
        fixture.sidecar().as_deref(),
        expected,
        "where no mode is observed, nothing to say is no sidecar"
    );
    assert!(captured.withheld.is_empty() && captured.incomplete.is_empty());
}

#[test]
fn a_reserved_name_is_withheld_once_and_never_entered() {
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    fixture.write(".git/config", b"[core]\n");
    fixture.write(".git/objects/aa/bbbb", b"an object");
    fixture.write(SIDECAR, b"the user's own file of that name");
    fixture.write(&format!("sub/{SIDECAR}"), b"nested, so not reserved");

    let captured = fixture.capture(&CapturePolicy::default(), MOMENT);

    assert_eq!(
        fixture.sidecar(),
        Some(format!(
            "{HEADER}w reserved_name .git\n\
             w reserved_name .keepster-meta\n"
        )),
        "one record for the directory, none for what is inside it"
    );
    let mut withheld: Vec<_> = captured
        .withheld
        .iter()
        .map(|withheld| (withheld.path.as_str().to_owned(), withheld.reason))
        .collect();
    withheld.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        withheld,
        [
            (".git".to_owned(), WithheldReason::ReservedName),
            (SIDECAR.to_owned(), WithheldReason::ReservedName),
        ]
    );
    assert_eq!(
        fixture.held(&format!("sub/{SIDECAR}")).as_deref(),
        Some(&b"nested, so not reserved"[..])
    );
    assert!(captured.incomplete.is_empty(), "withholding is not failure");
    assert_eq!(
        captured.changed_paths,
        Ok(4),
        "the two files and the two Withheld paths, never the sidecar itself: {captured:?}"
    );
}

#[test]
fn a_fenced_directory_is_withheld_with_the_reason_the_daemon_gave() {
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    fixture.write("data/keepster/1/history.git/HEAD", b"ref: x\n");
    let policy = CapturePolicy::default()
        .with_fences(vec![(rp("data/keepster"), WithheldReason::HistoryStore)]);

    let captured = fixture.capture(&policy, MOMENT);

    assert_eq!(fixture.held("data/keepster/1/history.git/HEAD"), None);
    assert!(
        fixture
            .sidecar()
            .expect("a sidecar")
            .contains("\nw history_store data/keepster\n")
    );
    assert_eq!(captured.withheld.len(), 1);
}

#[test]
fn a_file_growing_past_its_cap_does_not_mint_a_snapshot_per_pass() {
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    fixture.write("growing.log", &[b'x'; 64]);
    let policy = CapturePolicy::new(RuleSet::default(), RuleSet::default(), 32, Vec::new());

    let first = fixture.pass(&policy, MOMENT);
    assert!(first.snapshot.is_some(), "the first pass has a Snapshot");
    assert!(
        fixture
            .sidecar()
            .expect("a sidecar")
            .contains("\nw too_large growing.log\n"),
        "a Withheld record carries no size, so growth cannot change it"
    );

    fixture.write("growing.log", &[b'x'; 128]);
    let second = fixture.pass(&policy, MOMENT + 60);

    assert!(
        second.snapshot.is_none(),
        "nothing the Snapshot holds or says changed: {second:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_fifo_is_withheld_as_a_special_file_and_never_opened() {
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    fs::create_dir_all(fixture.root.join("run")).expect("a directory");
    let status = std::process::Command::new("mkfifo")
        .arg(fixture.root.join("run/control"))
        .status()
        .expect("mkfifo runs");
    assert!(status.success());

    // A pipe with no writer blocks whoever opens it for reading; the capture runs on a thread so a
    // regression fails the test rather than hanging it.
    let (done, finished) = std::sync::mpsc::channel();
    let root = fixture.root.clone();
    let store_path = fixture.store.path().to_path_buf();
    let index = store_path.with_file_name("index.sqlite");
    let id = fixture.id;
    std::thread::spawn(move || {
        let store = HistoryStore::open(store_path, index).expect("the store");
        let captured = store
            .capture(
                id,
                &root,
                &CapturePolicy::default(),
                Trigger::Manual,
                at(MOMENT),
            )
            .expect("a Snapshot");
        let _ = done.send(captured);
    });
    let captured = finished
        .recv_timeout(Duration::from_secs(30))
        .expect("the capture finished rather than blocking on the pipe");

    assert_eq!(
        fixture.held("run/control"),
        None,
        "no tree entry (ADR 0009)"
    );
    assert_eq!(
        fixture.sidecar().as_deref(),
        Some(format!("{HEADER}w special_file:fifo run/control\n").as_str())
    );
    assert_eq!(
        captured.withheld[0].reason,
        WithheldReason::SpecialFile(keepster_core::SpecialKind::Fifo)
    );
    assert!(captured.incomplete.is_empty());
}

#[cfg(unix)]
#[test]
fn a_read_error_names_its_errno_and_keeps_the_previous_version() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    fixture.write("b.txt", b"before");
    fixture.capture(&CapturePolicy::default(), MOMENT);

    fixture.write("b.txt", b"after");
    fs::set_permissions(
        fixture.root.join("b.txt"),
        fs::Permissions::from_mode(0o000),
    )
    .expect("the file becomes unreadable");
    let captured = fixture.capture(&CapturePolicy::default(), MOMENT + 60);

    assert_eq!(
        captured.incomplete,
        [IncompletePath {
            path: rp("b.txt"),
            why: Incomplete::Error("eacces".to_owned()),
        }]
    );
    assert_eq!(fixture.held("b.txt").as_deref(), Some(&b"before"[..]));
    assert_eq!(
        fixture.sidecar().as_deref(),
        Some(format!("{HEADER}i eacces b.txt\n").as_str())
    );
    assert!(fixture.message().contains("Keepster-Omitted: 1\n"));
}

#[cfg(unix)]
#[test]
fn a_directory_that_cannot_be_listed_names_its_errno_and_keeps_what_it_held() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    fixture.write("private/notes.txt", b"kept");
    fixture.capture(&CapturePolicy::default(), MOMENT);

    fs::set_permissions(
        fixture.root.join("private"),
        fs::Permissions::from_mode(0o000),
    )
    .expect("the directory becomes unlistable");
    let captured = fixture.capture(&CapturePolicy::default(), MOMENT + 60);
    fs::set_permissions(
        fixture.root.join("private"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("restored, so the directory can be cleaned up");

    assert_eq!(
        captured.incomplete,
        [IncompletePath {
            path: rp("private"),
            why: Incomplete::Error("eacces".to_owned()),
        }]
    );
    assert_eq!(
        fixture.held("private/notes.txt").as_deref(),
        Some(&b"kept"[..])
    );
}

/// Put a file at `busy.log` that no read finds as its `stat` described: a followed link to a procfs
/// file, which reports a size of zero and always holds more — unstable on every read, whatever else the
/// machine is doing.
///
/// Linux only, deliberately: elsewhere the only way to keep a file changing is a thread writing to it
/// while it is read, and a busy machine fails to interleave the two often enough that such a test says
/// nothing. The retry and carry-forward it exercises are the same code on every platform.
#[cfg(target_os = "linux")]
fn make_busy(fixture: &Fixture) {
    let path = fixture.root.join("busy.log");
    let _ = fs::remove_file(&path);
    std::os::unix::fs::symlink("/proc/self/stat", &path).expect("a link to procfs");
}

/// What a pass needs to read `busy.log` through its link.
#[cfg(target_os = "linux")]
fn busy_policy() -> CapturePolicy {
    CapturePolicy::default().following(RuleSet::new(vec![
        keepster_core::Rule::parse(
            "busy.log",
            RootPath::root(),
            keepster_core::RuleOrigin {
                level: keepster_core::Level::Global,
                source: "test".to_owned(),
                line: None,
            },
        )
        .expect("a pattern"),
    ]))
}

#[cfg(target_os = "linux")]
#[test]
fn a_file_that_never_settles_is_unstable_and_carries_its_previous_version_forward() {
    let fixture = Fixture::new();
    let policy = busy_policy();

    make_busy(&fixture);
    let first = fixture.capture(&policy, MOMENT);

    assert_eq!(
        first.incomplete,
        [IncompletePath {
            path: rp("busy.log"),
            why: Incomplete::Unstable,
        }],
        "with no previous version, it is absent and named"
    );
    assert_eq!(fixture.held("busy.log"), None);
    assert!(
        fixture
            .sidecar()
            .expect("a sidecar")
            .contains("\ni unstable busy.log\n")
    );

    fs::remove_file(fixture.root.join("busy.log")).expect("the busy file goes");
    fixture.write("busy.log", b"settled\n");
    let settled = fixture.capture(&policy, MOMENT + 60);
    assert!(settled.incomplete.is_empty());
    assert_eq!(
        fixture.sidecar().as_deref(),
        Some(HEADER),
        "the record goes once it is read"
    );

    make_busy(&fixture);
    let third = fixture.capture(&policy, MOMENT + 120);

    assert_eq!(third.incomplete.len(), 1);
    assert_eq!(
        fixture.held("busy.log").as_deref(),
        Some(&b"settled\n"[..]),
        "the previous version is carried forward"
    );
}

/// A Root holding a file, a Withheld pipe in a directory of its own, an over-cap file and a reserved
/// `.git`, captured once.
#[cfg(unix)]
fn withholding() -> (Fixture, CapturePolicy, CapturedSnapshot) {
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    fixture.write("big.bin", &[b'x'; 64]);
    fixture.write(".git/config", b"[core]\n");
    fs::create_dir_all(fixture.root.join("run")).expect("a directory");
    let status = std::process::Command::new("mkfifo")
        .arg(fixture.root.join("run/control"))
        .status()
        .expect("mkfifo runs");
    assert!(status.success());
    let policy = CapturePolicy::new(RuleSet::default(), RuleSet::default(), 32, Vec::new());
    let captured = fixture.capture(&policy, MOMENT);

    (fixture, policy, captured)
}

#[cfg(unix)]
fn listed(entries: &[Entry]) -> Vec<(String, EntryKind, Option<WithheldReason>)> {
    entries
        .iter()
        .map(|entry| (entry.path.as_str().to_owned(), entry.kind, entry.withheld))
        .collect()
}

#[cfg(unix)]
#[test]
fn entries_list_withheld_paths_with_their_reasons_and_never_the_sidecar() {
    let (fixture, _, captured) = withholding();

    let top = fixture
        .store
        .entries(captured.id, &RootPath::root(), Depth::One)
        .expect("a listing");
    assert_eq!(
        listed(&top.entries),
        [
            (
                ".git".to_owned(),
                EntryKind::Withheld,
                Some(WithheldReason::ReservedName)
            ),
            ("a.txt".to_owned(), EntryKind::File, None),
            (
                "big.bin".to_owned(),
                EntryKind::Withheld,
                Some(WithheldReason::TooLarge)
            ),
            ("run".to_owned(), EntryKind::Directory, None),
        ],
        "a directory holding only Withheld paths is listed though git holds no tree for it"
    );
    assert!(top.unrecognized.is_empty());

    let run = fixture
        .store
        .entries(captured.id, &rp("run"), Depth::One)
        .expect("the directory git does not hold is still listed");
    assert_eq!(
        listed(&run.entries),
        [(
            "run/control".to_owned(),
            EntryKind::Withheld,
            Some(WithheldReason::SpecialFile(
                keepster_core::SpecialKind::Fifo
            ))
        )]
    );

    let everything = fixture
        .store
        .entries(captured.id, &RootPath::root(), Depth::Infinity)
        .expect("a listing");
    assert_eq!(
        everything
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        [".git", "a.txt", "big.bin", "run", "run/control"]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn an_unstable_path_is_listed_as_omitted_or_marked_where_its_previous_version_is_kept() {
    let fixture = Fixture::new();
    make_busy(&fixture);
    let captured = fixture.capture(&busy_policy(), MOMENT);

    let listing = fixture
        .store
        .entries(captured.id, &RootPath::root(), Depth::One)
        .expect("a listing");

    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].kind, EntryKind::Omitted);
    assert_eq!(listing.entries[0].incomplete, Some(Incomplete::Unstable));
}

#[cfg(unix)]
#[test]
fn the_sidecar_has_no_content_to_read_and_a_withheld_path_says_why_it_has_none() {
    let (fixture, _, captured) = withholding();

    assert!(matches!(
        fixture.store.content(captured.id, &rp(SIDECAR)),
        Err(StoreError::PathNotFound(_))
    ));
    assert!(matches!(
        fixture.store.content(captured.id, &rp("big.bin")),
        Err(StoreError::Withheld {
            reason: WithheldReason::TooLarge,
            ..
        })
    ));
}

#[cfg(unix)]
#[test]
fn a_diff_between_snapshots_never_shows_the_sidecar_and_names_a_path_becoming_withheld() {
    let (fixture, policy, first) = withholding();
    fixture.write("a.txt", &[b'y'; 64]);
    let second = fixture.capture(&policy, MOMENT + 60);

    let comparison = fixture
        .store
        .diff(first.id, &DiffTarget::Snapshot(second.id), None)
        .expect("a diff");

    assert_eq!(comparison.files.len(), 1, "{:?}", comparison.files);
    assert_eq!(comparison.files[0].path, rp("a.txt"));
    assert_eq!(comparison.files[0].change, Some(FileChange::Deleted));
    assert_eq!(comparison.files[0].withheld, Some(WithheldReason::TooLarge));

    let working = fixture
        .store
        .diff(
            second.id,
            &DiffTarget::WorkingTree {
                root_path: &fixture.root,
                policy: &policy,
                now: at(MOMENT + 120),
            },
            None,
        )
        .expect("a diff with the Working Tree");
    assert!(working.files.is_empty(), "{:?}", working.files);
}

#[cfg(unix)]
#[test]
fn a_path_becoming_withheld_with_nothing_else_changing_is_a_change_of_that_path() {
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    fixture.capture(&CapturePolicy::default(), MOMENT);
    let status = std::process::Command::new("mkfifo")
        .arg(fixture.root.join("pipe"))
        .status()
        .expect("mkfifo runs");
    assert!(status.success());

    let pass = fixture.pass(&CapturePolicy::default(), MOMENT + 60);

    let captured = pass.snapshot.expect("what the Snapshot says changed");
    assert_eq!(
        captured.changed_paths,
        Ok(1),
        "not a Snapshot that looks empty"
    );
}

#[cfg(unix)]
#[test]
fn a_path_beneath_a_withheld_directory_says_so() {
    let (fixture, _, captured) = withholding();

    assert!(matches!(
        fixture.store.content(captured.id, &rp(".git/HEAD")),
        Err(StoreError::Withheld {
            reason: WithheldReason::ReservedName,
            ..
        })
    ));
}

#[test]
fn a_file_deleted_between_the_walk_and_the_read_is_simply_gone() {
    // Not reproducible by timing, so by its effect: a path the walk found that is gone when it is
    // read is a deletion, never an incomplete Snapshot. The store's own walk is the only caller, so
    // what this pins is that a vanished file leaves no record behind once the next pass sees it gone.
    let fixture = Fixture::new();
    fixture.write("a.txt", b"one");
    fixture.write("b.txt", b"two");
    fixture.capture(&CapturePolicy::default(), MOMENT);
    fs::remove_file(fixture.root.join("b.txt")).expect("deleted");

    let captured = fixture.capture(&CapturePolicy::default(), MOMENT + 60);

    assert!(captured.incomplete.is_empty());
    assert_eq!(fixture.held("b.txt"), None);
}
