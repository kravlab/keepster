//! Which Segment a moment belongs to.

use std::fmt;
use std::time::SystemTime;

/// A moment the calendar cannot name.
///
/// Reported rather than clamped: a clock far outside the representable range is a broken machine, and a
/// Segment named by guesswork would put Snapshots in the wrong period for as long as the clock stays
/// wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the moment is outside the range a Segment can be named for")]
pub struct UnnameableMoment;

/// A Segment's name, which is also the last component of its ref.
///
/// Snapshots are grouped into Segments so that dropping a whole period is a ref deletion (ADR 0001),
/// and the timeline is assembled by enumerating those refs — so the name has to sort chronologically as
/// text, and it has to stay a leaf, because git's files backend cannot hold both `refs/x` and
/// `refs/x/y`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SegmentName(String);

impl SegmentName {
    /// The hourly Segment containing `moment`.
    ///
    /// Hourly is the shipped default and the only period this build writes; ticket 51 makes the period
    /// configurable, and it is a property of each Segment rather than of the store, so a store may hold
    /// several periods at once.
    ///
    /// **UTC, always.** A local bucket would renumber Segments at every offset change and would sort
    /// wrongly across one, which is exactly what the ref ordering must not do.
    pub fn hourly(moment: SystemTime) -> Result<Self, UnnameableMoment> {
        let timestamp = jiff::Timestamp::try_from(moment).map_err(|_| UnnameableMoment)?;
        let utc = timestamp.to_zoned(jiff::tz::TimeZone::UTC);

        Ok(Self(utc.strftime("%Y-%m-%dT%H").to_string()))
    }

    /// The name as text, for building a ref or reading a listing.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SegmentName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
