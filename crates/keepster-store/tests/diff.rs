//! What changed between two versions of a Root, computed where both are (ticket 46): two Snapshots, or a
//! Snapshot and the Working Tree as the next Snapshot would hold it.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use keepster_core::{
    CapturePolicy, DEFAULT_MAX_SIZE, Level, RootId, RootPath, Rule, RuleOrigin, RuleSet,
    SnapshotId, Trigger,
};
use keepster_store::{
    DIFF_MAX_SIZE, DiffTarget, FileChange, FileDiff, HistoryStore, LineKind, StoreError,
};

/// A fixed moment, 2026-09-30T14:37:05Z; each Snapshot is a minute after the one before.
const MOMENT: u64 = 1_790_779_025;

fn at(epoch_seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(epoch_seconds)
}

struct Fixture {
    _directory: tempfile::TempDir,
    store: HistoryStore,
    root: PathBuf,
    id: RootId,
    policy: CapturePolicy,
    taken: u64,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = directory.path().join("project");
    fs::create_dir_all(&root).expect("the Root");
    let store = HistoryStore::open(
        directory.path().join("history.git"),
        directory.path().join("index.sqlite"),
    )
    .expect("a store");

    Fixture {
        store,
        root,
        id: RootId::generate(at(MOMENT)),
        policy: CapturePolicy::default(),
        taken: 0,
        _directory: directory,
    }
}

impl Fixture {
    fn write(&self, path: &str, content: &[u8]) {
        let file = self.root.join(path);
        fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
        fs::write(file, content).expect("a file");
    }

    fn remove(&self, path: &str) {
        fs::remove_file(self.root.join(path)).expect("removed");
    }

    fn take(&mut self) -> SnapshotId {
        self.taken += 1;
        self.store
            .capture(
                self.id,
                &self.root,
                &self.policy,
                Trigger::Manual,
                at(MOMENT + 60 * self.taken),
            )
            .expect("a Snapshot")
            .id
    }

    fn working(&self) -> DiffTarget<'_> {
        DiffTarget::WorkingTree {
            root_path: &self.root,
            policy: &self.policy,
            now: at(MOMENT + 60 * (self.taken + 1)),
        }
    }

    fn summary(&self, from: SnapshotId, to: &DiffTarget<'_>) -> Vec<FileDiff> {
        self.store.diff(from, to, None).expect("a diff")
    }

    fn file(&self, from: SnapshotId, to: &DiffTarget<'_>, path: &str) -> FileDiff {
        let mut files = self.store.diff(from, to, Some(&rp(path))).expect("a diff");
        assert_eq!(
            files.len(),
            1,
            "one file asked for, one answered: {files:?}"
        );
        files.remove(0)
    }
}

fn rp(path: &str) -> RootPath {
    RootPath::parse(path).expect("a Root-relative path")
}

fn changes(files: &[FileDiff]) -> Vec<(String, Option<FileChange>)> {
    files
        .iter()
        .map(|file| (file.path.as_str().to_owned(), file.change))
        .collect()
}

/// A hunk's lines as a diff prints them: a sign and the text.
fn printed(file: &FileDiff) -> Vec<String> {
    file.hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .map(|line| {
            let sign = match line.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Deleted => '-',
            };
            format!("{sign}{}", String::from_utf8_lossy(&line.text))
        })
        .collect()
}

#[test]
fn two_snapshots_differ_by_a_summary_of_changed_paths_in_path_order() {
    let mut fixture = fixture();
    fixture.write("kept.txt", b"same\n");
    fixture.write("edited.txt", b"one\n");
    fixture.write("gone.txt", b"bye\n");
    let before = fixture.take();
    fixture.write("edited.txt", b"two\n");
    fixture.remove("gone.txt");
    fixture.write("sub/new.txt", b"hello\n");
    let after = fixture.take();

    let summary = fixture.summary(before, &DiffTarget::Snapshot(after));

    assert_eq!(
        changes(&summary),
        [
            ("edited.txt".to_owned(), Some(FileChange::Modified)),
            ("gone.txt".to_owned(), Some(FileChange::Deleted)),
            ("sub/new.txt".to_owned(), Some(FileChange::Added)),
        ]
    );
    assert!(
        summary.iter().all(|file| file.hunks.is_empty()),
        "a summary carries no hunks"
    );
}

#[test]
fn a_path_is_answered_with_its_hunks() {
    let mut fixture = fixture();
    fixture.write("notes.txt", b"a\nb\nc\nd\ne\nf\ng\nh\n");
    let before = fixture.take();
    fixture.write("notes.txt", b"a\nb\nc\nD\ne\nf\ng\nh\n");
    let after = fixture.take();

    let file = fixture.file(before, &DiffTarget::Snapshot(after), "notes.txt");

    assert_eq!(file.change, Some(FileChange::Modified));
    assert_eq!(file.hunks.len(), 1);
    let hunk = &file.hunks[0];
    assert_eq!(
        (
            hunk.old_start,
            hunk.old_lines,
            hunk.new_start,
            hunk.new_lines
        ),
        (1, 7, 1, 7)
    );
    assert_eq!(
        printed(&file),
        [" a", " b", " c", "-d", "+D", " e", " f", " g"]
    );
}

#[test]
fn a_last_line_without_a_newline_says_so() {
    let mut fixture = fixture();
    fixture.write("notes.txt", b"one\ntwo");
    let before = fixture.take();
    fixture.write("notes.txt", b"one\ntwo\n");
    let after = fixture.take();

    let file = fixture.file(before, &DiffTarget::Snapshot(after), "notes.txt");

    let lines = &file.hunks[0].lines;
    let deleted = lines
        .iter()
        .find(|line| line.kind == LineKind::Deleted)
        .expect("a deletion");
    let added = lines
        .iter()
        .find(|line| line.kind == LineKind::Added)
        .expect("an addition");
    assert_eq!(
        (deleted.text.as_slice(), deleted.newline),
        (&b"two"[..], false)
    );
    assert_eq!((added.text.as_slice(), added.newline), (&b"two"[..], true));
}

#[test]
fn a_moved_file_is_one_rename_not_a_deletion_and_an_addition() {
    let mut fixture = fixture();
    let body = b"a long enough body that a small edit leaves it mostly the same\n".repeat(20);
    fixture.write("old/name.txt", &body);
    let before = fixture.take();
    fixture.remove("old/name.txt");
    let mut edited = body.clone();
    edited.extend_from_slice(b"one more line\n");
    fixture.write("new/name.txt", &edited);
    let after = fixture.take();

    let summary = fixture.summary(before, &DiffTarget::Snapshot(after));

    assert_eq!(summary.len(), 1, "{summary:?}");
    assert_eq!(summary[0].change, Some(FileChange::Renamed));
    assert_eq!(summary[0].path.as_str(), "new/name.txt");
    assert_eq!(
        summary[0].old_path.as_ref().map(RootPath::as_str),
        Some("old/name.txt")
    );
    // Asked for by either name, the answer is the rename with its one added line.
    for name in ["new/name.txt", "old/name.txt"] {
        let file = fixture.file(before, &DiffTarget::Snapshot(after), name);
        assert_eq!(file.change, Some(FileChange::Renamed));
        assert_eq!(
            printed(&file)
                .iter()
                .filter(|line| !line.starts_with(' '))
                .collect::<Vec<_>>(),
            ["+one more line"]
        );
    }
}

#[test]
fn a_binary_file_is_reported_as_binary_without_hunks() {
    let mut fixture = fixture();
    fixture.write("image.bin", b"\x89PNG\0\0\0one");
    let before = fixture.take();
    fixture.write("image.bin", b"\x89PNG\0\0\0two");
    let after = fixture.take();

    let file = fixture.file(before, &DiffTarget::Snapshot(after), "image.bin");

    assert!(file.binary);
    assert!(!file.too_large);
    assert!(file.hunks.is_empty());
    let unified = fixture
        .store
        .unified(before, &DiffTarget::Snapshot(after), Some(&rp("image.bin")))
        .expect("a patch");
    assert!(
        String::from_utf8_lossy(&unified)
            .contains("Binary files a/image.bin and b/image.bin differ"),
        "{}",
        String::from_utf8_lossy(&unified)
    );
}

#[test]
fn a_file_past_the_threshold_is_reported_as_too_large_without_hunks() {
    let mut fixture = fixture();
    let size = usize::try_from(DIFF_MAX_SIZE).expect("a size") + 1;
    fixture.write("big.log", &b"x\n".repeat(size / 2 + 1));
    let before = fixture.take();
    fixture.write("big.log", &b"y\n".repeat(size / 2 + 1));
    fixture.write("small.txt", b"fits\n");
    let after = fixture.take();

    let file = fixture.file(before, &DiffTarget::Snapshot(after), "big.log");
    assert!(file.too_large);
    assert!(file.hunks.is_empty());
    let summary = fixture.summary(before, &DiffTarget::Snapshot(after));
    let flags: Vec<(&str, bool)> = summary
        .iter()
        .map(|file| (file.path.as_str(), file.too_large))
        .collect();
    assert_eq!(flags, [("big.log", true), ("small.txt", false)]);
}

#[test]
fn the_working_tree_is_compared_as_the_next_snapshot_would_hold_it() {
    let mut fixture = fixture();
    let rule = Rule::parse(
        "*.log",
        RootPath::root(),
        RuleOrigin {
            level: Level::Global,
            source: "test".to_owned(),
            line: Some(1),
        },
    )
    .expect("a valid pattern");
    fixture.policy = CapturePolicy::new(
        RuleSet::new(vec![rule]),
        RuleSet::default(),
        DEFAULT_MAX_SIZE,
        Vec::new(),
    );
    fixture.write("notes.txt", b"before\n");
    let before = fixture.take();
    fixture.write("notes.txt", b"after\n");
    fixture.write("debug.log", b"excluded, so never in a Snapshot\n");
    fixture.write("draft.md", b"new\n");

    let working = fixture.working();
    let summary = fixture.summary(before, &working);

    assert_eq!(
        changes(&summary),
        [
            ("draft.md".to_owned(), Some(FileChange::Added)),
            ("notes.txt".to_owned(), Some(FileChange::Modified)),
        ]
    );
    assert_eq!(
        printed(&fixture.file(before, &working, "notes.txt")),
        ["-before", "+after"]
    );
}

#[test]
fn the_unified_format_is_the_patch_git_would_print() {
    let mut fixture = fixture();
    fixture.write("notes.txt", b"one\ntwo\n");
    let before = fixture.take();
    fixture.write("notes.txt", b"one\nthree\n");
    fixture.write("added.txt", b"new\n");
    let after = fixture.take();

    let unified = fixture
        .store
        .unified(before, &DiffTarget::Snapshot(after), None)
        .expect("a patch");
    let text = String::from_utf8(unified).expect("text");

    assert!(
        text.contains("diff --git a/added.txt b/added.txt\nnew file mode 100644\n"),
        "{text}"
    );
    assert!(
        text.contains("--- a/notes.txt\n+++ b/notes.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+three\n"),
        "{text}"
    );
    assert!(
        text.find("added.txt") < text.find("notes.txt"),
        "files in path order: {text}"
    );
}

#[test]
fn a_path_that_did_not_change_is_answered_as_unchanged_and_one_in_neither_is_not_found() {
    let mut fixture = fixture();
    fixture.write("same.txt", b"same\n");
    let before = fixture.take();
    fixture.write("other.txt", b"other\n");
    let after = fixture.take();

    let same = fixture.file(before, &DiffTarget::Snapshot(after), "same.txt");
    assert_eq!(same.change, None);
    assert!(same.hunks.is_empty());

    let missing = fixture.store.diff(
        before,
        &DiffTarget::Snapshot(after),
        Some(&rp("nowhere.txt")),
    );
    assert!(
        matches!(missing, Err(StoreError::PathNotFound(_))),
        "{missing:?}"
    );
    let unknown = fixture.store.diff(
        SnapshotId::generate(at(MOMENT)),
        &DiffTarget::Snapshot(after),
        None,
    );
    assert!(
        matches!(unknown, Err(StoreError::SnapshotNotFound(_))),
        "{unknown:?}"
    );
}

#[test]
fn a_directory_is_not_a_file_to_diff() {
    let mut fixture = fixture();
    fixture.write("sub/a.txt", b"a\n");
    let before = fixture.take();
    fixture.write("sub/a.txt", b"b\n");
    let after = fixture.take();

    let answer = fixture
        .store
        .diff(before, &DiffTarget::Snapshot(after), Some(&rp("sub")));

    assert!(matches!(answer, Err(StoreError::NotAFile(_))), "{answer:?}");
}
