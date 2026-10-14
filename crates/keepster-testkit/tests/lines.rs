//! The tripwire's lines (ticket 30): each exists only because it protects a named invariant or mirrors an
//! outcome line, the committed thresholds hold a limit for exactly the gated ones, and every outcome line
//! is compared with its budget at the ceiling.

use std::collections::HashSet;

use keepster_testkit::lines::{
    Budget, CAPTURE_FIRST_UNCACHED, CAPTURE_READ_TREE, CAPTURE_STEP, Guard, LINES, Line,
    PREVIEW_FIRST_ROW, PREVIEW_WALK, READ_CHRONOLOGY, READ_DIFF, READ_VERSION,
    RECONCILE_EXCLUDED_SUBTREE, RECONCILE_UNCHANGED, STORE_GROWTH, Small, WATCH_EXCLUDED_SUBTREE,
    gated,
};
use keepster_testkit::report::Unit;
use keepster_testkit::thresholds::Thresholds;

#[test]
fn the_committed_thresholds_limit_exactly_the_gated_lines() {
    let committed = Thresholds::committed().expect("the committed thresholds file parses");

    // On failure: a line was added, retired or renamed. Recalibrate with `make bench-calibrate` on the
    // reference machine and commit the new thresholds file with the change.
    assert_eq!(committed.mismatches(&gated()), Vec::new());
}

#[test]
fn every_line_has_a_name_of_its_own() {
    let mut seen = HashSet::new();
    for line in LINES {
        assert!(seen.insert(line.name), "{} twice", line.name);
        assert!(
            line.name
                .chars()
                .all(|character| character.is_ascii_lowercase() || "._".contains(character)),
            "{}",
            line.name
        );
    }
}

#[test]
fn every_line_runs_somewhere() {
    for line in LINES {
        assert!(
            line.small.is_some() || line.ceiling.is_some(),
            "{} is never measured",
            line.name
        );
    }
}

#[test]
fn an_outcome_line_is_compared_with_its_budget_at_the_ceiling_and_an_invariant_line_is_not() {
    // A mirror at small scale is only half of an outcome line: the promise is stated at the ceiling.
    for line in LINES {
        match line.guard {
            Guard::Outcome { .. } => assert!(line.ceiling.is_some(), "{}", line.name),
            Guard::Invariant { .. } => assert!(line.ceiling.is_none(), "{}", line.name),
        }
    }
}

#[test]
fn a_baseline_says_why_it_is_not_gated() {
    for line in LINES {
        if let Some(Small::Baseline { why }) = line.small {
            assert!(!why.trim().is_empty(), "{}", line.name);
        }
    }
}

#[test]
fn the_ceiling_budgets_are_ticket_30s() {
    let budget = |line: &Line| {
        line.ceiling
            .unwrap_or_else(|| panic!("{} has a ceiling budget", line.name))
    };

    assert_eq!(budget(&CAPTURE_STEP), Budget::Most(5_000.0));
    assert_eq!(budget(&READ_VERSION), Budget::Most(100.0));
    assert_eq!(budget(&READ_DIFF), Budget::Most(250.0));
    assert_eq!(budget(&READ_CHRONOLOGY), Budget::Most(100.0));
    assert_eq!(budget(&RECONCILE_UNCHANGED), Budget::Most(30_000.0));
    assert_eq!(budget(&PREVIEW_FIRST_ROW), Budget::Most(200.0));
    assert_eq!(budget(&PREVIEW_WALK), Budget::PathsPerSecond(20_000.0));
    assert_eq!(budget(&STORE_GROWTH), Budget::Most(2.0));
}

#[test]
fn a_rate_budget_allows_the_time_its_paths_take_at_that_rate() {
    // Ticket 30's Capture Preview promise is at least 20,000 paths a second; every bound is a most, so
    // it is held as the time a walk of so many paths may take.
    assert_eq!(
        Budget::PathsPerSecond(20_000.0).most(Some(100_000)),
        Some(5_000.0)
    );
    assert_eq!(Budget::Most(250.0).most(Some(100_000)), Some(250.0));
    assert_eq!(Budget::Most(250.0).most(None), Some(250.0));
    // A rate with no count of the paths walked bounds nothing.
    assert_eq!(Budget::PathsPerSecond(20_000.0).most(None), None);
}

#[test]
fn only_the_store_growth_is_a_ratio() {
    for line in LINES {
        let expected = if line.name == STORE_GROWTH.name {
            Unit::Ratio
        } else {
            Unit::Milliseconds
        };
        assert_eq!(line.unit, expected, "{}", line.name);
    }
}

#[test]
fn read_tree_is_a_baseline_held_at_the_ceiling_to_ticket_12s_estimate() {
    // Nothing has measured it yet: ticket 12 estimated "low hundreds of milliseconds" at the ceiling,
    // and the ceiling run says whether that holds, without gating anything.
    let line = CAPTURE_READ_TREE;

    assert!(
        matches!(line.small, Some(Small::Baseline { why }) if why.contains("ticket 12")),
        "{line:?}"
    );
    assert!(
        matches!(line.ceiling, Some(Budget::Estimate { most, source }) if most == 500.0 && source.contains("ticket 12")),
        "{line:?}"
    );
}

#[test]
fn the_uncached_first_capture_is_a_baseline_until_ticket_87() {
    // Strict object validation is still on: gating it would defend an invariant the code does not have.
    let line = CAPTURE_FIRST_UNCACHED;

    assert!(
        matches!(line.small, Some(Small::Baseline { why }) if why.contains("ticket 87")),
        "{line:?}"
    );
}

#[test]
fn only_the_exclusion_invariants_are_gated() {
    // Entering an excluded directory, or watching one before the rules, breaches its line; the order and
    // the seed cost too small a part of theirs, and validation is not in the code yet: those are
    // baselines, each saying why.
    let gated_invariants: Vec<&str> = LINES
        .iter()
        .filter(|line| {
            matches!(line.guard, Guard::Invariant { .. }) && line.small == Some(Small::Gated)
        })
        .map(|line| line.name)
        .collect();

    assert_eq!(
        gated_invariants,
        [RECONCILE_EXCLUDED_SUBTREE.name, WATCH_EXCLUDED_SUBTREE.name]
    );
}

#[test]
fn a_capture_step_is_gated_at_small_scale_as_the_capture_outcomes_mirror() {
    // Ticket 69: a regression that makes capture five times slower fails a command. The invariant
    // lines that capture are baselines, so the step of the declared churn is the gated capture.
    let line = CAPTURE_STEP;

    assert!(
        matches!(line.guard, Guard::Outcome { line } if line.starts_with("Capture,")),
        "{line:?}"
    );
    assert_eq!(line.small, Some(Small::Gated));
}
