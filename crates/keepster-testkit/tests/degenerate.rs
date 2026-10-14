//! The degenerate catalogue (ticket 69): each case makes exactly what it declares, and a case the
//! platform cannot hold says why, at compile time, rather than being skipped at run time (ticket 70).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use keepster_core::DEFAULT_MAX_SIZE;
use keepster_testkit::degenerate::{CATALOGUE, Case, Here, Kind, Outcome, Unmade, make, native};
use keepster_testkit::host;
use keepster_testkit::platform::{Column, REPORT_DIR, Row, report_dir};

mod common;

use common::scratch;

/// The test the platform table records (ticket 70): each case's outcome on this machine is written as
/// this platform's column when [`REPORT_DIR`] names a directory, and nothing is written otherwise.
#[test]
fn every_case_makes_exactly_the_entries_it_declares() {
    let mut rows = Vec::new();
    for case in CATALOGUE {
        let directory = scratch();

        let outcome = match make(case, directory.path()) {
            Ok(()) => {
                let declared: BTreeSet<Vec<u8>> = case
                    .entries
                    .iter()
                    .map(|entry| entry.path.to_vec())
                    .collect();
                assert_eq!(found(directory.path()), declared, "{}", case.name);
                for entry in case.entries {
                    let path = directory
                        .path()
                        .join(native(entry.path).expect("a name possible here"));
                    let metadata = fs::symlink_metadata(&path)
                        .unwrap_or_else(|error| panic!("{}: {error}", case.name));
                    match entry.kind {
                        Kind::File { size } => {
                            assert!(metadata.is_file(), "{}", case.name);
                            assert_eq!(metadata.len(), size, "{}", case.name);
                        }
                        Kind::Directory => assert!(metadata.is_dir(), "{}", case.name),
                        Kind::Symlink { target, directory } => {
                            assert!(metadata.is_symlink(), "{}", case.name);
                            // Through the link: a directory exactly when declared one, and a dangling
                            // link resolves to nothing.
                            assert_eq!(
                                fs::metadata(&path).is_ok_and(|followed| followed.is_dir()),
                                directory,
                                "{}",
                                case.name
                            );
                            assert_eq!(
                                fs::read_link(&path).expect("a link"),
                                native(target).expect("a name possible here"),
                                "{}",
                                case.name
                            );
                        }
                    }
                }
                Outcome::Ran
            }
            Err(unmade) => {
                match &unmade {
                    Unmade::Impossible(reason) => {
                        assert!(matches!(case.here, Here::Impossible(_)), "{}", case.name);
                        assert!(!reason.trim().is_empty(), "{}", case.name);
                    }
                    Unmade::Unavailable(reason) => {
                        // A capability this machine lacks, which only a case that names one may:
                        // reported with its reason, never a failure.
                        assert!(case.capability.is_some(), "{}: {reason}", case.name);
                        assert!(!reason.trim().is_empty(), "{}", case.name);
                    }
                    Unmade::Failed(_) => {}
                }
                unmade
                    .outcome()
                    .unwrap_or_else(|error| panic!("{}: {error}", case.name))
            }
        };
        rows.push(Row {
            case: case.name.to_owned(),
            outcome,
        });
    }

    let reports =
        report_dir(std::env::var_os(REPORT_DIR)).unwrap_or_else(|error| panic!("{error}"));
    if let Some(reports) = reports {
        let column = Column {
            host: host::detect(scratch().path()),
            rows,
        };
        column
            .write(&reports)
            .unwrap_or_else(|error| panic!("the column cannot be written: {error}"));
    }
}

#[test]
fn the_catalogue_holds_each_case_ticket_69_names_once() {
    let names: Vec<&str> = CATALOGUE.iter().map(|case| case.name).collect();

    assert_eq!(
        names,
        [
            "long_name",
            "non_utf8_name",
            "symlinks",
            "empty_directories",
            "past_the_cap"
        ]
    );
    for case in CATALOGUE {
        assert!(!case.summary.trim().is_empty(), "{}", case.name);
        assert!(!case.source.trim().is_empty(), "{}", case.name);
        assert!(!case.entries.is_empty(), "{}", case.name);
    }
}

#[test]
fn only_the_cases_that_need_a_capability_name_one() {
    // A machine may refuse a name that is not UTF-8 or a symbolic link; nothing else may be unavailable.
    let needing: Vec<&str> = CATALOGUE
        .iter()
        .filter(|case| case.capability.is_some())
        .map(|case| case.name)
        .collect();

    assert_eq!(needing, ["non_utf8_name", "symlinks"]);
}

#[test]
fn the_symlinks_are_to_a_file_to_a_directory_and_to_nothing() {
    let case = case("symlinks");
    let declared: Vec<&[u8]> = case.entries.iter().map(|entry| entry.path).collect();
    let links: Vec<(&[u8], bool)> = case
        .entries
        .iter()
        .filter_map(|entry| match entry.kind {
            Kind::Symlink { target, directory } => Some((target, directory)),
            _ => None,
        })
        .collect();

    assert_eq!(links.len(), 3);
    assert!(
        links
            .iter()
            .any(|(target, directory)| !directory && declared.contains(target))
    );
    assert!(
        links
            .iter()
            .any(|(target, directory)| *directory && declared.contains(target))
    );
    assert!(links.iter().any(|(target, _)| !declared.contains(target)));
}

#[test]
fn the_long_name_is_at_the_255_byte_limit() {
    let case = case("long_name");

    assert_eq!(case.entries.len(), 1);
    assert_eq!(case.entries[0].path.len(), 255);
}

#[cfg(target_os = "linux")]
#[test]
fn a_name_one_byte_longer_is_refused() {
    // So the catalogued name is at the limit, not merely long.
    let directory = scratch();

    let refused =
        fs::File::create_new(directory.path().join("a".repeat(256))).expect_err("too long");

    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidFilename);
}

#[test]
fn the_non_utf8_name_is_not_utf8_and_is_impossible_only_where_names_are_not_bytes() {
    let case = case("non_utf8_name");

    assert!(std::str::from_utf8(case.entries[0].path).is_err());
    assert_eq!(matches!(case.here, Here::Possible), cfg!(unix));
    assert_eq!(native(case.entries[0].path).is_some(), cfg!(unix));
}

#[test]
fn a_declared_path_is_split_on_slashes_into_components() {
    let path = native(b"hollow/empty").expect("UTF-8");

    assert_eq!(
        path.iter().collect::<Vec<_>>(),
        [
            std::ffi::OsStr::new("hollow"),
            std::ffi::OsStr::new("empty")
        ]
    );
}

#[test]
fn the_empty_directories_hold_nothing_but_each_other() {
    let directory = scratch();
    make(case("empty_directories"), directory.path()).expect("made");

    assert_eq!(entries(&directory.path().join("empty")), 0);
    assert_eq!(entries(&directory.path().join("hollow")), 1);
    assert_eq!(entries(&directory.path().join("hollow").join("empty")), 0);
}

#[test]
fn the_file_past_the_cap_is_one_byte_over_it() {
    let case = case("past_the_cap");

    assert!(matches!(
        case.entries[0].kind,
        Kind::File { size } if size == DEFAULT_MAX_SIZE + 1
    ));
}

#[cfg(unix)]
#[test]
fn the_file_past_the_cap_takes_almost_no_disk() {
    // A hole rather than a hundred mebibytes of zeros, so the catalogue stays small to make.
    use std::os::unix::fs::MetadataExt;

    let directory = scratch();
    make(case("past_the_cap"), directory.path()).expect("made");
    let metadata = fs::metadata(directory.path().join("large.bin")).expect("the file");

    assert!(
        metadata.blocks() * 512 < 1024 * 1024,
        "{} bytes allocated",
        metadata.blocks() * 512
    );
}

#[test]
fn a_case_made_twice_is_refused_rather_than_merged() {
    // Each case is made into a directory of its own: what is already there is not part of the case.
    for case in CATALOGUE {
        let directory = scratch();
        match make(case, directory.path()) {
            Ok(_) => assert!(
                matches!(make(case, directory.path()), Err(Unmade::Failed(_))),
                "{}",
                case.name
            ),
            // Not made here, and only as a case may not be: nothing to make twice.
            Err(Unmade::Impossible(_)) => {
                assert!(matches!(case.here, Here::Impossible(_)), "{}", case.name);
            }
            Err(Unmade::Unavailable(reason)) => {
                assert!(case.capability.is_some(), "{}: {reason}", case.name);
            }
            Err(Unmade::Failed(error)) => panic!("{}: {error}", case.name),
        }
    }
}

#[test]
fn a_case_impossible_here_comes_to_impossible_with_its_reason() {
    let outcome = Unmade::Impossible("names here are UTF-16").outcome();

    assert_eq!(
        outcome.expect("an outcome"),
        Outcome::Impossible("names here are UTF-16".to_owned())
    );
}

#[test]
fn a_case_this_machine_cannot_make_comes_to_unavailable_with_its_reason() {
    let outcome = Unmade::Unavailable("no Developer Mode".to_owned()).outcome();

    assert_eq!(
        outcome.expect("an outcome"),
        Outcome::Unavailable("no Developer Mode".to_owned())
    );
}

#[test]
fn a_case_that_failed_to_be_made_comes_to_no_outcome_but_its_error() {
    // A failure is the test's to report, never a row of the table.
    let outcome = Unmade::Failed(std::io::Error::from(std::io::ErrorKind::StorageFull)).outcome();

    // The error that kept the case from being made, itself rather than one wrapping it.
    let error = outcome.expect_err("a failure");
    assert_eq!(error.kind(), std::io::ErrorKind::StorageFull);
    assert!(error.get_ref().is_none(), "{error:?}");
}

/// The catalogued case `name`.
fn case(name: &str) -> &'static Case {
    CATALOGUE
        .iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("{name} is catalogued"))
}

/// How many entries `directory` holds.
fn entries(directory: &Path) -> usize {
    fs::read_dir(directory).expect("a directory").count()
}

/// Every path under `root`, relative and `/`-joined, as bytes.
fn found(root: &Path) -> BTreeSet<Vec<u8>> {
    let mut paths = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("a directory") {
            let entry = entry.expect("an entry");
            let path = entry.path();
            if entry.file_type().expect("a type").is_dir() {
                pending.push(path.clone());
            }
            let relative = path.strip_prefix(root).expect("beneath the root");
            let parts: Vec<Vec<u8>> = relative.iter().map(bytes).collect();
            paths.insert(parts.join(&b'/'));
        }
    }
    paths
}

/// A path's component as the bytes the catalogue declares it in.
fn bytes(component: &std::ffi::OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        component.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        component
            .to_str()
            .expect("every name possible here is UTF-8")
            .as_bytes()
            .to_vec()
    }
}
