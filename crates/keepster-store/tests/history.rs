//! The History Store's invariants, asserted against a real repository — which is the only place they
//! exist. A fake could not tell us whether git probed the filesystem behind our back (ADR 0001).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use keepster_core::{RootId, Trigger};
use keepster_store::HistoryStore;

#[test]
fn a_new_store_is_bare_with_file_modes_and_reflogs_off() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("history.git");

    let store =
        HistoryStore::open(&path, directory.path().join("index.sqlite")).expect("a new store");

    let repository = git2::Repository::open_bare(store.path()).expect("a bare repository");
    assert!(
        repository.is_bare(),
        "the store holds no working tree of its own"
    );

    let config = repository.config().expect("the repository's configuration");

    // git probes the filesystem at init and sets this true on Linux and macOS, so it is a step to
    // perform rather than an invariant to assume: with it on, the same Working Tree would produce
    // different trees on different platforms.
    assert_eq!(
        config.get_bool("core.filemode").ok(),
        Some(false),
        "file modes must not be probed from the filesystem"
    );

    // Reflogs keep a deleted ref reachable, which would make Retention free nothing and turn the whole
    // ladder into decoration.
    assert_ne!(
        config.get_bool("core.logAllRefUpdates").ok(),
        Some(true),
        "reflogs must stay off in a History Store"
    );
}

#[test]
fn opening_a_store_twice_leaves_it_as_it_was() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("history.git");

    let first =
        HistoryStore::open(&path, directory.path().join("index.sqlite")).expect("a new store");
    drop(first);
    let again = HistoryStore::open(&path, directory.path().join("index.sqlite"))
        .expect("the same store, reopened");

    assert_eq!(again.path(), path);
    let repository = git2::Repository::open_bare(&path).expect("still a bare repository");
    assert_eq!(
        repository
            .config()
            .expect("the configuration")
            .get_bool("core.filemode")
            .ok(),
        Some(false),
        "reopening must not undo the invariants"
    );
}

/// A fixed moment, so that what the store records is compared against a value rather than against
/// whatever the clock said while the test ran: 2026-09-30T14:37:05Z.
const MOMENT: u64 = 1_790_779_025;

fn at(epoch_seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(epoch_seconds)
}

/// The tip of a Segment's chain.
///
/// The tests reach for git themselves rather than being handed an object id: ticket 08 keeps every git
/// concept out of the store's public signatures, which is what makes the storage engine replaceable in
/// one crate.
fn tip<'a>(repository: &'a git2::Repository, segment: &str) -> git2::Commit<'a> {
    repository
        .find_reference(&format!("refs/keepster/segments/{segment}"))
        .expect("the Segment's ref")
        .peel_to_commit()
        .expect("its tip")
}

/// A Root with two files and a subdirectory, plus a `.git` of its own.
fn working_tree(directory: &Path) -> PathBuf {
    let root = directory.join("project");
    fs::create_dir_all(root.join("sub")).expect("a subdirectory");
    fs::create_dir_all(root.join(".git")).expect("the directory's own git");
    fs::write(root.join("a.txt"), "one").expect("a file");
    fs::write(root.join("sub/b.txt"), "two").expect("a nested file");
    fs::write(root.join(".git/config"), "[core]\n").expect("its own config");

    root
}

#[test]
fn the_first_snapshot_of_a_segment_is_a_parentless_commit_carrying_its_identity() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = working_tree(directory.path());
    let store = HistoryStore::open(
        directory.path().join("history.git"),
        directory.path().join("index.sqlite"),
    )
    .expect("a store");
    let root_id = RootId::generate(at(MOMENT));

    let captured = store
        .capture(root_id, &root, Trigger::Manual, at(MOMENT))
        .expect("a Snapshot");

    assert_eq!(
        captured.segment.as_str(),
        "2026-09-30T14",
        "a Segment is named for the hour its first Snapshot fell in"
    );

    let repository = git2::Repository::open_bare(store.path()).expect("the store");
    let commit = tip(&repository, captured.segment.as_str());

    assert_eq!(
        commit.parent_count(),
        0,
        "a Segment starts from a parentless commit, or dropping one would rewrite the others"
    );

    // The identity lives in a trailer and never in the hash, because thinning a Segment recreates its
    // commits (ADR 0001).
    let message = commit.message().expect("a message");
    assert!(
        message.contains(&format!("Keepster-Snapshot: {}", captured.id)),
        "{message}"
    );
    assert!(
        message.contains(&format!("Keepster-Root: {root_id}")),
        "{message}"
    );

    // Every timestamp the store records derives from the moment it was given (ADR 0012).
    assert_eq!(commit.time().seconds(), MOMENT as i64);
    assert_eq!(commit.time().offset_minutes(), 0, "recorded in UTC");

    let tree = commit.tree().expect("the Snapshot's tree");
    let file = tree.get_name("a.txt").expect("a.txt is captured");
    assert_eq!(
        file.filemode(),
        0o100644,
        "modes are set by us, never probed from the filesystem"
    );
    assert_eq!(
        repository
            .find_blob(file.id())
            .expect("its content")
            .content(),
        b"one"
    );
    assert!(
        tree.get_name("sub").is_some(),
        "a subdirectory becomes a tree of its own"
    );
}

#[test]
fn the_roots_own_git_directory_never_enters_history() {
    // The map rules out any interaction with a watched directory's own git, and libgit2's index refuses
    // such a path on every platform anyway. Ticket 48 adds the record that says so; not capturing it is
    // this ticket's half.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = working_tree(directory.path());
    let store = HistoryStore::open(
        directory.path().join("history.git"),
        directory.path().join("index.sqlite"),
    )
    .expect("a store");

    let captured = store
        .capture(
            RootId::generate(at(MOMENT)),
            &root,
            Trigger::Manual,
            at(MOMENT),
        )
        .expect("a Snapshot");

    let repository = git2::Repository::open_bare(store.path()).expect("the store");
    let tree = tip(&repository, captured.segment.as_str())
        .tree()
        .expect("its tree");

    assert!(
        tree.get_name(".git").is_none(),
        "the directory's own git must not be in the Snapshot"
    );
}

#[test]
fn a_later_snapshot_in_the_same_hour_chains_onto_the_first() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = working_tree(directory.path());
    let store = HistoryStore::open(
        directory.path().join("history.git"),
        directory.path().join("index.sqlite"),
    )
    .expect("a store");
    let root_id = RootId::generate(at(MOMENT));

    let first = store
        .capture(root_id, &root, Trigger::Manual, at(MOMENT))
        .expect("the first");
    fs::write(root.join("a.txt"), "changed").expect("an edit");
    let second = store
        .capture(root_id, &root, Trigger::Manual, at(MOMENT + 60))
        .expect("the second");

    assert_eq!(
        first.segment, second.segment,
        "the same hour is the same Segment"
    );
    assert_ne!(first.id, second.id);

    let repository = git2::Repository::open_bare(store.path()).expect("the store");
    let commit = tip(&repository, second.segment.as_str());
    assert_eq!(commit.parent_count(), 1, "Snapshots chain inside a Segment");
    let parent = commit.parent(0).expect("its parent");
    assert!(
        parent
            .message()
            .expect("a message")
            .contains(&format!("Keepster-Snapshot: {}", first.id)),
        "the parent is the Snapshot before it"
    );

    let tree = commit.tree().expect("its tree");
    let file = tree.get_name("a.txt").expect("a.txt");
    assert_eq!(
        repository.find_blob(file.id()).expect("content").content(),
        b"changed"
    );
}

#[test]
fn capturing_an_unchanged_working_tree_repeats_the_previous_tree() {
    // The baseline is read back from the previous Snapshot's tree at every capture (ADR 0004), so an
    // unchanged Root must produce the very same tree object — which is also what makes an idle Root
    // nearly free.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = working_tree(directory.path());
    let store = HistoryStore::open(
        directory.path().join("history.git"),
        directory.path().join("index.sqlite"),
    )
    .expect("a store");
    let root_id = RootId::generate(at(MOMENT));

    let first = store
        .capture(root_id, &root, Trigger::Manual, at(MOMENT))
        .expect("the first");
    let second = store
        .capture(root_id, &root, Trigger::Manual, at(MOMENT + 60))
        .expect("the second");

    let repository = git2::Repository::open_bare(store.path()).expect("the store");
    let after = tip(&repository, second.segment.as_str());
    let before = after.parent(0).expect("the Snapshot before it");

    assert_eq!(
        before.tree_id(),
        after.tree_id(),
        "nothing changed, so the tree is the same object"
    );
    assert_ne!(
        before.id(),
        after.id(),
        "but it is a Snapshot of its own, with its own identity and moment"
    );
    assert_ne!(first.id, second.id);
}

#[cfg(unix)]
#[test]
fn an_executable_file_is_recorded_as_one() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = working_tree(directory.path());
    let script = root.join("run.sh");
    fs::write(&script, "#!/bin/sh\n").expect("a script");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("its mode");

    let store = HistoryStore::open(
        directory.path().join("history.git"),
        directory.path().join("index.sqlite"),
    )
    .expect("a store");
    let captured = store
        .capture(
            RootId::generate(at(MOMENT)),
            &root,
            Trigger::Manual,
            at(MOMENT),
        )
        .expect("a Snapshot");

    let repository = git2::Repository::open_bare(store.path()).expect("the store");
    let commit = tip(&repository, captured.segment.as_str());
    let tree = commit.tree().expect("its tree");
    let entry = tree.get_name("run.sh").expect("run.sh").filemode();

    assert_eq!(
        entry, 0o100755,
        "the executable bit is read from stat and carried into the tree mode"
    );
}

#[test]
fn a_file_removed_from_the_working_tree_leaves_the_next_snapshot() {
    // A Snapshot is "the captured state of a Root's included files at one moment" (CONTEXT.md), so a
    // file that is gone must be gone from it. Carrying it forward would make history claim content the
    // Working Tree no longer has — the silent disagreement with reality that ADR 0004 refuses.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = working_tree(directory.path());
    let store = HistoryStore::open(
        directory.path().join("history.git"),
        directory.path().join("index.sqlite"),
    )
    .expect("a store");
    let root_id = RootId::generate(at(MOMENT));

    store
        .capture(root_id, &root, Trigger::Manual, at(MOMENT))
        .expect("the first");
    fs::remove_file(root.join("a.txt")).expect("a deletion");
    let second = store
        .capture(root_id, &root, Trigger::Manual, at(MOMENT + 60))
        .expect("the second");

    let repository = git2::Repository::open_bare(store.path()).expect("the store");
    let commit = tip(&repository, second.segment.as_str());
    let tree = commit.tree().expect("its tree");

    assert!(
        tree.get_name("a.txt").is_none(),
        "the deleted file must not be in the Snapshot"
    );
    assert!(
        tree.get_name("sub").is_some(),
        "and the rest of the Root must still be"
    );
}

#[test]
fn a_file_beside_a_directory_of_the_same_stem_is_captured() {
    // `sub.txt` and `sub/` differ by one byte — `.` against `/` — and that is exactly where a
    // component-wise ordering and git's byte ordering disagree. Ticket 12 makes the insertion order
    // load-bearing (0.86 s against 22.74 s on 400k entries), so the pair has to be handled.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = working_tree(directory.path());
    fs::write(root.join("sub.txt"), "beside").expect("a file beside the directory");

    let store = HistoryStore::open(
        directory.path().join("history.git"),
        directory.path().join("index.sqlite"),
    )
    .expect("a store");
    let captured = store
        .capture(
            RootId::generate(at(MOMENT)),
            &root,
            Trigger::Manual,
            at(MOMENT),
        )
        .expect("a Snapshot");

    let repository = git2::Repository::open_bare(store.path()).expect("the store");
    let commit = tip(&repository, captured.segment.as_str());
    let tree = commit.tree().expect("its tree");

    assert!(tree.get_name("sub.txt").is_some(), "the file is captured");
    assert!(tree.get_name("sub").is_some(), "and so is the directory");
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_does_not_cost_the_whole_snapshot() {
    use std::os::unix::fs::PermissionsExt;

    // Ticket 05: an honest partial Snapshot beats no Snapshot, provided it never claims to be whole.
    // One root-owned artefact or a bad mode must not freeze a Root's history.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = working_tree(directory.path());
    let unreadable = root.join("locked.txt");
    fs::write(&unreadable, "secret").expect("a file");
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).expect("its mode");

    let store = HistoryStore::open(
        directory.path().join("history.git"),
        directory.path().join("index.sqlite"),
    )
    .expect("a store");
    let captured = store
        .capture(
            RootId::generate(at(MOMENT)),
            &root,
            Trigger::Manual,
            at(MOMENT),
        )
        .expect("a Snapshot is still taken");

    assert_eq!(
        captured.unreadable.len(),
        1,
        "and it names what it could not read: {:?}",
        captured.unreadable
    );

    let repository = git2::Repository::open_bare(store.path()).expect("the store");
    let commit = tip(&repository, captured.segment.as_str());
    let tree = commit.tree().expect("its tree");
    assert!(
        tree.get_name("a.txt").is_some(),
        "everything readable is captured"
    );
}

#[cfg(unix)]
#[test]
fn the_first_snapshot_of_a_new_hour_is_still_based_on_the_previous_one() {
    use std::os::unix::fs::PermissionsExt;

    // ADR 0004: "the previous Snapshot's tree must be readable at every capture, including the first of
    // a new Segment… it is now depended upon and must be tested rather than assumed". The test has to
    // discriminate, and tree equality cannot — trees are addressed by content, so rebuilding from
    // scratch yields the same id. A file that *becomes* unreadable can only keep its content if the
    // baseline crossed the boundary.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = working_tree(directory.path());
    let store = HistoryStore::open(
        directory.path().join("history.git"),
        directory.path().join("index.sqlite"),
    )
    .expect("a store");
    let root_id = RootId::generate(at(MOMENT));

    let first = store
        .capture(root_id, &root, Trigger::Manual, at(MOMENT))
        .expect("the first");
    fs::set_permissions(root.join("a.txt"), fs::Permissions::from_mode(0o000))
        .expect("the file becomes unreadable");

    const AN_HOUR: u64 = 3_600;
    let second = store
        .capture(root_id, &root, Trigger::Manual, at(MOMENT + AN_HOUR))
        .expect("the second");

    assert_ne!(
        first.segment, second.segment,
        "an hour later is a new Segment"
    );

    let repository = git2::Repository::open_bare(store.path()).expect("the store");
    let fresh = tip(&repository, second.segment.as_str());
    assert_eq!(
        fresh.parent_count(),
        0,
        "a new Segment starts from a parentless commit even though it has a baseline"
    );

    let tree = fresh.tree().expect("its tree");
    let carried = tree
        .get_name("a.txt")
        .expect("the unreadable file keeps its previous content across the boundary");
    assert_eq!(
        repository
            .find_blob(carried.id())
            .expect("that content")
            .content(),
        b"one"
    );
    assert_eq!(
        second.unreadable.len(),
        1,
        "and the Snapshot says it could not read it: {:?}",
        second.unreadable
    );
}
