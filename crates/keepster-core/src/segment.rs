//! Which Segment a moment belongs to.

use std::fmt;
use std::time::{Duration, SystemTime};

const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(24 * 3600);

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
    /// Hourly is the shipped default and the only period this build writes; ticket 79 makes the period
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

    /// The Segment a ref's last component names, if it names one this build writes or will write: an
    /// hour, `2026-09-30T14`, or a day, `2026-09-30` (ticket 79). Anything else — another spelling of the
    /// same moment included — is not a Segment, and Retention leaves its ref alone.
    pub fn parse(name: &str) -> Option<Self> {
        bounds(name).map(|_| Self(name.to_owned()))
    }

    /// The moments this Segment covers: from its start, inclusive, to the next Segment's, exclusive.
    pub fn span(&self) -> (SystemTime, SystemTime) {
        let (start, length) =
            bounds(&self.0).expect("every name is built by `hourly` or accepted by `parse`");

        (start, start + length)
    }

    /// The name as text, for building a ref or reading a listing.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where the Segment named `name` starts and how long it is, if it is a name this build knows.
fn bounds(name: &str) -> Option<(SystemTime, Duration)> {
    let (day, hour) = match name.split_once('T') {
        Some((day, hour)) => (day, Some(hour)),
        None => (name, None),
    };
    // Fixed width and digits only, so one moment has one name and every name sorts as text.
    let number = |text: &str, digits: usize| {
        (text.len() == digits && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse::<i16>().ok())
            .flatten()
    };
    let mut parts = day.split('-');
    let year = number(parts.next()?, 4)?;
    let month = i8::try_from(number(parts.next()?, 2)?).ok()?;
    let date = i8::try_from(number(parts.next()?, 2)?).ok()?;
    if parts.next().is_some() {
        return None;
    }
    let (hour, length) = match hour {
        Some(hour) => (i8::try_from(number(hour, 2)?).ok()?, HOUR),
        None => (0, DAY),
    };
    let start = jiff::civil::DateTime::new(year, month, date, hour, 0, 0, 0)
        .and_then(|civil| civil.to_zoned(jiff::tz::TimeZone::UTC))
        .ok()?;

    Some((SystemTime::from(start.timestamp()), length))
}

impl fmt::Display for SegmentName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
