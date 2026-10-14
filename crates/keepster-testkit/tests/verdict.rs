//! How a tripwire run turns repeated measurements into verdicts and an exit code (ticket 30): the median
//! against a committed threshold fails the run, a p95 against the outcome budget is only reported, and a
//! baseline is recorded and never fails anything.

use keepster_testkit::verdict::{Bound, Outcome, Summary, Verdict, judge, outcome};

#[test]
fn the_median_of_an_odd_count_is_its_middle_value_whatever_the_order() {
    assert_eq!(Summary::of(&[5.0, 1.0, 4.0, 2.0, 3.0]).median(), 3.0);
    assert_eq!(Summary::of(&[7.0]).median(), 7.0);
    assert_eq!(Summary::of(&[5.0, 1.0, 4.0, 2.0, 3.0]).samples(), 5);
}

#[test]
fn the_median_of_an_even_count_is_the_mean_of_its_middle_two() {
    assert_eq!(Summary::of(&[4.0, 1.0, 3.0, 2.0]).median(), 2.5);
}

#[test]
fn the_p95_is_the_nearest_rank_value() {
    // The smallest value at least 95 % of the samples do not exceed: the largest of five, the 95th of a
    // hundred.
    assert_eq!(Summary::of(&[5.0, 1.0, 4.0, 2.0, 3.0]).p95(), 5.0);
    let hundred: Vec<f64> = (1..=100).rev().map(f64::from).collect();
    assert_eq!(Summary::of(&hundred).p95(), 95.0);
    assert_eq!(Summary::of(&[7.0]).p95(), 7.0);
}

#[test]
#[should_panic(expected = "no samples")]
fn a_summary_of_nothing_is_a_mistake() {
    let _ = Summary::of(&[]);
}

#[test]
#[should_panic(expected = "not a number")]
fn a_summary_of_a_sample_that_is_not_a_number_is_a_mistake() {
    let _ = Summary::of(&[1.0, f64::NAN, 2.0]);
}

#[test]
fn a_threshold_is_breached_only_above_it() {
    // The thresholds file holds the most a run may measure, so measuring exactly that passes.
    assert_eq!(judge(9.9, Bound::Threshold(10.0)), Verdict::Within);
    assert_eq!(judge(10.0, Bound::Threshold(10.0)), Verdict::Within);
    assert_eq!(judge(10.1, Bound::Threshold(10.0)), Verdict::Breached);
}

#[test]
fn a_budget_is_exceeded_but_never_breached() {
    assert_eq!(judge(4_999.0, Bound::Budget(5_000.0)), Verdict::Within);
    assert_eq!(judge(5_000.0, Bound::Budget(5_000.0)), Verdict::Within);
    assert_eq!(judge(5_001.0, Bound::Budget(5_000.0)), Verdict::OverBudget);
}

#[test]
fn a_baseline_is_recorded_whatever_it_measures() {
    for measured in [0.0, 1.0, 1e12, f64::INFINITY] {
        assert_eq!(
            judge(measured, Bound::Baseline),
            Verdict::Recorded,
            "{measured}"
        );
    }
}

#[test]
#[should_panic(expected = "not a number")]
fn judging_a_measurement_that_is_not_a_number_is_a_mistake() {
    // `NaN > limit` is false: left unchecked, a broken measurement would pass the gate.
    let _ = judge(f64::NAN, Bound::Threshold(10.0));
}

#[test]
#[should_panic(expected = "not a number")]
fn judging_against_a_bound_that_is_not_a_number_is_a_mistake() {
    let _ = judge(1.0, Bound::Budget(f64::NAN));
}

#[test]
fn a_run_fails_only_when_a_threshold_is_breached() {
    assert_eq!(outcome(&[]), Outcome::Passed);
    assert_eq!(
        outcome(&[Verdict::Within, Verdict::Recorded, Verdict::OverBudget]),
        Outcome::Passed
    );
    assert_eq!(
        outcome(&[Verdict::Within, Verdict::Breached, Verdict::Recorded]),
        Outcome::Breached
    );
}

#[test]
fn each_outcome_exits_with_its_own_code() {
    // Ticket 30: a breach fails the command; an environment the tripwire refuses to measure in is told
    // apart from a breach.
    assert_eq!(Outcome::Passed.code(), 0);
    assert_eq!(Outcome::Breached.code(), 1);
    assert_eq!(Outcome::Refused.code(), 2);
}
