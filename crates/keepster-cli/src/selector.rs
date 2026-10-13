//! Snapshot selectors: `latest`, `~N`, a moment, a duration — CLI conveniences, not an API mechanism.
//!
//! Every selector reduces to an ordinary timeline query (`?limit=1`, `?before=<moment>&limit=1`), so a
//! third-party client has the same power rather than a surrogate of ours, and no URL ever says
//! `latest` — a saved link that shows a different Snapshot tomorrow while looking like a link to one
//! is what ticket 07 rejected. The arithmetic lives here, over API types and a `now` the caller passes
//! in: the CLI reads no clock of its own (ADR 0012).

use std::time::SystemTime;

/// Which Snapshot a person meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// The newest Snapshot.
    Latest,
    /// The Snapshot `n` before the newest; `~0` is the newest itself.
    Back(u64),
    /// One Snapshot, by its full identity.
    Id(String),
    /// The Snapshot that was current at this moment: the newest taken at or before it.
    AsOf(jiff::Timestamp),
}

/// Read a selector.
///
/// A moment may be RFC 3339 (`2026-09-30T14:37:05Z`), or a date and time with no offset, read in this
/// machine's time zone because that is the clock the person looked at (`2026-09-30 14:37`,
/// `2026-09-30`); a duration (`90s`, `15m`, `2h`, `3d`, `1w`) means that long before `now`.
pub fn parse(text: &str, now: SystemTime) -> Result<Selector, String> {
    let text = text.trim();

    if text.eq_ignore_ascii_case("latest") {
        return Ok(Selector::Latest);
    }
    if let Some(count) = text.strip_prefix('~') {
        let count = if count.is_empty() {
            Ok(1)
        } else {
            count.parse()
        };
        return count.map(Selector::Back).map_err(|_| {
            format!("`{text}` is not a selector: `~` takes a whole number, as in `~2`")
        });
    }
    if is_identity(text) {
        return Ok(Selector::Id(text.to_ascii_uppercase()));
    }

    moment(text, now).map(Selector::AsOf)
}

/// Read a moment: a timestamp, a local date and time, or a duration before `now`.
pub fn moment(text: &str, now: SystemTime) -> Result<jiff::Timestamp, String> {
    let text = text.trim();

    if let Some(seconds) = duration_seconds(text) {
        let now = jiff::Timestamp::try_from(now).map_err(|_| {
            "this machine's clock is outside the range a moment can name".to_owned()
        })?;
        let span = jiff::SignedDuration::from_secs(seconds);
        return now
            .checked_sub(span)
            .map_err(|_| format!("`{text}` reaches before the earliest moment that can be named"));
    }
    if let Ok(timestamp) = text.parse::<jiff::Timestamp>() {
        return Ok(timestamp);
    }

    let local = jiff::tz::TimeZone::system();
    if let Ok(datetime) = text.parse::<jiff::civil::DateTime>() {
        if let Ok(zoned) = datetime.to_zoned(local.clone()) {
            return Ok(zoned.timestamp());
        }
    }
    if let Ok(date) = text.parse::<jiff::civil::Date>() {
        if let Ok(zoned) = date.to_zoned(local) {
            return Ok(zoned.timestamp());
        }
    }

    Err(format!(
        "`{text}` is neither a Snapshot nor a moment: try `latest`, `~1`, `2h`, \
         `2026-09-30 14:00` or a Snapshot's full identity"
    ))
}

/// A duration such as `2h`, in seconds, or `None` if `text` is not one.
pub(crate) fn duration_seconds(text: &str) -> Option<i64> {
    let unit = text.chars().last()?;
    let amount: i64 = text[..text.len() - unit.len_utf8()].parse().ok()?;
    let seconds = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3_600,
        'd' => 86_400,
        'w' => 604_800,
        _ => return None,
    };

    (amount >= 0).then_some(amount)?.checked_mul(seconds)
}

/// Whether `text` is spelled like a Snapshot's full identity: 26 characters of Crockford base32.
///
/// A moment always contains a `-` or a `:` and a duration ends in a unit after digits, so the three
/// forms cannot be mistaken for one another.
pub(crate) fn is_identity(text: &str) -> bool {
    text.len() == 26
        && text.chars().all(|character| {
            character.is_ascii_digit()
                || (character.is_ascii_alphabetic()
                    && !matches!(character.to_ascii_uppercase(), 'I' | 'L' | 'O' | 'U'))
        })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{Selector, moment, parse};

    /// 2026-09-30T14:37:05Z.
    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_779_025)
    }

    fn at(text: &str) -> jiff::Timestamp {
        text.parse().expect("a timestamp")
    }

    #[test]
    fn latest_and_back_count_snapshots_from_the_newest() {
        assert_eq!(parse("latest", now()), Ok(Selector::Latest));
        assert_eq!(parse("~0", now()), Ok(Selector::Back(0)));
        assert_eq!(parse("~3", now()), Ok(Selector::Back(3)));
        assert_eq!(
            parse("~", now()),
            Ok(Selector::Back(1)),
            "as git reads `HEAD~`"
        );
        assert!(parse("~x", now()).is_err());
    }

    #[test]
    fn a_full_identity_is_taken_as_it_is() {
        assert_eq!(
            parse("01k6dx3qr7m2v8c9wz4pn5yq1a", now()),
            Ok(Selector::Id("01K6DX3QR7M2V8C9WZ4PN5YQ1A".to_owned()))
        );
    }

    #[test]
    fn a_duration_is_that_long_before_now() {
        assert_eq!(
            parse("2h", now()),
            Ok(Selector::AsOf(at("2026-09-30T12:37:05Z")))
        );
        assert_eq!(moment("90s", now()), Ok(at("2026-09-30T14:35:35Z")));
        assert_eq!(moment("3d", now()), Ok(at("2026-09-27T14:37:05Z")));
        assert_eq!(moment("1w", now()), Ok(at("2026-09-23T14:37:05Z")));
    }

    #[test]
    fn a_timestamp_with_an_offset_names_one_moment_anywhere() {
        assert_eq!(
            parse("2026-09-30T16:00:00+02:00", now()),
            Ok(Selector::AsOf(at("2026-09-30T14:00:00Z")))
        );
    }

    #[test]
    fn a_date_and_time_without_an_offset_is_read_on_this_machines_clock() {
        let expected = "2026-09-30T14:00"
            .parse::<jiff::civil::DateTime>()
            .expect("a civil time")
            .to_zoned(jiff::tz::TimeZone::system())
            .expect("a local time")
            .timestamp();

        assert_eq!(moment("2026-09-30T14:00", now()), Ok(expected));
        assert_eq!(moment("2026-09-30 14:00", now()), Ok(expected));
    }

    #[test]
    fn anything_else_is_refused_with_the_forms_that_work() {
        let refused = parse("yesterday", now()).expect_err("not a selector");
        assert!(refused.contains("latest"), "{refused}");
    }
}
