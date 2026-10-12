//! Which Segment a moment belongs to.
//!
//! A Segment's name is also its ref's last component, so two properties matter beyond correctness: it
//! must sort chronologically as text, and it must not move when the machine's time zone does.

use std::time::{Duration, SystemTime};

use keepster_core::{ContentFormat, SegmentName};

/// Every moment in these tests is representable, so a failure here is a defect rather than a case.
fn hourly(moment: SystemTime) -> SegmentName {
    SegmentName::hourly(moment).expect("a representable moment")
}

fn at(epoch_seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(epoch_seconds)
}

#[test]
fn a_moment_names_the_hour_it_falls_in() {
    // 2026-09-30T14:37:05Z
    assert_eq!(hourly(at(1_790_779_025)).as_str(), "2026-09-30T14");
}

#[test]
fn the_epoch_itself_is_named() {
    assert_eq!(hourly(at(0)).as_str(), "1970-01-01T00");
}

#[test]
fn two_moments_in_one_hour_share_a_segment() {
    let early = hourly(at(1_790_776_800));
    let late = hourly(at(1_790_780_399));

    assert_eq!(early, late, "the whole hour is one Segment");
}

#[test]
fn the_next_hour_is_a_different_segment() {
    let before = hourly(at(1_790_780_399));
    let after = hourly(at(1_790_780_400));

    assert_ne!(before, after);
    assert!(
        before.as_str() < after.as_str(),
        "names must sort chronologically, because the timeline is assembled by enumerating refs"
    );
}

#[test]
fn names_sort_chronologically_across_a_year_boundary() {
    // 2026-12-31T23:00:00Z and 2027-01-01T00:00:00Z — the case a naive numeric format gets wrong.
    let last = hourly(at(1_798_758_000));
    let first = hourly(at(1_798_761_600));

    assert_eq!(last.as_str(), "2026-12-31T23");
    assert_eq!(first.as_str(), "2027-01-01T00");
    assert!(last.as_str() < first.as_str());
}

#[test]
fn a_leap_day_is_named_as_itself() {
    // 2028-02-29T12:00:00Z: a year the calendar has to be right about.
    assert_eq!(hourly(at(1_835_438_400)).as_str(), "2028-02-29T12");
}

#[test]
fn a_segment_s_name_reads_back_as_the_span_it_covers() {
    let name = SegmentName::parse("2026-09-30T14").expect("an hourly name");
    assert_eq!(name, hourly(at(1_790_779_025)));
    assert_eq!(name.span(), (at(1_790_776_800), at(1_790_780_400)));

    // Ticket 79 writes daily Segments; a store may already hold them beside hourly ones.
    let day = SegmentName::parse("2026-09-30").expect("a daily name");
    assert_eq!(day.span(), (at(1_790_726_400), at(1_790_812_800)));
    assert!(day < name, "a day sorts before the hours inside it");

    for foreign in [
        "2026-09-30T24",
        "2026-9-30",
        "2026-09-30T14:00",
        "latest",
        "",
    ] {
        assert_eq!(SegmentName::parse(foreign), None, "{foreign}");
    }
}

#[test]
fn the_first_format_is_read_and_is_the_one_this_build_writes() {
    // ADR 0010: every Segment written before the format was recorded is in the first.
    assert_eq!(ContentFormat::FIRST, ContentFormat::new(1));
    assert_eq!(ContentFormat::parse("1"), Some(ContentFormat::FIRST));
    assert_eq!(ContentFormat::CURRENT, ContentFormat::new(1));
    assert!(ContentFormat::FIRST.is_readable());
}

#[test]
fn a_format_this_build_does_not_know_is_not_read() {
    assert!(
        !ContentFormat::new(2).is_readable(),
        "newer than this build"
    );
    assert!(!ContentFormat::new(0).is_readable(), "no format was ever 0");
    for unknown in ["two", "", "-1", "1.0"] {
        assert_eq!(
            ContentFormat::parse(unknown),
            None,
            "what is no number names no format: {unknown:?}"
        );
    }
    assert_eq!(
        ContentFormat::new(2).to_string(),
        "2",
        "as the trailer spells it"
    );
}

#[test]
fn a_segment_of_another_format_in_the_same_period_is_its_sibling() {
    // ADR 0010: a capture never appends to a Segment of another format; it opens one of its own in the
    // same period, named by the format, so each format has at most one Segment per period.
    let hour = hourly(at(1_790_779_025));
    let sibling = hour.in_format(ContentFormat::new(2));

    assert_eq!(sibling.as_str(), "2026-09-30T14.2");
    assert_eq!(SegmentName::parse("2026-09-30T14.2"), Some(sibling.clone()));
    assert_eq!(sibling.span(), hour.span(), "the same period");
    assert_eq!(sibling.period(), hour);
    assert_eq!(hour.period(), hour);
    let next = hourly(at(1_790_780_400));
    assert!(
        hour < sibling && sibling < next,
        "it sorts inside its period: {hour} {sibling} {next}"
    );
    assert_eq!(
        SegmentName::parse("2026-09-30.3").map(|day| day.span()),
        SegmentName::parse("2026-09-30").map(|day| day.span()),
        "a day has siblings as an hour does"
    );

    for foreign in [
        "2026-09-30T14.",
        "2026-09-30T14.0",
        "2026-09-30T14.01",
        "2026-09-30T14.x",
        "2026-09-30T14.1.2",
        "2026-09-30T14.-1",
    ] {
        assert_eq!(SegmentName::parse(foreign), None, "{foreign}");
    }
}
