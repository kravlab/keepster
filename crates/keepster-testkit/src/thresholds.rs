//! The tripwire's committed thresholds (ticket 30): the numbers a run is held to, and the profile
//! digests they were calibrated against.
//!
//! The threshold is committed, never the measurement: measurements are noisy, thresholds are not.
//! Changing one is a commit somebody has to justify, the shape ticket 14 chose for `openapi.json`. The
//! file names no machine — what a run measured, and on what, lives in that run's report.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The thresholds file, relative to this crate.
pub const THRESHOLDS_FILE: &str = "bench/thresholds.toml";

/// What heads the thresholds file, and what it says above its limits: [`Thresholds::render`] writes the
/// whole file, so its comments live here.
const HEADER: &str = "\
# The operation tripwire's committed thresholds (ticket 30). `make bench` fails when a gated line
# measures above its limit here.
#
# The threshold is committed, never the measurement. Each limit is twice what the reference machine
# measured when it was calibrated (`make bench-calibrate`), a headroom sized to the failures this design
# has — every one of them a multiple — rather than to run-to-run noise. Changing a limit is a commit of
# its own that says why. No machine is named here: what a run measured, and on what, is in its report.
#
# `digests` holds each profile's digest at calibration. A test compares them with the profiles as they
# are, so a fixture's shape cannot move without this file moving in the same commit (ticket 31).
";
const LIMITS_NOTE: &str = "\
# Line name to the most a run may measure, in the line's unit: milliseconds for a duration, the ratio
# itself for a ratio.
";

/// The limit calibration commits for a line whose median measured `median`: twice it, for ticket 30's
/// headroom, rounded up to two significant digits so the number reads as a choice rather than a
/// measurement and never falls below twice what was measured.
pub fn calibrated_limit(median: f64) -> f64 {
    let doubled = 2.0 * median;
    if doubled <= 0.0 || !doubled.is_finite() {
        return doubled;
    }
    let scale = 10f64.powf(doubled.log10().floor() - 1.0);
    // A quotient a float lands a hair above a whole number is that number, not the next one up.
    let rounded = ((doubled / scale) - 1e-9).ceil() * scale;
    // Through the two-digit decimal text, so the limit is the float nearest those digits — the one that
    // prints as them — rather than one the multiplication left a hair off.
    format!("{rounded:.1e}")
        .parse()
        .expect("a float's own exponent notation parses")
}

/// The committed thresholds: per profile, the digest it had when calibrated; per gated line, its limit.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    /// Profile name to [`crate::bulk::digest`].
    digests: BTreeMap<String, String>,
    /// Line name to the most a run may measure, in the line's own unit.
    limits: BTreeMap<String, f64>,
}

/// Why a thresholds file could not be used. Each names the file; the cause, where there is one, is the
/// error's source rather than part of its message, so a report printing the chain says it once.
#[derive(Debug, thiserror::Error)]
pub enum ThresholdsError {
    /// The file could not be read.
    #[error("the thresholds file {path} cannot be read")]
    Unreadable {
        /// Where it was looked for.
        path: PathBuf,
        /// What reading it said.
        source: io::Error,
    },
    /// The text is not a thresholds file.
    #[error("{path} is not a thresholds file")]
    Malformed {
        /// Where the text came from.
        path: PathBuf,
        /// What parsing it said.
        source: toml::de::Error,
    },
    /// A limit that is not a finite number above zero: every run would fail it, or none could.
    #[error("the limit {path} sets for `{line}` is not a finite number above zero")]
    InvalidLimit {
        /// Where the text came from.
        path: PathBuf,
        /// The line it limits.
        line: String,
    },
}

/// A disagreement between the lines a tripwire gates on and the limits the file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    /// A gated line with no limit: it could never fail.
    Unlimited(String),
    /// A limit with no gated line: it would be decoration, the failure ticket 30 names.
    Orphaned(String),
}

impl Thresholds {
    /// The thresholds file committed beside this crate.
    ///
    /// # Errors
    ///
    /// When it cannot be read or is not a valid thresholds file.
    pub fn committed() -> Result<Self, ThresholdsError> {
        Self::read(&Self::committed_path())
    }

    /// The thresholds file at `path`.
    ///
    /// # Errors
    ///
    /// When it cannot be read or is not a valid thresholds file.
    pub fn read(path: &Path) -> Result<Self, ThresholdsError> {
        let text = fs::read_to_string(path).map_err(|source| ThresholdsError::Unreadable {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&text, path)
    }

    /// Where the committed thresholds file is.
    pub fn committed_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(THRESHOLDS_FILE)
    }

    /// Thresholds from the text of a thresholds file, `origin` being where the text came from, for the
    /// errors to name.
    ///
    /// # Errors
    ///
    /// When the text is not one, or a limit is not a finite number above zero.
    pub fn parse(text: &str, origin: &Path) -> Result<Self, ThresholdsError> {
        let thresholds: Self =
            toml::from_str(text).map_err(|source| ThresholdsError::Malformed {
                path: origin.to_owned(),
                source,
            })?;
        if let Some((line, _)) = thresholds
            .limits
            .iter()
            .find(|(_, limit)| !limit.is_finite() || **limit <= 0.0)
        {
            return Err(ThresholdsError::InvalidLimit {
                path: origin.to_owned(),
                line: line.clone(),
            });
        }
        Ok(thresholds)
    }

    /// Thresholds as calibration writes them: `digests` per profile and `limits` per gated line.
    pub fn calibrated(digests: BTreeMap<String, String>, limits: BTreeMap<String, f64>) -> Self {
        Self { digests, limits }
    }

    /// The thresholds file's text: its comments, then digests and limits in name order, every limit
    /// written as the float it is, so reading the text back gives these thresholds exactly.
    pub fn render(&self) -> String {
        let mut text = String::new();
        self.write(&mut text)
            .expect("writing to a `String` cannot fail");
        text
    }

    /// Write [`Thresholds::render`]'s text to `text`.
    fn write(&self, text: &mut String) -> fmt::Result {
        writeln!(text, "{HEADER}\n[digests]")?;
        for (profile, digest) in &self.digests {
            writeln!(text, "{profile} = {digest:?}")?;
        }
        writeln!(text, "\n{LIMITS_NOTE}[limits]")?;
        for (line, limit) in &self.limits {
            writeln!(text, "{line:?} = {limit:?}")?;
        }
        Ok(())
    }

    /// The profiles these thresholds hold a digest for, in name order.
    pub fn profiles(&self) -> impl Iterator<Item = &str> {
        self.digests.keys().map(String::as_str)
    }

    /// The digest `profile` had when these thresholds were calibrated.
    pub fn digest(&self, profile: &str) -> Option<&str> {
        self.digests.get(profile).map(String::as_str)
    }

    /// The limit on `line`.
    pub fn limit(&self, line: &str) -> Option<f64> {
        self.limits.get(line).copied()
    }

    /// Every way these limits and the `gated` lines disagree: gated lines without a limit, in the order
    /// given and each once however often it is given, then limits without a line, in name order. Empty
    /// when they agree.
    pub fn mismatches(&self, gated: &[&str]) -> Vec<Mismatch> {
        let mut seen = BTreeSet::new();
        let unlimited = gated
            .iter()
            .filter(|line| !self.limits.contains_key(**line) && seen.insert(**line))
            .map(|line| Mismatch::Unlimited((*line).to_owned()));
        let orphaned = self
            .limits
            .keys()
            .filter(|line| !gated.contains(&line.as_str()))
            .map(|line| Mismatch::Orphaned(line.clone()));
        unlimited.chain(orphaned).collect()
    }
}
