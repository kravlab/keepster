//! The committed thresholds and the profiles they were calibrated against (ticket 30, ticket 31): a
//! fixture's shape cannot move without the thresholds file moving in the same commit.

use std::path::Path;
use std::time::Duration;

use keepster_testkit::bulk::{
    BENCH_PROFILES, Churn, Contents, Excluded, Profile, SMALL, SizeBucket, canonical, content,
    digest, plan,
};
use keepster_testkit::thresholds::{Mismatch, Thresholds, ThresholdsError, calibrated_limit};

#[test]
fn the_committed_thresholds_were_calibrated_against_the_profiles_as_they_are() {
    let committed = Thresholds::committed().expect("the committed thresholds file parses");

    // On failure: a profile, or the generator behind it, moved. Recalibrate with `make bench-calibrate`
    // on the reference machine and commit the new thresholds file with the change that moved it.
    for (profile, contents) in BENCH_PROFILES {
        assert_eq!(
            committed.digest(profile.name),
            Some(digest(profile, *contents).as_str()),
            "the {} profile moved",
            profile.name
        );
    }
    // A digest for a profile the tripwire no longer runs is a calibration nobody checks.
    let mut benched: Vec<&str> = BENCH_PROFILES
        .iter()
        .map(|(profile, _)| profile.name)
        .collect();
    benched.sort_unstable();
    assert_eq!(committed.profiles().collect::<Vec<_>>(), benched);
}

#[test]
fn every_benched_profile_is_digested_with_its_contents_but_the_ceiling() {
    let benched: Vec<(&str, Contents)> = BENCH_PROFILES
        .iter()
        .map(|(profile, contents)| (profile.name, *contents))
        .collect();

    assert_eq!(
        benched,
        [
            ("small", Contents::Hashed),
            ("ceiling", Contents::Omitted),
            ("flat", Contents::Hashed),
            ("exclusion", Contents::Hashed),
        ]
    );
}

#[test]
fn the_digest_follows_every_parameter_and_nothing_else() {
    let tiny = tiny();
    let same = digest(&tiny, Contents::Hashed);
    assert_eq!(same, digest(&tiny, Contents::Hashed));

    let moved = [
        Profile {
            name: "other",
            ..tiny
        },
        Profile { seed: 8, ..tiny },
        Profile { bytes: 501, ..tiny },
        Profile {
            compressibility_permille: 2_701,
            ..tiny
        },
        Profile {
            noise_permille: 36,
            ..tiny
        },
        Profile {
            churn: Churn {
                interval: Duration::from_secs(61),
                ..tiny.churn
            },
            ..tiny
        },
    ];
    for profile in moved {
        assert_ne!(digest(&profile, Contents::Hashed), same, "{profile:?}");
    }
}

#[test]
fn the_canonical_text_carries_each_files_content_only_when_hashed() {
    // The contents enter the small profile's digest so that a change to how content is written — the
    // vocabulary, the line lengths — moves it even though no parameter moved.
    let tiny = tiny();
    let tree = plan(&tiny);
    let file = tree
        .files
        .iter()
        .find(|file| file.size > 0)
        .expect("a file with content");
    let blob = git2::Oid::hash_object(git2::ObjectType::Blob, &content(&tiny, file))
        .expect("hashing bytes in memory")
        .to_string();
    let line = format!("file {} {} {}", file.path, file.size, file.content_seed);

    let hashed = canonical(&tiny, Contents::Hashed);
    let omitted = canonical(&tiny, Contents::Omitted);

    assert!(
        hashed.lines().any(|text| text == format!("{line} {blob}")),
        "{hashed}"
    );
    assert!(omitted.lines().any(|text| text == line), "{omitted}");
    assert!(!omitted.contains(&blob), "{omitted}");
    assert_ne!(
        digest(&tiny, Contents::Hashed),
        digest(&tiny, Contents::Omitted)
    );
}

#[test]
#[should_panic(expected = "whitespace")]
fn a_profile_named_with_whitespace_is_refused() {
    // The canonical text is a line per fact with fields split by spaces: a name holding either would
    // let two different profiles write the same text.
    plan(&Profile {
        name: "two words",
        ..tiny()
    });
}

#[test]
#[should_panic(expected = "whitespace")]
fn an_excluded_directory_named_with_a_line_break_is_refused() {
    let tiny = tiny();
    plan(&Profile {
        excluded: Excluded {
            name: "cache\nfile",
            ..tiny.excluded
        },
        ..tiny
    });
}

#[test]
#[should_panic(expected = "a name")]
fn a_profile_without_a_name_is_refused() {
    plan(&Profile { name: "", ..tiny() });
}

#[test]
fn a_thresholds_file_is_read_from_its_text() {
    let thresholds = Thresholds::parse(
        "[digests]\nsmall = \"abc\"\n\n[limits]\n\"read.diff\" = 12.5\n",
        origin(),
    )
    .expect("a valid thresholds file");

    assert_eq!(thresholds.digest("small"), Some("abc"));
    assert_eq!(thresholds.digest("ceiling"), None);
    assert_eq!(thresholds.profiles().collect::<Vec<_>>(), ["small"]);
    assert_eq!(thresholds.limit("read.diff"), Some(12.5));
    assert_eq!(thresholds.limit("read.version"), None);
}

#[test]
fn a_thresholds_file_that_is_not_one_is_refused_naming_it() {
    for text in [
        "[digests]\n\n[limits]\n\"read.version\" = \"fast\"\n",
        "[digests]\nlimits = 3\n",
        "[digests]\n\n[limits]\n\n[extra]\n",
    ] {
        let refused = Thresholds::parse(text, origin()).expect_err(text);
        assert!(
            matches!(&refused, ThresholdsError::Malformed { path, .. } if path == origin()),
            "{text}: {refused:?}"
        );
        assert!(refused.to_string().contains("thresholds.toml"), "{refused}");
    }
}

#[test]
fn a_limit_that_is_not_a_finite_number_above_zero_is_refused() {
    // A limit of nothing or less would fail every run, and one of no number or no end could fail none.
    for value in ["0.0", "-1.0", "nan", "inf"] {
        let text = format!("[digests]\n\n[limits]\n\"read.version\" = {value}\n");
        let refused = Thresholds::parse(&text, origin()).expect_err(value);
        assert!(
            matches!(
                &refused,
                ThresholdsError::InvalidLimit { path, line }
                    if path == origin() && line == "read.version"
            ),
            "{value}: {refused:?}"
        );
    }
}

#[test]
fn an_unreadable_thresholds_file_is_refused_naming_it() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let missing = directory.path().join("thresholds.toml");

    let refused = Thresholds::read(&missing).expect_err("no file there");

    assert!(
        matches!(&refused, ThresholdsError::Unreadable { path, .. } if *path == missing),
        "{refused:?}"
    );
}

#[test]
fn every_gated_line_needs_a_limit_and_every_limit_a_gated_line() {
    let thresholds = Thresholds::parse(
        "[digests]\n\n[limits]\n\"read.version\" = 1.0\n\"read.retired\" = 2.0\n\
         \"capture.retired\" = 3.0\n",
        origin(),
    )
    .expect("a valid thresholds file");

    // Unlimited lines in the order given, each once; then orphaned limits in name order.
    assert_eq!(
        thresholds.mismatches(&["read.version", "read.diff", "capture.new", "read.diff"]),
        vec![
            Mismatch::Unlimited("read.diff".to_owned()),
            Mismatch::Unlimited("capture.new".to_owned()),
            Mismatch::Orphaned("capture.retired".to_owned()),
            Mismatch::Orphaned("read.retired".to_owned()),
        ]
    );
    assert!(
        thresholds
            .mismatches(&["read.version", "read.retired", "capture.retired"])
            .is_empty()
    );
}

/// Where the test texts claim to come from.
fn origin() -> &'static Path {
    Path::new("bench/thresholds.toml")
}

/// Ten files in three directories and a short stream: cheap to digest with its contents.
fn tiny() -> Profile {
    Profile {
        name: "tiny",
        seed: 7,
        files: 10,
        directories: 3,
        bytes: 500,
        fan_out: 2,
        sizes: &[SizeBucket {
            permille: 1000,
            smallest: 0,
            largest: 100,
        }],
        excluded: Excluded {
            name: "cache",
            files: 1,
            directories: 0,
            file_size: 1,
        },
        churn: Churn {
            steps: 2,
            interval: Duration::from_secs(60),
            modified: 2,
            created: 1,
            deleted: 1,
        },
        ..SMALL
    }
}

#[test]
fn a_calibrated_limit_is_twice_the_median_rounded_up_to_two_significant_digits() {
    // Twice, for the headroom ticket 30 sizes to this design's failures, every one a multiple; rounded
    // up, so the committed number is one a person reads and no limit falls below twice what was measured.
    for (median, limit) in [
        (12.3, 25.0),
        (0.43, 0.86),
        (1.42, 2.9),
        (0.0123, 0.025),
        (450.0, 900.0),
        (4_567.0, 9_200.0),
        (5.0, 10.0),
    ] {
        assert_eq!(calibrated_limit(median), limit, "{median}");
    }
}

#[test]
fn a_calibrated_limit_is_never_below_twice_the_median() {
    for median in [0.001, 0.0137, 0.5, 1.0, 3.3333, 49.99, 123.456, 98_765.4] {
        assert!(calibrated_limit(median) >= 2.0 * median, "{median}");
    }
}

#[test]
fn rendered_thresholds_read_back_as_themselves() {
    let thresholds = Thresholds::calibrated(
        [("small".to_owned(), "abc".to_owned())].into(),
        [
            ("read.diff".to_owned(), 25.0),
            ("store.growth".to_owned(), 2.9),
            ("read.version".to_owned(), 0.025),
        ]
        .into(),
    );

    let text = thresholds.render();

    assert_eq!(
        Thresholds::parse(&text, Path::new("bench/thresholds.toml")).expect("a valid file"),
        thresholds
    );
    assert!(text.contains("\n\"read.diff\" = 25.0\n"), "{text}");
}

#[test]
fn the_committed_thresholds_file_is_as_calibration_writes_it() {
    // `make bench-calibrate` writes the whole file, so a hand edit would be lost at the next calibration
    // without anyone noticing: the committed file must already be what it writes.
    let path = Thresholds::committed_path();
    let text = std::fs::read_to_string(&path).expect("the committed thresholds file");

    assert_eq!(
        Thresholds::parse(&text, &path).expect("it parses").render(),
        text
    );
}
