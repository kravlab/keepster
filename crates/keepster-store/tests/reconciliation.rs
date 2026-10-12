//! Reconciliation's evidence of change (ticket 44, ADR 0014): a pass compares the Working Tree against its
//! latest Snapshot with no watcher at all, reads only what the `stat` filter cannot vouch for, and the
//! verdict is always the object id.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use keepster_core::{CapturePolicy, RootId, Trigger, Verify};
use keepster_store::{CaptureRequest, HistoryStore, Pass};

/// A fixed moment, 2026-09-30T14:37:05Z. Every file is dated an hour before it, so nothing a test writes
/// is racily clean unless the test means it to be.
const MOMENT: u64 = 1_790_779_025;

fn at(epoch_seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(epoch_seconds)
}

struct Fixture {
    _directory: tempfile::TempDir,
    store: HistoryStore,
    index: PathBuf,
    root: PathBuf,
    id: RootId,
    /// Each pass is a minute after the one before.
    passes: u64,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = directory.path().join("project");
    fs::create_dir_all(root.join("sub")).expect("a subdirectory");
    write(&root, "a.txt", b"one");
    write(&root, "sub/b.txt", b"two");
    let index = directory.path().join("index.sqlite");
    let store = HistoryStore::open(directory.path().join("history.git"), &index).expect("a store");

    Fixture {
        store,
        index,
        root,
        id: RootId::generate(at(MOMENT)),
        passes: 0,
        _directory: directory,
    }
}

/// Write `content` at `path`, dated an hour before [`MOMENT`] plus `skew` seconds.
fn write_dated(root: &Path, path: &str, content: &[u8], skew: u64) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
    fs::write(&file, content).expect("a file");
    date(&file, at(MOMENT - 3600 + skew));
}

fn write(root: &Path, path: &str, content: &[u8]) {
    write_dated(root, path, content, 0);
}

fn date(file: &Path, moment: SystemTime) {
    File::options()
        .write(true)
        .open(file)
        .expect("the file")
        .set_modified(moment)
        .expect("its mtime");
}

impl Fixture {
    fn pass_with(&mut self, verify: Verify) -> Pass {
        self.passes += 1;
        self.store
            .pass(
                self.id,
                &self.root,
                &CapturePolicy::default(),
                &CaptureRequest {
                    trigger: Trigger::Reconcile,
                    now: at(MOMENT + 60 * self.passes),
                    verify,
                    record_unchanged: false,
                    hold_emptied: false,
                },
            )
            .expect("a pass")
    }

    fn pass(&mut self) -> Pass {
        self.pass_with(Verify::Stat)
    }

    /// The next pass's moment, for a test that dates a file at it.
    fn next_moment(&self) -> SystemTime {
        at(MOMENT + 60 * (self.passes + 1))
    }
}

fn changed(pass: &Pass) -> bool {
    pass.snapshot.is_some()
}

#[test]
fn a_second_pass_over_an_unchanged_tree_reads_nothing() {
    let mut fixture = fixture();

    let first = fixture.pass();
    assert!(changed(&first));
    assert_eq!(
        first.hashed, 2,
        "nothing is known yet, so everything is read"
    );
    assert_eq!(first.observed, 2);

    let second = fixture.pass();
    assert!(!changed(&second));
    assert_eq!(second.hashed, 0, "size and mtime vouch for every file");
    assert_eq!(second.observed, 2);
}

#[test]
fn a_change_made_while_nobody_watched_is_captured_and_only_it_is_read() {
    let mut fixture = fixture();
    fixture.pass();

    write_dated(
        &fixture.root,
        "a.txt",
        b"edited while the daemon was stopped",
        5,
    );
    let pass = fixture.pass();

    assert!(changed(&pass));
    assert_eq!(pass.hashed, 1);
}

#[test]
fn a_file_restored_with_its_original_timestamp_and_size_is_still_read() {
    let mut fixture = fixture();
    write_dated(&fixture.root, "a.txt", b"new", 600);
    fixture.pass();

    // What `cp -p` of an older copy does: same size, an older mtime. "Newer than" would never see it.
    write_dated(&fixture.root, "a.txt", b"old", 0);
    let pass = fixture.pass();

    assert!(
        changed(&pass),
        "the comparison is by inequality, not by newer"
    );
    assert_eq!(pass.hashed, 1);
}

#[cfg(unix)]
#[test]
fn a_permission_only_change_is_seen_from_the_stat_alone() {
    use std::os::unix::fs::PermissionsExt;

    let mut fixture = fixture();
    fixture.pass();

    let file = fixture.root.join("a.txt");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("made executable");
    let pass = fixture.pass();

    assert!(changed(&pass), "the executable bit is part of the tree");
    assert_eq!(pass.hashed, 0, "and no content had to be read to see it");
}

#[test]
fn a_file_touched_during_a_pass_is_read_again_by_the_next() {
    let mut fixture = fixture();
    // Its mtime is the pass's own moment, so an edit later in the same tick would look identical.
    let racy = fixture.next_moment();
    date(&fixture.root.join("a.txt"), racy);
    fixture.pass();

    let next = fixture.pass();

    assert_eq!(next.hashed, 1, "the racily clean entry is not trusted");
    let after = fixture.pass();
    assert_eq!(
        after.hashed, 0,
        "once its mtime is older than a pass, it is"
    );
}

#[test]
fn verifying_by_hash_reads_everything() {
    let mut fixture = fixture();
    fixture.pass();

    let pass = fixture.pass_with(Verify::Hash);

    assert_eq!(pass.hashed, 2);
    assert!(!changed(&pass));
}

#[test]
fn a_deleted_index_costs_exactly_one_pass_that_reads_everything() {
    let mut fixture = fixture();
    fixture.pass();
    fixture.pass();

    drop(std::mem::replace(
        &mut fixture.store,
        HistoryStore::open(fixture.root.with_file_name("history.git"), &fixture.index)
            .expect("the store"),
    ));
    for suffix in ["", "-wal", "-shm"] {
        let mut name = fixture.index.clone().into_os_string();
        name.push(suffix);
        let _ = fs::remove_file(name);
    }
    fixture.store = HistoryStore::open(fixture.root.with_file_name("history.git"), &fixture.index)
        .expect("the store, its index gone");

    let cold = fixture.pass();
    assert_eq!(cold.hashed, 2);
    assert!(
        !changed(&cold),
        "nothing changed, only the evidence was lost"
    );
    let warm = fixture.pass();
    assert_eq!(warm.hashed, 0);
}

#[test]
fn a_populated_directory_moved_in_is_captured_whole() {
    let mut fixture = fixture();
    fixture.pass();
    let outside = fixture.root.with_file_name("elsewhere");
    write(&outside, "lib/one.rs", b"// one");
    write(&outside, "lib/deep/two.rs", b"// two");

    fs::rename(outside.join("lib"), fixture.root.join("lib")).expect("moved in");
    let pass = fixture.pass();

    assert!(changed(&pass));
    assert_eq!(pass.hashed, 2, "only what arrived is read");
    assert_eq!(pass.snapshot.expect("a Snapshot").changed_paths, Ok(2));
}

#[cfg(unix)]
#[test]
fn a_write_through_a_second_hardlink_is_captured() {
    let mut fixture = fixture();
    let outside = fixture.root.with_file_name("link.txt");
    fs::hard_link(fixture.root.join("a.txt"), &outside).expect("a second link");
    fixture.pass();

    // Written through the link outside the Root: nothing names `a.txt`, but its inode changed.
    fs::write(&outside, b"written through the other name").expect("a write");
    let pass = fixture.pass();

    assert!(changed(&pass));
    assert_eq!(pass.hashed, 1);
}

#[test]
#[allow(
    unsafe_code,
    reason = "mapping a file is unsafe by construction, and a mapped write is the loss under test; the \
              workspace keeps unsafe out of shipped code, not out of a test that must reproduce one"
)]
fn an_mmap_write_is_captured() {
    let mut fixture = fixture();
    fixture.pass();

    let file = File::options()
        .read(true)
        .write(true)
        .open(fixture.root.join("a.txt"))
        .expect("the file");
    // SAFETY: the file is private to this test and nothing else maps or truncates it.
    let mut map = unsafe { memmap2::MmapMut::map_mut(&file) }.expect("a mapping");
    map.copy_from_slice(b"ONE");
    map.flush().expect("flushed");
    drop(map);
    drop(file);
    // A mapped write updates the mtime when the page is written back, and on some platforms only then;
    // dating it explicitly keeps the test about the pass rather than about the kernel's writeback.
    date(&fixture.root.join("a.txt"), at(MOMENT - 1800));

    let pass = fixture.pass();

    assert!(changed(&pass));
    assert_eq!(pass.hashed, 1);
}

#[test]
fn whole_second_mtimes_are_reported_as_a_coarse_filter() {
    let mut fixture = fixture();
    // Every file the fixture writes is dated on a whole second.
    let coarse = fixture.pass();
    assert!(coarse.coarse_mtime);

    let fine = fixture.root.join("a.txt");
    File::options()
        .write(true)
        .open(&fine)
        .expect("the file")
        .set_modified(at(MOMENT - 3600) + Duration::from_nanos(123_456_789))
        .expect("a fine mtime");
    let pass = fixture.pass();
    assert!(
        !pass.coarse_mtime,
        "one sub-second mtime proves the filesystem keeps them"
    );
}

#[test]
fn on_a_filesystem_with_a_coarse_tick_a_file_written_within_it_is_read_again() {
    let mut fixture = fixture();
    // A second before the pass starts, on a whole second: on FAT the next edit within the same two-second
    // tick would carry this very mtime, so this observation cannot vouch for anything.
    let pass_starts = fixture.next_moment();
    date(
        &fixture.root.join("a.txt"),
        pass_starts - Duration::from_secs(1),
    );
    let first = fixture.pass();
    assert!(first.coarse_mtime);

    let next = fixture.pass();

    assert_eq!(next.hashed, 1);
}
