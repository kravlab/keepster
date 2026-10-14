//! The volume a path is on, asked of the machine running the tests (ticket 56).
//!
//! What a machine answers depends on its disks — a container's overlay has no UUID, a laptop's disk has
//! one — so these assert what must hold everywhere: the answer is the same every time it is asked, a
//! path is beneath the mount point named for it, a volume names its filesystem, and a volume that is
//! named is found mounted there.

use std::path::{Path, PathBuf};

use keepster_service::{mount_points, volume_of};

/// Whether `a` and `b` name the same directory, however differently they spell it — a macOS firmlink,
/// a Windows short name.
fn same_directory(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let real = |path: &Path| {
            std::fs::canonicalize(path).map(|path| path.to_string_lossy().to_lowercase())
        };
        matches!((real(a), real(b)), (Ok(a), Ok(b)) if a == b)
    }
}

/// Where `within` puts a path beneath `mount_point`.
fn at(mount_point: &Path, within: &str) -> PathBuf {
    within
        .split('/')
        .filter(|part| !part.is_empty())
        .fold(mount_point.to_path_buf(), |path, part| path.join(part))
}

#[test]
fn a_directory_is_on_one_volume_named_the_same_every_time() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path();

    let first = volume_of(path).expect("its volume");
    let again = volume_of(path).expect("its volume");

    assert_eq!(first, again, "the same answer every time");
    assert!(
        same_directory(&at(&first.mount_point, &first.within), path),
        "{} within {} is {}",
        first.within,
        first.mount_point.display(),
        path.display()
    );
    assert!(
        !first.within.is_empty(),
        "a temporary directory is no mount point"
    );
    assert!(
        !first.within.contains('\\'),
        "`/`-separated: {}",
        first.within
    );
}

#[test]
fn a_directory_inside_another_is_on_the_same_volume_one_step_further_within() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let inner = directory.path().join("inner");
    std::fs::create_dir(&inner).expect("a directory");

    let outer = volume_of(directory.path()).expect("its volume");
    let found = volume_of(&inner).expect("its volume");

    assert_eq!(found.identity, outer.identity);
    assert_eq!(found.mount_point, outer.mount_point);
    assert_eq!(found.within, format!("{}/inner", outer.within));
}

#[test]
fn a_volume_that_is_named_is_found_where_it_is_mounted() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let found = volume_of(directory.path()).expect("its volume");
    let Some(identity) = found.identity else {
        // Nothing to look for: this machine's temporary directory is on a filesystem with no identity.
        return;
    };

    let mounted = mount_points(&identity).expect("its mount points");

    assert!(
        mounted
            .iter()
            .any(|point| same_directory(point, &found.mount_point)),
        "{} among {mounted:?}",
        found.mount_point.display()
    );
}

#[test]
fn a_volume_names_its_filesystem() {
    // Every platform says what a local disk is formatted as; a report of what ran where carries it
    // (ticket 70).
    let directory = tempfile::tempdir().expect("a temporary directory");

    let found = volume_of(directory.path()).expect("its volume");

    assert!(!found.filesystem.trim().is_empty(), "{found:?}");
}

#[test]
fn a_path_that_is_not_there_has_no_volume() {
    let directory = tempfile::tempdir().expect("a temporary directory");

    assert!(volume_of(&directory.path().join("missing")).is_err());
}

#[cfg(windows)]
#[test]
fn a_relative_path_is_refused_rather_than_answered_with_the_boot_volume() {
    let refused = volume_of(Path::new(r"relative\path")).expect_err("refused");

    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
}
