//! The report a tripwire run prints and writes beside its numbers (ticket 30): the profiles its fixtures
//! were generated from and the environment it measured in, then one row per line with its statistic,
//! bound and verdict.

use std::path::PathBuf;

use keepster_testkit::environment::{Cpu, Environment, Filesystem};
use keepster_testkit::report::{EstimateCheck, Report, Row, Statistic, Unit};
use keepster_testkit::verdict::{Bound, Verdict};

#[test]
fn a_report_names_the_environment_then_aligns_one_row_per_line() {
    let rows = [
        Row::new(
            "read.version",
            Unit::Milliseconds,
            &[12.0, 12.5, 12.3, 12.1, 12.9],
            Bound::Threshold(24.6),
        ),
        Row::new("store.growth", Unit::Ratio, &[1.42], Bound::Threshold(1.2)),
        Row::new("read.tree", Unit::Milliseconds, &[310.0], Bound::Baseline)
            .because("an example reason"),
        Row::new(
            "capture.ceiling",
            Unit::Milliseconds,
            &[4_000.0, 4_100.0, 6_000.0],
            Bound::Budget(5_000.0),
        ),
    ];
    let report = Report {
        profiles: &[
            ("small", "7cbcacec22afbc2e269e4c7795226fd995830d2e"),
            ("flat", "0123456789abcdef0123456789abcdef01234567"),
        ],
        environment: &environment(Some("Example CPU")),
        rows: &rows,
        estimates: &[EstimateCheck {
            line: "read.tree",
            p95: 310.0,
            most: 500.0,
            source: "an example estimate",
            capture_budget: 5_000.0,
        }],
    };

    assert_eq!(
        report.to_string(),
        "\
profile     small, digest 7cbcacec22afbc2e269e4c7795226fd995830d2e
profile     flat, digest 0123456789abcdef0123456789abcdef01234567
processor   Example CPU, 8 threads, 4 available to this run
memory      16.00 GiB
kernel      6.1.0-example
filesystem  btrfs from /dev/mapper/example, mounted at /data

read.version     median of 5    12.300 ms  threshold 24.6 ms  within
store.growth     median of 1       1.420×  threshold 1.2×     BREACHED
read.tree        median of 1   310.000 ms  baseline           recorded
  not gated: an example reason
capture.ceiling  p95 of 3     6000.000 ms  budget 5000 ms     over budget

estimate held: read.tree p95 310.000 ms within an example estimate, at most 500 ms; 6.2 % of the capture \
budget of 5000 ms
"
    );
}

#[test]
fn a_row_is_judged_on_the_statistic_its_bound_is_stated_in() {
    // A threshold is judged on the median, which one slow run barely moves; the outcome budget is
    // stated as a p95; a baseline is recorded as a median, the figure a threshold would be set from.
    let samples = [1.0, 2.0, 3.0, 4.0, 100.0];
    let row = |bound| Row::new("a", Unit::Milliseconds, &samples, bound);

    let threshold = row(Bound::Threshold(50.0));
    let budget = row(Bound::Budget(50.0));
    let baseline = row(Bound::Baseline);

    assert_eq!(
        (
            threshold.statistic(),
            threshold.measured(),
            threshold.verdict()
        ),
        (Statistic::Median, 3.0, Verdict::Within)
    );
    assert_eq!(
        (budget.statistic(), budget.measured(), budget.verdict()),
        (Statistic::P95, 100.0, Verdict::OverBudget)
    );
    assert_eq!(
        (
            baseline.statistic(),
            baseline.measured(),
            baseline.verdict()
        ),
        (Statistic::Median, 3.0, Verdict::Recorded)
    );
}

#[test]
fn a_measurement_just_over_its_threshold_reads_as_over_it() {
    let rows = [Row::new(
        "read.version",
        Unit::Milliseconds,
        &[24.604],
        Bound::Threshold(24.6),
    )];
    let report = Report {
        profiles: &[("small", "abc")],
        environment: &environment(Some("Example CPU")),
        rows: &rows,
        estimates: &[],
    };

    assert!(
        report
            .to_string()
            .contains("24.604 ms  threshold 24.6 ms  BREACHED\n"),
        "{report}"
    );
}

#[test]
fn a_processor_without_a_model_name_is_reported_as_unnamed() {
    let report = Report {
        profiles: &[("small", "abc")],
        environment: &environment(None),
        rows: &[],
        estimates: &[],
    };

    assert!(
        report
            .to_string()
            .contains("processor   unnamed, 8 threads, 4 available to this run\n"),
        "{report}"
    );
}

/// A machine of eight processors, four of them this run's, with the fixture on btrfs.
fn environment(model: Option<&str>) -> Environment {
    Environment {
        cpu: Cpu {
            model: model.map(str::to_owned),
            threads: 8,
        },
        available_threads: 4,
        memory_bytes: 16 * 1024 * 1024 * 1024,
        kernel: "6.1.0-example".to_owned(),
        filesystem: Filesystem {
            kind: "btrfs".to_owned(),
            source: "/dev/mapper/example".to_owned(),
            mount_point: PathBuf::from("/data"),
        },
    }
}
