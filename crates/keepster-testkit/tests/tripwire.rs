//! The tripwire runner (ticket 30): which run a command asks for, the bound each measured line is held
//! to, what calibration commits, and — on fixtures small enough for every platform's tests — that every
//! line's workload runs, so the bench cannot rot between the runs nobody makes by hand.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use keepster_store::Git;
use keepster_testkit::bulk::{
    self, Change, Churn, EXCLUSION, Excluded, FLAT, Profile, SMALL, SizeBucket,
};
use keepster_testkit::environment;
use keepster_testkit::lines::{
    CAPTURE_FIRST_UNCACHED, CAPTURE_READ_TREE, CAPTURE_STEP, LINES, Line, PREVIEW_WALK, READ_DIFF,
    READ_VERSION, STORE_GROWTH,
};
use keepster_testkit::thresholds::{Thresholds, calibrated_limit};
use keepster_testkit::tripwire::{
    Fixtures, Measurement, Mode, Repeats, Scope, TripwireError, calibrate, estimates, measure, rows,
};
use keepster_testkit::verdict::Bound;

mod common;

use common::scratch;

#[test]
fn a_run_is_named_by_its_first_argument_and_cargo_benchs_own_flag_is_passed_over() {
    let mode = |args: &[&str]| Mode::from_args(args.iter().map(|arg| (*arg).to_owned()));

    assert_eq!(mode(&[]), Ok(Mode::Gate));
    assert_eq!(mode(&["--bench"]), Ok(Mode::Gate));
    assert_eq!(mode(&["gate", "--bench"]), Ok(Mode::Gate));
    assert_eq!(mode(&["calibrate", "--bench"]), Ok(Mode::Calibrate));
    assert_eq!(mode(&["--bench", "ceiling"]), Ok(Mode::Ceiling));
    assert_eq!(
        mode(&["fast"]),
        Err("no run is called `fast`: gate, calibrate or ceiling".to_owned())
    );
    assert_eq!(
        mode(&["gate", "ceiling"]),
        Err("one run at a time, not also `ceiling`".to_owned())
    );
}

#[test]
fn each_run_is_named_as_its_command_names_it() {
    // The name its report is written under, so a gate and a calibration never overwrite each other's.
    assert_eq!(Mode::Gate.name(), "gate");
    assert_eq!(Mode::Calibrate.name(), "calibrate");
    assert_eq!(Mode::Ceiling.name(), "ceiling");
}

#[test]
fn each_run_measures_its_own_scope() {
    assert_eq!(Mode::Gate.scope(), Scope::Small);
    assert_eq!(Mode::Calibrate.scope(), Scope::Small);
    assert_eq!(Mode::Ceiling.scope(), Scope::Ceiling);
}

#[test]
fn a_small_run_uses_every_fixture_and_a_ceiling_run_only_the_main_one() {
    // The flat and exclusion fixtures carry the small run's invariant lines; the ceiling measures none.
    let fixtures = tiny_fixtures();
    let names = |scope| -> Vec<&str> {
        fixtures
            .used(scope)
            .iter()
            .map(|profile| profile.name)
            .collect()
    };

    assert_eq!(names(Scope::Small), ["tiny", "flat", "exclusion"]);
    assert_eq!(names(Scope::Ceiling), ["tiny"]);
}

#[test]
fn a_gate_holds_gated_lines_to_their_limits_and_records_baselines() {
    let thresholds = thresholds(&[(READ_VERSION.name, 4.0)]);
    let measured = [
        measurement(&READ_VERSION, &[3.0, 5.0, 4.5]),
        measurement(&CAPTURE_FIRST_UNCACHED, &[900.0]),
    ];

    let rows =
        rows(&measured, Mode::Gate, Some(&thresholds)).expect("every gated line has a limit");

    assert_eq!(rows[0].bound, Bound::Threshold(4.0));
    assert_eq!(rows[0].why, None);
    assert_eq!(rows[1].bound, Bound::Baseline);
    // Ticket 87 is named: the report says why the line is not gated.
    assert!(
        matches!(rows[1].why, Some(why) if why.contains("ticket 87")),
        "{:?}",
        rows[1]
    );
}

#[test]
fn a_gated_line_without_a_limit_cannot_be_judged() {
    for thresholds in [Some(&thresholds(&[])), None] {
        let refused = rows(
            &[measurement(&READ_VERSION, &[3.0])],
            Mode::Gate,
            thresholds,
        )
        .expect_err("no limit for a gated line");

        assert!(
            matches!(&refused, TripwireError::Unlimited(line) if line == READ_VERSION.name),
            "{refused:?}"
        );
    }
}

#[test]
fn calibration_measures_without_judging() {
    let rows = rows(
        &[
            measurement(&READ_VERSION, &[3.0]),
            measurement(&CAPTURE_FIRST_UNCACHED, &[900.0]),
        ],
        Mode::Calibrate,
        None,
    )
    .expect("calibration needs no limits");

    assert_eq!(rows[0].bound, Bound::Baseline);
    // Every line is a baseline while calibrating, so none says why it is not gated.
    assert_eq!((rows[0].why, rows[1].why), (None, None));
}

#[test]
fn a_ceiling_run_holds_each_line_to_its_budget() {
    let walk = Measurement {
        paths: Some(100_000),
        ..measurement(&PREVIEW_WALK, &[4_000.0])
    };

    let rows = rows(
        &[measurement(&READ_DIFF, &[100.0]), walk],
        Mode::Ceiling,
        None,
    )
    .expect("budgets need no limits");

    assert_eq!(rows[0].bound, Bound::Budget(250.0));
    // At 20,000 paths a second, a hundred thousand take five seconds.
    assert_eq!(rows[1].bound, Bound::Budget(5_000.0));
}

#[test]
fn a_rate_measured_without_its_paths_cannot_be_held_to_its_budget() {
    let refused = rows(
        &[measurement(&PREVIEW_WALK, &[4_000.0])],
        Mode::Ceiling,
        None,
    )
    .expect_err("a rate needs the paths walked");

    assert!(
        matches!(&refused, TripwireError::Mismeasured { line, .. } if *line == PREVIEW_WALK.name),
        "{refused:?}"
    );
}

#[test]
fn an_estimate_is_recorded_as_a_baseline_and_checked_beside_the_report() {
    // An estimate is not ticket 30's budget: the row records the measurement, and the check says whether
    // the estimate held, apart from every verdict.
    let measured = [
        measurement(&CAPTURE_READ_TREE, &[300.0, 320.0, 610.0]),
        measurement(&READ_DIFF, &[100.0]),
    ];

    let rows = rows(&measured, Mode::Ceiling, None).expect("no limits needed");
    let checks = estimates(&measured, Scope::Ceiling);

    assert_eq!(rows[0].bound, Bound::Baseline);
    assert_eq!(checks.len(), 1, "only the estimate line is checked");
    assert!(!checks[0].held());
    assert_eq!(
        checks[0].to_string(),
        "estimate failed: capture.read_tree p95 610.000 ms over ticket 12's \"low hundreds of \
         milliseconds\", at most 500 ms; 12.2 % of the capture budget of 5000 ms"
    );
}

#[test]
fn an_estimate_measured_at_or_under_its_most_holds() {
    let checks = estimates(
        &[measurement(&CAPTURE_READ_TREE, &[200.0, 500.0])],
        Scope::Ceiling,
    );

    assert!(checks[0].held());
    assert!(
        checks[0]
            .to_string()
            .starts_with("estimate held: capture.read_tree p95 500.000 ms within ticket 12's"),
        "{}",
        checks[0]
    );
}

#[test]
fn no_estimate_is_checked_outside_the_ceiling() {
    // The estimate is for the ceiling's hundred thousand files; the small fixture's figure says nothing
    // about whether it holds.
    let checks = estimates(&[measurement(&CAPTURE_READ_TREE, &[900.0])], Scope::Small);

    assert_eq!(checks, Vec::new());
}

#[test]
fn calibration_commits_twice_each_gated_median_and_every_digest() {
    let measured = [
        measurement(&READ_VERSION, &[3.0, 5.0, 4.0]),
        measurement(&CAPTURE_FIRST_UNCACHED, &[900.0]),
    ];
    let digests: BTreeMap<String, String> = [("small".to_owned(), "abc".to_owned())].into();

    let calibrated = calibrate(&measured, digests);

    assert_eq!(
        calibrated.limit("read.version"),
        Some(calibrated_limit(4.0))
    );
    assert_eq!(
        calibrated.limit("capture.first_uncached"),
        None,
        "a baseline"
    );
    assert_eq!(calibrated.digest("small"), Some("abc"));
}

#[test]
fn every_small_line_runs_on_fixtures_of_any_size() {
    let directory = scratch();

    let measured = measure(
        &tiny_fixtures(),
        Scope::Small,
        directory.path(),
        Repeats {
            warmups: 0,
            measured: 2,
        },
        &git(),
    )
    .expect("every workload runs");

    let names: Vec<&str> = measured.iter().map(|found| found.line.name).collect();
    let expected: Vec<&str> = LINES
        .iter()
        .filter(|line| line.small.is_some())
        .map(|line| line.name)
        .collect();
    assert_eq!(names, expected);
    for found in &measured {
        assert!(!found.samples.is_empty(), "{}", found.line.name);
        assert!(
            found
                .samples
                .iter()
                .all(|sample| sample.is_finite() && *sample >= 0.0),
            "{}: {:?}",
            found.line.name,
            found.samples
        );
    }
    let walk = measured
        .iter()
        .find(|found| found.line.name == PREVIEW_WALK.name)
        .expect("the walk");
    assert!(
        walk.paths.is_some_and(|paths| paths > 0),
        "{:?}",
        walk.paths
    );
    let growth = measured
        .iter()
        .find(|found| found.line.name == STORE_GROWTH.name)
        .expect("the growth");
    assert!(growth.samples[0] > 0.0, "{:?}", growth.samples);
}

#[test]
fn every_ceiling_line_runs_and_each_stream_step_is_a_capture_sample() {
    let directory = scratch();
    let fixtures = tiny_fixtures();

    let measured = measure(
        &fixtures,
        Scope::Ceiling,
        directory.path(),
        Repeats {
            warmups: 0,
            measured: 1,
        },
        &git(),
    )
    .expect("every workload runs");

    let names: Vec<&str> = measured.iter().map(|found| found.line.name).collect();
    let expected: Vec<&str> = LINES
        .iter()
        .filter(|line| line.ceiling.is_some())
        .map(|line| line.name)
        .collect();
    assert_eq!(names, expected);
    let steps = measured
        .iter()
        .find(|found| found.line.name == CAPTURE_STEP.name)
        .expect("the capture");
    assert_eq!(steps.samples.len(), fixtures.main.churn.steps);
}

#[test]
fn store_growth_counts_the_whole_store_its_derived_index_included() {
    // The derived SQLite index lies beside the git store (ticket 03) and takes the user's disk as the
    // objects do: the steady-state store size counts both.
    let directory = scratch();
    let fixtures = tiny_fixtures();

    let measured = measure(
        &fixtures,
        Scope::Ceiling,
        directory.path(),
        Repeats {
            warmups: 0,
            measured: 1,
        },
        &git(),
    )
    .expect("every workload runs");

    let growth = measured
        .iter()
        .find(|found| found.line.name == STORE_GROWTH.name)
        .expect("the growth")
        .samples[0];
    let store = directory.path().join("keepster-bench/main/store");
    let index = std::fs::metadata(store.join("index.sqlite"))
        .expect("the derived index")
        .len();
    let history = bytes_under(&store.join("history.git"));
    let counted = growth * included(&fixtures.main) as f64;
    // The workloads after the growth line add a few objects of their own, so the store is read here a
    // little larger than it was measured; the git store alone is smaller than the index, though.
    assert!(
        counted >= (history + index) as f64 * 0.9,
        "{counted} bytes counted for {history} in git and {index} in the index"
    );
}

#[test]
fn the_diff_line_refuses_a_fixture_whose_modified_files_are_all_over_a_mebibyte() {
    // The outcome line is "Diff two versions, file ≤ 1 MiB": a larger file would hold the budget to a
    // diff it does not promise. Every file here is over a mebibyte, so no version qualifies.
    let directory = scratch();
    let large: &'static [SizeBucket] = &[SizeBucket {
        permille: 1000,
        smallest: 1024 * 1024 + 1,
        largest: 1024 * 1024 + 4096,
    }];
    let fixtures = Fixtures {
        main: Profile {
            files: 4,
            directories: 1,
            bytes: 4 * (1024 * 1024 + 2048),
            sizes: large,
            ..tiny_fixtures().main
        },
        ..tiny_fixtures()
    };

    let refused = measure(
        &fixtures,
        Scope::Ceiling,
        directory.path(),
        Repeats {
            warmups: 0,
            measured: 1,
        },
        &git(),
    )
    .expect_err("no file to diff");

    assert!(
        matches!(&refused, TripwireError::Failed { doing, .. } if doing.contains("to diff")),
        "{refused:?}"
    );
}

#[test]
fn a_run_starts_from_a_fresh_fixture() {
    // What a previous run left behind is not part of this one's tree.
    let directory = scratch();
    let leftover = directory
        .path()
        .join("keepster-bench")
        .join("main")
        .join("tree")
        .join("leftover.txt");
    std::fs::create_dir_all(leftover.parent().expect("a parent")).expect("a directory");
    std::fs::write(&leftover, b"from before").expect("a file");

    measure(
        &tiny_fixtures(),
        Scope::Ceiling,
        directory.path(),
        Repeats {
            warmups: 0,
            measured: 1,
        },
        &git(),
    )
    .expect("every workload runs");

    assert!(!leftover.exists());
}

/// A measurement of `line`.
fn measurement(line: &'static Line, samples: &[f64]) -> Measurement {
    Measurement {
        line,
        samples: samples.to_vec(),
        paths: None,
    }
}

/// Thresholds holding `limits` and no digests.
fn thresholds(limits: &[(&str, f64)]) -> Thresholds {
    Thresholds::calibrated(
        BTreeMap::new(),
        limits
            .iter()
            .map(|(line, limit)| ((*line).to_owned(), *limit))
            .collect(),
    )
}

/// Fixtures of a few files each, with a short stream.
fn tiny_fixtures() -> Fixtures {
    let excluded = Excluded {
        name: "node_modules",
        files: 6,
        directories: 2,
        file_size: 16,
    };
    let sizes: &'static [SizeBucket] = &[SizeBucket {
        permille: 1000,
        smallest: 1,
        largest: 200,
    }];
    let main = Profile {
        name: "tiny",
        files: 24,
        directories: 4,
        bytes: 2_400,
        sizes,
        fan_out: 2,
        excluded,
        churn: Churn {
            steps: 3,
            interval: Duration::from_secs(60),
            modified: 3,
            created: 1,
            deleted: 1,
        },
        ..SMALL
    };

    Fixtures {
        main,
        flat: Profile { files: 12, ..FLAT },
        exclusion: Profile {
            files: 5,
            directories: 2,
            bytes: 250,
            sizes,
            excluded,
            ..EXCLUSION
        },
    }
}

/// The `git` the daemon would maintain stores with, for the Maintenance Pass store growth is measured
/// after.
fn git() -> Git {
    environment::git().expect("a git on the machine running the tests")
}

/// The bytes the files of `profile`'s Root include once its whole change stream is applied.
fn included(profile: &Profile) -> u64 {
    let plan = bulk::plan(profile);
    let mut live: BTreeMap<String, u64> = plan
        .files
        .iter()
        .map(|file| (file.path.clone(), file.size))
        .collect();
    for change in bulk::change_stream(profile, &plan).into_iter().flatten() {
        match change {
            Change::Modified(file) | Change::Created(file) => {
                live.insert(file.path, file.size);
            }
            Change::Deleted(path) => {
                live.remove(&path);
            }
        }
    }
    live.values().sum()
}

/// Every byte of every file under `directory`.
fn bytes_under(directory: &Path) -> u64 {
    std::fs::read_dir(directory)
        .expect("a directory")
        .map(|entry| {
            let entry = entry.expect("an entry");
            if entry.file_type().expect("a file type").is_dir() {
                bytes_under(&entry.path())
            } else {
                entry.metadata().expect("its metadata").len()
            }
        })
        .sum()
}
