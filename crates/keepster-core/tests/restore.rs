//! A Restore's preflight as a value (ticket 47): given what is on disk where each file would go, which
//! paths are written, which need a confirmation or a resolution first, and which are refused — decided
//! before a single byte is written, the same way on every platform.

use std::collections::BTreeMap;

use keepster_core::{
    Ancestry, Confirmation, Confirmed, Existing, Found, Issue, NameCase, Naming, Resolution,
    RootPath, Step, Unrecoverable, plan, target_of,
};

fn rp(path: &str) -> RootPath {
    RootPath::parse(path).expect("a Root-relative path")
}

/// A file whose parent is there and whose place is free.
fn absent(path: &str) -> Found {
    Found {
        path: rp(path),
        target: rp(path),
        ancestry: Ancestry::Present,
        existing: Existing::Absent,
        case_variant: None,
    }
}

/// A file in place with other content, linked once, recoverable.
fn differing(path: &str) -> Found {
    Found {
        existing: Existing::File {
            links: 1,
            links_inside: Vec::new(),
            unrecoverable: None,
        },
        ..absent(path)
    }
}

const LINUX: Naming = Naming {
    folds_case: false,
    folds_normalization: false,
    windows_names: false,
};

const MACOS: Naming = Naming {
    folds_case: true,
    folds_normalization: true,
    windows_names: false,
};

const WINDOWS: Naming = Naming {
    folds_case: true,
    folds_normalization: false,
    windows_names: true,
};

fn steps(found: Vec<Found>, naming: Naming, confirmed: &Confirmed) -> Vec<(String, Step)> {
    plan(found, naming, confirmed, &BTreeMap::new(), false)
        .paths
        .into_iter()
        .map(|path| (path.path.as_str().to_owned(), path.step))
        .collect()
}

fn only_issues(found: Found, naming: Naming, confirmed: &Confirmed) -> Vec<Issue> {
    let mut planned = plan(vec![found], naming, confirmed, &BTreeMap::new(), false);
    planned.paths.remove(0).issues
}

#[test]
fn a_free_place_is_created_and_a_differing_file_restored_without_asking() {
    assert_eq!(
        steps(
            vec![absent("new.txt"), differing("old.txt")],
            LINUX,
            &Confirmed::none()
        ),
        [
            ("new.txt".to_owned(), Step::Create),
            ("old.txt".to_owned(), Step::Restore),
        ]
    );
}

#[test]
fn a_file_already_holding_the_version_is_unchanged() {
    let same = Found {
        existing: Existing::Same,
        ..absent("same.txt")
    };

    assert_eq!(
        steps(vec![same], LINUX, &Confirmed::none()),
        [("same.txt".to_owned(), Step::Unchanged)]
    );
}

#[test]
fn each_structural_change_needs_its_own_confirmation() {
    let cases = [
        (
            Found {
                ancestry: Ancestry::Missing,
                ..absent("deep/new.txt")
            },
            Issue::ParentMissing,
            Confirmation::CreateParents,
        ),
        (
            Found {
                ancestry: Ancestry::File {
                    at: rp("deep"),
                    unrecoverable: None,
                },
                ..absent("deep/new.txt")
            },
            Issue::TypeChange { at: rp("deep") },
            Confirmation::ReplaceType,
        ),
        (
            Found {
                existing: Existing::File {
                    links: 3,
                    links_inside: vec![rp("twin.txt")],
                    unrecoverable: None,
                },
                ..absent("linked.txt")
            },
            Issue::Hardlinked {
                inside: vec![rp("twin.txt")],
                outside: 1,
            },
            Confirmation::BreakHardlinks,
        ),
        (
            Found {
                existing: Existing::File {
                    links: 1,
                    links_inside: Vec::new(),
                    unrecoverable: Some(Unrecoverable::TooLarge),
                },
                ..absent("huge.bin")
            },
            Issue::OverwriteUnrecoverable {
                why: Unrecoverable::TooLarge,
            },
            Confirmation::OverwriteUnrecoverable,
        ),
    ];

    for (found, issue, confirmation) in cases {
        assert_eq!(
            only_issues(found.clone(), LINUX, &Confirmed::none()),
            std::slice::from_ref(&issue)
        );
        assert_eq!(issue.confirmation(), Some(confirmation));
        assert!(
            only_issues(found.clone(), LINUX, &Confirmed::these([confirmation])).is_empty(),
            "{issue:?} is settled by {confirmation:?}"
        );
        assert!(
            only_issues(found, LINUX, &Confirmed::all()).is_empty(),
            "and by the blanket force"
        );
    }
}

#[test]
fn replacing_a_link_or_a_special_file_is_a_type_change_whose_old_self_cannot_be_kept() {
    let link = Found {
        existing: Existing::Other,
        ..absent("link.txt")
    };

    assert_eq!(
        only_issues(link, LINUX, &Confirmed::none()),
        [
            Issue::TypeChange { at: rp("link.txt") },
            Issue::OverwriteUnrecoverable {
                why: Unrecoverable::Unsupported
            },
        ]
    );
}

#[test]
fn a_directory_in_the_way_and_a_link_on_the_way_are_refused_whatever_is_confirmed() {
    let directory = Found {
        existing: Existing::Directory,
        ..absent("dir")
    };
    let through_a_link = Found {
        ancestry: Ancestry::Link { at: rp("linked") },
        ..absent("linked/file.txt")
    };

    let planned = plan(
        vec![directory, through_a_link],
        LINUX,
        &Confirmed::all(),
        &BTreeMap::new(),
        false,
    );

    assert!(!planned.proceeds());
    let issues: Vec<Vec<Issue>> = planned.paths.into_iter().map(|path| path.issues).collect();
    assert_eq!(
        issues,
        [
            vec![Issue::TypeConflict],
            vec![Issue::LinkCheckFailed { at: rp("linked") }],
        ]
    );
}

#[test]
fn two_files_one_name_apart_by_case_collide_where_the_destination_folds_case() {
    let both = || vec![absent("README.md"), absent("Readme.md")];

    assert_eq!(
        steps(both(), LINUX, &Confirmed::none()),
        [
            ("README.md".to_owned(), Step::Create),
            ("Readme.md".to_owned(), Step::Create),
        ],
        "two files where case is significant"
    );

    let planned = plan(both(), MACOS, &Confirmed::all(), &BTreeMap::new(), false);
    assert_eq!(
        planned.paths[0].issues,
        [Issue::NameCollision {
            with: vec![rp("Readme.md")]
        }],
        "the force does not choose for the user"
    );

    let picked = BTreeMap::from([(rp("Readme.md"), Resolution::Write)]);
    let planned = plan(both(), MACOS, &Confirmed::none(), &picked, false);
    let outcome: Vec<(String, Step)> = planned
        .paths
        .into_iter()
        .map(|path| (path.path.as_str().to_owned(), path.step))
        .collect();
    assert_eq!(
        outcome,
        [
            ("README.md".to_owned(), Step::Skipped),
            ("Readme.md".to_owned(), Step::Create),
        ]
    );
}

#[test]
fn names_apart_only_by_unicode_normalisation_collide_where_the_destination_normalises() {
    let composed = "caf\u{e9}.txt";
    let decomposed = "cafe\u{301}.txt";

    let planned = plan(
        vec![absent(composed), absent(decomposed)],
        MACOS,
        &Confirmed::none(),
        &BTreeMap::new(),
        false,
    );

    assert!(matches!(
        planned.paths[0].issues[..],
        [Issue::NameCollision { .. }]
    ));
}

#[test]
fn a_name_windows_cannot_hold_is_renamed_by_the_user() {
    for name in [
        "con.txt",
        "aux",
        "trailing.",
        "trailing ",
        "a:b",
        "what?",
        "lpt1.log",
    ] {
        assert_eq!(
            only_issues(absent(name), WINDOWS, &Confirmed::all()),
            [Issue::NameInvalid],
            "{name}"
        );
        assert!(only_issues(absent(name), LINUX, &Confirmed::none()).is_empty());
    }

    let renamed = target_of(
        &rp("dir/con.txt"),
        Some(&Resolution::RenameTo("console.txt".into())),
    );
    assert_eq!(renamed, Ok(rp("dir/console.txt")));
    let still_invalid = Found {
        target: rp("dir/nul"),
        ..absent("dir/con.txt")
    };
    assert_eq!(
        only_issues(still_invalid, WINDOWS, &Confirmed::none()),
        [Issue::NameInvalid],
        "a new name must be one the destination can hold"
    );
    for bare in ["", "sub/name", ".", ".."] {
        assert!(
            target_of(&rp("x.txt"), Some(&Resolution::RenameTo(bare.into()))).is_err(),
            "{bare:?} is not a bare name"
        );
    }
}

#[test]
fn an_existing_name_in_another_case_is_kept_or_replaced_as_the_user_says() {
    let differs = Found {
        case_variant: Some("Foo.txt".to_owned()),
        ..differing("foo.txt")
    };

    assert_eq!(
        only_issues(differs.clone(), MACOS, &Confirmed::all()),
        [Issue::NameCase {
            existing: "Foo.txt".to_owned()
        }]
    );

    for (choice, target) in [
        (NameCase::KeepExisting, "Foo.txt"),
        (NameCase::UseSnapshot, "foo.txt"),
    ] {
        let resolutions = BTreeMap::from([(rp("foo.txt"), Resolution::NameCase(choice))]);
        let mut planned = plan(
            vec![differs.clone()],
            MACOS,
            &Confirmed::none(),
            &resolutions,
            false,
        );
        let path = planned.paths.remove(0);
        assert_eq!(path.step, Step::Restore);
        assert_eq!(path.target, rp(target));
        assert_eq!(path.replaces, Some(rp("Foo.txt")));
    }

    let same = Found {
        case_variant: Some("Foo.txt".to_owned()),
        existing: Existing::Same,
        ..absent("foo.txt")
    };
    assert_eq!(
        steps(vec![same], MACOS, &Confirmed::none()),
        [("foo.txt".to_owned(), Step::Unchanged)],
        "identical content is not a question"
    );
}

#[test]
fn skipping_what_cannot_be_resolved_lets_the_rest_proceed() {
    let found = vec![
        absent("fine.txt"),
        Found {
            existing: Existing::Directory,
            ..absent("dir")
        },
        Found {
            ancestry: Ancestry::Missing,
            ..absent("deep/x.txt")
        },
    ];

    let planned = plan(found, LINUX, &Confirmed::none(), &BTreeMap::new(), true);

    assert!(planned.proceeds());
    let outcome: Vec<(Step, usize)> = planned
        .paths
        .iter()
        .map(|path| (path.step, path.issues.len()))
        .collect();
    assert_eq!(
        outcome,
        [(Step::Create, 0), (Step::Skipped, 1), (Step::Skipped, 1)],
        "each skipped path keeps the reason it was skipped for"
    );
}

#[test]
fn every_reason_has_a_stable_wire_name() {
    let named = [
        (Issue::ParentMissing, "parent_missing"),
        (Issue::TypeChange { at: rp("a") }, "type_change"),
        (
            Issue::Hardlinked {
                inside: Vec::new(),
                outside: 1,
            },
            "hardlinked",
        ),
        (
            Issue::OverwriteUnrecoverable {
                why: Unrecoverable::Excluded,
            },
            "overwrite_unrecoverable",
        ),
        (Issue::NameCollision { with: Vec::new() }, "name_collision"),
        (Issue::NameInvalid, "name_invalid"),
        (
            Issue::NameCase {
                existing: "A".to_owned(),
            },
            "name_case",
        ),
        (Issue::TypeConflict, "type_conflict"),
        (Issue::LinkCheckFailed { at: rp("a") }, "link_check_failed"),
    ];
    for (issue, name) in named {
        assert_eq!(issue.code(), name);
    }
    for confirmation in Confirmation::ALL {
        assert_eq!(
            Confirmation::parse(confirmation.as_str()),
            Some(confirmation)
        );
    }
}

#[test]
fn a_new_name_must_be_free_and_two_files_cannot_take_the_same_one() {
    let taken = Found {
        target: rp("dir/existing.txt"),
        existing: Existing::File {
            links: 1,
            links_inside: Vec::new(),
            unrecoverable: None,
        },
        ..absent("dir/con.txt")
    };
    let renamed = BTreeMap::from([(
        rp("dir/con.txt"),
        Resolution::RenameTo("existing.txt".to_owned()),
    )]);
    let mut planned = plan(vec![taken], WINDOWS, &Confirmed::all(), &renamed, false);
    assert_eq!(
        planned.paths.remove(0).issues,
        [Issue::NameInvalid],
        "a rename never overwrites what is already there"
    );

    let twins = vec![
        Found {
            target: rp("same.txt"),
            ..absent("a.txt")
        },
        Found {
            target: rp("same.txt"),
            ..absent("b.txt")
        },
    ];
    let renames = BTreeMap::from([
        (rp("a.txt"), Resolution::RenameTo("same.txt".to_owned())),
        (rp("b.txt"), Resolution::RenameTo("same.txt".to_owned())),
    ]);
    let planned = plan(twins, LINUX, &Confirmed::none(), &renames, false);
    assert!(
        planned
            .paths
            .iter()
            .all(|path| matches!(path.issues[..], [Issue::NameCollision { .. }])),
        "even where case is significant: {planned:?}"
    );
}
