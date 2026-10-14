//! How a tripwire run turns repeated measurements into verdicts and an exit code (ticket 30).
//!
//! A line is measured several times and summarized; the summary is judged against its [`Bound`]. Only a
//! committed threshold can fail a run: the outcome budget is a promise reviewed at release, so a ceiling
//! run exceeding it is reported, and a baseline — such as `read_tree`, measured for the first time — is
//! recorded and never fails anything.
//!
//! Every bound is a most: lower is better on every line. A promise where more is better, such as Capture
//! Preview's paths per second, is held as the time it allows for a fixed number of paths.

/// What repeated measurements of one line came to. Only [`Summary::of`] makes one, so every summary
/// is of at least one measurement, none of them not a number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    /// How many measurements there were.
    samples: usize,
    /// See [`Summary::median`].
    median: f64,
    /// See [`Summary::p95`].
    p95: f64,
}

impl Summary {
    /// The summary of `samples`.
    ///
    /// # Panics
    ///
    /// When there are none, or one is not a number: either is a mistake in the run, never a measurement.
    pub fn of(samples: &[f64]) -> Self {
        assert!(!samples.is_empty(), "a summary of no samples");
        assert!(
            samples.iter().all(|sample| !sample.is_nan()),
            "a sample that is not a number: {samples:?}"
        );
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let count = sorted.len();
        let median = if count % 2 == 1 {
            sorted[count / 2]
        } else {
            (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0
        };
        // The rank is ⌈0.95 n⌉, counted from one; integer arithmetic keeps it exact.
        let rank = (count * 95).div_ceil(100);

        Self {
            samples: count,
            median,
            p95: sorted[rank - 1],
        }
    }

    /// How many measurements there were.
    pub fn samples(&self) -> usize {
        self.samples
    }

    /// The middle measurement, or the mean of the middle two: what a threshold is judged on, since one
    /// slow run moves it at most to its neighbouring measurement.
    pub fn median(&self) -> f64 {
        self.median
    }

    /// The nearest-rank 95th percentile: the smallest measurement that at least 95 % of them do not
    /// exceed — of five, the largest. What the outcome budget is stated in.
    pub fn p95(&self) -> f64 {
        self.p95
    }
}

/// What a line's measurement is held to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bound {
    /// A committed threshold (`bench/thresholds.toml`): exceeding it fails the run.
    Threshold(f64),
    /// The outcome budget a ceiling run is compared with: exceeding it is reported, never failed.
    Budget(f64),
    /// No bound: the measurement is recorded as a baseline.
    Baseline,
}

/// How a line's measurement stands against its bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// At or under its threshold or budget.
    Within,
    /// Over its threshold: the run fails.
    Breached,
    /// Over its budget: reported for the release checklist, the run does not fail.
    OverBudget,
    /// A baseline, recorded.
    Recorded,
}

/// The verdict on `measured` against `bound`. A bound is the most a run may measure, so measuring exactly
/// it is within.
///
/// # Panics
///
/// When `measured` or the bound is not a number. Every comparison with one is false, so judging it would
/// pass a gate that measured nothing; it fails closed instead.
pub fn judge(measured: f64, bound: Bound) -> Verdict {
    let limit = match bound {
        Bound::Threshold(limit) | Bound::Budget(limit) => limit,
        Bound::Baseline => 0.0,
    };
    assert!(
        !measured.is_nan() && !limit.is_nan(),
        "judging {measured} against {bound:?}: not a number"
    );
    match bound {
        Bound::Threshold(limit) if measured > limit => Verdict::Breached,
        Bound::Budget(limit) if measured > limit => Verdict::OverBudget,
        Bound::Threshold(_) | Bound::Budget(_) => Verdict::Within,
        Bound::Baseline => Verdict::Recorded,
    }
}

/// How a tripwire run ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// No threshold was breached.
    Passed,
    /// A threshold was breached.
    Breached,
    /// The run refused to measure: the environment would make its numbers meaningless.
    Refused,
}

impl Outcome {
    /// The bench binary's exit code: 0 passed, 1 breached, 2 refused, so a refusal is never read as a
    /// breach. `make` turns any failing command into its own 2, so through `make bench` the two are told
    /// apart by the message the run prints, not by the code.
    pub fn code(self) -> u8 {
        match self {
            Self::Passed => 0,
            Self::Breached => 1,
            Self::Refused => 2,
        }
    }
}

/// How a run that reached `verdicts` ends: breached when any threshold was, passed otherwise.
pub fn outcome(verdicts: &[Verdict]) -> Outcome {
    if verdicts.contains(&Verdict::Breached) {
        Outcome::Breached
    } else {
        Outcome::Passed
    }
}
