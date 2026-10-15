//! Search across a Root's Snapshots, at the store (ticket 59): paths found from the change rows the index
//! holds, and content found by reading each distinct version once — attributed to the Snapshots it was
//! current for, never one row per Snapshot — with binaries counted and Withheld paths named.

use std::fs;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use keepster_core::{
    CapturePolicy, ContentQuery, PathQuery, RootId, RootPath, RuleSet, SnapshotId, Trigger,
    WithheldReason,
};
use keepster_store::{ContentFound, HistoryStore};

/// 2026-09-30T14:37:05Z.
const MOMENT: u64 = 1_790_779_025;

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

        Self {
            id: RootId::generate(SystemTime::UNIX_EPOCH + Duration::from_secs(MOMENT)),
            store: Self::open(&directory),
            directory,
            root,
        }
    }

    /// The store in `directory`, and its index beside it.
    fn open(directory: &tempfile::TempDir) -> HistoryStore {
        HistoryStore::open(
            directory.path().join("history.git"),
            directory.path().join("index.sqlite"),
        )
        .expect("a store")
    }

    fn write(&self, path: &str, content: &[u8]) {
        let file = self.root.join(path);
        fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
        fs::write(file, content).expect("a file");
    }

    /// A Snapshot `minutes` after the first, under a policy capping files at 64 bytes.
    fn capture(&self, minutes: u64) -> SnapshotId {
        let policy = CapturePolicy::new(RuleSet::default(), RuleSet::default(), 64, Vec::new());
        self.store
            .capture(
                self.id,
                &self.root,
                &policy,
                Trigger::Manual,
                SystemTime::UNIX_EPOCH + Duration::from_secs(MOMENT + minutes * 60),
            )
            .expect("a Snapshot")
            .id
    }
}

/// Three Snapshots: `notes.txt` and an identical `copy.txt` hold the needle in the first, `notes.txt`
/// loses it in the second while `later.txt` brings one, `later.txt` goes in the third while `main.rs`
/// gains one. A binary image and an over-cap file are there throughout.
fn history() -> (Fixture, [SnapshotId; 3]) {
    let fixture = Fixture::new();
    fixture.write("notes.txt", b"first line\nthe needle is here\n");
    fixture.write("copy.txt", b"first line\nthe needle is here\n");
    fixture.write("src/main.rs", b"fn main() {}\n");
    fixture.write("image.png", b"\x89PNG\0\0needle\0");
    fixture.write("big.bin", &[b'z'; 100]);
    let first = fixture.capture(0);

    fixture.write("notes.txt", b"nothing to see\n");
    fixture.write("later.txt", b"a NEEDLE, shouting\n");
    let second = fixture.capture(1);

    fs::remove_file(fixture.root.join("later.txt")).expect("removed");
    fixture.write("src/main.rs", b"fn main() {}\n// needle\n");
    let third = fixture.capture(2);

    (fixture, [first, second, third])
}

#[test]
fn a_path_is_found_with_its_span_and_whether_it_is_still_there() {
    let (fixture, [first, second, third]) = history();

    let (found, more) = fixture
        .store
        .find_paths(&PathQuery::new("TXT", false).expect("a query"), 100)
        .expect("a search");
    assert!(
        found.is_empty() && !more,
        "a capital asks for case: {found:?}"
    );

    let (found, more) = fixture
        .store
        .find_paths(&PathQuery::new(".txt", false).expect("a query"), 100)
        .expect("a search");
    assert!(!more);
    let rows: Vec<(&str, u64, SnapshotId, SnapshotId, bool)> = found
        .iter()
        .map(|hit| {
            (
                hit.path.to_str().expect("a text path"),
                hit.snapshots,
                hit.first,
                hit.last,
                hit.exists,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("copy.txt", 1, first, first, true),
            ("later.txt", 2, second, third, false),
            ("notes.txt", 2, first, second, true),
        ],
        "found, and whether it is already gone"
    );

    let (pattern, _) = fixture
        .store
        .find_paths(&PathQuery::new("src/*.rs", true).expect("a pattern"), 100)
        .expect("a search");
    assert_eq!(pattern.len(), 1);
    assert_eq!(pattern[0].path.to_lossy(), "src/main.rs");

    let (limited, more) = fixture
        .store
        .find_paths(&PathQuery::new(".txt", false).expect("a query"), 2)
        .expect("a search");
    assert_eq!(limited.len(), 2);
    assert!(more, "there were more than were asked for");
}

/// Every match as (path, first, last, line).
fn matches(
    fixture: &Fixture,
    under: &RootPath,
    query: &str,
) -> (
    Vec<(String, SnapshotId, SnapshotId, u64)>,
    keepster_store::SearchCounts,
) {
    let plan = fixture.store.content_plan(under).expect("a plan");
    let query = ContentQuery::new(query, false).expect("a query");
    let mut found = Vec::new();
    let counts = fixture.store.search_content(&plan, &query, &mut |hit| {
        if let ContentFound::Match { version, line, .. } = hit {
            found.push((
                version.path.to_lossy().into_owned(),
                version.first,
                version.last,
                line.line,
            ));
        }
        ControlFlow::Continue(())
    });
    found.sort();

    (found, counts)
}

#[test]
fn content_is_found_once_per_version_with_the_snapshots_it_was_current_for() {
    let (fixture, [first, second, third]) = history();

    let (found, counts) = matches(&fixture, &RootPath::root(), "needle");

    assert_eq!(
        found,
        [
            ("copy.txt".to_owned(), first, third, 2),
            ("later.txt".to_owned(), second, second, 1),
            ("notes.txt".to_owned(), first, first, 2),
            ("src/main.rs".to_owned(), third, third, 2),
        ],
        "one row per version, never one per Snapshot"
    );
    assert_eq!(
        counts.binary, 1,
        "the image is skipped as binary, and counted"
    );
    // `notes.txt` and `copy.txt` held one content: it is read once.
    assert_eq!(counts.scanned, 5, "{counts:?}");
}

#[test]
fn the_plan_knows_its_cost_and_names_what_it_cannot_search() {
    let (fixture, [first, ..]) = history();

    let plan = fixture
        .store
        .content_plan(&RootPath::root())
        .expect("a plan");

    assert_eq!(plan.versions(), 6, "every distinct version, each once");
    assert!(plan.bytes > 0);
    let withheld: Vec<(&str, WithheldReason, SnapshotId)> = plan
        .withheld
        .iter()
        .map(|met| {
            (
                met.path.to_str().expect("a text path"),
                met.reason,
                met.snapshot,
            )
        })
        .collect();
    assert_eq!(withheld, [("big.bin", WithheldReason::TooLarge, first)]);

    // The metadata sidecar names `big.bin`; it is not a path of the Root, and is never searched.
    let (found, _) = matches(&fixture, &RootPath::root(), "big.bin");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_search_beneath_a_directory_reads_only_what_is_there_and_stops_when_told() {
    let (fixture, [.., third]) = history();

    let (found, _) = matches(&fixture, &RootPath::parse("src").expect("a path"), "needle");
    assert_eq!(found, [("src/main.rs".to_owned(), third, third, 2)]);

    let plan = fixture
        .store
        .content_plan(&RootPath::root())
        .expect("a plan");
    let query = ContentQuery::new("needle", false).expect("a query");
    let mut seen = 0;
    fixture.store.search_content(&plan, &query, &mut |hit| {
        if matches!(hit, ContentFound::Match { .. }) {
            seen += 1;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    assert_eq!(seen, 1, "nothing after the reader stops");
}

#[test]
fn an_index_rebuilt_from_git_finds_the_same() {
    let (fixture, _) = history();
    let (before, _) = matches(&fixture, &RootPath::root(), "needle");
    let Fixture {
        directory,
        root,
        store,
        id,
    } = fixture;
    // Closed first: Windows deletes no file a connection holds open.
    drop(store);
    fs::remove_file(directory.path().join("index.sqlite")).expect("the index, discarded");
    let rebuilt = Fixture {
        store: Fixture::open(&directory),
        directory,
        root,
        id,
    };

    let (after, _) = matches(&rebuilt, &RootPath::root(), "needle");
    assert_eq!(
        after, before,
        "the blob column is derived from git, like every other"
    );
}
