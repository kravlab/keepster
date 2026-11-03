//! Registering and moving a Root whose path passes through a link (ticket 77).
//!
//! The seam is `keepster_app::roots`, against a real Registry and real directories: whether a path
//! passes through a link is the filesystem's to say, and a fake would prove nothing.

#![cfg(unix)]

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use keepster_app::roots::{self, PathModeRequired, RegisterError, RootError};
use keepster_core::{PathMode, VolumePlace};
use keepster_path::retarget_link;
use keepster_store::Registry;

/// A Registry and a place for watched directories, both under one temporary directory.
struct Fixture {
    _dir: tempfile::TempDir,
    /// Where the directories live, with no link on the way: a temporary directory may itself sit behind
    /// one, as `/tmp` does on macOS.
    tree: PathBuf,
    registry: Registry,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let base = std::fs::canonicalize(dir.path()).expect("its real path");
        let tree = base.join("tree");
        std::fs::create_dir(&tree).expect("the tree");
        let registry = Registry::open(base.join("store")).expect("an empty registry");

        Self {
            _dir: dir,
            tree,
            registry,
        }
    }

    /// A directory `name` in the tree.
    fn directory(&self, name: &str) -> PathBuf {
        let path = self.tree.join(name);
        std::fs::create_dir(&path).expect("a directory");
        path
    }

    /// A link `name` in the tree to `target`.
    fn link(&self, name: &str, target: &Path) -> PathBuf {
        let path = self.tree.join(name);
        symlink(target, &path).expect("a link");
        path
    }
}

fn nowhere(_: &Path) -> Option<VolumePlace> {
    None
}

#[test]
fn a_path_through_a_link_without_a_path_mode_is_refused_and_nothing_is_registered() {
    let fixture = Fixture::new();
    let real = fixture.directory("release");
    let link = fixture.link("current", &real);

    let refused = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link,
        None,
        &nowhere,
    );

    match refused {
        Err(RegisterError::PathModeRequired(PathModeRequired { path, real: found })) => {
            assert_eq!(path, link, "the path as typed");
            assert_eq!(found, real, "where it really is");
        }
        other => panic!("expected the path mode to be asked for, got {other:?}"),
    }
    assert!(
        fixture.registry.readable().expect("read").is_empty(),
        "nothing is registered until the person has chosen"
    );
}

#[test]
fn a_canonical_root_is_registered_at_the_real_path() {
    let fixture = Fixture::new();
    let real = fixture.directory("release");
    let link = fixture.link("current", &real);

    let record = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link,
        Some(PathMode::Canonical),
        &nowhere,
    )
    .expect("registered");

    assert_eq!(record.path, real);
    assert_eq!(record.path_mode, Some(PathMode::Canonical));
    assert_eq!(record.target, None, "the link no longer matters");
    assert_eq!(fixture.registry.readable().expect("read"), vec![record]);
}

#[test]
fn a_link_mode_root_is_registered_at_the_link_with_where_it_leads() {
    let fixture = Fixture::new();
    let real = fixture.directory("release");
    let link = fixture.link("current", &real);

    let record = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link,
        Some(PathMode::Link),
        &nowhere,
    )
    .expect("registered");

    assert_eq!(record.path, link);
    assert_eq!(record.path_mode, Some(PathMode::Link));
    assert_eq!(record.target, Some(real));
    assert_eq!(fixture.registry.readable().expect("read"), vec![record]);
}

#[test]
fn a_path_through_a_link_further_up_is_a_path_through_a_link() {
    let fixture = Fixture::new();
    let real = fixture.directory("release");
    std::fs::create_dir(real.join("app")).expect("a directory inside");
    let link = fixture.link("current", &real);

    let refused = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link.join("app"),
        None,
        &nowhere,
    );

    assert!(
        matches!(refused, Err(RegisterError::PathModeRequired(_))),
        "any component that is a link counts, got {refused:?}"
    );
}

#[test]
fn a_path_through_a_link_undone_by_a_later_dot_dot_is_a_path_through_a_link() {
    let fixture = Fixture::new();
    fixture.directory("plain");
    let real = fixture.directory("release");
    let link = fixture.link("current", &real);
    // Where it really is and how `..` reads by name agree, yet the path goes through the link: pointed
    // one level deeper, the same path would lead somewhere else.
    let spelt = link.join("..").join("plain");

    let refused = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &spelt,
        None,
        &nowhere,
    );

    assert!(
        matches!(refused, Err(RegisterError::PathModeRequired(_))),
        "a component that is a link counts wherever the path ends up, got {refused:?}"
    );
}

#[test]
fn a_path_through_no_link_records_no_path_mode_even_when_one_is_given() {
    let fixture = Fixture::new();
    let plain = fixture.directory("plain");
    fixture.directory("other");
    // `..` resolved by name is where it resolves on disk: not a link.
    let spelt = fixture.tree.join("other").join("..").join("plain");

    let record = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &spelt,
        Some(PathMode::Link),
        &nowhere,
    )
    .expect("registered");

    assert_eq!(record.path_mode, None);
    assert_eq!(record.target, None);
    assert_eq!(std::fs::canonicalize(&record.path).ok(), Some(plain));
}

#[test]
fn a_canonical_root_unwatched_is_watched_again_under_its_identity_through_the_link() {
    let fixture = Fixture::new();
    let real = fixture.directory("release");
    let link = fixture.link("current", &real);
    let first = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link,
        Some(PathMode::Canonical),
        &nowhere,
    )
    .expect("registered");
    fixture.registry.remove(first.id).expect("unwatched");

    let again = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
        &link,
        Some(PathMode::Canonical),
        &nowhere,
    )
    .expect("registered again");

    assert_eq!(
        again.id, first.id,
        "matched on the path it would be stored at"
    );
}

#[test]
fn a_root_unwatched_is_watched_again_under_its_identity_through_the_link_in_the_other_mode() {
    let fixture = Fixture::new();
    let real = fixture.directory("release");
    let link = fixture.link("current", &real);
    let first = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link,
        Some(PathMode::Link),
        &nowhere,
    )
    .expect("registered");
    fixture.registry.remove(first.id).expect("unwatched");

    let canonical = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
        &link,
        Some(PathMode::Canonical),
        &nowhere,
    )
    .expect("registered again");

    assert_eq!(
        canonical.id, first.id,
        "matched on where the link led when it was unwatched"
    );
    assert_eq!(canonical.path, real);
    assert_eq!(canonical.path_mode, Some(PathMode::Canonical));
    fixture.registry.remove(first.id).expect("unwatched");

    let linked = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2),
        &link,
        Some(PathMode::Link),
        &nowhere,
    )
    .expect("registered again");

    assert_eq!(linked.id, first.id, "matched on where it really is");
    assert_eq!(linked.path, link);
    assert_eq!(linked.target, Some(real));
    assert_eq!(
        fixture.registry.readable().expect("read"),
        vec![linked],
        "one Root, no second identity"
    );
}

#[test]
fn a_root_unwatched_at_the_path_itself_is_matched_before_one_whose_link_led_there() {
    let fixture = Fixture::new();
    let real = fixture.directory("release");
    let link = fixture.link("current", &real);
    let at_path = keepster_store::RootRecord {
        id: keepster_core::RootId::generate(SystemTime::UNIX_EPOCH),
        path: real.clone(),
        volume: None,
        path_mode: None,
        target: None,
    };
    // Unwatched later, so the newer of the two.
    let led_there = keepster_store::RootRecord {
        id: keepster_core::RootId::generate(
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
        ),
        path: link,
        volume: None,
        path_mode: Some(PathMode::Link),
        target: Some(real.clone()),
    };
    for record in [&at_path, &led_there] {
        fixture.registry.insert(record).expect("recorded");
        fixture.registry.remove(record.id).expect("unwatched");
    }

    let again = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2),
        &real,
        None,
        &nowhere,
    )
    .expect("registered again");

    assert_eq!(
        again.id, at_path.id,
        "the path as recorded is matched first"
    );
}

#[test]
fn a_move_through_a_link_obeys_the_same_rule() {
    let fixture = Fixture::new();
    let start = fixture.directory("start");
    let real = fixture.directory("release");
    let link = fixture.link("current", &real);
    let record = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &start,
        None,
        &nowhere,
    )
    .expect("registered");

    let refused = roots::move_root(&fixture.registry, record.id, &link, None, &nowhere);
    match refused {
        Err(RootError::PathModeRequired(PathModeRequired { path, real: found })) => {
            assert_eq!(path, link);
            assert_eq!(found, real);
        }
        other => panic!("expected the path mode to be asked for, got {other:?}"),
    }
    assert_eq!(
        fixture.registry.readable().expect("read"),
        vec![record.clone()],
        "a refused move leaves the Root where it was"
    );

    let linked = roots::move_root(
        &fixture.registry,
        record.id,
        &link,
        Some(PathMode::Link),
        &nowhere,
    )
    .expect("moved");
    assert_eq!(linked.path, link);
    assert_eq!(linked.path_mode, Some(PathMode::Link));
    assert_eq!(linked.target, Some(real.clone()));

    let canonical = roots::move_root(
        &fixture.registry,
        record.id,
        &link,
        Some(PathMode::Canonical),
        &nowhere,
    )
    .expect("moved");
    assert_eq!(canonical.path, real);
    assert_eq!(canonical.path_mode, Some(PathMode::Canonical));
    assert_eq!(canonical.target, None);

    let back = roots::move_root(
        &fixture.registry,
        record.id,
        &start,
        Some(PathMode::Link),
        &nowhere,
    )
    .expect("moved back");
    assert_eq!(
        back.path_mode, None,
        "a path through no link has no path mode"
    );
    assert_eq!(back.target, None);

    roots::move_root(
        &fixture.registry,
        record.id,
        &link,
        Some(PathMode::Link),
        &nowhere,
    )
    .expect("moved through the link again");
    let plain = roots::move_root(&fixture.registry, record.id, &start, None, &nowhere)
        .expect("moved from the link to a plain path");
    assert_eq!(plain.path, start);
    assert_eq!(
        plain.path_mode, None,
        "following a link before leaves no path mode behind"
    );
    assert_eq!(plain.target, None, "nor where the link led");
}

#[test]
fn a_link_mode_root_whose_real_path_lies_inside_another_root_is_refused() {
    let fixture = Fixture::new();
    let real = fixture.directory("release");
    std::fs::create_dir(real.join("app")).expect("a directory inside");
    let link = fixture.link("current", &real);
    let existing = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &real,
        None,
        &nowhere,
    )
    .expect("registered");

    let refused = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link.join("app"),
        Some(PathMode::Link),
        &nowhere,
    );

    match refused {
        Err(RegisterError::Overlaps {
            existing: found, ..
        }) => assert_eq!(found, existing.id),
        other => panic!("expected an overlap by real paths, got {other:?}"),
    }
    assert_eq!(
        fixture.registry.readable().expect("read"),
        vec![existing],
        "nothing else is registered"
    );
}

#[test]
fn a_root_inside_the_real_path_of_a_link_mode_root_is_refused() {
    let fixture = Fixture::new();
    let real = fixture.directory("release");
    std::fs::create_dir(real.join("app")).expect("a directory inside");
    let link = fixture.link("current", &real);
    let existing = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link,
        Some(PathMode::Link),
        &nowhere,
    )
    .expect("registered");

    let refused = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &real.join("app"),
        None,
        &nowhere,
    );

    match refused {
        Err(RegisterError::Overlaps {
            existing: found, ..
        }) => assert_eq!(found, existing.id),
        other => panic!("expected an overlap by real paths, got {other:?}"),
    }
}

#[test]
fn a_move_onto_a_path_whose_real_path_overlaps_another_root_is_refused() {
    let fixture = Fixture::new();
    let start = fixture.directory("start");
    let real = fixture.directory("release");
    std::fs::create_dir(real.join("app")).expect("a directory inside");
    let link = fixture.link("current", &real);
    let other = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &real,
        None,
        &nowhere,
    )
    .expect("registered");
    let record = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
        &start,
        None,
        &nowhere,
    )
    .expect("registered");

    let refused = roots::move_root(
        &fixture.registry,
        record.id,
        &link.join("app"),
        Some(PathMode::Link),
        &nowhere,
    );

    match refused {
        Err(RootError::Overlaps { existing, .. }) => assert_eq!(existing, other.id),
        other => panic!("expected an overlap by real paths, got {other:?}"),
    }
    assert!(
        fixture.registry.readable().expect("read").contains(&record),
        "a refused move leaves the Root where it was"
    );
}

#[test]
fn a_link_mode_root_whose_link_now_leads_inside_another_root_overlaps_it() {
    let fixture = Fixture::new();
    let release = fixture.directory("release");
    let other = fixture.directory("other");
    std::fs::create_dir(other.join("inner")).expect("a directory inside");
    let link = fixture.link("current", &release);
    let linked = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link,
        Some(PathMode::Link),
        &nowhere,
    )
    .expect("registered");
    let existing = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
        &other,
        None,
        &nowhere,
    )
    .expect("registered");
    let records = fixture.registry.readable().expect("read");
    assert_eq!(
        roots::overlapped_now(&linked, &records),
        None,
        "where it was registered, it overlaps nothing"
    );

    retarget_link(&link, &other.join("inner"));

    assert_eq!(roots::overlapped_now(&linked, &records), Some(existing.id));
    assert_eq!(
        roots::overlapped_now(&existing, &records),
        None,
        "the Root it leads into is not the one overlapping"
    );
}

#[test]
fn a_link_mode_root_whose_link_is_pointed_elsewhere_records_where_it_leads_and_its_volume_anew() {
    let fixture = Fixture::new();
    let release = fixture.directory("release");
    let next = fixture.directory("next");
    let link = fixture.link("current", &release);
    let first = VolumePlace {
        identity: "uuid:first".to_owned(),
        within: "release".to_owned(),
    };
    let record = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link,
        Some(PathMode::Link),
        &|_| Some(first.clone()),
    )
    .expect("registered");
    let second = VolumePlace {
        identity: "uuid:second".to_owned(),
        within: "next".to_owned(),
    };
    let unchanged = roots::retarget(&fixture.registry, record.id, &|_| Some(second.clone()))
        .expect("looked at");
    assert_eq!(unchanged, None, "the link leads where it was recorded");
    assert_eq!(
        fixture.registry.readable().expect("read"),
        vec![record.clone()]
    );

    retarget_link(&link, &next);
    let retargeted = roots::retarget(&fixture.registry, record.id, &|_| Some(second.clone()))
        .expect("re-recorded");

    let expected = keepster_store::RootRecord {
        target: Some(next),
        volume: Some(second),
        ..record
    };
    assert_eq!(retargeted, Some(expected.clone()));
    assert_eq!(
        fixture.registry.readable().expect("read"),
        vec![expected],
        "the path and the path mode are kept; where it leads and its volume are recorded anew"
    );
}

#[test]
fn a_link_mode_root_whose_link_leads_inside_another_root_is_not_re_recorded() {
    let fixture = Fixture::new();
    let release = fixture.directory("release");
    let other = fixture.directory("other");
    std::fs::create_dir(other.join("inner")).expect("a directory inside");
    let link = fixture.link("current", &release);
    let record = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link,
        Some(PathMode::Link),
        &nowhere,
    )
    .expect("registered");
    roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
        &other,
        None,
        &nowhere,
    )
    .expect("registered");
    retarget_link(&link, &other.join("inner"));

    let retargeted = roots::retarget(&fixture.registry, record.id, &nowhere).expect("looked at");

    assert_eq!(
        retargeted, None,
        "an overlapping Root is Unavailable, not followed"
    );
    assert!(
        fixture.registry.readable().expect("read").contains(&record),
        "where it led before is kept, for the link pointed back"
    );
}

#[test]
fn a_link_mode_root_whose_link_now_leads_to_another_root_itself_overlaps_it() {
    let fixture = Fixture::new();
    let release = fixture.directory("release");
    let other = fixture.directory("other");
    let link = fixture.link("current", &release);
    let linked = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link,
        Some(PathMode::Link),
        &nowhere,
    )
    .expect("registered");
    let existing = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
        &other,
        None,
        &nowhere,
    )
    .expect("registered");

    retarget_link(&link, &other);

    let records = fixture.registry.readable().expect("read");
    assert_eq!(roots::overlapped_now(&linked, &records), Some(existing.id));
}

#[test]
fn a_link_mode_root_is_walked_where_its_link_led_when_looked_at() {
    let fixture = Fixture::new();
    let release = fixture.directory("release");
    let next = fixture.directory("next");
    let plain = fixture.directory("plain");
    let link = fixture.link("current", &release);
    let linked = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH,
        &link,
        Some(PathMode::Link),
        &nowhere,
    )
    .expect("registered");
    let other = roots::register(
        &fixture.registry,
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
        &plain,
        None,
        &nowhere,
    )
    .expect("registered");
    let records = fixture.registry.readable().expect("read");

    let walked = roots::walk_path(&linked, &records);
    // Pointed elsewhere between the look and the walk: the walk still goes where the look found.
    retarget_link(&link, &next);

    assert_eq!(walked, Ok(release), "the real path, not the link");
    assert_eq!(
        roots::walk_path(&other, &records),
        Ok(plain.clone()),
        "any other Root is walked at its path"
    );
    retarget_link(&link, &plain);
    assert_eq!(
        roots::walk_path(&linked, &records),
        Err(other.id),
        "nothing is walked inside another Root"
    );
}
