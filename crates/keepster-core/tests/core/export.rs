//! An Export's preflight as a value (ticket 58, ticket 19): Restore's rules less the three that exist
//! because Restore overwrites the Working Tree, and two of its own — overwriting is confirmed, and so is
//! writing into a Watched Root.

use std::collections::BTreeMap;

use keepster_core::{
    Ancestry, Confirmation, Confirmed, Existing, Found, Issue, Naming, RootPath, Step,
    Unrecoverable, WithheldReason, export_plan, fence_other_roots,
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
        withheld: None,
        modes: None,
    }
}

/// A file already in place with other content, under other names too.
fn differing(path: &str) -> Found {
    Found {
        existing: Existing::File {
            links: 3,
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

/// Every path's step and the wire names of its issues.
fn planned(found: Vec<Found>, confirmed: &Confirmed, into_root: bool) -> Vec<(Step, Vec<String>)> {
    export_plan(found, LINUX, confirmed, &BTreeMap::new(), false, into_root)
        .paths
        .into_iter()
        .map(|path| {
            (
                path.step,
                path.issues
                    .iter()
                    .map(|issue| issue.code().to_owned())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn directories_that_are_not_there_are_made_without_asking() {
    let found = Found {
        ancestry: Ancestry::Missing,
        ..absent("deep/down/notes.txt")
    };

    assert_eq!(
        planned(vec![found], &Confirmed::none(), false),
        [(Step::Create, Vec::new())],
        "making a directory destroys nothing"
    );
}

#[test]
fn overwriting_is_asked_about_and_a_file_s_other_names_are_not() {
    let blocked = planned(vec![differing("notes.txt")], &Confirmed::none(), false);
    assert_eq!(
        blocked,
        [(Step::Blocked, vec!["overwrite".to_owned()])],
        "off by default: outside a Root nothing brings it back"
    );

    let confirmed = Confirmed::these([Confirmation::Overwrite]);
    assert_eq!(
        planned(vec![differing("notes.txt")], &confirmed, false),
        [(Step::Restore, Vec::new())]
    );
    assert_eq!(
        planned(vec![differing("notes.txt")], &Confirmed::all(), false),
        [(Step::Restore, Vec::new())],
        "the blanket force covers it"
    );
}

#[test]
fn a_file_where_a_directory_goes_is_overwritten_only_when_both_are_confirmed() {
    let found = Found {
        ancestry: Ancestry::File {
            at: rp("docs"),
            unrecoverable: None,
        },
        ..absent("docs/notes.txt")
    };

    let codes = &planned(vec![found.clone()], &Confirmed::none(), false)[0].1;
    assert_eq!(codes, &["type_change", "overwrite"]);
    let confirmed = Confirmed::these([Confirmation::ReplaceType, Confirmation::Overwrite]);
    assert_eq!(planned(vec![found], &confirmed, false)[0].0, Step::Create);
}

#[test]
fn a_directory_in_the_way_is_refused_whatever_is_confirmed() {
    let found = Found {
        existing: Existing::Directory,
        ..absent("notes.txt")
    };

    assert_eq!(
        planned(vec![found], &Confirmed::all(), false),
        [(Step::Blocked, vec!["type_conflict".to_owned()])]
    );
}

#[test]
fn writing_into_a_root_is_asked_about_for_every_path_that_writes() {
    let found = vec![
        absent("new.txt"),
        differing("old.txt"),
        Found {
            existing: Existing::Same,
            ..absent("same.txt")
        },
    ];

    let asked = planned(found.clone(), &Confirmed::none(), true);
    assert_eq!(asked[0], (Step::Blocked, vec!["dest_in_root".to_owned()]));
    assert_eq!(
        asked[1],
        (
            Step::Blocked,
            vec!["overwrite".to_owned(), "dest_in_root".to_owned()]
        )
    );
    assert_eq!(
        asked[2],
        (Step::Unchanged, Vec::new()),
        "a file already holding the version writes nothing"
    );

    let confirmed = Confirmed::these([Confirmation::DestInRoot, Confirmation::Overwrite]);
    let steps: Vec<Step> = planned(found, &confirmed, true)
        .into_iter()
        .map(|(step, _)| step)
        .collect();
    assert_eq!(steps, [Step::Create, Step::Restore, Step::Unchanged]);
}

#[test]
fn inside_a_root_what_the_safety_snapshot_cannot_hold_is_asked_about_too() {
    let found = Found {
        existing: Existing::File {
            links: 1,
            links_inside: Vec::new(),
            unrecoverable: Some(Unrecoverable::TooLarge),
        },
        ..absent("big.iso")
    };

    let confirmed = Confirmed::these([Confirmation::DestInRoot, Confirmation::Overwrite]);
    assert_eq!(
        planned(vec![found], &confirmed, true),
        [(Step::Blocked, vec!["overwrite_unrecoverable".to_owned()])]
    );
}

#[test]
fn a_withheld_path_is_skipped_with_its_reason_never_written_empty() {
    let found = Found {
        withheld: Some(WithheldReason::TooLarge),
        ..absent("video.mp4")
    };

    let plan = export_plan(
        vec![found, absent("notes.txt")],
        LINUX,
        &Confirmed::none(),
        &BTreeMap::new(),
        false,
        false,
    );

    assert!(plan.proceeds(), "policy is not failure: the rest goes on");
    assert_eq!(plan.paths[0].step, Step::Skipped);
    assert_eq!(
        plan.paths[0].issues,
        [Issue::Withheld {
            reason: WithheldReason::TooLarge
        }]
    );
    assert_eq!(plan.paths[1].step, Step::Create);
}

#[test]
fn the_new_confirmations_have_stable_wire_names() {
    assert_eq!(Issue::Overwrite.code(), "overwrite");
    assert_eq!(Issue::DestInRoot.code(), "dest_in_root");
    assert_eq!(
        Issue::Overwrite.confirmation(),
        Some(Confirmation::Overwrite)
    );
    assert_eq!(
        Confirmation::parse("dest_in_root"),
        Some(Confirmation::DestInRoot)
    );
}

#[test]
fn a_file_landing_in_another_root_beneath_the_destination_is_refused_whatever_is_confirmed() {
    let mut plan = export_plan(
        vec![absent("proj/notes.txt"), absent("notes.txt")],
        LINUX,
        &Confirmed::all(),
        &BTreeMap::new(),
        false,
        false,
    );

    fence_other_roots(
        &mut plan,
        |target| target.as_bytes().starts_with(b"proj/"),
        false,
    );

    assert_eq!(plan.paths[0].step, Step::Blocked);
    assert_eq!(plan.paths[0].issues, [Issue::OtherRoot]);
    assert_eq!(
        Issue::OtherRoot.confirmation(),
        None,
        "no confirmation lets it through"
    );
    assert_eq!(plan.paths[1].step, Step::Create);

    let mut skipping = export_plan(
        vec![absent("proj/notes.txt")],
        LINUX,
        &Confirmed::all(),
        &BTreeMap::new(),
        true,
        false,
    );
    fence_other_roots(&mut skipping, |_| true, true);
    assert_eq!(skipping.paths[0].step, Step::Skipped);
}
