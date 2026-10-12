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
    /// hour, `2026-09-30T14`, or a day, `2026-09-30` (ticket 79), either perhaps followed by the format
    /// of a sibling, `2026-09-30T14.2` (ADR 0010). Anything else — another spelling of the same moment
    /// included — is not a Segment, and Retention leaves its ref alone.
    pub fn parse(name: &str) -> Option<Self> {
        bounds(name).map(|_| Self(name.to_owned()))
    }

    /// The sibling of this Segment's period in `format`: the Segment a capture opens when the period's
    /// own already holds another format, which it must never append to (ADR 0010).
    ///
    /// Named by the format, so each format has at most one Segment per period and finding it again
    /// takes no counting. Siblings sort inside their period, after its own name and before the next
    /// period's — what the timeline's ordering by ref needs.
    pub fn in_format(&self, format: ContentFormat) -> Self {
        Self(format!("{}.{format}", self.period()))
    }

    /// The period's own Segment: this one, or the one this is a sibling of.
    pub fn period(&self) -> Self {
        match self.0.split_once('.') {
            Some((period, _)) => Self(period.to_owned()),
            None => self.clone(),
        }
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
    let name = match name.split_once('.') {
        // One spelling per format: digits only, and no leading zero.
        Some((period, format)) => {
            let spelled = !format.starts_with('0') && ContentFormat::parse(format).is_some();
            if !spelled {
                return None;
            }
            period
        }
        None => name,
    };
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

/// How the Snapshots of a Segment are laid out on disk: a property of each Segment, fixed when it is
/// created, of which a store may hold a mix indefinitely (ADR 0010).
///
/// There is no migration: a build writes its own format into new Segments and reads every format from
/// the first up to its own. A Segment in any other is unreadable to it — never read, never rewritten —
/// and costs only that Segment's history, never capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentFormat(u32);

impl ContentFormat {
    /// The first format: that of every Segment written before formats were recorded at all.
    pub const FIRST: Self = Self(1);

    /// The format this build writes, and the newest it reads.
    pub const CURRENT: Self = Self(1);

    /// The format numbered `number`.
    pub const fn new(number: u32) -> Self {
        Self(number)
    }

    /// The format `written` names: a number, digits only. Anything else names none this build — or any
    /// other — can read.
    pub fn parse(written: &str) -> Option<Self> {
        if written.is_empty() || !written.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }

        written.parse().ok().map(Self)
    }

    /// Whether this build reads Segments in this format: every one from the first to its own.
    pub fn is_readable(self) -> bool {
        (Self::FIRST..=Self::CURRENT).contains(&self)
    }

    /// The format as a number, for a version report.
    pub fn number(self) -> u32 {
        self.0
    }
}

impl fmt::Display for ContentFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl fmt::Display for SegmentName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
