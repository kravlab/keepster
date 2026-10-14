//! The report a tripwire run prints and writes to `target/bench/` (ticket 30): the environment it measured
//! in, then one row per line — its statistic, measurement, bound and verdict.
//!
//! The environment heads every report because a number without one cannot be reproduced, which is the
//! hole ticket 30 found in the measurements before it. None of it is committed: the report stays in
//! `target/`, and only thresholds reach the repository.

use std::fmt;

use crate::environment::Environment;
use crate::verdict::{Bound, Summary, Verdict, judge};

/// What a line's numbers count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// A duration, in milliseconds.
    Milliseconds,
    /// A ratio of one quantity to another, such as the store's size to the content it holds.
    Ratio,
}

/// Which statistic of a line's measurements its bound is held to. It follows from the bound, never
/// chosen apart from it: a threshold judged on a p95 — of five runs, the slowest — would fail on one
/// throttled run, the flapping gate ticket 30 warns of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Statistic {
    /// The median: what a threshold is judged on, and what a baseline is recorded as, being the figure a
    /// threshold would be set from.
    Median,
    /// The 95th percentile: what the outcome budget is stated in.
    P95,
}

/// One line of a run, measured and judged.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The line's name, as the thresholds file keys it.
    pub line: String,
    /// What its numbers count.
    pub unit: Unit,
    /// What its measurements came to.
    pub summary: Summary,
    /// What it is held to.
    pub bound: Bound,
    /// Why a line the gate could hold to a limit is only recorded, printed beneath it; `None` for a line
    /// that is judged, and for every line of a run that judges none.
    pub why: Option<&'static str>,
}

impl Row {
    /// The row for `line`, measured `samples` times.
    ///
    /// # Panics
    ///
    /// As [`Summary::of`]: when `samples` is empty or holds something that is not a number.
    pub fn new(line: &str, unit: Unit, samples: &[f64], bound: Bound) -> Self {
        Self {
            line: line.to_owned(),
            unit,
            summary: Summary::of(samples),
            bound,
            why: None,
        }
    }

    /// The same row, saying beneath it why its line is not gated.
    pub fn because(self, why: &'static str) -> Self {
        Self {
            why: Some(why),
            ..self
        }
    }

    /// The statistic its bound is held to.
    pub fn statistic(&self) -> Statistic {
        match self.bound {
            Bound::Threshold(_) | Bound::Baseline => Statistic::Median,
            Bound::Budget(_) => Statistic::P95,
        }
    }

    /// The value of that statistic.
    pub fn measured(&self) -> f64 {
        match self.statistic() {
            Statistic::Median => self.summary.median(),
            Statistic::P95 => self.summary.p95(),
        }
    }

    /// How it stands against its bound.
    ///
    /// # Panics
    ///
    /// As [`judge`]: when the bound is not a number.
    pub fn verdict(&self) -> Verdict {
        judge(self.measured(), self.bound)
    }

    /// Its cells, in column order: line, statistic, measurement, bound, verdict.
    fn cells(&self) -> [String; 5] {
        let statistic = match self.statistic() {
            Statistic::Median => "median",
            Statistic::P95 => "p95",
        };
        let bound = match self.bound {
            Bound::Threshold(limit) => format!("threshold {}", bound(limit, self.unit)),
            Bound::Budget(limit) => format!("budget {}", bound(limit, self.unit)),
            Bound::Baseline => "baseline".to_owned(),
        };
        let verdict = match self.verdict() {
            Verdict::Within => "within",
            // In capitals: the one verdict that fails the run is the one a reader must not miss.
            Verdict::Breached => "BREACHED",
            Verdict::OverBudget => "over budget",
            Verdict::Recorded => "recorded",
        };
        [
            self.line.clone(),
            format!("{statistic} of {}", self.summary.samples()),
            measurement(self.measured(), self.unit),
            bound,
            verdict.to_owned(),
        ]
    }
}

/// A measurement written in `unit`, to three decimals: a microsecond for a duration, fine enough that one
/// just over its threshold never prints as equal to it.
fn measurement(value: f64, unit: Unit) -> String {
    with_unit(&format!("{value:.3}"), unit)
}

/// A bound written in `unit` exactly as it is committed, so the report quotes the thresholds file.
fn bound(value: f64, unit: Unit) -> String {
    with_unit(&value.to_string(), unit)
}

/// `number` followed by `unit`'s symbol.
fn with_unit(number: &str, unit: Unit) -> String {
    match unit {
        Unit::Milliseconds => format!("{number} ms"),
        Unit::Ratio => format!("{number}×"),
    }
}

/// A run's report: the profiles and environment, then its rows and estimate checks.
#[derive(Debug, Clone, Copy)]
pub struct Report<'a> {
    /// Each profile a fixture of the run was generated from, with its digest, tying the numbers to the
    /// exact fixtures.
    pub profiles: &'a [(&'a str, &'a str)],
    /// The machine measured on.
    pub environment: &'a Environment,
    /// One per line, in the order measured.
    pub rows: &'a [Row],
    /// Whether each estimate a line was checked against held, after the rows so the record keeps it.
    pub estimates: &'a [EstimateCheck],
}

impl fmt::Display for Report<'_> {
    /// The profiles and the environment, a blank line, then the rows in aligned columns: every column but
    /// the last padded to its widest cell and the measurement right-aligned, and a row that says why it is
    /// not gated followed by an indented line saying so; then, after another blank line, each estimate
    /// check, when there are any.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Environment {
            cpu,
            available_threads,
            memory_bytes,
            kernel,
            filesystem,
        } = self.environment;
        // An approximation for reading, never compared: the exact count would only be noise in a report.
        let gibibytes = *memory_bytes as f64 / f64::from(1u32 << 30);
        for (profile, digest) in self.profiles {
            writeln!(formatter, "profile     {profile}, digest {digest}")?;
        }
        writeln!(
            formatter,
            "processor   {}, {} threads, {available_threads} available to this run",
            cpu.model.as_deref().unwrap_or("unnamed"),
            cpu.threads
        )?;
        writeln!(formatter, "memory      {gibibytes:.2} GiB")?;
        writeln!(formatter, "kernel      {kernel}")?;
        writeln!(
            formatter,
            "filesystem  {} from {}, mounted at {}",
            filesystem.kind,
            filesystem.source,
            filesystem.mount_point.display()
        )?;
        writeln!(formatter)?;

        let cells: Vec<[String; 5]> = self.rows.iter().map(Row::cells).collect();
        let mut widths = [0; 4];
        for row in &cells {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.chars().count());
            }
        }
        for (row, [line, statistic, measured, bound, verdict]) in self.rows.iter().zip(&cells) {
            writeln!(
                formatter,
                "{line:<0$}  {statistic:<1$}  {measured:>2$}  {bound:<3$}  {verdict}",
                widths[0], widths[1], widths[2], widths[3]
            )?;
            if let Some(why) = row.why {
                writeln!(formatter, "  not gated: {why}")?;
            }
        }
        if !self.estimates.is_empty() {
            writeln!(formatter)?;
        }
        for estimate in self.estimates {
            writeln!(formatter, "{estimate}")?;
        }
        Ok(())
    }
}

/// Whether a line's measurement held to the estimate its ceiling budget states.
#[derive(Debug, Clone, PartialEq)]
pub struct EstimateCheck {
    /// The line.
    pub line: &'static str,
    /// Its p95, the statistic the outcome budget is stated in.
    pub p95: f64,
    /// The most the estimate allows.
    pub most: f64,
    /// Whose estimate it is.
    pub source: &'static str,
    /// The capture budget the estimate is a part of, for the share it takes.
    pub capture_budget: f64,
}

impl EstimateCheck {
    /// Whether the estimate held: a measurement of exactly its most holds, as every bound does.
    pub fn held(&self) -> bool {
        self.p95 <= self.most
    }
}

impl fmt::Display for EstimateCheck {
    /// One line: whether the estimate held, the measurement against it, and the share of the capture
    /// budget the measurement takes.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (verdict, relation) = if self.held() {
            ("held", "within")
        } else {
            ("failed", "over")
        };
        write!(
            formatter,
            "estimate {verdict}: {} p95 {:.3} ms {relation} {}, at most {} ms; {:.1} % of the capture \
             budget of {} ms",
            self.line,
            self.p95,
            self.source,
            self.most,
            self.p95 / self.capture_budget * 100.0,
            self.capture_budget
        )
    }
}
