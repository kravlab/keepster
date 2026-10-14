//! The bulk generator's change stream (ticket 30): the declared churn the outcome lines that exist only
//! over time — store growth, coverage — are measured against.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use keepster_testkit::bulk::{
    CEILING, Change, Churn, Excluded, PlannedFile, Profile, SMALL, SizeBucket, apply,
    change_stream, content, materialize, plan,
};

mod common;

use common::parent_of;

#[test]
fn one_seed_streams_one_history_of_the_declared_churn() {
    let declared = [(SMALL, (20, 2, 1)), (CEILING, (100, 10, 5))];
    for (profile, per_step) in declared {
        let stream = change_stream(&profile, &plan(&profile));

        assert_eq!(
            stream,
            change_stream(&profile, &plan(&profile)),
            "{}",
            profile.name
        );
        assert_eq!(stream.len(), 50, "{}", profile.name);
        for step in &stream {
            assert_eq!(kinds(step), per_step, "{}", profile.name);
        }
        // One step a minute: ticket 04's rate limit, the most often a Root is ever captured.
        assert_eq!(
            profile.churn.interval,
            Duration::from_secs(60),
            "{}",
            profile.name
        );
    }
}

#[test]
fn every_change_names_a_file_as_it_stands_at_that_moment() {
    // The dense profile puts every file in one directory and creates as many as it changes, so a name
    // taken twice or a file named after it is gone would show at once.
    for profile in [SMALL, CEILING, dense()] {
        let tree = plan(&profile);
        let mut live: HashSet<String> = tree.files.iter().map(|file| file.path.clone()).collect();

        for change in change_stream(&profile, &tree).iter().flatten() {
            match change {
                Change::Modified(file) => assert!(live.contains(&file.path), "{}", file.path),
                Change::Created(file) => {
                    assert!(
                        live.insert(file.path.clone()),
                        "{}: {} already exists",
                        profile.name,
                        file.path
                    );
                    let home = parent_of(&file.path);
                    assert!(
                        home.is_empty()
                            || tree.directories.iter().any(|directory| directory == home),
                        "{} is created outside the planned directories",
                        file.path
                    );
                }
                Change::Deleted(path) => assert!(live.remove(path), "{path} is not there"),
            }
        }
    }
}

#[test]
fn a_step_touches_each_path_at_most_once() {
    // A step is what one capture sees, so a file modified and deleted within it would be a change no
    // capture could observe. Only the dense profile draws the same file twice often enough to tell.
    for profile in [SMALL, dense()] {
        for step in change_stream(&profile, &plan(&profile)) {
            let mut touched: HashSet<&str> = HashSet::new();
            for change in &step {
                let path = match change {
                    Change::Modified(file) | Change::Created(file) => file.path.as_str(),
                    Change::Deleted(path) => path.as_str(),
                };
                assert!(
                    touched.insert(path),
                    "{}: {path} twice in one step",
                    profile.name
                );
            }
        }
    }
}

#[test]
fn a_modified_file_gets_new_content_and_stays_in_its_bucket() {
    for profile in [SMALL, CEILING] {
        let tree = plan(&profile);
        let mut current: HashMap<String, PlannedFile> = tree
            .files
            .iter()
            .map(|file| (file.path.clone(), file.clone()))
            .collect();

        for change in change_stream(&profile, &tree).iter().flatten() {
            match change {
                Change::Modified(file) => {
                    let before = current
                        .insert(file.path.clone(), file.clone())
                        .expect("a live file");
                    assert_ne!(before.content_seed, file.content_seed, "{}", file.path);
                    assert_eq!(
                        bucket_of(&profile, before.size),
                        bucket_of(&profile, file.size),
                        "{}: {} → {} bytes",
                        file.path,
                        before.size,
                        file.size
                    );
                }
                Change::Created(file) => {
                    current.insert(file.path.clone(), file.clone());
                }
                Change::Deleted(path) => {
                    current.remove(path);
                }
            }
        }
    }
}

#[test]
fn no_change_empties_a_directory() {
    // An empty directory is a degenerate case (ticket 25), as it is in the tree itself. The sparse
    // profile leaves many directories holding a single file, where a careless deletion would show.
    for profile in [SMALL, sparse()] {
        let tree = plan(&profile);
        let mut held: HashMap<String, usize> = HashMap::new();
        for file in &tree.files {
            *held.entry(parent_of(&file.path).to_owned()).or_default() += 1;
        }

        for change in change_stream(&profile, &tree).iter().flatten() {
            match change {
                Change::Modified(_) => {}
                Change::Created(file) => {
                    *held.entry(parent_of(&file.path).to_owned()).or_default() += 1;
                }
                Change::Deleted(path) => {
                    let home = parent_of(path);
                    let count = held.get_mut(home).expect("a directory holding files");
                    *count -= 1;
                    let branching = tree
                        .directories
                        .iter()
                        .any(|directory| parent_of(directory) == home);
                    assert!(
                        *count > 0 || branching,
                        "{}: {home} emptied by deleting {path}",
                        profile.name
                    );
                }
            }
        }
    }
}

#[test]
fn every_written_size_stays_in_the_profiles_buckets() {
    for profile in [SMALL, CEILING] {
        let tree = plan(&profile);
        for change in change_stream(&profile, &tree).iter().flatten() {
            if let Change::Modified(file) | Change::Created(file) = change {
                assert!(
                    bucket_of(&profile, file.size).is_some(),
                    "{}: {} bytes is in no bucket",
                    profile.name,
                    file.size
                );
            }
        }
    }
}

#[test]
fn applying_the_stream_leaves_exactly_the_files_it_describes() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let profile = tiny();
    let tree = plan(&profile);
    materialize(&profile, &tree, directory.path()).expect("the tree is written");
    let mut expected: BTreeMap<String, PlannedFile> = tree
        .files
        .iter()
        .chain(&tree.excluded_files)
        .map(|file| (file.path.clone(), file.clone()))
        .collect();

    for step in change_stream(&profile, &tree) {
        apply(&profile, &step, directory.path()).expect("the step is written");
        for change in &step {
            match change {
                Change::Modified(file) | Change::Created(file) => {
                    expected.insert(file.path.clone(), file.clone());
                }
                Change::Deleted(path) => {
                    expected.remove(path);
                }
            }
        }

        let on_disk = files_beneath(directory.path(), directory.path());
        assert!(
            on_disk.keys().eq(expected.keys()),
            "on disk {:?}, expected {:?}",
            on_disk.keys(),
            expected.keys()
        );
        for (path, file) in &expected {
            assert_eq!(on_disk[path], content(&profile, file), "{path}");
        }
    }
}

#[test]
fn materializing_over_a_file_already_there_is_refused() {
    // Every file of the dense profile sits at the Root, where no directory creation would fail first.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let profile = dense();
    let tree = plan(&profile);
    let taken = &tree.files[0].path;
    fs::write(directory.path().join(taken), b"already here").expect("a file in the way");

    let refused =
        materialize(&profile, &tree, directory.path()).expect_err("a planned path is taken");

    assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(
        fs::read(directory.path().join(taken)).expect("the file in the way"),
        b"already here"
    );
}

/// Six files in the Root alone, changed, created and deleted heavily: drawing the same file twice in
/// a step, or reusing a name, is likely here and nowhere else.
fn dense() -> Profile {
    Profile {
        name: "dense",
        files: 6,
        directories: 0,
        bytes: 60,
        sizes: &[SizeBucket {
            permille: 1000,
            smallest: 1,
            largest: 20,
        }],
        excluded: small_exclusion(),
        churn: Churn {
            steps: 20,
            modified: 3,
            created: 3,
            deleted: 1,
            interval: Duration::from_secs(60),
        },
        ..SMALL
    }
}

/// Twenty directories for thirty files: many hold a single one, which no deletion may take.
fn sparse() -> Profile {
    Profile {
        name: "sparse",
        files: 30,
        directories: 20,
        bytes: 300,
        sizes: &[SizeBucket {
            permille: 1000,
            smallest: 1,
            largest: 100,
        }],
        fan_out: 2,
        excluded: small_exclusion(),
        churn: Churn {
            steps: 10,
            modified: 0,
            created: 0,
            deleted: 1,
            interval: Duration::from_secs(60),
        },
        ..SMALL
    }
}

/// Small enough to check the whole tree on disk after every step.
fn tiny() -> Profile {
    Profile {
        name: "tiny",
        files: 12,
        directories: 3,
        bytes: 600,
        sizes: &[SizeBucket {
            permille: 1000,
            smallest: 1,
            largest: 100,
        }],
        fan_out: 2,
        excluded: small_exclusion(),
        churn: Churn {
            steps: 3,
            modified: 2,
            created: 1,
            deleted: 1,
            interval: Duration::from_secs(60),
        },
        ..SMALL
    }
}

/// An excluded directory of one file, so a test profile does not plan the small profile's twenty
/// thousand.
fn small_exclusion() -> Excluded {
    Excluded {
        name: "cache",
        files: 1,
        directories: 0,
        file_size: 1,
    }
}

/// The index of the bucket `size` lies in.
fn bucket_of(profile: &Profile, size: u64) -> Option<usize> {
    profile.sizes.iter().position(|bucket| bucket.holds(size))
}

/// Every file beneath `directory`, keyed by its `/`-joined path relative to `root`, with its bytes.
fn files_beneath(root: &Path, directory: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(directory).expect("a readable directory") {
        let entry = entry.expect("a directory entry");
        if entry.file_type().expect("a file type").is_dir() {
            files.extend(files_beneath(root, &entry.path()));
        } else {
            let relative = entry
                .path()
                .strip_prefix(root)
                .expect("beneath the root")
                .to_owned();
            let path = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            files.insert(path, fs::read(entry.path()).expect("a readable file"));
        }
    }
    files
}

/// How many changes of each kind a step holds: modified, created, deleted.
fn kinds(step: &[Change]) -> (usize, usize, usize) {
    step.iter().fold(
        (0, 0, 0),
        |(modified, created, deleted), change| match change {
            Change::Modified(_) => (modified + 1, created, deleted),
            Change::Created(_) => (modified, created + 1, deleted),
            Change::Deleted(_) => (modified, created, deleted + 1),
        },
    )
}
