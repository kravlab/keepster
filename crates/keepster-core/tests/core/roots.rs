//! Which Root a path belongs to, and when two registrations would collide.
//!
//! Pure path logic, so it is asserted as values: the cases that matter are the ones a string prefix
//! gets wrong.

use std::path::{Path, PathBuf};

use keepster_core::{RootId, containing, overlapping};

fn roots(paths: &[(&str, &str)]) -> Vec<(RootId, PathBuf)> {
    paths
        .iter()
        .map(|(id, path)| (id.parse().expect("a ULID"), PathBuf::from(path)))
        .collect()
}

const ALPHA: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const BETA: &str = "01BX5ZZKBKACTAV9WEVGEMMVRZ";

#[test]
fn a_path_belongs_to_the_root_that_contains_it() {
    let registered = roots(&[(ALPHA, "/home/dev/projects/site")]);

    let found = containing(
        Path::new("/home/dev/projects/site/src/main.rs"),
        &registered,
    );

    assert_eq!(found, Some(ALPHA.parse().unwrap()));
}

#[test]
fn a_root_contains_its_own_directory() {
    let registered = roots(&[(ALPHA, "/home/dev/projects/site")]);

    let found = containing(Path::new("/home/dev/projects/site"), &registered);

    assert_eq!(found, Some(ALPHA.parse().unwrap()));
}

#[test]
fn a_sibling_whose_name_merely_starts_the_same_belongs_to_nobody() {
    // The case a string prefix gets wrong: `site-old` is not inside `site`.
    let registered = roots(&[(ALPHA, "/home/dev/projects/site")]);

    let found = containing(Path::new("/home/dev/projects/site-old/file"), &registered);

    assert_eq!(found, None);
}

#[test]
fn a_path_under_no_root_belongs_to_nobody() {
    let registered = roots(&[(ALPHA, "/home/dev/projects/site")]);

    assert_eq!(containing(Path::new("/etc/hosts"), &registered), None);
}

#[test]
fn the_nearest_root_wins_when_several_would_match() {
    // Roots may not overlap, so this cannot arise from registration — but a Registry read from disk is
    // not something to trust blindly, and answering with the nearest is the only defensible choice.
    let registered = roots(&[
        (ALPHA, "/home/dev/projects"),
        (BETA, "/home/dev/projects/site"),
    ]);

    let found = containing(Path::new("/home/dev/projects/site/src"), &registered);

    assert_eq!(found, Some(BETA.parse().unwrap()));
}

#[test]
fn registering_inside_an_existing_root_overlaps_it() {
    let registered = roots(&[(ALPHA, "/home/dev/projects")]);

    let clash = overlapping(Path::new("/home/dev/projects/site"), &registered);

    assert_eq!(clash, Some(ALPHA.parse().unwrap()));
}

#[test]
fn registering_a_directory_that_contains_an_existing_root_overlaps_it() {
    // The direction a containment check alone would miss: the new Root would swallow the old one.
    let registered = roots(&[(ALPHA, "/home/dev/projects/site")]);

    let clash = overlapping(Path::new("/home/dev"), &registered);

    assert_eq!(clash, Some(ALPHA.parse().unwrap()));
}

#[test]
fn registering_the_same_directory_twice_overlaps() {
    let registered = roots(&[(ALPHA, "/home/dev/projects/site")]);

    let clash = overlapping(Path::new("/home/dev/projects/site"), &registered);

    assert_eq!(clash, Some(ALPHA.parse().unwrap()));
}

#[test]
fn an_unrelated_directory_overlaps_nothing() {
    let registered = roots(&[(ALPHA, "/home/dev/projects/site")]);

    assert_eq!(overlapping(Path::new("/home/dev/notes"), &registered), None);
}
