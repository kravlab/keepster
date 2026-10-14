//! The tripwire's lines (ticket 30): what each measures, what it defends, and where it is measured.
//!
//! A line exists only where it protects a named invariant or mirrors an outcome line — no other reason
//! qualifies, and [`Guard`] has no third variant, so the rule is held by the type rather than by review.
//! The small run gates on most lines and records the rest as baselines; the ceiling run compares the
//! outcome lines' p95 with ticket 30's budget — or, for a part of one, with a decision's estimate — and
//! gates on nothing.

use crate::report::Unit;

/// Why a line exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    /// It is measured for a performance-carrying invariant. Gated, it defends the invariant: breaking it
    /// breaches the line. A baseline only records what the line measures, defending nothing: the code
    /// does not hold the invariant yet, or breaking it costs too small a part of the line to breach it.
    Invariant {
        /// The invariant, in the words of the decision that established it.
        name: &'static str,
        /// That decision.
        source: &'static str,
    },
    /// It mirrors a line of the outcome budget at small scale, catching a regression assembled from
    /// parts each within the noise.
    Outcome {
        /// The outcome line, as ticket 30's table names it.
        line: &'static str,
    },
}

/// What the small run does with a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Small {
    /// Its median is held to the committed threshold, and a breach fails the run.
    Gated,
    /// It is measured and recorded, never failed.
    Baseline {
        /// Why it is not gated, printed beneath it in a gate's report.
        why: &'static str,
    },
}

/// What the ceiling run compares a line's p95 with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Budget {
    /// The most it may measure, in the line's unit.
    Most(f64),
    /// The fewest paths a second it must walk: held, as every bound is, as a most — the milliseconds
    /// the paths it walked may take at that rate.
    PathsPerSecond(f64),
    /// Not ticket 30's budget but a decision's estimate of a part of one, checked rather than judged:
    /// the line is recorded as a baseline, and the run says beside its report whether the estimate held.
    Estimate {
        /// The most the estimate allows, in the line's unit.
        most: f64,
        /// Whose estimate it is, in its own words.
        source: &'static str,
    },
}

impl Budget {
    /// The most a line that walked `paths` paths may measure; `None` for a rate whose paths were not
    /// counted, which bounds nothing.
    pub fn most(self, paths: Option<usize>) -> Option<f64> {
        match self {
            Self::Most(most) | Self::Estimate { most, .. } => Some(most),
            Self::PathsPerSecond(rate) => paths.map(|paths| paths as f64 / rate * 1_000.0),
        }
    }
}

/// One line of the tripwire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    /// Its name, as the thresholds file and the report key it.
    pub name: &'static str,
    /// What its numbers count.
    pub unit: Unit,
    /// Why it exists.
    pub guard: Guard,
    /// What the small run does with it; `None` when only the ceiling measures it.
    pub small: Option<Small>,
    /// What the ceiling run compares it with; `None` when only the small run measures it.
    pub ceiling: Option<Budget>,
}

/// A first capture into a fresh store: its index has no TREE cache, so every entry is validated.
pub const CAPTURE_FIRST_UNCACHED: Line = Line {
    name: "capture.first_uncached",
    unit: Unit::Milliseconds,
    guard: Guard::Invariant {
        name: "strict object validation is off on an uncached index",
        source: "ticket 12",
    },
    small: Some(Small::Baseline {
        why: "the invariant is not in the code yet: ticket 87 turns validation off and gates this line",
    }),
    ceiling: None,
};

/// A capture of one step of the declared churn: the daemon's own part of a capture.
pub const CAPTURE_STEP: Line = Line {
    name: "capture.step",
    unit: Unit::Milliseconds,
    guard: Guard::Outcome {
        line: "Capture, the daemon's own contribution",
    },
    small: Some(Small::Gated),
    ceiling: Some(Budget::Most(5_000.0)),
};

/// Reading the previous Snapshot's tree into the index a capture is seeded from.
pub const CAPTURE_READ_TREE: Line = Line {
    name: "capture.read_tree",
    unit: Unit::Milliseconds,
    guard: Guard::Outcome {
        line: "Capture, the daemon's own contribution",
    },
    small: Some(Small::Baseline {
        why: "measured for the first time: ticket 12 estimated it \"in the low hundreds of \
              milliseconds for 100k files\", and nothing has measured it since",
    }),
    ceiling: Some(Budget::Estimate {
        most: 500.0,
        source: "ticket 12's \"low hundreds of milliseconds\"",
    }),
};

/// The store's size after the change stream and a Maintenance Pass, over the content it holds.
pub const STORE_GROWTH: Line = Line {
    name: "store.growth",
    unit: Unit::Ratio,
    guard: Guard::Outcome {
        line: "Steady-state store size",
    },
    small: Some(Small::Gated),
    ceiling: Some(Budget::Most(2.0)),
};

/// Fetching one version of a file.
pub const READ_VERSION: Line = Line {
    name: "read.version",
    unit: Unit::Milliseconds,
    guard: Guard::Outcome {
        line: "Fetch one version of a file",
    },
    small: Some(Small::Gated),
    ceiling: Some(Budget::Most(100.0)),
};

/// Diffing two versions of a file of at most a mebibyte.
pub const READ_DIFF: Line = Line {
    name: "read.diff",
    unit: Unit::Milliseconds,
    guard: Guard::Outcome {
        line: "Diff two versions, file ≤ 1 MiB",
    },
    small: Some(Small::Gated),
    ceiling: Some(Budget::Most(250.0)),
};

/// Reading a path's chronology.
pub const READ_CHRONOLOGY: Line = Line {
    name: "read.chronology",
    unit: Unit::Milliseconds,
    guard: Guard::Outcome {
        line: "A path's chronology",
    },
    small: Some(Small::Gated),
    ceiling: Some(Budget::Most(100.0)),
};

/// A daemon's start over an unchanged tree: the store recovered, a Reconciliation pass, the watches
/// installed.
pub const RECONCILE_UNCHANGED: Line = Line {
    name: "reconcile.unchanged",
    unit: Unit::Milliseconds,
    guard: Guard::Outcome {
        line: "Startup Reconciliation",
    },
    small: Some(Small::Gated),
    ceiling: Some(Budget::Most(30_000.0)),
};

/// The first row of a Capture Preview.
pub const PREVIEW_FIRST_ROW: Line = Line {
    name: "preview.first_row",
    unit: Unit::Milliseconds,
    guard: Guard::Outcome {
        line: "Capture Preview, first row",
    },
    small: Some(Small::Gated),
    ceiling: Some(Budget::Most(200.0)),
};

/// A whole Capture Preview walk, held at the ceiling to a rate of paths.
pub const PREVIEW_WALK: Line = Line {
    name: "preview.walk",
    unit: Unit::Milliseconds,
    guard: Guard::Outcome {
        line: "Capture Preview, throughput",
    },
    small: Some(Small::Gated),
    ceiling: Some(Budget::PathsPerSecond(20_000.0)),
};

/// A capture after one path changed.
pub const CAPTURE_ONE_CHANGED_PATH: Line = Line {
    name: "capture.one_changed_path",
    unit: Unit::Milliseconds,
    guard: Guard::Invariant {
        name: "the baseline is seeded from the previous Snapshot's tree",
        source: "ADR 0004",
    },
    small: Some(Small::Baseline {
        why: "breaking ADR 0004 cannot breach it: in the check that broke it, unseeded, a capture of \
              the small profile's 5k files grew 1.2×, since the walk stats every file and the rebuild builds every tree, \
              both growing with the files",
    }),
    ceiling: None,
};

/// A Reconciliation pass over a tree beside a large excluded subtree.
pub const RECONCILE_EXCLUDED_SUBTREE: Line = Line {
    name: "reconcile.excluded_subtree",
    unit: Unit::Milliseconds,
    guard: Guard::Invariant {
        name: "an excluded directory is never entered",
        source: "ADR 0007",
    },
    small: Some(Small::Gated),
    ceiling: None,
};

/// Installing the watches of a tree beside a large excluded subtree.
pub const WATCH_EXCLUDED_SUBTREE: Line = Line {
    name: "watch.excluded_subtree",
    unit: Unit::Milliseconds,
    guard: Guard::Invariant {
        name: "Include Rules are applied before watches are installed",
        source: "ticket 02",
    },
    small: Some(Small::Gated),
    ceiling: None,
};

/// A first capture of one directory of many files.
pub const CAPTURE_FIRST_FLAT_TREE: Line = Line {
    name: "capture.first_flat_tree",
    unit: Unit::Milliseconds,
    guard: Guard::Invariant {
        name: "entries are applied in lexicographic order",
        source: "ticket 12",
    },
    small: Some(Small::Baseline {
        why: "breaking ticket 12's order cannot breach it: in the check that broke it, 200k entries \
              took 3.6 s in reverse against 0.17 s in order, a quadratic cost that doubles this \
              capture, about 68 µs a file, only near 800k files; at 20k it added about 24 ms to \
              1.36 s",
    }),
    ceiling: None,
};

/// Every line, in the order a run measures them.
pub const LINES: &[Line] = &[
    CAPTURE_FIRST_UNCACHED,
    CAPTURE_STEP,
    CAPTURE_READ_TREE,
    STORE_GROWTH,
    READ_VERSION,
    READ_DIFF,
    READ_CHRONOLOGY,
    RECONCILE_UNCHANGED,
    PREVIEW_FIRST_ROW,
    PREVIEW_WALK,
    CAPTURE_ONE_CHANGED_PATH,
    RECONCILE_EXCLUDED_SUBTREE,
    WATCH_EXCLUDED_SUBTREE,
    CAPTURE_FIRST_FLAT_TREE,
];

/// The names of the lines the small run gates on, in the order measured.
pub fn gated() -> Vec<&'static str> {
    LINES
        .iter()
        .filter(|line| line.small == Some(Small::Gated))
        .map(|line| line.name)
        .collect()
}
